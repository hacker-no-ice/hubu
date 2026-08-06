//! Google Gemini image-generation adapter.
//!
//! The adapter performs one generation request. It never retries or falls back.
//! Returned bytes are still untrusted and must pass `ArtifactService::store_image`.

use super::{
    contract::{
        AdapterCapabilities, AdapterOutcome, ContractError, NormalizedArtifact, NormalizedRequest,
        NormalizedUsage, OutcomeKind, ProviderAdapter, Result, RetryPolicy,
    },
    targets::{GeminiImageConfig, ProviderConfigVersion},
};
use crate::{redaction::Redactor, secrets::ProviderSecret};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{
    blocking::Client,
    header::{HeaderName, HeaderValue},
    Url,
};
use serde_json::{json, Value};
use std::{error::Error as StdError, fmt, io::Read, time::Duration};

pub const PROVIDER_ID: &str = "google";
pub const ADAPTER_ID: &str = "gemini_image";
const MAX_ARTIFACT_BYTES: usize = 20 * 1024 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug)]
struct MessageError(String);
impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl StdError for MessageError {}

pub trait GeminiTransport: Send + Sync {
    fn generate(
        &self,
        url: &Url,
        bearer: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;

    fn fetch_artifact(
        &self,
        url: &Url,
        bearer: &[u8],
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>>;
}

impl<T: GeminiTransport + ?Sized> GeminiTransport for std::sync::Arc<T> {
    fn generate(
        &self,
        url: &Url,
        bearer: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref().generate(url, bearer, timeout, headers, body)
    }

    fn fetch_artifact(
        &self,
        url: &Url,
        bearer: &[u8],
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        self.as_ref().fetch_artifact(url, bearer, timeout)
    }
}

#[derive(Clone, Debug)]
pub struct TransportResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
    pub body: Value,
}

pub struct ReqwestGeminiTransport;
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
        f.write_str("Google HTTP request failed")
    }
}
impl StdError for HttpFailure {}

impl GeminiTransport for ReqwestGeminiTransport {
    fn generate(
        &self,
        url: &Url,
        bearer: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let token = std::str::from_utf8(bearer)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = client.post(url.clone()).bearer_auth(token);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        // Once `send` begins, a failure cannot prove that Google did not receive,
        // generate, or bill the request. Classify every such failure conservatively.
        let mut response = request.json(body).send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome {
                request_id: None,
                operation_id: None,
            }) as Box<dyn StdError + Send + Sync>
        })?;
        let status = response.status().as_u16();
        let request_id = header(&response, &["x-goog-request-id", "x-request-id"]);
        let operation_id = header(&response, &["x-goog-operation-id"]);
        if (400..500).contains(&status) {
            return Ok(TransportResponse {
                status,
                request_id,
                operation_id,
                body: Value::Null,
            });
        }
        let body_bytes =
            read_bounded(&mut response, MAX_PROVIDER_RESPONSE_BYTES).map_err(|_| {
                Box::new(HttpFailure::UnknownOutcome {
                    request_id: request_id.clone(),
                    operation_id: operation_id.clone(),
                }) as Box<dyn StdError + Send + Sync>
            })?;
        let body = serde_json::from_slice(&body_bytes).map_err(|_| {
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
        bearer: &[u8],
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        let token =
            std::str::from_utf8(bearer).map_err(|_| MessageError("credential encoding".into()))?;
        let mut response = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| MessageError("artifact client failure".into()))?
            .get(url.clone())
            .bearer_auth(token)
            .send()?;
        if !response.status().is_success() {
            return Err(Box::new(MessageError("artifact fetch rejected".into())));
        }
        read_bounded(&mut response, MAX_ARTIFACT_BYTES)
    }
}

fn read_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    reader.take((limit as u64) + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Box::new(MessageError(
            "provider body limit exceeded".into(),
        )));
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

pub struct GeminiImageAdapter<T = ReqwestGeminiTransport> {
    config: GeminiImageConfig,
    model: String,
    transport: T,
}

impl GeminiImageAdapter<ReqwestGeminiTransport> {
    pub fn from_target(target: &ProviderConfigVersion) -> Result<Self> {
        if target.provider != PROVIDER_ID || target.adapter != ADAPTER_ID {
            return Err(provider_error("target_mismatch"));
        }
        let config = target
            .gemini_image
            .clone()
            .ok_or_else(|| provider_error("config_invalid"))?;
        Self::new(config, target.model.clone(), ReqwestGeminiTransport)
    }
}

impl<T: GeminiTransport> GeminiImageAdapter<T> {
    pub fn new(config: GeminiImageConfig, model: String, transport: T) -> Result<Self> {
        RetryPolicy {
            max_retries: config.max_retries,
        }
        .validate(AdapterCapabilities {
            vendor_enforced_idempotency: false,
        })?;
        if model.trim().is_empty() || config.timeout_ms == 0 {
            return Err(provider_error("config_invalid"));
        }
        for (name, value) in &config.headers {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            HeaderValue::from_str(value).map_err(|_| provider_error("config_invalid"))?;
        }
        endpoint_url(&config, &model)?;
        Ok(Self {
            config,
            model,
            transport,
        })
    }

    fn invoke_inner(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
    ) -> Result<AdapterOutcome> {
        self.validate_request(request)?;
        if request.provider != PROVIDER_ID
            || request.model != self.model
            || request.image_count != Some(1)
        {
            return Err(provider_error("invalid_request"));
        }
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 32_000)
            .ok_or_else(|| provider_error("invalid_request"))?;
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": prompt}]}],
            "generationConfig": {"responseModalities": ["IMAGE"]}
        });
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let response = self
            .transport
            .generate(
                &endpoint_url(&self.config, &self.model)?,
                secret.expose(),
                timeout,
                &self.config.headers,
                &body,
            )
            .map_err(|error| classify_transport(error, secret, false))?;
        let request_id = response.request_id.or_else(|| {
            response
                .body
                .get("responseId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let operation_id = response.operation_id;
        if !(200..300).contains(&response.status) {
            if (400..500).contains(&response.status) {
                return Err(provider_error("provider_rejected"));
            }
            // Redirects and server errors occur after transmission and do not
            // prove that Google neither generated nor billed the request.
            return Err(ContractError::ProviderWithEvidence {
                code: "provider_failure".into(),
                request_id,
                operation_id,
            });
        }
        let artifacts = extract_artifacts(
            &response.body,
            &self.config,
            &self.transport,
            secret,
            timeout,
        )
        .map_err(|error| match error {
            ContractError::Provider { code } => ContractError::ProviderWithEvidence {
                code,
                request_id: request_id.clone(),
                operation_id: operation_id.clone(),
            },
            other => other,
        })?;
        if artifacts.is_empty() {
            return Err(ContractError::ProviderWithEvidence {
                code: "missing_image".into(),
                request_id,
                operation_id,
            });
        }
        let usage = response.body.get("usageMetadata");
        Ok(AdapterOutcome {
            outcome: OutcomeKind::Succeeded,
            usage: Some(NormalizedUsage {
                images: Some(artifacts.len() as i64),
                input_tokens: usage
                    .and_then(|v| v.get("promptTokenCount"))
                    .and_then(Value::as_i64),
                output_tokens: usage
                    .and_then(|v| v.get("candidatesTokenCount"))
                    .and_then(Value::as_i64),
            }),
            provider_amount_minor: None,
            provider_currency: None,
            provider_request_id: request_id,
            provider_operation_id: operation_id,
            artifacts,
        })
    }
}

impl<T: GeminiTransport> ProviderAdapter for GeminiImageAdapter<T> {
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
    ) -> Result<AdapterOutcome> {
        if vendor_idempotency_key.is_some() || self.config.max_retries != 0 {
            return Err(provider_error("retry_not_supported"));
        }
        self.invoke_inner(request, input, secret)
    }
    fn redact_error(&self, error: &(dyn StdError + 'static)) -> ContractError {
        let _ = Redactor::default().error_chain(error);
        provider_error("provider_failure")
    }
}

fn endpoint_url(config: &GeminiImageConfig, model: &str) -> Result<Url> {
    let mut base = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
    if base.scheme() != "https" || base.host_str().is_none() || base.query().is_some() {
        return Err(provider_error("config_invalid"));
    }
    let path = format!(
        "{}/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
        config.api_version.trim_matches('/'),
        config.project,
        config.location,
        model
    );
    base.set_path(&path);
    Ok(base)
}

fn extract_artifacts<T: GeminiTransport>(
    body: &Value,
    config: &GeminiImageConfig,
    transport: &T,
    secret: &ProviderSecret,
    timeout: Duration,
) -> Result<Vec<NormalizedArtifact>> {
    let parts = body
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.pointer("/content/parts"))
        .and_then(Value::as_array)
        .ok_or_else(|| provider_error("malformed_response"))?;
    let image_parts = parts
        .iter()
        .filter(|part| {
            part.get("inlineData")
                .or_else(|| part.get("inline_data"))
                .or_else(|| part.get("fileData"))
                .or_else(|| part.get("file_data"))
                .is_some()
        })
        .count();
    // Gemini image requests are normalized to exactly one output. Enforce the
    // bound before fetching any references so a small response cannot fan out
    // into unbounded provider-controlled downloads.
    if image_parts > 1 {
        return Err(provider_error("artifact_policy_failure"));
    }
    let mut artifacts = Vec::new();
    for part in parts {
        if let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data")) {
            let media_type = inline
                .get("mimeType")
                .or_else(|| inline.get("mime_type"))
                .and_then(Value::as_str)
                .ok_or_else(|| provider_error("malformed_response"))?;
            let data = inline
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| provider_error("malformed_response"))?;
            // Reject before allocating the decoded image. Base64 expands input
            // by 4/3, so this is a conservative bound for the artifact limit.
            if data.len() > MAX_ARTIFACT_BYTES.saturating_mul(4).div_ceil(3) {
                return Err(provider_error("artifact_policy_failure"));
            }
            let bytes = STANDARD
                .decode(data)
                .map_err(|_| provider_error("malformed_response"))?;
            artifacts.push(NormalizedArtifact {
                media_type: media_type.into(),
                bytes,
            });
        } else if let Some(file) = part.get("fileData").or_else(|| part.get("file_data")) {
            let media_type = file
                .get("mimeType")
                .or_else(|| file.get("mime_type"))
                .and_then(Value::as_str)
                .ok_or_else(|| provider_error("malformed_response"))?;
            let uri = file
                .get("fileUri")
                .or_else(|| file.get("file_uri"))
                .and_then(Value::as_str)
                .ok_or_else(|| provider_error("malformed_response"))?;
            let url = Url::parse(uri).map_err(|_| provider_error("artifact_policy_failure"))?;
            if url.scheme() != "https"
                || !config
                    .approved_artifact_hosts
                    .iter()
                    .any(|host| Some(host.as_str()) == url.host_str())
            {
                return Err(provider_error("artifact_policy_failure"));
            }
            let bytes = transport
                .fetch_artifact(&url, secret.expose(), timeout)
                .map_err(|error| classify_transport(error, secret, true))?;
            artifacts.push(NormalizedArtifact {
                media_type: media_type.into(),
                bytes,
            });
        }
    }
    Ok(artifacts)
}

fn classify_transport(
    error: Box<dyn StdError + Send + Sync>,
    secret: &ProviderSecret,
    artifact: bool,
) -> ContractError {
    let redactor = Redactor::new([secret.expose()]);
    let _redacted_evidence = redactor.error_chain(error.as_ref());
    let unknown = error
        .downcast_ref::<HttpFailure>()
        .and_then(|failure| match failure {
            HttpFailure::UnknownOutcome {
                request_id,
                operation_id,
            } => Some((request_id.clone(), operation_id.clone())),
            HttpFailure::BeforeSend => None,
        });
    let before_send = matches!(
        error.downcast_ref::<HttpFailure>(),
        Some(HttpFailure::BeforeSend)
    );
    if before_send && !artifact {
        provider_error("provider_pre_send_failure")
    } else if let Some((request_id, operation_id)) = unknown.filter(|_| !artifact) {
        ContractError::ProviderWithEvidence {
            code: "timeout_unknown_outcome".into(),
            request_id,
            operation_id,
        }
    } else if artifact {
        provider_error("artifact_policy_failure")
    } else {
        provider_error("provider_failure")
    }
}

fn provider_error(code: &str) -> ContractError {
    ContractError::Provider { code: code.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::secret_for_test;
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use std::{
        io::Cursor,
        sync::{Arc, Mutex},
    };

    #[derive(Clone)]
    struct FixtureTransport {
        response: Arc<Mutex<Option<std::result::Result<TransportResponse, String>>>>,
        calls: Arc<Mutex<u32>>,
        referenced: Vec<u8>,
    }
    impl GeminiTransport for FixtureTransport {
        fn generate(
            &self,
            _: &Url,
            bearer: &[u8],
            _: Duration,
            _: &std::collections::BTreeMap<String, String>,
            _: &Value,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            assert_eq!(bearer, b"secret-canary");
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
                    } else if message == "pre_send" {
                        Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>
                    } else {
                        Box::new(MessageError(message)) as Box<dyn StdError + Send + Sync>
                    }
                })
        }
        fn fetch_artifact(
            &self,
            _: &Url,
            _: &[u8],
            _: Duration,
        ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
            Ok(self.referenced.clone())
        }
    }
    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
            .unwrap();
        bytes
    }
    fn config() -> GeminiImageConfig {
        GeminiImageConfig {
            endpoint: "https://us-central1-aiplatform.googleapis.com".into(),
            api_version: "v1".into(),
            project: "sensitive-project".into(),
            location: "us-central1".into(),
            timeout_ms: 10_000,
            max_retries: 0,
            approved_artifact_hosts: vec!["storage.googleapis.com".into()],
            headers: std::collections::BTreeMap::new(),
        }
    }
    fn request() -> NormalizedRequest {
        NormalizedRequest {
            provider: PROVIDER_ID.into(),
            model: "gemini-image-v1".into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
        }
    }
    fn adapter(
        body: Value,
        status: u16,
    ) -> (GeminiImageAdapter<FixtureTransport>, Arc<Mutex<u32>>) {
        let calls = Arc::new(Mutex::new(0));
        let transport = FixtureTransport {
            response: Arc::new(Mutex::new(Some(Ok(TransportResponse {
                status,
                request_id: Some("request-1".into()),
                operation_id: Some("operation-1".into()),
                body,
            })))),
            calls: calls.clone(),
            referenced: png(),
        };
        (
            GeminiImageAdapter::new(config(), "gemini-image-v1".into(), transport).unwrap(),
            calls,
        )
    }
    #[test]
    fn fixture_happy_path_maps_one_call_usage_ids_and_inline_image() {
        let bytes = png();
        let (adapter, calls) = adapter(
            json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":STANDARD.encode(&bytes)}}]}}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":7}}),
            200,
        );
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"draw a cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(outcome.provider_request_id.as_deref(), Some("request-1"));
        assert_eq!(
            outcome.provider_operation_id.as_deref(),
            Some("operation-1")
        );
        assert_eq!(outcome.usage.unwrap().images, Some(1));
        assert_eq!(outcome.artifacts[0].bytes, bytes);
    }
    #[test]
    fn response_id_body_field_is_used_when_headers_are_absent() {
        let bytes = png();
        let (adapter, calls) = adapter(
            json!({"responseId":"body-response-1","candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":STANDARD.encode(&bytes)}}]}}]}),
            200,
        );
        {
            let mut response = adapter.transport.response.lock().unwrap();
            let response = response.as_mut().unwrap().as_mut().unwrap();
            response.request_id = None;
            response.operation_id = None;
        }
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"draw a cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(
            outcome.provider_request_id.as_deref(),
            Some("body-response-1")
        );
        assert_eq!(outcome.provider_operation_id, None);
    }
    #[test]
    fn post_response_artifact_failure_retains_provider_identifiers() {
        let (adapter, calls) = adapter(
            json!({"candidates":[{"content":{"parts":[{"text":"no image"}]}}]}),
            200,
        );
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ContractError::ProviderWithEvidence {
                code,
                request_id: Some(request_id),
                operation_id: Some(operation_id),
            } if code == "missing_image"
                && request_id == "request-1"
                && operation_id == "operation-1"
        ));
        assert_eq!(*calls.lock().unwrap(), 1);
    }
    #[test]
    fn referenced_output_requires_approved_host() {
        let (approved_adapter, _) = adapter(
            json!({"candidates":[{"content":{"parts":[{"fileData":{"mimeType":"image/png","fileUri":"https://storage.googleapis.com/output.png"}}]}}]}),
            200,
        );
        assert_eq!(
            approved_adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat"}),
                    &secret_for_test("secret-canary"),
                    None
                )
                .unwrap()
                .artifacts[0]
                .bytes,
            png()
        );
        let (rejected_adapter, _) = adapter(
            json!({"candidates":[{"content":{"parts":[{"fileData":{"mimeType":"image/png","fileUri":"https://evil.example/output.png"}}]}}]}),
            200,
        );
        assert!(
            matches!(rejected_adapter.invoke(&request(), &json!({"prompt":"cat"}), &secret_for_test("secret-canary"), None), Err(ContractError::ProviderWithEvidence { code, .. }) if code == "artifact_policy_failure")
        );

        let (multiple_adapter, _) = adapter(
            json!({"candidates":[{"content":{"parts":[
                {"fileData":{"mimeType":"image/png","fileUri":"https://storage.googleapis.com/one.png"}},
                {"fileData":{"mimeType":"image/png","fileUri":"https://storage.googleapis.com/two.png"}}
            ]}}]}),
            200,
        );
        assert!(matches!(
            multiple_adapter.invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None
            ),
            Err(ContractError::ProviderWithEvidence { code, .. }) if code == "artifact_policy_failure"
        ));
    }
    #[test]
    fn stable_failures_and_no_retry() {
        for (body, status, code) in [
            (
                json!({"error":{"message":"secret-canary"}}),
                403,
                "provider_rejected",
            ),
            (json!({"candidates":"bad"}), 200, "malformed_response"),
            (
                json!({"candidates":[{"content":{"parts":[{"text":"no image"}]}}]}),
                200,
                "missing_image",
            ),
        ] {
            let (adapter, calls) = adapter(body, status);
            let error = adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat"}),
                    &secret_for_test("secret-canary"),
                    None,
                )
                .unwrap_err();
            let actual = match &error {
                ContractError::Provider { code }
                | ContractError::ProviderWithEvidence { code, .. } => code,
                _ => panic!("unexpected contract error"),
            };
            assert_eq!(actual, code);
            assert_eq!(*calls.lock().unwrap(), 1);
            assert!(!error.to_string().contains("secret-canary"));
            assert!(!error.to_string().contains("sensitive-project"));
        }
    }
    #[test]
    fn server_error_after_transmission_is_not_a_proven_rejection() {
        let (adapter, calls) = adapter(json!({"error":{"message":"internal"}}), 503);
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ContractError::ProviderWithEvidence {
                code,
                request_id: Some(request_id),
                operation_id: Some(operation_id),
            } if code == "provider_failure"
                && request_id == "request-1"
                && operation_id == "operation-1"
        ));
        assert_eq!(*calls.lock().unwrap(), 1);
    }
    #[test]
    fn rejects_retry_and_invalid_normalized_input_before_network() {
        let (adapter, calls) = adapter(json!({}), 200);
        assert!(adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                Some("key")
            )
            .is_err());
        let mut bad = request();
        bad.image_count = Some(2);
        assert!(adapter
            .invoke(
                &bad,
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None
            )
            .is_err());
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[test]
    fn invalid_operator_headers_fail_adapter_construction_before_send() {
        let mut invalid_name = config();
        invalid_name
            .headers
            .insert("invalid header".into(), "value".into());
        assert!(matches!(
            GeminiImageAdapter::new(invalid_name, "gemini-image-v1".into(), FixtureTransport {
                response: Arc::new(Mutex::new(None)),
                calls: Arc::new(Mutex::new(0)),
                referenced: Vec::new(),
            }),
            Err(ContractError::Provider { code }) if code == "config_invalid"
        ));

        let mut invalid_value = config();
        invalid_value
            .headers
            .insert("x-valid".into(), "value\0bad".into());
        assert!(matches!(
            GeminiImageAdapter::new(invalid_value, "gemini-image-v1".into(), FixtureTransport {
                response: Arc::new(Mutex::new(None)),
                calls: Arc::new(Mutex::new(0)),
                referenced: Vec::new(),
            }),
            Err(ContractError::Provider { code }) if code == "config_invalid"
        ));
    }

    #[test]
    fn timeout_after_possible_transmission_is_ambiguous_and_called_once() {
        let calls = Arc::new(Mutex::new(0));
        let transport = FixtureTransport {
            response: Arc::new(Mutex::new(Some(Err("timeout".into())))),
            calls: calls.clone(),
            referenced: Vec::new(),
        };
        let adapter =
            GeminiImageAdapter::new(config(), "gemini-image-v1".into(), transport).unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ContractError::ProviderWithEvidence {
                code,
                request_id: None,
                operation_id: None,
            } if code == "timeout_unknown_outcome"
        ));
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn response_decode_failure_retains_captured_header_ids() {
        let error = classify_transport(
            Box::new(HttpFailure::UnknownOutcome {
                request_id: Some("request-header".into()),
                operation_id: Some("operation-header".into()),
            }),
            &secret_for_test("secret-canary"),
            false,
        );
        assert!(matches!(
            error,
            ContractError::ProviderWithEvidence {
                code,
                request_id: Some(request_id),
                operation_id: Some(operation_id),
            } if code == "timeout_unknown_outcome"
                && request_id == "request-header"
                && operation_id == "operation-header"
        ));
    }

    #[test]
    fn pre_send_transport_failure_has_a_distinct_stable_code() {
        let calls = Arc::new(Mutex::new(0));
        let transport = FixtureTransport {
            response: Arc::new(Mutex::new(Some(Err("pre_send".into())))),
            calls: calls.clone(),
            referenced: Vec::new(),
        };
        let adapter =
            GeminiImageAdapter::new(config(), "gemini-image-v1".into(), transport).unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(error, ContractError::Provider { code } if code == "provider_pre_send_failure")
        );
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn nested_secret_bearing_transport_error_is_not_exposed() {
        let calls = Arc::new(Mutex::new(0));
        let transport = FixtureTransport {
            response: Arc::new(Mutex::new(Some(Err(
                "outer SDK error: Authorization: Bearer secret-canary?project=sensitive-project"
                    .into(),
            )))),
            calls: calls.clone(),
            referenced: Vec::new(),
        };
        let adapter =
            GeminiImageAdapter::new(config(), "gemini-image-v1".into(), transport).unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "provider error (provider_failure)");
        assert!(!error.to_string().contains("secret-canary"));
        assert!(!error.to_string().contains("sensitive-project"));
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn response_and_inline_artifact_buffers_are_bounded() {
        let mut oversized = std::io::Cursor::new(vec![0; 9]);
        assert!(read_bounded(&mut oversized, 8).is_err());

        let encoded = "A".repeat(MAX_ARTIFACT_BYTES.saturating_mul(4).div_ceil(3) + 1);
        let (adapter, calls) = adapter(
            json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":encoded}}]}}]}),
            200,
        );
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ContractError::ProviderWithEvidence { code, .. }
                if code == "artifact_policy_failure"
        ));
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn referenced_artifact_transport_never_follows_redirects() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let destination_url = format!("http://{}/secret", destination.local_addr().unwrap());
        let redirector = TcpListener::bind("127.0.0.1:0").unwrap();
        let redirect_url = Url::parse(&format!(
            "http://{}/artifact",
            redirector.local_addr().unwrap()
        ))
        .unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = redirector.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {destination_url}\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
        });
        let result = ReqwestGeminiTransport.fetch_artifact(
            &redirect_url,
            b"credential",
            Duration::from_secs(2),
        );
        server.join().unwrap();
        assert!(result.is_err());
        assert!(
            matches!(destination.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn definitive_client_rejection_does_not_require_a_decodable_body() {
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
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 403 Forbidden\r\nx-goog-request-id: rejected-1\r\nContent-Length: 8\r\n\r\nnot-json"
            )
            .unwrap();
        });
        let response = ReqwestGeminiTransport
            .generate(
                &url,
                b"credential",
                Duration::from_secs(2),
                &std::collections::BTreeMap::new(),
                &json!({"prompt":"cat"}),
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(response.status, 403);
        assert_eq!(response.request_id.as_deref(), Some("rejected-1"));
        assert_eq!(response.body, Value::Null);
    }

    #[test]
    fn generated_bytes_only_become_durable_through_artifact_policy() {
        use crate::{
            artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference, Repository},
        };
        use tempfile::tempdir;
        let bytes = png();
        let (adapter, _) = adapter(
            json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":STANDARD.encode(&bytes)}}]}}]}),
            200,
        );
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("secret-canary"),
                None,
            )
            .unwrap();
        let repository = Repository::in_memory().unwrap();
        let execution = repository.create_execution(&CreateExecutionParams {
            account_id: "account".into(), operation_key: "gemini-fixture".into(), hubu_authorization_id: "auth".into(), hubu_claim_id: Some("claim".into()), hubu_token_reference: HubuTokenReference::new("token-ref").unwrap(), authorized_minor: 25, authorization_currency: "USD".into(), normalized_input: json!({"prompt":"cat","image_count":1}), input_hash: "hash".into(), input_schema_version: 1, target: "google/gemini-image-v1".into(), config_version: "cfg".into(), workload_type: "image_generation".into(), provider: "google".into(), adapter: "gemini_image".into(), model: "gemini-image-v1".into(), provider_config_version: "pcv".into(), pricing_snapshot: json!({"provider":"google","model":"gemini-image-v1","catalog_version":"prices","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"gemini-image","unit":"image","unit_amount_minor":25,"quantity":1,"estimated_amount_minor":25,"currency":"USD"}), pricing_schema_version: 1, created_at: "now".into()
        }).unwrap();
        let root = tempdir().unwrap();
        let service = ArtifactService::new(
            repository.clone(),
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
            bytes
        );
        let strict = ArtifactService::new(
            repository,
            LocalFsStorage::new(root.path()),
            ArtifactLimits {
                max_encoded_bytes: 1,
                ..ArtifactLimits::default()
            },
        );
        assert!(matches!(
            strict.store_image(&execution.execution_id, None, "image/png", &png(), "later"),
            Err(crate::artifact::Error::Limit("encoded size"))
        ));
    }

    #[test]
    #[ignore = "explicit live Google spend; see docs/gemini-image-e2e.md"]
    fn live_gemini_e2e_requires_explicit_spend_guard_and_never_uses_fixture() {
        use crate::{
            provider::contract::{enforce_cost, preflight_selected_secret, PricingCatalog},
            provider::targets::ProviderTargetConfig,
            secrets::MacOsKeychain,
        };
        use std::{env, path::Path};
        assert_eq!(
            env::var("GONGBU_LIVE_GEMINI_CONFIRM").as_deref(),
            Ok("I_ACCEPT_GOOGLE_CHARGES")
        );
        let config_path =
            env::var("GONGBU_PROVIDER_CONFIG").expect("operator target config is required");
        let pricing_path = env::var("GONGBU_PRICING_CATALOG").expect("pricing catalog is required");
        let max_spend: i64 = env::var("GONGBU_LIVE_GEMINI_MAX_MINOR")
            .expect("explicit spend guard is required")
            .parse()
            .expect("spend guard must be integer minor units");
        assert!(max_spend > 0);
        let targets = ProviderTargetConfig::from_path(Path::new(&config_path)).unwrap();
        let target = targets
            .provider_configs
            .iter()
            .find(|target| {
                target.provider == PROVIDER_ID && target.adapter == ADAPTER_ID && target.enabled
            })
            .expect("one enabled Gemini image target is required");
        let request = NormalizedRequest {
            provider: PROVIDER_ID.into(),
            model: target.model.clone(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
        };
        let snapshot = PricingCatalog::load(pricing_path)
            .unwrap()
            .snapshot(&request)
            .unwrap();
        enforce_cost(&snapshot, max_spend, "USD").unwrap();
        let adapter = GeminiImageAdapter::from_target(target).unwrap();
        let secret = preflight_selected_secret(&adapter, &MacOsKeychain, target, &request).unwrap();
        let prompt = env::var("GONGBU_LIVE_GEMINI_PROMPT").expect("explicit prompt is required");
        let outcome = adapter
            .invoke(&request, &json!({"prompt": prompt}), &secret, None)
            .unwrap();
        assert_eq!(outcome.outcome, OutcomeKind::Succeeded);
        assert_eq!(outcome.artifacts.len(), 1);
        image::load_from_memory(&outcome.artifacts[0].bytes)
            .expect("Google returned a decodable image");
        assert_eq!(
            snapshot
                .settle(outcome.usage.as_ref().unwrap(), max_spend)
                .unwrap(),
            snapshot.estimated_amount_minor
        );
    }
}
