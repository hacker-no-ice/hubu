//! Gemini Developer API image adapter using the Interactions endpoint.
//!
//! This is deliberately distinct from the Vertex `gemini_image` adapter. It
//! sends one API-key-authenticated request, never retries, and accepts only one
//! inline image for the shared artifact policy and persistence pipeline.

use super::{
    contract::{
        canonical_image_media_type, AdapterCapabilities, AdapterOutcome, ContractError,
        NormalizedArtifact, NormalizedRequest, NormalizedUsage, ProviderAdapter, ProviderFailure,
        ProviderPhase, Result, RetryPolicy,
    },
    http_kernel::{
        provider_request_id, read_bounded, shared_client, validate_https_origin, InvocationDeadline,
    },
    targets::{GeminiDeveloperImageConfig, ProviderConfigVersion},
};
use crate::{redaction::Redactor, secrets::ProviderSecret};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{
    header::{HeaderName, HeaderValue},
    Url,
};
use serde_json::{json, Value};
use std::{error::Error as StdError, fmt, time::Duration};

pub const PROVIDER_ID: &str = "google";
pub const ADAPTER_ID: &str = "gemini_developer_image";
const MAX_PROVIDER_RESPONSE_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug)]
enum HttpFailure {
    BeforeSend,
    UnknownOutcome { request_id: Option<String> },
}
impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Google Developer API HTTP request failed")
    }
}
impl StdError for HttpFailure {}

#[derive(Clone, Debug)]
pub struct TransportResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: Value,
}

pub trait GeminiDeveloperTransport: Send + Sync {
    fn create_interaction(
        &self,
        url: &Url,
        api_key: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;
}

impl<T: GeminiDeveloperTransport + ?Sized> GeminiDeveloperTransport for std::sync::Arc<T> {
    fn create_interaction(
        &self,
        url: &Url,
        api_key: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref()
            .create_interaction(url, api_key, timeout, headers, body)
    }
}

pub struct ReqwestGeminiDeveloperTransport;
impl GeminiDeveloperTransport for ReqwestGeminiDeveloperTransport {
    fn create_interaction(
        &self,
        url: &Url,
        api_key: &[u8],
        timeout: Duration,
        headers: &std::collections::BTreeMap<String, String>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let api_key = std::str::from_utf8(api_key)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .post(url.clone())
            .timeout(timeout)
            .header("x-goog-api-key", api_key);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let mut response = request.json(body).send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome { request_id: None })
                as Box<dyn StdError + Send + Sync>
        })?;
        let status = response.status().as_u16();
        let request_id =
            provider_request_id(response.headers(), &["x-goog-request-id", "x-request-id"]);
        if (400..500).contains(&status) {
            return Ok(TransportResponse {
                status,
                request_id,
                body: Value::Null,
            });
        }
        let body_bytes =
            read_bounded(&mut response, MAX_PROVIDER_RESPONSE_BYTES).map_err(|_| {
                Box::new(HttpFailure::UnknownOutcome {
                    request_id: request_id.clone(),
                }) as Box<dyn StdError + Send + Sync>
            })?;
        let body = serde_json::from_slice(&body_bytes).map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome {
                request_id: request_id.clone(),
            }) as Box<dyn StdError + Send + Sync>
        })?;
        Ok(TransportResponse {
            status,
            request_id,
            body,
        })
    }
}

pub struct GeminiDeveloperImageAdapter<T = ReqwestGeminiDeveloperTransport> {
    config: GeminiDeveloperImageConfig,
    model: String,
    transport: T,
    max_artifact_bytes: usize,
}

impl GeminiDeveloperImageAdapter<ReqwestGeminiDeveloperTransport> {
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
                .gemini_developer_image()
                .cloned()
                .ok_or_else(|| provider_error("config_invalid"))?,
            target.model.clone(),
            ReqwestGeminiDeveloperTransport,
            max_artifact_bytes,
        )
    }
}

impl<T: GeminiDeveloperTransport> GeminiDeveloperImageAdapter<T> {
    pub fn new(config: GeminiDeveloperImageConfig, model: String, transport: T) -> Result<Self> {
        Self::new_with_artifact_limit(
            config,
            model,
            transport,
            crate::artifact::DEFAULT_MAX_ENCODED_BYTES,
        )
    }

    pub fn new_with_artifact_limit(
        config: GeminiDeveloperImageConfig,
        model: String,
        transport: T,
        max_artifact_bytes: u64,
    ) -> Result<Self> {
        RetryPolicy {
            max_retries: config.max_retries,
        }
        .validate(AdapterCapabilities {
            vendor_enforced_idempotency: false,
        })?;
        if model.trim().is_empty()
            || config.timeout_ms == 0
            || max_artifact_bytes == 0
            || max_artifact_bytes > usize::MAX as u64
        {
            return Err(provider_error("config_invalid"));
        }
        for (name, value) in &config.headers {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            HeaderValue::from_str(value).map_err(|_| provider_error("config_invalid"))?;
            if name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("x-goog-api-key")
            {
                return Err(provider_error("config_invalid"));
            }
        }
        endpoint_url(&config)?;
        Ok(Self {
            config,
            model,
            transport,
            max_artifact_bytes: max_artifact_bytes as usize,
        })
    }
}

impl<T: GeminiDeveloperTransport> ProviderAdapter for GeminiDeveloperImageAdapter<T> {
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
        if input.get("options").is_some() {
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
        self.preflight_input(request, input)
            .map_err(|_| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        if vendor_idempotency_key.is_some() || self.config.max_retries != 0 {
            return Err(ProviderFailure::release(
                "retry_not_supported",
                ProviderPhase::PreSend,
            ));
        }
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 32_000)
            .ok_or_else(|| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let mut body = json!({
            "model": self.model,
            "input": [{"type": "text", "text": prompt}],
            "response_format": {"type": "image"}
        });
        if let Some(size) = &request.image_size {
            body["response_format"]["image_size"] = json!(size.to_ascii_uppercase());
        }
        let deadline =
            InvocationDeadline::from_timeout(Duration::from_millis(self.config.timeout_ms))
                .map_err(|_| ProviderFailure::release("config_invalid", ProviderPhase::PreSend))?;
        let response = self
            .transport
            .create_interaction(
                &endpoint_url(&self.config).map_err(|_| {
                    ProviderFailure::release("config_invalid", ProviderPhase::PreSend)
                })?,
                secret.expose(),
                deadline.remaining().map_err(|_| {
                    ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
                })?,
                &self.config.headers,
                &body,
            )
            .map_err(|error| classify_transport(error, secret))?;
        let request_id = response.request_id.or_else(|| {
            response
                .body
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        if !(200..300).contains(&response.status) {
            if (400..500).contains(&response.status) {
                return Err(ProviderFailure::release(
                    format!("provider_rejected_http_{}", response.status),
                    ProviderPhase::Submission,
                ));
            }
            return Err(
                ProviderFailure::reconcile("provider_failure", ProviderPhase::Submission)
                    .with_evidence(request_id, None),
            );
        }
        let artifact =
            extract_artifact(&response.body, self.max_artifact_bytes).map_err(|error| {
                let code = match error {
                    ContractError::Provider { code } => code,
                    _ => "provider_contract_failure".into(),
                };
                ProviderFailure::reconcile(code, ProviderPhase::Artifact)
                    .with_evidence(request_id.clone(), None)
            })?;
        let usage = response.body.get("usage");
        Ok(AdapterOutcome {
            usage: Some(NormalizedUsage {
                images: Some(1),
                input_tokens: usage
                    .and_then(|v| v.get("input_tokens"))
                    .and_then(Value::as_i64),
                output_tokens: usage
                    .and_then(|v| v.get("output_tokens"))
                    .and_then(Value::as_i64),
            }),
            provider_amount_minor: None,
            provider_currency: None,
            provider_request_id: request_id,
            provider_operation_id: None,
            artifacts: vec![artifact],
        })
    }
    fn redact_error(&self, error: &(dyn StdError + 'static)) -> ContractError {
        let _ = Redactor::default().error_chain(error);
        provider_error("provider_failure")
    }
}

fn endpoint_url(config: &GeminiDeveloperImageConfig) -> Result<Url> {
    let mut base = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
    if validate_https_origin(&base, Some("generativelanguage.googleapis.com")).is_err() {
        return Err(provider_error("config_invalid"));
    }
    base.set_path(&format!(
        "{}/interactions",
        config.api_version.trim_matches('/')
    ));
    Ok(base)
}

fn extract_artifact(body: &Value, max_artifact_bytes: usize) -> Result<NormalizedArtifact> {
    let images: Vec<_> = body
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|step| step.get("type").and_then(Value::as_str) == Some("model_output"))
        .filter_map(|step| step.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("image"))
        .collect();
    if images.len() != 1 {
        return Err(provider_error(if images.is_empty() {
            "missing_image"
        } else {
            "artifact_policy_failure"
        }));
    }
    let image = images[0];
    let media_type = image
        .get("mime_type")
        .and_then(Value::as_str)
        .ok_or_else(|| provider_error("malformed_response"))?;
    let data = image
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| provider_error("malformed_response"))?;
    if data.len() > max_artifact_bytes.saturating_mul(4).div_ceil(3) {
        return Err(provider_error("artifact_policy_failure"));
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| provider_error("malformed_response"))?;
    if bytes.len() > max_artifact_bytes {
        return Err(provider_error("artifact_policy_failure"));
    }
    let media_type = canonical_image_media_type(Some(media_type), &bytes)?;
    Ok(NormalizedArtifact {
        media_type: media_type.into(),
        bytes,
    })
}

fn classify_transport(
    error: Box<dyn StdError + Send + Sync>,
    secret: &ProviderSecret,
) -> ProviderFailure {
    let _ = Redactor::new([secret.expose()]).error_chain(error.as_ref());
    match error.downcast_ref::<HttpFailure>() {
        Some(HttpFailure::BeforeSend) => {
            ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
        }
        Some(HttpFailure::UnknownOutcome { request_id }) => {
            ProviderFailure::reconcile("timeout_unknown_outcome", ProviderPhase::Submission)
                .with_evidence(request_id.clone(), None)
        }
        None => ProviderFailure::reconcile("provider_failure", ProviderPhase::Submission),
    }
}

fn provider_error(code: &str) -> ContractError {
    ContractError::Provider { code: code.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::secret_for_test;
    use std::sync::{Arc, Mutex};

    type RecordedCalls = Arc<Mutex<Vec<(String, Vec<u8>, Value)>>>;

    #[derive(Clone)]
    struct FixtureTransport {
        response: TransportResponse,
        calls: RecordedCalls,
    }
    impl GeminiDeveloperTransport for FixtureTransport {
        fn create_interaction(
            &self,
            url: &Url,
            api_key: &[u8],
            _: Duration,
            _: &std::collections::BTreeMap<String, String>,
            body: &Value,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            self.calls
                .lock()
                .unwrap()
                .push((url.as_str().into(), api_key.to_vec(), body.clone()));
            Ok(self.response.clone())
        }
    }
    fn config() -> GeminiDeveloperImageConfig {
        GeminiDeveloperImageConfig {
            endpoint: "https://generativelanguage.googleapis.com".into(),
            api_version: "v1beta".into(),
            timeout_ms: 120_000,
            max_retries: 0,
            headers: Default::default(),
        }
    }
    fn request() -> NormalizedRequest {
        NormalizedRequest {
            provider: PROVIDER_ID.into(),
            model: "gemini-3.1-flash-lite-image".into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: None,
        }
    }
    fn adapter(
        body: Value,
        status: u16,
    ) -> (GeminiDeveloperImageAdapter<FixtureTransport>, RecordedCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let transport = FixtureTransport {
            response: TransportResponse {
                status,
                request_id: None,
                body,
            },
            calls: calls.clone(),
        };
        (
            GeminiDeveloperImageAdapter::new(config(), request().model, transport).unwrap(),
            calls,
        )
    }

    #[test]
    fn fixture_request_auth_response_and_usage_are_normalized() {
        let response: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/gemini_developer_interaction_success.json"
        ))
        .unwrap();
        let (adapter, calls) = adapter(response, 200);
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"draw a cat"}),
                &secret_for_test("api-key-canary"),
                None,
            )
            .unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].0,
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
        assert_eq!(calls[0].1, b"api-key-canary");
        assert_eq!(
            calls[0].2,
            json!({"model":"gemini-3.1-flash-lite-image","input":[{"type":"text","text":"draw a cat"}],"response_format":{"type":"image"}})
        );
        assert_eq!(
            outcome.provider_request_id.as_deref(),
            Some("interaction-fixture-1")
        );
        assert_eq!(outcome.usage.unwrap().input_tokens, Some(4));
        assert_eq!(outcome.artifacts.len(), 1);
        image::load_from_memory(&outcome.artifacts[0].bytes).unwrap();
    }

    #[test]
    fn normalized_resolution_is_validated_and_transmitted() {
        let response: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/gemini_developer_interaction_success.json"
        ))
        .unwrap();
        let (sized_adapter, calls) = adapter(response.clone(), 200);
        let mut sized = request();
        sized.image_size = Some("4k".into());
        sized_adapter
            .invoke(
                &sized,
                &json!({"prompt":"draw a cat","image_size":"4k"}),
                &secret_for_test("api-key-canary"),
                None,
            )
            .unwrap();
        assert_eq!(
            calls.lock().unwrap()[0].2["response_format"]["image_size"],
            "4K"
        );

        let (adapter, calls) = adapter(response, 200);
        assert!(adapter
            .invoke(
                &sized,
                &json!({"prompt":"draw a cat","image_size":"2k"}),
                &secret_for_test("api-key-canary"),
                None,
            )
            .is_err());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn secret_and_provider_message_are_redacted_and_there_is_no_retry() {
        let (adapter, calls) = adapter(json!({"error":{"message":"api-key-canary"}}), 401);
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("api-key-canary"),
                None,
            )
            .unwrap_err();
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(
            error.to_string(),
            "provider error (provider_rejected_http_401)"
        );
        assert!(!error.to_string().contains("api-key-canary"));
    }

    #[test]
    fn rejects_multiple_images_and_reserved_auth_headers() {
        let image = json!({"type":"image","mime_type":"image/png","data":"AA=="});
        let (adapter, _) = adapter(
            json!({"steps":[{"type":"model_output","content":[image.clone(), image]}]}),
            200,
        );
        assert!(
            matches!(adapter.invoke(&request(), &json!({"prompt":"cat"}), &secret_for_test("key"), None), Err(ProviderFailure { code, .. }) if code == "artifact_policy_failure")
        );
        let mut invalid = config();
        invalid
            .headers
            .insert("X-Goog-Api-Key".into(), "leak".into());
        assert!(GeminiDeveloperImageAdapter::new(
            invalid,
            request().model,
            FixtureTransport {
                response: TransportResponse {
                    status: 200,
                    request_id: None,
                    body: json!({})
                },
                calls: Default::default()
            }
        )
        .is_err());

        let mut non_google = config();
        non_google.endpoint = "https://credentials.example".into();
        assert!(GeminiDeveloperImageAdapter::new(
            non_google,
            request().model,
            FixtureTransport {
                response: TransportResponse {
                    status: 200,
                    request_id: None,
                    body: json!({})
                },
                calls: Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn normalized_image_enters_shared_local_artifact_pipeline() {
        use crate::{
            artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference, Repository},
        };
        let response: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/gemini_developer_interaction_success.json"
        ))
        .unwrap();
        let (adapter, _) = adapter(response, 200);
        let outcome = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat"}),
                &secret_for_test("key"),
                None,
            )
            .unwrap();
        let repository = Repository::in_memory().unwrap();
        let execution = repository.create_execution(&CreateExecutionParams { account_id:"account".into(), operation_key:"gemini-dev-fixture".into(), hubu_authorization_id:"auth".into(), hubu_claim_id:Some("claim".into()), hubu_token_reference:HubuTokenReference::new("token-ref").unwrap(), authorized_minor:10, authorization_currency:"USD".into(), normalized_input:json!({"prompt":"cat","image_count":1}), input_hash:"hash".into(), input_schema_version:1, target:"google/gemini-3.1-flash-lite-image".into(), config_version:"cfg".into(), workload_type:"image_generation".into(), provider:"google".into(), adapter:ADAPTER_ID.into(), model:request().model, provider_config_version:"pcv".into(), provider_config_digest:format!("sha256:{}", "a".repeat(64)), pricing_snapshot:json!({"provider":"google","model":"gemini-3.1-flash-lite-image","catalog_version":"prices","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"gemini-dev-image","unit":"image","unit_amount_minor":10,"quantity":1,"estimated_amount_minor":10,"currency":"USD"}), pricing_schema_version:1, created_at:"now".into() }).unwrap();
        let root = tempfile::tempdir().unwrap();
        let service = ArtifactService::new(
            repository,
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let stored = service
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
                .retrieve_for_account(&stored.artifact_id, "account")
                .unwrap()
                .bytes,
            outcome.artifacts[0].bytes
        );
    }

    #[test]
    #[ignore = "explicit live Google spend; see docs/gongbu/gemini-developer-image-e2e.md"]
    fn live_developer_api_e2e_requires_explicit_spend_guard() {
        use crate::{
            provider::contract::{enforce_cost, preflight_selected_secret, PricingCatalog},
            provider::targets::ProviderTargetConfig,
            secrets::MacOsKeychain,
        };
        use std::{env, fs::OpenOptions, io::Write, path::Path};
        assert_eq!(
            env::var("GONGBU_LIVE_GEMINI_DEVELOPER_CONFIRM").as_deref(),
            Ok("I_ACCEPT_GOOGLE_CHARGES")
        );
        let max_spend: i64 = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_MAX_MINOR")
            .expect("explicit spend guard is required")
            .parse()
            .expect("spend guard must be integer minor units");
        assert!(max_spend > 0);
        let targets = ProviderTargetConfig::from_path(Path::new(
            &env::var("GONGBU_PROVIDER_CONFIG").expect("operator target config is required"),
        ))
        .unwrap();
        let target = targets
            .revisions()
            .find(|target| {
                target.provider == PROVIDER_ID && target.adapter == ADAPTER_ID && target.is_active()
            })
            .expect("one enabled Developer API image target is required");
        let image_size = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_IMAGE_SIZE").ok();
        let request = NormalizedRequest {
            provider: PROVIDER_ID.into(),
            model: target.model.clone(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: image_size.clone(),
        };
        let snapshot = PricingCatalog::load(
            env::var("GONGBU_PRICING_CATALOG").expect("pricing catalog is required"),
        )
        .unwrap()
        .snapshot(&request)
        .unwrap();
        enforce_cost(&snapshot, max_spend, "USD").unwrap();
        let adapter = GeminiDeveloperImageAdapter::from_target(target).unwrap();
        let secret = preflight_selected_secret(&adapter, &MacOsKeychain, target, &request).unwrap();
        let prompt =
            env::var("GONGBU_LIVE_GEMINI_DEVELOPER_PROMPT").expect("explicit prompt is required");
        let output_path = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_OUTPUT")
            .expect("explicit output image path is required");
        let output_path = Path::new(&output_path);
        assert!(
            output_path.is_absolute(),
            "output image path must be absolute"
        );
        assert!(
            output_path.parent().is_some_and(Path::is_dir),
            "output image parent directory must exist"
        );
        assert!(
            !output_path.exists(),
            "output image path must not already exist"
        );
        let mut input = json!({"prompt":prompt});
        if let Some(size) = image_size {
            input["image_size"] = json!(size);
        }
        let outcome = adapter.invoke(&request, &input, &secret, None).unwrap();
        outcome.validate().unwrap();
        assert_eq!(outcome.artifacts.len(), 1);
        image::load_from_memory(&outcome.artifacts[0].bytes)
            .expect("Google returned a decodable image");
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_path)
            .expect("create output image without overwriting an existing file");
        output
            .write_all(&outcome.artifacts[0].bytes)
            .expect("write complete output image");
        output.sync_all().expect("flush output image to disk");
        assert_eq!(
            snapshot
                .settle(outcome.usage.as_ref().unwrap(), max_spend)
                .unwrap(),
            snapshot.estimated_amount_minor
        );
    }
}
