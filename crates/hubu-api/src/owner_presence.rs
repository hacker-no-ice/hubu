//! Owner presence (v0.3.0 spec, section 5).
//!
//! Owner actions need the owner's physical presence, confirmed by
//! `hubu-server` itself through a [`PresenceProvider`]. One confirmation opens
//! a five-minute owner window held in server memory only and bound to the
//! client that opened it by a random window token. The server stores only the
//! token's hash.
// Unit tests install fixture providers directly, so the environment-driven
// provider selection and the macOS provider are unused in the test build. On
// other platforms the macOS-only policy setting and methods are unused.
#![cfg_attr(any(test, not(target_os = "macos")), allow(dead_code))]
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::Read,
    path::Path,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
mod macos;

pub(crate) const OWNER_WINDOW_HEADER: &str = "x-hubu-owner-window";
pub(crate) const NO_PROMPT_HEADER: &str = "x-hubu-no-prompt";
pub(crate) const CLIENT_HEADER: &str = "x-hubu-client";
pub(crate) const PROVIDER_ENV: &str = "HUBU_OWNER_PRESENCE_PROVIDER";
pub(crate) const FIXTURE_ENV: &str = "HUBU_OWNER_PRESENCE_FIXTURE";
pub(crate) const POLICY_ENV: &str = "HUBU_OWNER_PRESENCE_POLICY";
/// `on` keeps the prompt rate limit for the fixture provider (proof kits that
/// exercise it). Scripted runs leave it off: no person is being prompted.
pub(crate) const FIXTURE_RATE_LIMIT_ENV: &str = "HUBU_OWNER_PRESENCE_FIXTURE_RATE_LIMIT";

/// A window lasts five minutes from confirmation and does not slide.
pub(crate) const OWNER_WINDOW_DURATION: Duration = Duration::from_secs(5 * 60);
pub(crate) const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const RATE_LIMIT_PERIOD: Duration = Duration::from_secs(10 * 60);
pub(crate) const RATE_LIMIT_MAX_PROMPTS: usize = 5;
const WINDOW_TOKEN_PREFIX: &str = "hubu_window_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresenceMethod {
    Biometric,
    DevicePassword,
    Fixture,
}

impl PresenceMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Biometric => "biometric",
            Self::DevicePassword => "device_password",
            Self::Fixture => "fixture",
        }
    }
}

pub(crate) struct PresenceRequest {
    /// A verb phrase built by Hubu from validated, stored data. Never caller
    /// free text.
    pub(crate) reason: String,
    pub(crate) timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PresenceOutcome {
    Confirmed {
        method: PresenceMethod,
    },
    /// Includes user, app and system cancellation and every dialog error.
    Denied,
    TimedOut,
    Unavailable(String),
}

pub(crate) trait PresenceProvider: Send + Sync {
    /// Blocks until the owner answers, the timeout passes, or the prompt fails.
    fn confirm(&self, request: &PresenceRequest) -> PresenceOutcome;
    /// The provider name reported by `GET /owner/presence`.
    fn name(&self) -> &'static str;
}

/// Answers from a script. Tests and proof kits only.
///
/// The script is a comma-separated list of `confirm`, `deny`, `timeout` and
/// `unavailable`. Answers are used in order and the last one repeats.
pub(crate) struct FixtureProvider {
    script: Mutex<VecDeque<PresenceOutcome>>,
    last: Mutex<PresenceOutcome>,
}

impl FixtureProvider {
    pub(crate) fn from_outcomes(outcomes: Vec<PresenceOutcome>) -> Self {
        let last = outcomes
            .last()
            .cloned()
            .unwrap_or(PresenceOutcome::Confirmed {
                method: PresenceMethod::Fixture,
            });
        Self {
            script: Mutex::new(outcomes.into()),
            last: Mutex::new(last),
        }
    }

    pub(crate) fn parse(script: &str) -> Result<Self> {
        let outcomes = script
            .split(',')
            .map(str::trim)
            .filter(|answer| !answer.is_empty())
            .map(|answer| match answer {
                "confirm" => Ok(PresenceOutcome::Confirmed {
                    method: PresenceMethod::Fixture,
                }),
                "deny" => Ok(PresenceOutcome::Denied),
                "timeout" => Ok(PresenceOutcome::TimedOut),
                "unavailable" => Ok(PresenceOutcome::Unavailable(
                    "fixture provider scripted unavailable".to_string(),
                )),
                other => Err(anyhow!(
                    "{FIXTURE_ENV} accepts confirm, deny, timeout or unavailable, not `{other}`"
                )),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::from_outcomes(outcomes))
    }
}

impl PresenceProvider for FixtureProvider {
    fn confirm(&self, _request: &PresenceRequest) -> PresenceOutcome {
        let next = self
            .script
            .lock()
            .ok()
            .and_then(|mut script| script.pop_front());
        match next {
            Some(outcome) => {
                if let Ok(mut last) = self.last.lock() {
                    *last = outcome.clone();
                }
                outcome
            }
            None => self
                .last
                .lock()
                .map(|last| last.clone())
                .unwrap_or(PresenceOutcome::Denied),
        }
    }

    fn name(&self) -> &'static str {
        "fixture"
    }
}

/// Used where no presence provider can run (no macOS GUI session support).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) struct UnavailableProvider(pub(crate) &'static str);

impl PresenceProvider for UnavailableProvider {
    fn confirm(&self, _request: &PresenceRequest) -> PresenceOutcome {
        PresenceOutcome::Unavailable(self.0.to_string())
    }

    fn name(&self) -> &'static str {
        "none"
    }
}

/// Picks the provider from the environment at server start.
///
/// `HUBU_OWNER_PRESENCE_PROVIDER=fixture` selects the scripted provider, and
/// is refused when the database lives inside a managed profile. Otherwise
/// macOS uses LocalAuthentication and other platforms have no provider.
pub(crate) fn provider_from_env(database_path: &Path) -> Result<Arc<dyn PresenceProvider>> {
    match std::env::var(PROVIDER_ENV).ok().as_deref().map(str::trim) {
        Some("fixture") => {
            if is_inside_managed_profile(database_path) {
                return Err(anyhow!(
                    "{PROVIDER_ENV}=fixture is for tests and proof kits; hubu-server refuses it for a database inside a managed profile (`{}`)",
                    database_path.display()
                ));
            }
            let script = std::env::var(FIXTURE_ENV).unwrap_or_else(|_| "confirm".to_string());
            Ok(Arc::new(FixtureProvider::parse(&script)?))
        }
        None | Some("") | Some("system") => system_provider(),
        Some(other) => Err(anyhow!(
            "{PROVIDER_ENV} accepts `system` or `fixture`, not `{other}`"
        )),
    }
}

#[cfg(target_os = "macos")]
fn system_provider() -> Result<Arc<dyn PresenceProvider>> {
    let policy = match std::env::var(POLICY_ENV).ok().as_deref().map(str::trim) {
        None | Some("") | Some("owner") => macos::LocalAuthPolicy::DeviceOwner,
        Some("biometrics") => macos::LocalAuthPolicy::Biometrics,
        Some(other) => {
            return Err(anyhow!(
                "{POLICY_ENV} accepts `owner` or `biometrics`, not `{other}`"
            ))
        }
    };
    Ok(Arc::new(macos::LocalAuthProvider::new(policy)))
}

#[cfg(not(target_os = "macos"))]
fn system_provider() -> Result<Arc<dyn PresenceProvider>> {
    Ok(Arc::new(UnavailableProvider(
        "owner presence needs macOS LocalAuthentication in the owner's GUI session",
    )))
}

/// A managed profile root holds both `stack.toml` and `credentials.toml`.
pub(crate) fn is_inside_managed_profile(database_path: &Path) -> bool {
    let absolute = if database_path.is_absolute() {
        database_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(database_path))
            .unwrap_or_else(|_| database_path.to_path_buf())
    };
    absolute
        .ancestors()
        .skip(1)
        .any(|dir| dir.join("stack.toml").is_file() && dir.join("credentials.toml").is_file())
}

/// Which client presented an owner request (spec section 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OwnerClient {
    Portal,
    Cli,
    Agent,
    Unknown,
}

impl OwnerClient {
    pub(crate) fn from_header(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("portal") => Self::Portal,
            Some(value) if value.eq_ignore_ascii_case("cli") => Self::Cli,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Portal => "portal",
            Self::Cli => "cli",
            Self::Agent => "agent",
            Self::Unknown => "unknown",
        }
    }
}

/// Why a prompt did not confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptRefusal {
    InProgress,
    RateLimited { retry_after: Duration },
    Denied,
    TimedOut,
    Unavailable(String),
}

impl PromptRefusal {
    /// The governance `outcome` for this refusal.
    pub(crate) fn outcome(&self) -> &'static str {
        match self {
            Self::InProgress => "superseded",
            Self::RateLimited { .. } => "rate_limited",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Unavailable(_) => "unavailable",
        }
    }

    pub(crate) fn error_code(&self) -> &'static str {
        match self {
            Self::InProgress => "owner_prompt_in_progress",
            Self::RateLimited { .. } => "owner_prompt_rate_limited",
            Self::Denied => "owner_presence_denied",
            Self::TimedOut => "owner_presence_timeout",
            Self::Unavailable(_) => "owner_presence_unavailable",
        }
    }

    pub(crate) fn status(&self) -> u16 {
        match self {
            Self::InProgress => 409,
            Self::RateLimited { .. } => 429,
            _ => 403,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::InProgress => {
                "another owner confirmation is already on screen; wait for it to finish".to_string()
            }
            Self::RateLimited { retry_after } => format!(
                "too many owner confirmations were requested recently; try again in {} seconds",
                retry_after.as_secs().max(1)
            ),
            Self::Denied => "the owner did not confirm this action".to_string(),
            Self::TimedOut => "the owner did not answer the confirmation in time".to_string(),
            Self::Unavailable(reason) => format!("owner confirmation is unavailable: {reason}"),
        }
    }

    pub(crate) fn retry_guidance(&self) -> Value {
        match self {
            Self::InProgress => json!({
                "action": "retry_later",
                "message": "retry after the owner answers the confirmation that is on screen"
            }),
            Self::RateLimited { retry_after } => json!({
                "action": "retry_later",
                "retry_after_seconds": retry_after.as_secs().max(1),
                "message": "do not keep prompting the owner; wait before asking again"
            }),
            Self::Denied => json!({
                "action": "ask_owner",
                "message": "the owner refused; do not retry unless the owner asks you to"
            }),
            Self::TimedOut => json!({
                "action": "ask_owner",
                "message": "ask the owner whether they are at the Mac before trying again"
            }),
            Self::Unavailable(_) => json!({
                "action": "ask_owner",
                "message": "run hubu-server in the owner's macOS GUI session so it can ask for Touch ID"
            }),
        }
    }
}

/// An open owner window, as seen by a request that presented its token.
#[derive(Clone, Debug)]
pub(crate) struct ActiveWindow {
    pub(crate) client: OwnerClient,
    pub(crate) record_id: String,
    pub(crate) reason: String,
    pub(crate) method: PresenceMethod,
    pub(crate) expires_at: DateTime<Utc>,
}

/// A newly opened window. The token is returned once and never stored.
#[derive(Clone, Debug)]
pub(crate) struct IssuedWindow {
    pub(crate) token: String,
    pub(crate) expires_at: DateTime<Utc>,
}

impl IssuedWindow {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "token": self.token,
            "expires_at": self.expires_at.to_rfc3339(),
        })
    }
}

struct OwnerWindow {
    token_hash: String,
    expires_at: Instant,
    expires_at_utc: DateTime<Utc>,
    record_id: String,
    reason: String,
    method: PresenceMethod,
}

#[derive(Default)]
struct PresenceState {
    windows: HashMap<OwnerClient, OwnerWindow>,
    prompting: bool,
    recent_prompts: VecDeque<Instant>,
}

/// Server-memory owner presence state. Restarting the server closes every
/// window.
pub(crate) struct OwnerPresence {
    provider: RwLock<Arc<dyn PresenceProvider>>,
    state: Mutex<PresenceState>,
    rate_limit: RwLock<Option<(usize, Duration)>>,
    clock_offset: Mutex<Duration>,
}

/// Clears `prompting` even if the provider panics.
struct PromptGuard<'a>(&'a OwnerPresence);

impl Drop for PromptGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.prompting = false;
        }
    }
}

impl OwnerPresence {
    pub(crate) fn new(provider: Arc<dyn PresenceProvider>) -> Self {
        Self {
            provider: RwLock::new(provider),
            state: Mutex::new(PresenceState::default()),
            rate_limit: RwLock::new(Some((RATE_LIMIT_MAX_PROMPTS, RATE_LIMIT_PERIOD))),
            clock_offset: Mutex::new(Duration::ZERO),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_provider(&self, provider: Arc<dyn PresenceProvider>) {
        *self.provider.write().expect("presence provider lock") = provider;
    }

    pub(crate) fn set_rate_limit(&self, limit: Option<(usize, Duration)>) {
        *self.rate_limit.write().expect("presence rate limit lock") = limit;
    }

    #[cfg(test)]
    pub(crate) fn advance_clock(&self, by: Duration) {
        *self.clock_offset.lock().expect("presence clock lock") += by;
    }

    fn now(&self) -> Instant {
        Instant::now() + self.clock_offset.lock().map(|o| *o).unwrap_or_default()
    }

    fn now_utc(&self) -> DateTime<Utc> {
        let offset = self.clock_offset.lock().map(|o| *o).unwrap_or_default();
        Utc::now() + chrono::Duration::from_std(offset).unwrap_or_else(|_| chrono::Duration::zero())
    }

    pub(crate) fn provider_name(&self) -> &'static str {
        self.provider
            .read()
            .map(|provider| provider.name())
            .unwrap_or("unknown")
    }

    /// The open window whose token was presented, if it has not expired.
    pub(crate) fn active_window(&self, token: Option<&str>) -> Option<ActiveWindow> {
        let token = token.map(str::trim).filter(|token| !token.is_empty())?;
        let presented = super::hash_token(token);
        let now = self.now();
        let mut state = self.state.lock().ok()?;
        state.windows.retain(|_, window| window.expires_at > now);
        state.windows.iter().find_map(|(client, window)| {
            super::constant_time_eq(presented.as_bytes(), window.token_hash.as_bytes()).then(|| {
                ActiveWindow {
                    client: *client,
                    record_id: window.record_id.clone(),
                    reason: window.reason.clone(),
                    method: window.method,
                    expires_at: window.expires_at_utc,
                }
            })
        })
    }

    /// Raises one dialog. Holds no lock while the provider waits for the owner.
    pub(crate) fn prompt(&self, reason: &str) -> Result<PresenceMethod, PromptRefusal> {
        let started =
            {
                let mut state = self.state.lock().map_err(|_| {
                    PromptRefusal::Unavailable("presence state poisoned".to_string())
                })?;
                if state.prompting {
                    return Err(PromptRefusal::InProgress);
                }
                let now = self.now();
                if let Some((max, period)) = *self.rate_limit.read().map_err(|_| {
                    PromptRefusal::Unavailable("presence state poisoned".to_string())
                })? {
                    while state
                        .recent_prompts
                        .front()
                        .is_some_and(|at| now.duration_since(*at) >= period)
                    {
                        state.recent_prompts.pop_front();
                    }
                    if state.recent_prompts.len() >= max {
                        let oldest = *state.recent_prompts.front().expect("non-empty when full");
                        return Err(PromptRefusal::RateLimited {
                            retry_after: period.saturating_sub(now.duration_since(oldest)),
                        });
                    }
                }
                state.recent_prompts.push_back(now);
                state.prompting = true;
                now
            };
        let _guard = PromptGuard(self);
        let provider = self
            .provider
            .read()
            .map(|provider| Arc::clone(&provider))
            .map_err(|_| PromptRefusal::Unavailable("presence provider poisoned".to_string()))?;
        let outcome = provider.confirm(&PresenceRequest {
            reason: reason.to_string(),
            timeout: PROMPT_TIMEOUT,
        });
        match outcome {
            PresenceOutcome::Confirmed { method } => Ok(method),
            PresenceOutcome::Denied => Err(PromptRefusal::Denied),
            PresenceOutcome::TimedOut => Err(PromptRefusal::TimedOut),
            PresenceOutcome::Unavailable(reason) => {
                // No dialog was shown, so it does not count toward the limit.
                if let Ok(mut state) = self.state.lock() {
                    if let Some(position) =
                        state.recent_prompts.iter().rposition(|at| *at == started)
                    {
                        state.recent_prompts.remove(position);
                    }
                }
                Err(PromptRefusal::Unavailable(reason))
            }
        }
    }

    /// Opens a window for `client`, replacing any window it already had.
    pub(crate) fn open_window(
        &self,
        client: OwnerClient,
        record_id: &str,
        reason: &str,
        method: PresenceMethod,
    ) -> Result<IssuedWindow> {
        let token = format!("{WINDOW_TOKEN_PREFIX}{}", random_hex(32)?);
        let expires_at_utc = self.now_utc()
            + chrono::Duration::from_std(OWNER_WINDOW_DURATION)
                .map_err(|_| anyhow!("owner window duration out of range"))?;
        let window = OwnerWindow {
            token_hash: super::hash_token(&token),
            expires_at: self.now() + OWNER_WINDOW_DURATION,
            expires_at_utc,
            record_id: record_id.to_string(),
            reason: reason.to_string(),
            method,
        };
        self.state
            .lock()
            .map_err(|_| anyhow!("presence state poisoned"))?
            .windows
            .insert(client, window);
        Ok(IssuedWindow {
            token,
            expires_at: expires_at_utc,
        })
    }

    /// Closes every open window. Returns how many were open.
    pub(crate) fn lock_all(&self) -> usize {
        let now = self.now();
        self.state.lock().map_or(0, |mut state| {
            let open = state
                .windows
                .values()
                .filter(|window| window.expires_at > now)
                .count();
            state.windows.clear();
            open
        })
    }

    pub(crate) fn status(&self, token: Option<&str>) -> Value {
        let window = self.active_window(token);
        let now = self.now();
        let (prompting, recent) = self.state.lock().map_or((false, 0), |state| {
            let period = self
                .rate_limit
                .read()
                .ok()
                .and_then(|limit| limit.map(|(_, period)| period))
                .unwrap_or(RATE_LIMIT_PERIOD);
            (
                state.prompting,
                state
                    .recent_prompts
                    .iter()
                    .filter(|at| now.duration_since(**at) < period)
                    .count(),
            )
        });
        let limit = self
            .rate_limit
            .read()
            .ok()
            .and_then(|limit| *limit)
            .map(|(max, _)| max);
        json!({
            "provider": self.provider_name(),
            "window": window.map(|window| json!({
                "open": true,
                "client": window.client.as_str(),
                "expires_at": window.expires_at.to_rfc3339(),
            })).unwrap_or_else(|| json!({ "open": false })),
            "prompt_in_progress": prompting,
            "prompts_remaining": limit.map(|max| max.saturating_sub(recent)),
            "window_seconds": OWNER_WINDOW_DURATION.as_secs(),
        })
    }
}

pub(crate) fn random_hex(bytes: usize) -> Result<String> {
    let mut buffer = vec![0_u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(|error| anyhow!("read randomness for an owner window token: {error}"))?;
    Ok(super::hex_encode(&buffer))
}

/// Reduces an agent-controlled label to `[A-Za-z0-9._-]`, at most 32 chars.
pub(crate) fn sanitize_label(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

/// Like [`sanitize_label`] for public ids, which can be longer.
pub(crate) fn sanitize_id(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(outcomes: Vec<PresenceOutcome>) -> OwnerPresence {
        OwnerPresence::new(Arc::new(FixtureProvider::from_outcomes(outcomes)))
    }

    fn confirm() -> PresenceOutcome {
        PresenceOutcome::Confirmed {
            method: PresenceMethod::Fixture,
        }
    }

    #[test]
    fn confirm_opens_a_window_bound_to_its_token() {
        let presence = presence(vec![confirm()]);
        let method = presence.prompt("unlock").unwrap();
        let window = presence
            .open_window(OwnerClient::Portal, "opr_1", "unlock", method)
            .unwrap();
        assert!(window.token.starts_with(WINDOW_TOKEN_PREFIX));
        assert_eq!(window.token.len(), WINDOW_TOKEN_PREFIX.len() + 64);
        let active = presence.active_window(Some(&window.token)).unwrap();
        assert_eq!(active.client, OwnerClient::Portal);
        assert_eq!(active.record_id, "opr_1");
        assert!(presence.active_window(Some("hubu_window_forged")).is_none());
        assert!(presence.active_window(None).is_none());
    }

    #[test]
    fn deny_timeout_and_unavailable_are_refusals() {
        let presence = presence(vec![
            PresenceOutcome::Denied,
            PresenceOutcome::TimedOut,
            PresenceOutcome::Unavailable("no gui".to_string()),
        ]);
        assert_eq!(presence.prompt("a"), Err(PromptRefusal::Denied));
        assert_eq!(presence.prompt("a"), Err(PromptRefusal::TimedOut));
        assert_eq!(
            presence.prompt("a"),
            Err(PromptRefusal::Unavailable("no gui".to_string()))
        );
    }

    #[test]
    fn window_expires_after_five_minutes_without_sliding() {
        let presence = presence(vec![confirm()]);
        let window = presence
            .open_window(OwnerClient::Cli, "opr_1", "r", PresenceMethod::Fixture)
            .unwrap();
        presence.advance_clock(Duration::from_secs(299));
        assert!(presence.active_window(Some(&window.token)).is_some());
        presence.advance_clock(Duration::from_secs(1));
        assert!(presence.active_window(Some(&window.token)).is_none());
    }

    #[test]
    fn lock_closes_every_window() {
        let presence = presence(vec![confirm()]);
        let portal = presence
            .open_window(OwnerClient::Portal, "a", "r", PresenceMethod::Fixture)
            .unwrap();
        let cli = presence
            .open_window(OwnerClient::Cli, "b", "r", PresenceMethod::Fixture)
            .unwrap();
        assert_eq!(presence.lock_all(), 2);
        assert!(presence.active_window(Some(&portal.token)).is_none());
        assert!(presence.active_window(Some(&cli.token)).is_none());
    }

    #[test]
    fn a_new_window_replaces_the_clients_old_one() {
        let presence = presence(vec![confirm()]);
        let first = presence
            .open_window(OwnerClient::Portal, "a", "r", PresenceMethod::Fixture)
            .unwrap();
        let second = presence
            .open_window(OwnerClient::Portal, "b", "r", PresenceMethod::Fixture)
            .unwrap();
        assert!(presence.active_window(Some(&first.token)).is_none());
        assert_eq!(
            presence
                .active_window(Some(&second.token))
                .unwrap()
                .record_id,
            "b"
        );
    }

    #[test]
    fn rate_limit_allows_five_prompts_per_ten_minutes() {
        let presence = presence(vec![PresenceOutcome::Denied]);
        for _ in 0..5 {
            assert_eq!(presence.prompt("a"), Err(PromptRefusal::Denied));
        }
        let Err(PromptRefusal::RateLimited { retry_after }) = presence.prompt("a") else {
            panic!("sixth prompt should be rate limited");
        };
        assert!(retry_after <= RATE_LIMIT_PERIOD);
        presence.advance_clock(RATE_LIMIT_PERIOD);
        assert_eq!(presence.prompt("a"), Err(PromptRefusal::Denied));
    }

    #[test]
    fn unavailable_prompts_do_not_count_toward_the_limit() {
        let presence = presence(vec![PresenceOutcome::Unavailable("x".to_string())]);
        for _ in 0..10 {
            assert!(matches!(
                presence.prompt("a"),
                Err(PromptRefusal::Unavailable(_))
            ));
        }
    }

    struct BlockingProvider {
        entered: std::sync::Barrier,
        release: std::sync::Barrier,
    }

    impl PresenceProvider for BlockingProvider {
        fn confirm(&self, _request: &PresenceRequest) -> PresenceOutcome {
            self.entered.wait();
            self.release.wait();
            PresenceOutcome::Confirmed {
                method: PresenceMethod::Fixture,
            }
        }
        fn name(&self) -> &'static str {
            "blocking"
        }
    }

    #[test]
    fn a_second_prompt_while_one_is_on_screen_is_rejected() {
        let provider = Arc::new(BlockingProvider {
            entered: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        });
        let presence = Arc::new(OwnerPresence::new(provider.clone()));
        let first = {
            let presence = Arc::clone(&presence);
            std::thread::spawn(move || presence.prompt("first"))
        };
        provider.entered.wait();
        assert_eq!(presence.prompt("second"), Err(PromptRefusal::InProgress));
        provider.release.wait();
        assert_eq!(first.join().unwrap(), Ok(PresenceMethod::Fixture));
        assert!(!presence.state.lock().unwrap().prompting);
    }

    #[test]
    fn fixture_script_parses_and_repeats_its_last_answer() {
        let fixture = FixtureProvider::parse("deny, confirm").unwrap();
        let request = PresenceRequest {
            reason: "r".to_string(),
            timeout: PROMPT_TIMEOUT,
        };
        assert_eq!(fixture.confirm(&request), PresenceOutcome::Denied);
        assert_eq!(fixture.confirm(&request), confirm());
        assert_eq!(fixture.confirm(&request), confirm());
        assert!(FixtureProvider::parse("approve").is_err());
    }

    #[test]
    fn fixture_is_refused_inside_a_managed_profile() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("stack.toml"), "").unwrap();
        std::fs::write(root.path().join("credentials.toml"), "").unwrap();
        std::fs::create_dir_all(root.path().join("state/hubu")).unwrap();
        assert!(is_inside_managed_profile(
            &root.path().join("state/hubu/hubu.sqlite3")
        ));
        let other = tempfile::tempdir().unwrap();
        assert!(!is_inside_managed_profile(
            &other.path().join("hubu.sqlite3")
        ));
    }

    #[test]
    fn labels_are_reduced_to_a_safe_alphabet() {
        assert_eq!(sanitize_label("design-agent"), "design-agent");
        assert_eq!(
            sanitize_label("evil\"\n approve $9999 now"),
            "evilapprove9999now"
        );
        assert_eq!(sanitize_label(&"a".repeat(80)).len(), 32);
        assert_eq!(sanitize_label("dеsign"), "dsign"); // Cyrillic е removed
        assert_eq!(sanitize_label("\u{202e}"), "unnamed");
    }
}
