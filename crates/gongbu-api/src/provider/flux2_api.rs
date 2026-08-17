//! Black Forest Labs FLUX.2 asynchronous image-generation adapter.
//!
//! A generation is submitted exactly once. If it is asynchronous, the returned
//! operation URL is polled under the same overall deadline; polling never
//! resubmits the generation. Returned bytes remain untrusted until the shared
//! artifact service validates and stores them.

use super::{
    contract::{
        canonical_image_media_type, AdapterCapabilities, AdapterOutcome, ContractError,
        NormalizedArtifact, NormalizedRequest, NormalizedUsage, ProviderAdapter, ProviderFailure,
        ProviderPhase, Result, RetryPolicy,
    },
    http_kernel::{
        provider_request_id, read_bounded, shared_client, validate_https_origin,
        ArtifactDownloadPolicy, CredentialForwarding, InvocationDeadline,
    },
    targets::{valid_artifact_hosts, Flux2ApiConfig, ProviderConfigVersion},
};
use crate::{redaction::Redactor, secrets::ProviderSecret};
use reqwest::{
    header::{HeaderName, HeaderValue},
    Url,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, error::Error as StdError, fmt, io::Read, thread, time::Duration};

pub const PROVIDER_ID: &str = "flux";
pub const ADAPTER_ID: &str = "flux2_api";
#[cfg(test)]
const MAX_ARTIFACT_BYTES: usize = crate::artifact::DEFAULT_MAX_ENCODED_BYTES as usize;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct TransportResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: Value,
}

pub trait Flux2Transport: Send + Sync {
    fn submit(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
        idempotency: Option<(&str, &str)>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;
    fn poll(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>>;
}

impl<T: Flux2Transport + ?Sized> Flux2Transport for std::sync::Arc<T> {
    fn submit(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
        idempotency: Option<(&str, &str)>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref()
            .submit(url, credential, timeout, headers, idempotency, body)
    }
    fn poll(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref().poll(url, credential, timeout, headers)
    }
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        self.as_ref().fetch_artifact(url, timeout, max_bytes)
    }
}

#[derive(Debug)]
enum HttpFailure {
    BeforeSend,
    UnknownOutcome,
}
impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FLUX HTTP failure")
    }
}
impl StdError for HttpFailure {}

pub struct ReqwestFlux2Transport;
impl ReqwestFlux2Transport {
    fn response(
        mut response: reqwest::blocking::Response,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let status = response.status().as_u16();
        let request_id =
            provider_request_id(response.headers(), &["x-request-id", "x-bfl-request-id"]);
        if (400..500).contains(&status) {
            return Ok(TransportResponse {
                status,
                request_id,
                body: Value::Null,
            });
        }
        let bytes =
            read_provider_response_bounded(&mut response, MAX_RESPONSE_BYTES).map_err(|_| {
                Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
            })?;
        let body = serde_json::from_slice(&bytes).map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
        })?;
        Ok(TransportResponse {
            status,
            request_id,
            body,
        })
    }
}
impl Flux2Transport for ReqwestFlux2Transport {
    fn submit(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
        idempotency: Option<(&str, &str)>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let credential = std::str::from_utf8(credential)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .post(url.clone())
            .timeout(timeout)
            .header("x-key", credential)
            .json(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some((name, value)) = idempotency {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
        })?;
        Self::response(response)
    }
    fn poll(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let credential = std::str::from_utf8(credential)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .get(url.clone())
            .timeout(timeout)
            .header("x-key", credential);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Self::response(request.send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
        })?)
    }
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        let mut response = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .get(url.clone())
            .timeout(timeout)
            .send()?;
        if !response.status().is_success() {
            return Err(Box::new(HttpFailure::UnknownOutcome));
        }
        read_artifact_response_bounded(&mut response, max_bytes)
    }
}

fn read_provider_response_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    read_bounded(reader, limit).map_err(|error| Box::new(error) as _)
}

fn read_artifact_response_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    read_bounded(reader, limit).map_err(|error| Box::new(error) as _)
}

pub struct Flux2ApiAdapter<T = ReqwestFlux2Transport> {
    config: Flux2ApiConfig,
    model: String,
    transport: T,
    max_artifact_bytes: usize,
}
impl Flux2ApiAdapter<ReqwestFlux2Transport> {
    pub fn from_target(target: &ProviderConfigVersion) -> Result<Self> {
        Self::from_target_with_artifact_limit(target, crate::artifact::DEFAULT_MAX_ENCODED_BYTES)
    }

    pub fn from_target_with_artifact_limit(
        target: &ProviderConfigVersion,
        max_artifact_bytes: u64,
    ) -> Result<Self> {
        if target.provider != PROVIDER_ID || target.adapter != ADAPTER_ID {
            return Err(provider_error("target_mismatch"));
        }
        Self::new_with_artifact_limit(
            target
                .flux2_api()
                .cloned()
                .ok_or_else(|| provider_error("config_invalid"))?,
            target.model.clone(),
            ReqwestFlux2Transport,
            max_artifact_bytes,
        )
    }
}
impl<T: Flux2Transport> Flux2ApiAdapter<T> {
    pub fn new(config: Flux2ApiConfig, model: String, transport: T) -> Result<Self> {
        Self::new_with_artifact_limit(
            config,
            model,
            transport,
            crate::artifact::DEFAULT_MAX_ENCODED_BYTES,
        )
    }

    pub fn new_with_artifact_limit(
        config: Flux2ApiConfig,
        model: String,
        transport: T,
        max_artifact_bytes: u64,
    ) -> Result<Self> {
        RetryPolicy {
            max_retries: config.max_retries,
        }
        .validate(AdapterCapabilities {
            // Header support is not evidence that the vendor enforces idempotency.
            vendor_enforced_idempotency: false,
        })?;
        if model.trim().is_empty()
            || config.timeout_ms == 0
            || config.poll_interval_ms == 0
            || config.max_retries != 0
            || !valid_artifact_hosts(&config.approved_artifact_hosts, true)
            || max_artifact_bytes == 0
            || max_artifact_bytes > usize::MAX as u64
        {
            return Err(provider_error("config_invalid"));
        }
        for (name, value) in &config.headers {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            HeaderValue::from_str(value).map_err(|_| provider_error("config_invalid"))?;
        }
        if let Some(name) = &config.idempotency_header {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            if name.eq_ignore_ascii_case("x-key")
                || name.eq_ignore_ascii_case("authorization")
                || config
                    .headers
                    .keys()
                    .any(|header| header.eq_ignore_ascii_case(name))
            {
                return Err(provider_error("config_invalid"));
            }
        }
        submit_url(&config, &model)?;
        Ok(Self {
            config,
            model,
            transport,
            max_artifact_bytes: max_artifact_bytes as usize,
        })
    }

    fn invoke_inner(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.preflight_input(request, input)
            .map_err(|_| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty() && p.len() <= 32_000)
            .ok_or_else(|| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let mut body = json!({"prompt": prompt});
        if let Some(size) = &request.image_size {
            body["image_size"] = json!(size);
        }
        let options = input.get("options").and_then(Value::as_object);
        for field in [
            "width",
            "height",
            "seed",
            "safety_tolerance",
            "output_format",
        ] {
            if let Some(value) = options.and_then(|options| options.get(field)) {
                body[field] = value.clone();
            }
        }
        let deadline =
            InvocationDeadline::from_timeout(Duration::from_millis(self.config.timeout_ms))
                .map_err(|_| ProviderFailure::release("config_invalid", ProviderPhase::PreSend))?;
        let idempotency = match (&self.config.idempotency_header, idempotency_key) {
            (Some(name), Some(key)) => Some((name.as_str(), key)),
            _ => None,
        };
        let response = self
            .transport
            .submit(
                &submit_url(&self.config, &self.model).map_err(|_| {
                    ProviderFailure::release("config_invalid", ProviderPhase::PreSend)
                })?,
                secret.expose(),
                deadline.remaining().map_err(|_| {
                    ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
                })?,
                &self.config.headers,
                idempotency,
                &body,
            )
            .map_err(|error| classify_transport(error, secret, true, None, None))?;
        let request_id = response
            .request_id
            .or_else(|| string_at(&response.body, &["request_id", "requestId"]));
        if (400..500).contains(&response.status) {
            return Err(ProviderFailure::release(
                "provider_rejected",
                ProviderPhase::Submission,
            ));
        }
        if !(200..300).contains(&response.status) {
            return Err(with_evidence("provider_failure", request_id, None));
        }
        let operation_id = string_at(&response.body, &["id", "operation_id", "operationId"]);
        let mut current = response.body;
        if is_failed(&current) {
            return Err(
                ProviderFailure::release("provider_rejected", ProviderPhase::Processing)
                    .with_evidence(request_id, operation_id),
            );
        }
        if !is_ready(&current) {
            let operation_id = operation_id
                .clone()
                .ok_or_else(|| with_evidence("malformed_response", request_id.clone(), None))?;
            let poll_url = poll_url(&self.config, &current, &operation_id).map_err(|error| {
                let code = match error {
                    ContractError::Provider { code } => code,
                    _ => "provider_contract_failure".into(),
                };
                ProviderFailure::reconcile(code, ProviderPhase::Processing)
                    .with_evidence(request_id.clone(), Some(operation_id.clone()))
            })?;
            loop {
                let wait = Duration::from_millis(self.config.poll_interval_ms).min(
                    deadline.remaining().map_err(|_| {
                        with_evidence(
                            "timeout_unknown_outcome",
                            request_id.clone(),
                            Some(operation_id.clone()),
                        )
                    })?,
                );
                thread::sleep(wait);
                let response = self
                    .transport
                    .poll(
                        &poll_url,
                        secret.expose(),
                        deadline.remaining().map_err(|_| {
                            with_evidence(
                                "timeout_unknown_outcome",
                                request_id.clone(),
                                Some(operation_id.clone()),
                            )
                        })?,
                        &self.config.headers,
                    )
                    .map_err(|error| {
                        classify_transport(
                            error,
                            secret,
                            false,
                            request_id.clone(),
                            Some(operation_id.clone()),
                        )
                    })?;
                if (400..500).contains(&response.status) {
                    return Err(with_evidence(
                        "provider_rejected",
                        request_id,
                        Some(operation_id),
                    ));
                }
                if !(200..300).contains(&response.status) {
                    return Err(with_evidence(
                        "provider_failure",
                        request_id,
                        Some(operation_id),
                    ));
                }
                current = response.body;
                if is_failed(&current) {
                    return Err(ProviderFailure::release(
                        "provider_rejected",
                        ProviderPhase::Processing,
                    )
                    .with_evidence(request_id, Some(operation_id)));
                }
                if is_ready(&current) {
                    break;
                }
            }
        }
        let operation_id =
            operation_id.or_else(|| string_at(&current, &["id", "operation_id", "operationId"]));
        let artifact_url = artifact_url(&current).ok_or_else(|| {
            with_evidence(
                if current.get("result").is_some() {
                    "missing_image"
                } else {
                    "malformed_response"
                },
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        let artifact_url = Url::parse(artifact_url).map_err(|_| {
            with_evidence(
                "artifact_policy_failure",
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        let artifact_policy = ArtifactDownloadPolicy::new(
            &self.config.approved_artifact_hosts,
            self.max_artifact_bytes as u64,
            CredentialForwarding::Prohibited,
        )
        .map_err(|_| {
            with_evidence(
                "artifact_policy_failure",
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        if artifact_policy.validate_url(&artifact_url).is_err() {
            return Err(with_evidence(
                "artifact_policy_failure",
                request_id,
                operation_id,
            ));
        }
        let bytes = self
            .transport
            .fetch_artifact(
                &artifact_url,
                deadline.remaining().map_err(|_| {
                    with_evidence(
                        "artifact_policy_failure",
                        request_id.clone(),
                        operation_id.clone(),
                    )
                })?,
                self.max_artifact_bytes,
            )
            .map_err(|error| {
                let _ = Redactor::new([secret.expose()]).error_chain(error.as_ref());
                with_evidence(
                    "artifact_policy_failure",
                    request_id.clone(),
                    operation_id.clone(),
                )
            })?;
        if bytes.len() > self.max_artifact_bytes {
            return Err(with_evidence(
                "artifact_policy_failure",
                request_id,
                operation_id,
            ));
        }
        let media_type = canonical_image_media_type(None, &bytes).map_err(|_| {
            with_evidence(
                "artifact_policy_failure",
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        let provider_amount_minor = current
            .pointer("/result/cost")
            .and_then(Value::as_f64)
            .and_then(|dollars| {
                if dollars.is_finite() && dollars >= 0.0 {
                    Some((dollars * 100.0).round() as i64)
                } else {
                    None
                }
            });
        Ok(AdapterOutcome {
            usage: Some(NormalizedUsage {
                images: Some(1),
                ..Default::default()
            }),
            provider_amount_minor,
            provider_currency: provider_amount_minor.map(|_| "USD".into()),
            provider_request_id: request_id,
            provider_operation_id: operation_id,
            artifacts: vec![NormalizedArtifact {
                media_type: media_type.into(),
                bytes,
            }],
        })
    }
}

impl<T: Flux2Transport> ProviderAdapter for Flux2ApiAdapter<T> {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: false,
        }
    }
    fn validate_request(&self, request: &NormalizedRequest) -> Result<()> {
        request.validate()?;
        if request.provider != PROVIDER_ID
            || request.model != self.model
            || request.image_count != Some(1)
        {
            return Err(provider_error("invalid_request"));
        }
        Ok(())
    }
    fn preflight_input(&self, request: &NormalizedRequest, input: &Value) -> Result<()> {
        self.validate_request(request)?;
        crate::provider_contract::validate_image_input(request, input)?;
        validate_flux_options(input)
    }
    fn invoke(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        if vendor_idempotency_key.is_some() != self.config.idempotency_header.is_some() {
            return Err(ProviderFailure::release(
                "idempotency_policy_mismatch",
                ProviderPhase::PreSend,
            ));
        }
        self.invoke_inner(request, input, secret, vendor_idempotency_key)
    }
    fn redact_error(&self, error: &(dyn StdError + 'static)) -> ContractError {
        let _ = Redactor::default().error_chain(error);
        provider_error("provider_failure")
    }
}

fn validate_flux_options(input: &Value) -> Result<()> {
    let Some(options) = input.get("options") else {
        return Ok(());
    };
    let options = options
        .as_object()
        .ok_or_else(|| provider_error("invalid_request"))?;
    if options.keys().any(|key| {
        !matches!(
            key.as_str(),
            "width" | "height" | "seed" | "safety_tolerance" | "output_format"
        )
    }) {
        return Err(provider_error("invalid_request"));
    }
    for dimension in ["width", "height"] {
        if options
            .get(dimension)
            .is_some_and(|value| !matches!(value.as_u64(), Some(64..=4096)))
        {
            return Err(provider_error("invalid_request"));
        }
    }
    if options
        .get("seed")
        .is_some_and(|value| value.as_u64().is_none())
        || options
            .get("safety_tolerance")
            .is_some_and(|value| !matches!(value.as_u64(), Some(0..=6)))
        || options
            .get("output_format")
            .is_some_and(|value| !matches!(value.as_str(), Some("png" | "jpeg")))
    {
        return Err(provider_error("invalid_request"));
    }
    Ok(())
}

fn submit_url(config: &Flux2ApiConfig, model: &str) -> Result<Url> {
    let mut url = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
    if validate_https_origin(&url, None).is_err() {
        return Err(provider_error("config_invalid"));
    }
    url.set_path(&format!(
        "{}/{}",
        config.api_version.trim_matches('/'),
        model
    ));
    Ok(url)
}
fn poll_url(config: &Flux2ApiConfig, body: &Value, operation_id: &str) -> Result<Url> {
    let url = if let Some(value) = string_at(body, &["polling_url", "pollingUrl"]) {
        Url::parse(&value).map_err(|_| provider_error("malformed_response"))?
    } else {
        let mut url = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
        url.set_path(&format!(
            "{}/get_result",
            config.api_version.trim_matches('/')
        ));
        url.query_pairs_mut().append_pair("id", operation_id);
        url
    };
    let base = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
    if url.scheme() != "https" || url.host_str() != base.host_str() {
        return Err(provider_error("artifact_policy_failure"));
    }
    Ok(url)
}
fn string_at(body: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| body.get(*name)?.as_str().map(str::to_owned))
}
fn status(body: &Value) -> Option<String> {
    body.get("status")
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_lowercase())
}
fn artifact_url(body: &Value) -> Option<&str> {
    body.pointer("/result/sample")
        .or_else(|| body.pointer("/result/image/url"))
        .or_else(|| body.get("sample"))
        .and_then(Value::as_str)
}
fn is_ready(body: &Value) -> bool {
    matches!(
        status(body).as_deref(),
        Some("ready" | "succeeded" | "completed")
    ) || artifact_url(body).is_some()
}
fn is_failed(body: &Value) -> bool {
    matches!(
        status(body).as_deref(),
        Some("error" | "failed" | "rejected" | "content moderated")
    )
}
fn provider_error(code: &str) -> ContractError {
    ContractError::Provider { code: code.into() }
}
fn with_evidence(
    code: &str,
    request_id: Option<String>,
    operation_id: Option<String>,
) -> ProviderFailure {
    ProviderFailure::reconcile(code, ProviderPhase::Processing)
        .with_evidence(request_id, operation_id)
}
fn classify_transport(
    error: Box<dyn StdError + Send + Sync>,
    secret: &ProviderSecret,
    submission: bool,
    request_id: Option<String>,
    operation_id: Option<String>,
) -> ProviderFailure {
    let _ = Redactor::new([secret.expose()]).error_chain(error.as_ref());
    match error.downcast_ref::<HttpFailure>() {
        Some(HttpFailure::BeforeSend) if submission => {
            ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
        }
        _ if submission => with_evidence("timeout_unknown_outcome", request_id, operation_id),
        _ => with_evidence("timeout_unknown_outcome", request_id, operation_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::conformance::{
            assert_adapter_conformance, assert_body_and_artifact_bounds, assert_redirect_blocked,
            Case, Observation,
        },
        provider_contract::SpendDisposition,
        secrets::secret_for_test,
    };
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use std::{
        io::Cursor,
        sync::{Arc, Mutex},
    };

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
            .unwrap();
        bytes
    }

    #[derive(Clone)]
    struct Fixture {
        submit: Arc<Mutex<u32>>,
        polls: Arc<Mutex<Vec<Value>>>,
        artifact: Vec<u8>,
        fail_submit: Option<String>,
        fail_poll: Option<String>,
        submit_body: Option<Value>,
        expected_options: Option<Value>,
    }
    impl Flux2Transport for Fixture {
        fn submit(
            &self,
            _: &Url,
            credential: &[u8],
            _: Duration,
            _: &BTreeMap<String, String>,
            idempotency: Option<(&str, &str)>,
            body: &Value,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            assert_eq!(credential, b"secret-canary");
            assert_eq!(idempotency, Some(("x-idempotency-key", "opaque-key")));
            assert_eq!(body["prompt"], "cat");
            if let Some(expected) = &self.expected_options {
                for field in [
                    "width",
                    "height",
                    "seed",
                    "safety_tolerance",
                    "output_format",
                ] {
                    assert_eq!(body.get(field), expected.get(field));
                }
            }
            *self.submit.lock().unwrap() += 1;
            if self.fail_submit.is_some() {
                return Err(Box::new(HttpFailure::UnknownOutcome));
            }
            Ok(TransportResponse {
                status: 202,
                request_id: Some("req-1".into()),
                body: self.submit_body.clone().unwrap_or_else(|| json!({"id":"op-1","polling_url":"https://api.bfl.ai/v1/get_result?id=op-1","status":"Pending"})),
            })
        }
        fn poll(
            &self,
            _: &Url,
            _: &[u8],
            _: Duration,
            _: &BTreeMap<String, String>,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            if self.fail_poll.is_some() {
                return Err(Box::new(HttpFailure::UnknownOutcome));
            }
            let body = self.polls.lock().unwrap().remove(0);
            Ok(TransportResponse {
                status: body
                    .get("_http_status")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(200),
                request_id: Some("poll-1".into()),
                body,
            })
        }
        fn fetch_artifact(
            &self,
            _: &Url,
            _: Duration,
            _: usize,
        ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
            Ok(self.artifact.clone())
        }
    }
    fn config() -> Flux2ApiConfig {
        Flux2ApiConfig {
            endpoint: "https://api.bfl.ai".into(),
            api_version: "v1".into(),
            timeout_ms: 1000,
            poll_interval_ms: 1,
            max_retries: 0,
            idempotency_header: Some("x-idempotency-key".into()),
            approved_artifact_hosts: vec!["cdn.bfl.ai".into()],
            headers: BTreeMap::new(),
        }
    }
    fn request() -> NormalizedRequest {
        NormalizedRequest {
            provider: PROVIDER_ID.into(),
            model: "flux-2-pro".into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: None,
        }
    }
    fn fixture(polls: Vec<Value>) -> (Flux2ApiAdapter<Fixture>, Arc<Mutex<u32>>) {
        let submit = Arc::new(Mutex::new(0));
        (
            Flux2ApiAdapter::new(
                config(),
                "flux-2-pro".into(),
                Fixture {
                    submit: submit.clone(),
                    polls: Arc::new(Mutex::new(polls)),
                    artifact: png(),
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: None,
                    expected_options: None,
                },
            )
            .unwrap(),
            submit,
        )
    }
    fn code(error: ProviderFailure) -> String {
        error.code
    }

    #[test]
    fn cross_adapter_conformance_matrix() {
        assert_body_and_artifact_bounds(
            |reader, limit| read_provider_response_bounded(reader, limit).is_err(),
            |reader, limit| read_artifact_response_bounded(reader, limit).is_err(),
        );
        assert_adapter_conformance(|case| {
            let (adapter, calls) = match case {
                Case::Rejection => fixture(vec![json!({"id":"op-1","status":"Rejected"})]),
                Case::AmbiguousPostSend => {
                    let calls = Arc::new(Mutex::new(0));
                    (
                        Flux2ApiAdapter::new(
                            config(),
                            "flux-2-pro".into(),
                            Fixture {
                                submit: calls.clone(),
                                polls: Arc::new(Mutex::new(vec![])),
                                artifact: Vec::new(),
                                fail_submit: Some("secret-canary".into()),
                                fail_poll: None,
                                submit_body: None,
                                expected_options: None,
                            },
                        )
                        .unwrap(),
                        calls,
                    )
                }
                Case::EvidenceRetention => {
                    let (mut adapter, calls) = fixture(vec![
                        json!({"id":"op-1","status":"Ready","result":{"sample":"https://cdn.bfl.ai/out.png"}}),
                    ]);
                    adapter.transport.artifact = vec![0];
                    (adapter, calls)
                }
                Case::HostPolicy => fixture(vec![
                    json!({"id":"op-1","status":"Ready","result":{"sample":"https://evil.example/out.png"}}),
                ]),
                Case::ArtifactBound => {
                    let (mut adapter, calls) = fixture(vec![
                        json!({"id":"op-1","status":"Ready","result":{"sample":"https://cdn.bfl.ai/out.png"}}),
                    ]);
                    adapter.transport.artifact = vec![0; MAX_ARTIFACT_BYTES + 1];
                    (adapter, calls)
                }
                Case::UnsafeRetry => fixture(Vec::new()),
                Case::InvalidRequest => fixture(Vec::new()),
            };
            let mut conformance_request = request();
            if matches!(case, Case::InvalidRequest) {
                conformance_request.image_count = Some(2);
            }
            let result = adapter.invoke(
                &conformance_request,
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                (!matches!(case, Case::UnsafeRetry)).then_some("opaque-key"),
            );
            let submissions = *calls.lock().unwrap();
            Observation {
                result,
                submissions,
            }
        });
    }

    #[test]
    fn conformance_artifact_redirect_is_blocked() {
        assert_redirect_blocked(|url| {
            ReqwestFlux2Transport
                .fetch_artifact(url, Duration::from_secs(2), MAX_ARTIFACT_BYTES)
                .is_err()
        });
    }

    #[test]
    fn submits_once_polls_same_operation_and_normalizes_missing_cost() {
        let (adapter, submits) = fixture(vec![
            json!({"id":"op-1","status":"Pending"}),
            json!({"id":"op-1","status":"Ready","result":{"sample":"https://cdn.bfl.ai/out.png"}}),
        ]);
        assert!(!adapter.capabilities().vendor_enforced_idempotency);
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        assert_eq!(*submits.lock().unwrap(), 1);
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("op-1"));
        assert_eq!(outcome.provider_request_id.as_deref(), Some("req-1"));
        assert_eq!(outcome.provider_amount_minor, None);
        assert_eq!(outcome.usage.unwrap().images, Some(1));
        assert_eq!(outcome.artifacts[0].bytes, png());
    }

    #[test]
    fn synchronous_readiness_matches_every_supported_artifact_result_shape() {
        for body in [
            json!({"id":"op-1","result":{"sample":"https://cdn.bfl.ai/out.png"}}),
            json!({"id":"op-1","result":{"image":{"url":"https://cdn.bfl.ai/out.png"}}}),
            json!({"id":"op-1","sample":"https://cdn.bfl.ai/out.png"}),
        ] {
            let submits = Arc::new(Mutex::new(0));
            let adapter = Flux2ApiAdapter::new(
                config(),
                "flux-2-pro".into(),
                Fixture {
                    submit: submits.clone(),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: png(),
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: Some(body),
                    expected_options: None,
                },
            )
            .unwrap();
            let outcome = adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat"}),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap();
            assert_eq!(outcome.artifacts[0].media_type, "image/png");
            assert_eq!(*submits.lock().unwrap(), 1);
        }
    }

    #[test]
    fn invalid_artifact_allowlist_rejects_adapter_before_submission() {
        for hosts in [vec![], vec!["https://cdn.bfl.ai".into()]] {
            let submits = Arc::new(Mutex::new(0));
            let mut invalid = config();
            invalid.approved_artifact_hosts = hosts;
            assert!(Flux2ApiAdapter::new(
                invalid,
                "flux-2-pro".into(),
                Fixture {
                    submit: submits.clone(),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: png(),
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: None,
                    expected_options: None,
                }
            )
            .is_err());
            assert_eq!(*submits.lock().unwrap(), 0);
        }
    }
    #[test]
    fn maps_v1_options_and_rejects_terminal_submission_without_polling() {
        let submits = Arc::new(Mutex::new(0));
        let options =
            json!({"width":1024,"height":768,"seed":42,"safety_tolerance":2,"output_format":"png"});
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: None,
                submit_body: Some(json!({"id":"op-rejected","status":"Rejected"})),
                expected_options: Some(options.clone()),
            },
        )
        .unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat","options":options}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(error.code, "provider_rejected");
        assert_eq!(error.evidence.operation_id.as_deref(), Some("op-rejected"));
        assert_eq!(*submits.lock().unwrap(), 1);
    }

    #[test]
    fn invalid_options_are_rejected_before_submission() {
        for options in [
            json!({"unknown":1}),
            json!({"width":63}),
            json!({"seed":-1}),
            json!({"safety_tolerance":7}),
            json!({"output_format":"gif"}),
        ] {
            let (adapter, submits) = fixture(Vec::new());
            assert!(adapter
                .preflight_input(&request(), &json!({"prompt":"cat","options":options}))
                .is_err());
            assert!(adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat","options":options}),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .is_err());
            assert_eq!(*submits.lock().unwrap(), 0);
        }
    }
    #[test]
    fn stable_rejection_malformed_missing_image_and_artifact_policy_failures() {
        for (body, expected) in [
            (
                json!({"id":"op-1","status":"Rejected"}),
                "provider_rejected",
            ),
            (json!({"id":"op-1","status":"Ready"}), "malformed_response"),
            (
                json!({"id":"op-1","status":"Ready","result":{}}),
                "missing_image",
            ),
            (
                json!({"id":"op-1","status":"Ready","result":{"sample":"https://evil.example/out.png"}}),
                "artifact_policy_failure",
            ),
        ] {
            let (adapter, submits) = fixture(vec![body]);
            let error = adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat"}),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(code(error), expected);
            assert_eq!(*submits.lock().unwrap(), 1);
        }
    }
    #[test]
    fn ambiguous_submission_and_poll_timeout_never_resubmit() {
        let submits = Arc::new(Mutex::new(0));
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: Some("secret-canary".into()),
                fail_poll: None,
                submit_body: None,
                expected_options: None,
            },
        )
        .unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(code(error), "timeout_unknown_outcome");
        assert_eq!(*submits.lock().unwrap(), 1);
        let submits = Arc::new(Mutex::new(0));
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: Some("nested secret-canary".into()),
                submit_body: None,
                expected_options: None,
            },
        )
        .unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "provider error (timeout_unknown_outcome)"
        );
        assert!(!error.to_string().contains("secret-canary"));
        assert_eq!(*submits.lock().unwrap(), 1);

        let (adapter, submits) = fixture(vec![json!({"_http_status":401})]);
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(error.code, "provider_rejected");
        assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
        assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
        assert_eq!(*submits.lock().unwrap(), 1);
    }
    #[test]
    fn rejects_invalid_request_and_retry_policy_before_network() {
        let (adapter, submits) = fixture(vec![]);
        let mut bad = request();
        bad.image_count = Some(2);
        assert_eq!(
            code(
                adapter
                    .invoke(
                        &bad,
                        &json!({"prompt":"cat"}),
                        &secret_for_test("secret-canary"),
                        Some("opaque-key")
                    )
                    .unwrap_err()
            ),
            "invalid_request"
        );
        assert_eq!(*submits.lock().unwrap(), 0);
        let mut invalid = config();
        invalid.max_retries = 1;
        assert!(Flux2ApiAdapter::new(
            invalid,
            "flux-2-pro".into(),
            Fixture {
                submit: submits,
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: None,
                submit_body: None,
                expected_options: None
            }
        )
        .is_err());

        for name in ["x-key", "Authorization", "X-STATIC"] {
            let mut invalid = config();
            invalid.idempotency_header = Some(name.into());
            invalid.headers.insert("x-static".into(), "operator".into());
            assert!(Flux2ApiAdapter::new(
                invalid,
                "flux-2-pro".into(),
                Fixture {
                    submit: Arc::new(Mutex::new(0)),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: vec![],
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: None,
                    expected_options: None,
                }
            )
            .is_err());
        }
    }

    #[test]
    fn fixture_output_uses_shared_artifact_policy_and_frozen_pricing_for_settlement() {
        use crate::{
            artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference, Repository},
            provider::contract::PricingCatalog,
        };
        use tempfile::tempdir;

        let png = png();
        let submits = Arc::new(Mutex::new(0));
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits,
                polls: Arc::new(Mutex::new(vec![json!({"id":"op-1","status":"Ready","result":{"sample":"https://cdn.bfl.ai/out.png"}})])),
                artifact: png.clone(),
                fail_submit: None,
                fail_poll: None,
                submit_body: None,
                expected_options: None,
            },
        )
        .unwrap();
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        let pricing = PricingCatalog::from_json(br#"{"schema_version":1,"catalog_version":"fixture-v1","rules":[{"rule_id":"flux2-pro","provider":"flux","model":"flux-2-pro","currency":"USD","unit":"image","unit_amount_minor":45}]}"#).unwrap();
        let snapshot = pricing.snapshot(&request()).unwrap();
        assert_eq!(snapshot.estimated_amount_minor, 45);
        assert_eq!(
            snapshot
                .settle(outcome.usage.as_ref().unwrap(), 45)
                .unwrap(),
            45
        );
        assert_eq!(outcome.provider_amount_minor, None);

        let repository = Repository::in_memory().unwrap();
        let execution = repository
            .create_execution(&CreateExecutionParams {
                account_id: "account".into(),
                operation_key: "flux-fixture".into(),
                hubu_authorization_id: "auth".into(),
                hubu_claim_id: Some("claim".into()),
                hubu_token_reference: HubuTokenReference::new("token-ref").unwrap(),
                authorized_minor: 45,
                authorization_currency: "USD".into(),
                normalized_input: json!({"prompt":"cat","image_count":1}),
                input_hash: "hash".into(),
                input_schema_version: 1,
                target: "flux/flux-2-pro".into(),
                config_version: "cfg".into(),
                workload_type: "image_generation".into(),
                provider: "flux".into(),
                adapter: "flux2_api".into(),
                model: "flux-2-pro".into(),
                provider_config_version: "pcv".into(),
                provider_config_digest: format!("sha256:{}", "a".repeat(64)),
                pricing_snapshot: serde_json::to_value(snapshot).unwrap(),
                pricing_schema_version: 1,
                created_at: "now".into(),
            })
            .unwrap();
        let root = tempdir().unwrap();
        let service = ArtifactService::new(
            repository,
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let artifact = service
            .store_image(
                &execution.execution_id,
                None,
                &outcome.artifacts[0].media_type,
                &outcome.artifacts[0].bytes,
                "now",
            )
            .unwrap();
        assert_eq!(
            service
                .retrieve_for_account(&artifact.artifact_id, "account")
                .unwrap()
                .bytes,
            png
        );
    }
}
