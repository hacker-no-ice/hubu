//! Shared outbound HTTP, deadline, and artifact-download policy.
//!
//! `timeout_ms` is an invocation-wide budget: submission, any polling, and
//! referenced-artifact downloads all consume the same deadline. Provider
//! payload construction and response DTO parsing intentionally remain local to
//! each adapter.

use reqwest::{
    blocking::{Client, Response},
    header::{HeaderMap, HeaderName, HeaderValue},
    Url,
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    io::Read,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};
use thiserror::Error;

pub const MAX_PROVIDER_RESPONSE_BYTES: usize = 1024 * 1024;
pub const TEMPORAL_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(300);
pub const ACTIVITY_OPERATIONAL_HEADROOM: Duration = Duration::from_secs(30);
pub const MAX_PROVIDER_DEADLINE: Duration = Duration::from_secs(270);

#[derive(Clone, Copy, Debug)]
pub struct InvocationDeadline(Instant);

thread_local! {
    static ACTIVITY_PROVIDER_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Installs the Temporal activity's provider-safe deadline on the blocking
/// activity thread. Adapter deadlines are capped by this value, so work done
/// before provider transmission consumes the same activity-wide budget.
pub struct ActivityDeadlineGuard(Option<Instant>);

impl ActivityDeadlineGuard {
    pub fn enter(activity_deadline: Option<SystemTime>) -> Result<Self, HttpKernelError> {
        let provider_deadline = activity_deadline
            .map(|deadline| {
                deadline
                    .duration_since(SystemTime::now())
                    .map_err(|_| HttpKernelError::DeadlineExceeded)?
                    .checked_sub(ACTIVITY_OPERATIONAL_HEADROOM)
                    .filter(|remaining| !remaining.is_zero())
                    .map(|remaining| Instant::now() + remaining)
                    .ok_or(HttpKernelError::DeadlineExceeded)
            })
            .transpose()?;
        let previous = ACTIVITY_PROVIDER_DEADLINE.with(|slot| slot.replace(provider_deadline));
        Ok(Self(previous))
    }
}

impl Drop for ActivityDeadlineGuard {
    fn drop(&mut self) {
        ACTIVITY_PROVIDER_DEADLINE.with(|slot| slot.set(self.0));
    }
}

impl InvocationDeadline {
    pub fn from_timeout(timeout: Duration) -> Result<Self, HttpKernelError> {
        if timeout.is_zero() || timeout > MAX_PROVIDER_DEADLINE {
            return Err(HttpKernelError::InvalidDeadline);
        }
        let configured = Instant::now() + timeout;
        let deadline = ACTIVITY_PROVIDER_DEADLINE
            .with(|slot| slot.get())
            .map_or(configured, |activity| configured.min(activity));
        if deadline <= Instant::now() {
            return Err(HttpKernelError::DeadlineExceeded);
        }
        Ok(Self(deadline))
    }

    pub fn remaining(self) -> Result<Duration, HttpKernelError> {
        self.0
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(HttpKernelError::DeadlineExceeded)
    }
}

pub fn valid_provider_deadline_ms(timeout_ms: u64) -> bool {
    timeout_ms > 0 && Duration::from_millis(timeout_ms) <= MAX_PROVIDER_DEADLINE
}

#[derive(Debug, Error)]
pub enum HttpKernelError {
    #[error("invalid provider deadline")]
    InvalidDeadline,
    #[error("provider deadline exceeded")]
    DeadlineExceeded,
    #[error("invalid HTTP policy")]
    InvalidPolicy,
    #[error("HTTP request failed")]
    Request,
    #[error("HTTP response exceeded bound")]
    ResponseTooLarge,
    #[error("HTTP response was unsuccessful")]
    UnsuccessfulResponse,
}

static SHARED_CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();

/// Process-wide connection pool. Per-operation budgets are applied to request
/// builders so one configured client can be reused by submit, poll, and fetch.
pub fn shared_client() -> Result<&'static Client, HttpKernelError> {
    SHARED_CLIENT
        .get_or_init(|| {
            Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|_| HttpKernelError::InvalidPolicy)
}

pub fn read_bounded(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, HttpKernelError> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| HttpKernelError::Request)?;
    if bytes.len() > limit {
        return Err(HttpKernelError::ResponseTooLarge);
    }
    Ok(bytes)
}

pub fn read_json_bounded(
    response: &mut Response,
    limit: usize,
) -> Result<serde_json::Value, HttpKernelError> {
    serde_json::from_slice(&read_bounded(response, limit)?).map_err(|_| HttpKernelError::Request)
}

pub fn provider_request_id(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let value = headers.get(*name)?.to_str().ok()?.trim();
        (!value.is_empty() && value.len() <= 255).then(|| value.to_owned())
    })
}

pub fn validate_safe_headers(
    headers: &BTreeMap<String, String>,
    forbidden: &[&str],
) -> Result<(), HttpKernelError> {
    let mut unique = BTreeSet::new();
    if headers.iter().any(|(name, value)| {
        !unique.insert(name.to_ascii_lowercase())
            || name.parse::<HeaderName>().is_err()
            || value.is_empty()
            || value.contains(['\r', '\n'])
            || value.parse::<HeaderValue>().is_err()
            || forbidden.iter().any(|item| name.eq_ignore_ascii_case(item))
    }) {
        return Err(HttpKernelError::InvalidPolicy);
    }
    Ok(())
}

pub fn validate_https_origin(
    url: &Url,
    expected_host: Option<&str>,
) -> Result<(), HttpKernelError> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.host_str().is_none()
        || expected_host.is_some_and(|host| url.host_str() != Some(host))
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(HttpKernelError::InvalidPolicy);
    }
    Ok(())
}

/// `url::Url` canonicalizes an explicit default port such as HTTPS `:443`
/// away. Inspect the raw authority when policy forbids any explicit port.
pub fn url_has_explicit_port(raw: &str) -> bool {
    let Some((_, remainder)) = raw.split_once(':') else {
        return false;
    };
    // Special-scheme URL parsing accepts and canonicalizes mixed or excess
    // slash/backslash separators. Skip the full run so the raw authority's
    // explicit default port cannot disappear during `Url` normalization.
    let remainder = remainder.trim_start_matches(['/', '\\']);
    let authority = remainder
        .split(['/', '\\', '?', '#'])
        .next()
        .unwrap_or_default();
    let host_and_port = authority.rsplit('@').next().unwrap_or_default();
    if host_and_port.starts_with('[') {
        return host_and_port.find(']').is_some_and(|end| {
            host_and_port
                .get(end + 1..)
                .is_some_and(|tail| tail.starts_with(':'))
        });
    }
    host_and_port.contains(':')
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialForwarding {
    #[default]
    Prohibited,
    Permitted,
}

#[derive(Clone, Debug)]
pub struct ArtifactDownloadPolicy {
    approved_hosts: BTreeSet<String>,
    max_bytes: usize,
    credential_forwarding: CredentialForwarding,
}

impl ArtifactDownloadPolicy {
    pub fn new(
        approved_hosts: &[String],
        max_bytes: u64,
        credential_forwarding: CredentialForwarding,
    ) -> Result<Self, HttpKernelError> {
        if approved_hosts.is_empty() || max_bytes == 0 || max_bytes > usize::MAX as u64 {
            return Err(HttpKernelError::InvalidPolicy);
        }
        let approved_hosts = approved_hosts
            .iter()
            .map(|host| host.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if approved_hosts.is_empty()
            || approved_hosts.iter().any(|host| {
                host.is_empty()
                    || host.contains(['/', ':', '@'])
                    || Url::parse(&format!("https://{host}/"))
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_owned))
                        .as_deref()
                        != Some(host.as_str())
            })
        {
            return Err(HttpKernelError::InvalidPolicy);
        }
        Ok(Self {
            approved_hosts,
            max_bytes: max_bytes as usize,
            credential_forwarding,
        })
    }

    pub fn validate_url(&self, url: &Url) -> Result<(), HttpKernelError> {
        let host = url.host_str().map(str::to_ascii_lowercase);
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.fragment().is_some()
            || host
                .as_ref()
                .is_none_or(|host| !self.approved_hosts.contains(host))
        {
            return Err(HttpKernelError::InvalidPolicy);
        }
        Ok(())
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn permits_credentials(&self) -> bool {
        self.credential_forwarding == CredentialForwarding::Permitted
    }
}

pub fn download_artifact(
    url: &Url,
    deadline: InvocationDeadline,
    policy: &ArtifactDownloadPolicy,
    credential: Option<(&str, &str)>,
) -> Result<Vec<u8>, HttpKernelError> {
    policy.validate_url(url)?;
    if credential.is_some() && !policy.permits_credentials() {
        return Err(HttpKernelError::InvalidPolicy);
    }
    let mut request = shared_client()?
        .get(url.clone())
        .timeout(deadline.remaining()?);
    if let Some((name, value)) = credential {
        request = request.header(name, value);
    }
    let mut response = request.send().map_err(|_| HttpKernelError::Request)?;
    if !response.status().is_success() {
        return Err(HttpKernelError::UnsuccessfulResponse);
    }
    read_bounded(&mut response, policy.max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn deadline_reserves_temporal_headroom() {
        assert_eq!(
            MAX_PROVIDER_DEADLINE + ACTIVITY_OPERATIONAL_HEADROOM,
            TEMPORAL_ACTIVITY_TIMEOUT
        );
        assert!(valid_provider_deadline_ms(270_000));
        assert!(!valid_provider_deadline_ms(270_001));
    }

    #[test]
    fn activity_remaining_time_caps_adapter_deadlines() {
        let _guard = ActivityDeadlineGuard::enter(Some(
            SystemTime::now() + ACTIVITY_OPERATIONAL_HEADROOM + Duration::from_millis(40),
        ))
        .unwrap();
        let deadline = InvocationDeadline::from_timeout(Duration::from_secs(1)).unwrap();
        assert!(deadline.remaining().unwrap() <= Duration::from_millis(40));
    }

    #[test]
    fn bounded_reads_and_safe_headers_are_shared() {
        assert_eq!(
            read_bounded(&mut io::Cursor::new(b"1234"), 4).unwrap(),
            b"1234"
        );
        assert!(matches!(
            read_bounded(&mut io::Cursor::new(b"12345"), 4),
            Err(HttpKernelError::ResponseTooLarge)
        ));
        let mut headers = BTreeMap::new();
        headers.insert("x-client".into(), "gongbu".into());
        assert!(validate_safe_headers(&headers, &["authorization"]).is_ok());
        headers.insert("Authorization".into(), "secret".into());
        assert!(validate_safe_headers(&headers, &["authorization"]).is_err());
    }

    #[test]
    fn configured_client_is_reused() {
        assert!(std::ptr::eq(
            shared_client().unwrap(),
            shared_client().unwrap()
        ));
    }

    #[test]
    fn artifact_policy_is_https_origin_bound_and_credentials_are_opt_in() {
        let policy = ArtifactDownloadPolicy::new(
            &["cdn.example".into()],
            10,
            CredentialForwarding::Prohibited,
        )
        .unwrap();
        assert!(policy
            .validate_url(&Url::parse("https://cdn.example/image.png?token=x").unwrap())
            .is_ok());
        for url in [
            "http://cdn.example/image.png",
            "https://cdn.example:444/image.png",
            "https://other.example/image.png",
            "https://user@cdn.example/image.png",
        ] {
            assert!(
                policy.validate_url(&Url::parse(url).unwrap()).is_err(),
                "{url}"
            );
        }
        assert!(!policy.permits_credentials());
    }

    #[test]
    fn provider_origins_are_https_root_urls_without_custom_ports() {
        assert!(validate_https_origin(
            &Url::parse("https://api.example/").unwrap(),
            Some("api.example")
        )
        .is_ok());
        for url in [
            "http://api.example/",
            "https://api.example:8443/",
            "https://api.example/v1",
            "https://user@api.example/",
        ] {
            assert!(validate_https_origin(&Url::parse(url).unwrap(), None).is_err());
        }
        assert!(url_has_explicit_port("https://api.example:443/"));
        assert!(url_has_explicit_port("https://api.example:8443/"));
        assert!(url_has_explicit_port("HTTPS://api.example:443/"));
        assert!(url_has_explicit_port("https:\\\\api.example:443/"));
        assert!(url_has_explicit_port("https:///api.example:443/"));
        assert!(url_has_explicit_port(r"https:/\api.example:443/"));
        assert!(url_has_explicit_port(r"https:\/api.example:443/"));
        assert!(url_has_explicit_port("https:////api.example:443/"));
        assert!(!url_has_explicit_port("https://api.example/"));
    }
}
