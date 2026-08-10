//! Ideogram synchronous image-generation adapter.
//!
//! Exactly one generation submission is made. Returned URLs are treated as
//! untrusted input and downloaded under a strict host, redirect, and size
//! boundary before Gongbu's artifact service performs canonical validation.

use super::{
    contract::{
        canonical_image_media_type, AdapterCapabilities, AdapterOutcome, ContractError,
        NormalizedArtifact, NormalizedRequest, NormalizedUsage, ProviderAdapter, ProviderFailure,
        ProviderPhase, Result, RetryPolicy,
    },
    targets::{valid_artifact_hosts, IdeogramImageConfig, ProviderConfigVersion},
};
use crate::{redaction::Redactor, secrets::ProviderSecret};
use reqwest::{
    blocking::Client,
    header::{HeaderName, HeaderValue},
    Url,
};
use serde_json::{json, Value};
use std::{error::Error as StdError, fmt, io::Read, time::Duration};

pub const PROVIDER_ID: &str = "ideogram";
pub const ADAPTER_ID: &str = "ideogram_image";
const MAX_ARTIFACT_BYTES: usize = 20 * 1024 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
enum HttpFailure {
    BeforeSend,
    UnknownOutcome {
        request_id: Option<String>,
        operation_id: Option<String>,
    },
}
impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Ideogram HTTP request failed")
    }
}
impl StdError for HttpFailure {}

#[derive(Clone, Debug)]
pub struct TransportResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
    pub body: Value,
}

pub trait IdeogramTransport: Send + Sync {
    fn generate(
        &self,
        url: &Url,
        api_key: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>>;
}

impl<T: IdeogramTransport + ?Sized> IdeogramTransport for std::sync::Arc<T> {
    fn generate(
        &self,
        url: &Url,
        api_key: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref().generate(url, api_key, timeout, headers, body)
    }
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        self.as_ref().fetch_artifact(url, timeout)
    }
}

pub struct ReqwestIdeogramTransport;
impl IdeogramTransport for ReqwestIdeogramTransport {
    fn generate(
        &self,
        url: &Url,
        api_key: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let api_key = std::str::from_utf8(api_key)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = client.post(url.clone()).header("Api-Key", api_key);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let form = reqwest::blocking::multipart::Form::new().text("prompt", prompt.to_owned());
        let mut response = request.multipart(form).send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome {
                request_id: None,
                operation_id: None,
            }) as Box<dyn StdError + Send + Sync>
        })?;
        let status = response.status().as_u16();
        let request_id = header(&response, &["x-request-id", "request-id"]);
        let operation_id = header(&response, &["x-operation-id", "generation-id"]);
        if (400..500).contains(&status) {
            return Ok(TransportResponse {
                status,
                request_id,
                operation_id,
                body: Value::Null,
            });
        }
        let bytes = read_provider_response_bounded(&mut response, MAX_PROVIDER_RESPONSE_BYTES)
            .map_err(|_| {
                Box::new(HttpFailure::UnknownOutcome {
                    request_id: request_id.clone(),
                    operation_id: operation_id.clone(),
                }) as Box<dyn StdError + Send + Sync>
            })?;
        let body = serde_json::from_slice(&bytes).map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome {
                request_id: request_id.clone(),
                operation_id: operation_id.clone(),
            }) as Box<dyn StdError + Send + Sync>
        })?;
        Ok(TransportResponse {
            status,
            request_id,
            operation_id,
            body,
        })
    }

    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        let mut response = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?
            .get(url.clone())
            .send()?;
        if !response.status().is_success() {
            return Err(Box::new(HttpFailure::UnknownOutcome {
                request_id: None,
                operation_id: None,
            }));
        }
        read_artifact_response_bounded(&mut response, MAX_ARTIFACT_BYTES)
    }
}

fn read_provider_response_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    read_bounded(reader, limit)
}

fn read_artifact_response_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    read_bounded(reader, limit)
}

fn read_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Box::new(HttpFailure::UnknownOutcome {
            request_id: None,
            operation_id: None,
        }));
    }
    Ok(bytes)
}

fn header(response: &reqwest::blocking::Response, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        response
            .headers()
            .get(*name)?
            .to_str()
            .ok()
            .map(str::to_owned)
    })
}

pub struct IdeogramImageAdapter<T = ReqwestIdeogramTransport> {
    config: IdeogramImageConfig,
    model: String,
    transport: T,
}

impl IdeogramImageAdapter<ReqwestIdeogramTransport> {
    pub fn from_target(target: &ProviderConfigVersion) -> Result<Self> {
        if target.provider != PROVIDER_ID || target.adapter != ADAPTER_ID {
            return Err(provider_error("target_mismatch"));
        }
        Self::new(
            target
                .ideogram_image()
                .cloned()
                .ok_or_else(|| provider_error("config_invalid"))?,
            target.model.clone(),
            ReqwestIdeogramTransport,
        )
    }
}

impl<T: IdeogramTransport> IdeogramImageAdapter<T> {
    pub fn new(config: IdeogramImageConfig, model: String, transport: T) -> Result<Self> {
        RetryPolicy {
            max_retries: config.max_retries,
        }
        .validate(AdapterCapabilities {
            vendor_enforced_idempotency: false,
        })?;
        if model.trim().is_empty()
            || config.timeout_ms == 0
            || !valid_artifact_hosts(&config.approved_artifact_hosts, true)
        {
            return Err(provider_error("config_invalid"));
        }
        for (name, value) in &config.headers {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            HeaderValue::from_str(value).map_err(|_| provider_error("config_invalid"))?;
            if name.eq_ignore_ascii_case("api-key") || name.eq_ignore_ascii_case("authorization") {
                return Err(provider_error("config_invalid"));
            }
        }
        endpoint_url(&config, &model)?;
        Ok(Self {
            config,
            model,
            transport,
        })
    }
}

impl<T: IdeogramTransport> ProviderAdapter for IdeogramImageAdapter<T> {
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
    fn invoke(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        if vendor_idempotency_key.is_some() || self.config.max_retries != 0 {
            return Err(ProviderFailure::release(
                "retry_not_supported",
                ProviderPhase::PreSend,
            ));
        }
        self.validate_request(request)
            .map_err(|_| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        crate::provider_contract::validate_image_size_input(request, input)
            .map_err(|_| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 32_000)
            .ok_or_else(|| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        if input.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| key != "prompt" && key != "image_count" && key != "image_size")
        }) {
            return Err(ProviderFailure::release(
                "invalid_request",
                ProviderPhase::PreSend,
            ));
        }
        let response = self
            .transport
            .generate(
                &endpoint_url(&self.config, &self.model).map_err(|_| {
                    ProviderFailure::release("config_invalid", ProviderPhase::PreSend)
                })?,
                secret.expose(),
                Duration::from_millis(self.config.timeout_ms),
                &self.config.headers,
                &match &request.image_size {
                    Some(size) => json!({"prompt": prompt, "image_size": size}),
                    None => json!({"prompt": prompt}),
                },
            )
            .map_err(|error| classify_transport(error, secret, false))?;
        let request_id = response
            .request_id
            .or_else(|| string_field(&response.body, "request_id"));
        let operation_id = response
            .operation_id
            .or_else(|| string_field(&response.body, "generation_id"));
        if !(200..300).contains(&response.status) {
            if (400..500).contains(&response.status) {
                return Err(ProviderFailure::release(
                    "provider_rejected",
                    ProviderPhase::Submission,
                ));
            }
            return Err(
                ProviderFailure::reconcile("provider_failure", ProviderPhase::Submission)
                    .with_evidence(request_id, operation_id),
            );
        }
        let data = response
            .body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| with_evidence("malformed_response", &request_id, &operation_id))?;
        if data.len() > 1 {
            return Err(with_evidence(
                "artifact_policy_failure",
                &request_id,
                &operation_id,
            ));
        }
        let image = data
            .first()
            .ok_or_else(|| with_evidence("missing_image", &request_id, &operation_id))?;
        let raw_url = image
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| with_evidence("missing_image", &request_id, &operation_id))?;
        let url = Url::parse(raw_url)
            .map_err(|_| with_evidence("artifact_policy_failure", &request_id, &operation_id))?;
        if url.scheme() != "https"
            || !self
                .config
                .approved_artifact_hosts
                .iter()
                .any(|host| Some(host.as_str()) == url.host_str())
        {
            return Err(with_evidence(
                "artifact_policy_failure",
                &request_id,
                &operation_id,
            ));
        }
        let bytes = self
            .transport
            .fetch_artifact(&url, Duration::from_millis(self.config.timeout_ms))
            .map_err(|error| {
                let _ = classify_transport(error, secret, true);
                with_evidence("artifact_policy_failure", &request_id, &operation_id)
            })?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(with_evidence(
                "artifact_policy_failure",
                &request_id,
                &operation_id,
            ));
        }
        let media_type = canonical_image_media_type(None, &bytes)
            .map_err(|_| with_evidence("artifact_policy_failure", &request_id, &operation_id))?;
        Ok(AdapterOutcome {
            usage: Some(NormalizedUsage {
                images: Some(1),
                input_tokens: None,
                output_tokens: None,
            }),
            provider_amount_minor: response
                .body
                .pointer("/usage/cost_minor")
                .and_then(Value::as_i64),
            provider_currency: response
                .body
                .pointer("/usage/currency")
                .and_then(Value::as_str)
                .map(str::to_owned),
            provider_request_id: request_id,
            provider_operation_id: operation_id,
            artifacts: vec![NormalizedArtifact {
                media_type: media_type.into(),
                bytes,
            }],
        })
    }
    fn redact_error(&self, error: &(dyn StdError + 'static)) -> ContractError {
        let _ = Redactor::default().error_chain(error);
        provider_error("provider_failure")
    }
}

fn endpoint_url(config: &IdeogramImageConfig, model: &str) -> Result<Url> {
    let mut base = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
    if base.scheme() != "https" || base.host_str().is_none() || base.query().is_some() {
        return Err(provider_error("config_invalid"));
    }
    base.set_path(&format!(
        "{}/{}/generate",
        config.api_version.trim_matches('/'),
        model.trim_matches('/')
    ));
    Ok(base)
}

fn string_field(body: &Value, field: &str) -> Option<String> {
    body.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn with_evidence(
    code: &str,
    request_id: &Option<String>,
    operation_id: &Option<String>,
) -> ProviderFailure {
    ProviderFailure::reconcile(code, ProviderPhase::Processing)
        .with_evidence(request_id.clone(), operation_id.clone())
}

fn classify_transport(
    error: Box<dyn StdError + Send + Sync>,
    secret: &ProviderSecret,
    artifact: bool,
) -> ProviderFailure {
    let redactor = Redactor::new([secret.expose()]);
    let _ = redactor.error_chain(error.as_ref());
    if artifact {
        return ProviderFailure::reconcile("artifact_policy_failure", ProviderPhase::Artifact);
    }
    match error.downcast_ref::<HttpFailure>() {
        Some(HttpFailure::BeforeSend) => {
            ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
        }
        Some(HttpFailure::UnknownOutcome {
            request_id,
            operation_id,
        }) => ProviderFailure::reconcile("timeout_unknown_outcome", ProviderPhase::Submission)
            .with_evidence(request_id.clone(), operation_id.clone()),
        None => ProviderFailure::reconcile("provider_failure", ProviderPhase::Submission),
    }
}

fn provider_error(code: &str) -> ContractError {
    ContractError::Provider { code: code.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::conformance::{
            assert_adapter_conformance, assert_body_and_artifact_bounds, assert_redirect_blocked,
            Case, Observation,
        },
        secrets::secret_for_test,
    };
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use std::{
        io::Cursor,
        sync::{Arc, Mutex},
    };

    #[derive(Clone)]
    struct FixtureTransport {
        response: Arc<Mutex<Option<std::result::Result<TransportResponse, String>>>>,
        calls: Arc<Mutex<u32>>,
        artifact: std::result::Result<Vec<u8>, String>,
    }
    impl IdeogramTransport for FixtureTransport {
        fn generate(
            &self,
            url: &Url,
            key: &[u8],
            _: Duration,
            _: &std::collections::BTreeMap<String, String>,
            body: &Value,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            assert_eq!(
                url.as_str(),
                "https://api.ideogram.ai/v1/ideogram-v3/generate"
            );
            assert_eq!(key, b"secret-canary");
            assert_eq!(body, &json!({"prompt":"cat"}));
            *self.calls.lock().unwrap() += 1;
            self.response
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .map_err(|message| {
                    if message == "timeout" {
                        Box::new(HttpFailure::UnknownOutcome {
                            request_id: None,
                            operation_id: None,
                        }) as Box<dyn StdError + Send + Sync>
                    } else {
                        Box::new(NestedError(message)) as Box<dyn StdError + Send + Sync>
                    }
                })
        }
        fn fetch_artifact(
            &self,
            _: &Url,
            _: Duration,
        ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
            self.artifact.clone().map_err(|message| {
                Box::new(NestedError(message)) as Box<dyn StdError + Send + Sync>
            })
        }
    }
    #[derive(Debug)]
    struct NestedError(String);
    impl fmt::Display for NestedError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl StdError for NestedError {}

    fn png() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::new(2, 3))
            .write_to(&mut bytes, ImageOutputFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    fn config() -> IdeogramImageConfig {
        IdeogramImageConfig {
            endpoint: "https://api.ideogram.ai".into(),
            api_version: "v1".into(),
            timeout_ms: 1000,
            max_retries: 0,
            approved_artifact_hosts: vec!["ideogram.ai".into()],
            headers: Default::default(),
        }
    }
    fn request() -> NormalizedRequest {
        NormalizedRequest {
            provider: "ideogram".into(),
            model: "ideogram-v3".into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: None,
        }
    }
    fn adapter(
        body: Value,
        status: u16,
        artifact: std::result::Result<Vec<u8>, String>,
    ) -> (IdeogramImageAdapter<FixtureTransport>, Arc<Mutex<u32>>) {
        let calls = Arc::new(Mutex::new(0));
        let transport = FixtureTransport {
            response: Arc::new(Mutex::new(Some(Ok(TransportResponse {
                status,
                request_id: Some("request-1".into()),
                operation_id: Some("generation-1".into()),
                body,
            })))),
            calls: calls.clone(),
            artifact,
        };
        (
            IdeogramImageAdapter::new(config(), "ideogram-v3".into(), transport).unwrap(),
            calls,
        )
    }

    #[test]
    fn cross_adapter_conformance_matrix() {
        assert_body_and_artifact_bounds(
            |reader, limit| read_provider_response_bounded(reader, limit).is_err(),
            |reader, limit| read_artifact_response_bounded(reader, limit).is_err(),
        );
        assert_adapter_conformance(|case| {
            let (adapter, calls) = match case {
                Case::Rejection => adapter(json!({}), 403, Ok(Vec::new())),
                Case::AmbiguousPostSend => {
                    let (adapter, calls) = adapter(json!({}), 200, Ok(Vec::new()));
                    *adapter.transport.response.lock().unwrap() = Some(Err("timeout".into()));
                    (adapter, calls)
                }
                Case::EvidenceRetention => adapter(
                    json!({"data":[{"url":"https://ideogram.ai/out.png"}]}),
                    200,
                    Err("secret-canary artifact error".into()),
                ),
                Case::HostPolicy => adapter(
                    json!({"data":[{"url":"https://evil.example/out.png"}]}),
                    200,
                    Ok(png()),
                ),
                Case::ArtifactBound => adapter(
                    json!({"data":[{"url":"https://ideogram.ai/out.png"}]}),
                    200,
                    Ok(vec![0; MAX_ARTIFACT_BYTES + 1]),
                ),
                Case::UnsafeRetry | Case::InvalidRequest => adapter(json!({}), 200, Ok(Vec::new())),
            };
            let mut conformance_request = request();
            if matches!(case, Case::InvalidRequest) {
                conformance_request.image_count = Some(2);
            }
            let result = adapter.invoke(
                &conformance_request,
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                matches!(case, Case::UnsafeRetry).then_some("unsafe-key"),
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
            ReqwestIdeogramTransport
                .fetch_artifact(url, Duration::from_secs(2))
                .is_err()
        });
    }

    #[test]
    fn fixture_happy_path_maps_request_and_accepts_missing_cost() {
        let (adapter, calls) = adapter(
            json!({"data":[{"url":"https://ideogram.ai/image.png"}]}),
            200,
            Ok(png()),
        );
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat","image_count":1}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap();
        assert_eq!(outcome.provider_request_id.as_deref(), Some("request-1"));
        assert_eq!(
            outcome.provider_operation_id.as_deref(),
            Some("generation-1")
        );
        assert_eq!(outcome.usage.unwrap().images, Some(1));
        assert_eq!(outcome.provider_amount_minor, None);
        assert_eq!(outcome.artifacts[0].bytes, png());
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn rejection_malformed_missing_image_and_artifact_policy_are_stable() {
        for (body, status, artifact, expected) in [
            (
                json!({"error":{"message":"secret-canary"}}),
                422,
                Ok(png()),
                "provider_rejected",
            ),
            (json!({"data":"bad"}), 200, Ok(png()), "malformed_response"),
            (json!({"data":[]}), 200, Ok(png()), "missing_image"),
            (
                json!({"data":[{"url":"https://evil.example/image.png"}]}),
                200,
                Ok(png()),
                "artifact_policy_failure",
            ),
            (
                json!({"data":[{"url":"https://ideogram.ai/image.png"}]}),
                200,
                Err("oversize".into()),
                "artifact_policy_failure",
            ),
        ] {
            let (adapter, calls) = adapter(body, status, artifact);
            let error = adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat"}),
                    &secret_for_test("secret-canary"),
                    None,
                )
                .unwrap_err();
            assert_eq!(error.code, expected);
            assert_eq!(*calls.lock().unwrap(), 1);
        }
    }

    #[test]
    fn artifact_fetch_failure_preserves_generation_evidence() {
        let (adapter, calls) = adapter(
            json!({"data":[{"url":"https://ideogram.ai/image.png"}]}),
            200,
            Err("oversize".into()),
        );
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "artifact_policy_failure");
        assert_eq!(error.evidence.request_id.as_deref(), Some("request-1"));
        assert_eq!(error.evidence.operation_id.as_deref(), Some("generation-1"));
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn production_transport_sends_prompt_as_multipart_form_data() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!(
            "http://{}/generate",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "request ended before its declared body");
                request.extend_from_slice(&buffer[..count]);
                let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .expect("multipart request declares content length");
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            let lowercase = request.to_ascii_lowercase();
            assert!(lowercase.contains("content-type: multipart/form-data; boundary="));
            assert!(request.contains("name=\"prompt\""));
            assert!(request.contains("cat"));
            assert!(!lowercase.contains("application/json"));
            write!(
                stream,
                "HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
        });
        let response = ReqwestIdeogramTransport
            .generate(
                &url,
                b"credential",
                Duration::from_secs(2),
                &Default::default(),
                &json!({"prompt":"cat"}),
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(response.status, 422);
    }

    #[test]
    fn ambiguous_timeout_is_submitted_once_and_redacted() {
        let calls = Arc::new(Mutex::new(0));
        let transport = FixtureTransport {
            response: Arc::new(Mutex::new(Some(Err("timeout".into())))),
            calls: calls.clone(),
            artifact: Ok(png()),
        };
        let timeout_adapter =
            IdeogramImageAdapter::new(config(), "ideogram-v3".into(), transport).unwrap();
        let error = timeout_adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "timeout_unknown_outcome");
        assert_eq!(*calls.lock().unwrap(), 1);

        let (adapter, calls) = adapter(json!({}), 200, Ok(png()));
        let transport_error =
            NestedError("outer: Api-Key secret-canary; nested provider payload".into());
        assert_eq!(
            adapter.redact_error(&transport_error).to_string(),
            "provider error (provider_failure)"
        );
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[test]
    fn unsupported_fields_and_retry_fail_before_network() {
        let (adapter, calls) = adapter(json!({}), 200, Ok(png()));
        assert!(adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat","seed":1}),
                &secret_for_test("secret-canary"),
                None
            )
            .is_err());
        assert!(adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("key")
            )
            .is_err());
        assert_eq!(*calls.lock().unwrap(), 0);
    }
}
