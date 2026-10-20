//! The sync fixed-price engine: one reviewed implementation for providers that
//! answer in a single request, driven entirely by a provider pack.
//!
//! It holds exactly from the request parameters under the frozen price and
//! settles from the same frozen price. It sends one request and never retries
//! a provider mutation. Only a failure proven to happen before transmission,
//! or a definitive 4xx rejection, releases the claim; everything after the
//! provider may have accepted the work goes to reconciliation with whatever
//! evidence was received.

use super::{
    contract::{
        AdapterCapabilities, AdapterOutcome, ContractError, NormalizedRequest, ProviderAdapter,
        ProviderFailure, ProviderPhase,
    },
    http_kernel::{
        read_bounded, shared_client, ArtifactDownloadPolicy, CredentialForwarding,
        InvocationDeadline,
    },
    pack::{ArtifactFetchError, ArtifactSource, Pack, ProviderResponse},
    targets::ProviderConfigVersion,
};
use crate::secrets::ProviderSecret;
use reqwest::Url;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub const ENGINE_ID: &str = "sync_fixed";
const MAX_PROVIDER_RESPONSE_BYTES: usize = 30 * 1024 * 1024;

/// Why a transport call failed, as far as the engine can prove it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportFailure {
    /// Nothing was sent; the provider cannot have seen the request.
    BeforeSend,
    /// The request may have reached the provider.
    UnknownOutcome { request_id: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportResponse {
    pub status: u16,
    /// Lowercase header names.
    pub headers: BTreeMap<String, String>,
    /// The parsed JSON body; `Null` for a 4xx rejection.
    pub body: Value,
}

/// The HTTP boundary of the sync engine, replaceable in tests.
pub trait SyncTransport: Send + Sync {
    /// POST `body` once. The credential goes only in `auth`.
    fn post_json(
        &self,
        url: &Url,
        auth: (&str, &str),
        headers: &BTreeMap<String, String>,
        body: &Value,
        timeout: Duration,
    ) -> Result<TransportResponse, TransportFailure>;

    /// Download an artifact from an allowlisted host without credentials.
    fn download(
        &self,
        url: &Url,
        policy: &ArtifactDownloadPolicy,
        deadline: InvocationDeadline,
    ) -> Result<Vec<u8>, ArtifactFetchError>;
}

pub struct ReqwestSyncTransport;

impl SyncTransport for ReqwestSyncTransport {
    fn post_json(
        &self,
        url: &Url,
        auth: (&str, &str),
        headers: &BTreeMap<String, String>,
        body: &Value,
        timeout: Duration,
    ) -> Result<TransportResponse, TransportFailure> {
        let client = shared_client().map_err(|_| TransportFailure::BeforeSend)?;
        let mut request = client
            .post(url.clone())
            .timeout(timeout)
            .header(auth.0, auth.1);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let mut response = request
            .json(body)
            .send()
            .map_err(|_| TransportFailure::UnknownOutcome { request_id: None })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().ok()?.trim().to_owned(),
                ))
            })
            .collect::<BTreeMap<_, _>>();
        if (400..500).contains(&status) {
            return Ok(TransportResponse {
                status,
                headers,
                body: Value::Null,
            });
        }
        let unknown = |headers: &BTreeMap<String, String>| TransportFailure::UnknownOutcome {
            request_id: headers
                .get("x-request-id")
                .filter(|id| !id.is_empty() && id.len() <= 255)
                .cloned(),
        };
        let bytes = read_bounded(&mut response, MAX_PROVIDER_RESPONSE_BYTES)
            .map_err(|_| unknown(&headers))?;
        let body = serde_json::from_slice(&bytes).map_err(|_| unknown(&headers))?;
        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }

    fn download(
        &self,
        url: &Url,
        policy: &ArtifactDownloadPolicy,
        deadline: InvocationDeadline,
    ) -> Result<Vec<u8>, ArtifactFetchError> {
        super::http_kernel::download_artifact(url, deadline, policy, None).map_err(|error| {
            match error {
                super::http_kernel::HttpKernelError::ResponseTooLarge => {
                    ArtifactFetchError::TooLarge
                }
                _ => ArtifactFetchError::Failed,
            }
        })
    }
}

pub struct SyncFixedEngine<T = ReqwestSyncTransport> {
    pack: Arc<Pack>,
    model: String,
    transport: T,
    max_artifact_bytes: usize,
}

impl SyncFixedEngine<ReqwestSyncTransport> {
    pub fn from_target(
        target: &ProviderConfigVersion,
        pack: &Arc<Pack>,
        max_artifact_bytes: u64,
    ) -> Result<Self, ContractError> {
        Self::new(
            pack.clone(),
            &target.model,
            ReqwestSyncTransport,
            max_artifact_bytes,
        )
    }
}

impl<T: SyncTransport> SyncFixedEngine<T> {
    pub fn new(
        pack: Arc<Pack>,
        model: &str,
        transport: T,
        max_artifact_bytes: u64,
    ) -> Result<Self, ContractError> {
        if pack.archetype().engine_id() != ENGINE_ID
            || pack.model(model).is_none()
            || max_artifact_bytes == 0
            || max_artifact_bytes > usize::MAX as u64
        {
            return Err(ContractError::Provider {
                code: "config_invalid".into(),
            });
        }
        Ok(Self {
            pack,
            model: model.to_owned(),
            transport,
            max_artifact_bytes: max_artifact_bytes as usize,
        })
    }
}

struct EngineDownloads<'a, T> {
    transport: &'a T,
    policy: Option<ArtifactDownloadPolicy>,
    deadline: InvocationDeadline,
}

impl<T: SyncTransport> ArtifactSource for EngineDownloads<'_, T> {
    fn fetch(&self, url: &Url, _max_bytes: usize) -> Result<Vec<u8>, ArtifactFetchError> {
        // The policy already bounds the download at the engine's artifact limit.
        let policy = self.policy.as_ref().ok_or(ArtifactFetchError::Failed)?;
        self.transport.download(url, policy, self.deadline)
    }
}

impl<T: SyncTransport> ProviderAdapter for SyncFixedEngine<T> {
    fn adapter_id(&self) -> &str {
        self.pack.adapter_id()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: false,
        }
    }

    fn validate_request(&self, request: &NormalizedRequest) -> Result<(), ContractError> {
        request.validate()?;
        if request.provider != self.pack.provider() || request.model != self.model {
            return Err(ContractError::Provider {
                code: "invalid_request".into(),
            });
        }
        Ok(())
    }

    fn preflight_input(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
    ) -> Result<(), ContractError> {
        self.validate_request(request)?;
        // The priced request must be exactly what this input prices to.
        if self.pack.prepare(&self.model, normalized_input)?.request != *request {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }

    fn invoke(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let pre_send =
            |code: &str| ProviderFailure::release(code.to_owned(), ProviderPhase::PreSend);
        // Proven before transmission: nothing below has contacted the provider.
        if vendor_idempotency_key.is_some() {
            return Err(pre_send("retry_not_supported"));
        }
        self.preflight_input(request, normalized_input)
            .map_err(|_| pre_send("invalid_request"))?;
        let prepared = self
            .pack
            .prepare(&self.model, normalized_input)
            .map_err(|_| pre_send("invalid_request"))?;
        let transport = self.pack.transport();
        let secret_text =
            std::str::from_utf8(secret.expose()).map_err(|_| pre_send("credential_invalid"))?;
        let deadline =
            InvocationDeadline::from_timeout(Duration::from_millis(transport.timeout_ms))
                .map_err(|_| pre_send("config_invalid"))?;
        let timeout = deadline
            .remaining()
            .map_err(|_| pre_send("provider_pre_send_failure"))?;
        let auth_value = transport.auth_scheme.header_value(secret_text);
        let response = self
            .transport
            .post_json(
                &self.pack.request_url(),
                (&transport.auth_header, &auth_value),
                &transport.headers,
                &prepared.body,
                timeout,
            )
            .map_err(|failure| match failure {
                TransportFailure::BeforeSend => pre_send("provider_pre_send_failure"),
                TransportFailure::UnknownOutcome { request_id } => {
                    ProviderFailure::reconcile("timeout_unknown_outcome", ProviderPhase::Submission)
                        .with_evidence(request_id, None)
                }
            })?;
        let status = response.status;
        let response = ProviderResponse {
            headers: response.headers,
            body: response.body,
        };
        let request_id = self.pack.provider_request_id(&response);
        match status {
            200..=299 => {}
            // A definitive rejection: the provider refused the work.
            400..=499 => {
                return Err(ProviderFailure::release(
                    "provider_rejected",
                    ProviderPhase::Submission,
                )
                .with_evidence(request_id, None))
            }
            _ => {
                return Err(ProviderFailure::reconcile(
                    "provider_failure",
                    ProviderPhase::Submission,
                )
                .with_evidence(request_id, None))
            }
        }
        let artifacts = self.pack.artifacts();
        let downloads = EngineDownloads {
            transport: &self.transport,
            policy: (!artifacts.hosts.is_empty())
                .then(|| {
                    ArtifactDownloadPolicy::new(
                        &artifacts.hosts,
                        self.max_artifact_bytes as u64,
                        CredentialForwarding::Prohibited,
                    )
                    .ok()
                })
                .flatten(),
            deadline,
        };
        self.pack
            .map_response(request, &response, &downloads, self.max_artifact_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::{
            conformance::{assert_adapter_conformance, Case, Observation},
            contract::{NormalizedUsage, PricingCatalog, SpendDisposition},
        },
        secrets::secret_for_test,
    };
    use base64::Engine as _;
    use serde_json::json;
    use std::sync::Mutex;

    const URL_PACK: &str = include_str!("../../../../fixtures/packs/sandbox-image.toml");
    const URL_RESPONSE: &str =
        include_str!("../../../../fixtures/packs/sandbox-image.response.json");
    const INLINE_PACK: &str = include_str!("../../../../fixtures/packs/inline-image.toml");
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    const MAX_ARTIFACT: u64 = 1024;

    type Posts = Arc<Mutex<Vec<(String, (String, String), Value)>>>;

    #[derive(Clone)]
    struct FakeTransport {
        posts: Posts,
        response: Result<TransportResponse, TransportFailure>,
        artifact: Vec<u8>,
    }

    impl FakeTransport {
        fn ok(body: Value) -> Self {
            Self::status(200, body)
        }
        fn status(status: u16, body: Value) -> Self {
            Self {
                posts: Posts::default(),
                response: Ok(TransportResponse {
                    status,
                    headers: BTreeMap::new(),
                    body,
                }),
                artifact: PNG.to_vec(),
            }
        }
    }

    impl SyncTransport for FakeTransport {
        fn post_json(
            &self,
            url: &Url,
            auth: (&str, &str),
            _headers: &BTreeMap<String, String>,
            body: &Value,
            _timeout: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            self.posts.lock().unwrap().push((
                url.to_string(),
                (auth.0.to_owned(), auth.1.to_owned()),
                body.clone(),
            ));
            self.response.clone()
        }
        fn download(
            &self,
            url: &Url,
            policy: &ArtifactDownloadPolicy,
            _deadline: InvocationDeadline,
        ) -> Result<Vec<u8>, ArtifactFetchError> {
            policy
                .validate_url(url)
                .map_err(|_| ArtifactFetchError::Failed)?;
            if self.artifact.len() > policy.max_bytes() {
                return Err(ArtifactFetchError::TooLarge);
            }
            Ok(self.artifact.clone())
        }
    }

    fn engine(pack: &str, model: &str, transport: FakeTransport) -> SyncFixedEngine<FakeTransport> {
        SyncFixedEngine::new(
            Arc::new(Pack::from_toml_str(pack).unwrap()),
            model,
            transport,
            MAX_ARTIFACT,
        )
        .unwrap()
    }

    fn url_input() -> Value {
        json!({"prompt": "a red circle", "image_count": 2, "quality": "final"})
    }

    fn url_request(engine: &SyncFixedEngine<FakeTransport>) -> NormalizedRequest {
        engine
            .pack
            .prepare("image-v1", &url_input())
            .unwrap()
            .request
    }

    #[test]
    fn multi_component_url_pack_passes_executor_conformance() {
        assert_adapter_conformance(|case| {
            let response = serde_json::from_str::<Value>(URL_RESPONSE).unwrap();
            let mut transport = match case {
                Case::Rejection => FakeTransport::status(400, Value::Null),
                Case::AmbiguousPostSend => FakeTransport {
                    response: Err(TransportFailure::UnknownOutcome { request_id: None }),
                    ..FakeTransport::ok(Value::Null)
                },
                Case::HostPolicy => {
                    let mut foreign = response.clone();
                    foreign["data"][1]["url"] = json!("https://evil.example/out.png");
                    FakeTransport::ok(foreign)
                }
                _ => FakeTransport::ok(response),
            };
            match case {
                Case::EvidenceRetention => transport.artifact = b"not an image".to_vec(),
                Case::ArtifactBound => transport.artifact = vec![0; MAX_ARTIFACT as usize + 1],
                _ => {}
            }
            let posts = transport.posts.clone();
            let engine = engine(URL_PACK, "image-v1", transport);
            let mut request = url_request(&engine);
            if matches!(case, Case::InvalidRequest) {
                request.set_quantity("image_count", Some(3));
            }
            let result = engine.invoke(
                &request,
                &url_input(),
                &secret_for_test("secret-canary"),
                matches!(case, Case::UnsafeRetry).then_some("opaque-key"),
            );
            let submissions = posts.lock().unwrap().len() as u32;
            Observation {
                result,
                submissions,
            }
        });
    }

    #[test]
    fn sends_one_rendered_request_and_settles_exactly_from_the_frozen_price() {
        let transport = FakeTransport::ok(serde_json::from_str(URL_RESPONSE).unwrap());
        let posts = transport.posts.clone();
        let engine = engine(URL_PACK, "image-v1", transport);
        let request = url_request(&engine);
        let outcome = engine
            .invoke(&request, &url_input(), &secret_for_test("sk-test"), None)
            .unwrap();
        let posts = posts.lock().unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, "https://api.sandbox-image.test/v1/images");
        assert_eq!(posts[0].1, ("x-api-key".to_owned(), "sk-test".to_owned()));
        assert_eq!(
            posts[0].2,
            json!({"prompt": "a red circle", "n": 2, "quality": "final", "response_format": "url"})
        );
        assert_eq!(outcome.artifacts.len(), 2);
        let pack = Pack::from_toml_str(URL_PACK).unwrap();
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2, "catalog_version": "v", "rules": pack.pricing_rules()
            }))
            .unwrap(),
        )
        .unwrap();
        let snapshot = catalog.snapshot(&request).unwrap();
        // 0.1¢ per request + 2 × 6¢ per final image, held and settled exactly.
        assert_eq!(snapshot.estimated_units, 121_000);
        assert_eq!(
            snapshot
                .settle(outcome.usage.as_ref().unwrap(), snapshot.estimated_units)
                .unwrap(),
            121_000
        );

        // A response delivering one of two images settles only what arrived.
        let mut partial: Value = serde_json::from_str(URL_RESPONSE).unwrap();
        partial["data"].as_array_mut().unwrap().pop();
        let engine = super::tests::engine(URL_PACK, "image-v1", FakeTransport::ok(partial));
        let outcome = engine
            .invoke(&request, &url_input(), &secret_for_test("sk-test"), None)
            .unwrap();
        assert_eq!(outcome.usage.as_ref().unwrap().images, Some(1));
        assert_eq!(
            snapshot
                .settle(outcome.usage.as_ref().unwrap(), snapshot.estimated_units)
                .unwrap(),
            61_000
        );
    }

    #[test]
    fn reported_cost_cannot_replace_fixed_pricing_or_partial_delivery_adjustments() {
        let source = URL_PACK.replace("[response]", "[response]\nreported_cost = \"/cost\"");
        let pack = Pack::from_toml_str(&source).unwrap();
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2, "catalog_version": "fixed-v1", "rules": pack.pricing_rules()
            }))
            .unwrap(),
        )
        .unwrap();
        let request = pack.prepare("image-v1", &url_input()).unwrap().request;
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.estimated_units, 121_000);
        for delivered in [1, 2] {
            for reported_cost in [json!(0), json!("0.121"), json!("unreadable")] {
                let mut body: Value = serde_json::from_str(URL_RESPONSE).unwrap();
                body["data"].as_array_mut().unwrap().truncate(delivered);
                body["cost"] = reported_cost.clone();
                let transport = FakeTransport::ok(body);
                let posts = transport.posts.clone();
                let outcome = engine(&source, "image-v1", transport)
                    .invoke(&request, &url_input(), &secret_for_test("sk-test"), None)
                    .unwrap();
                assert_eq!(posts.lock().unwrap().len(), 1);
                assert_eq!(outcome.artifacts.len(), delivered);
                assert_eq!(
                    outcome.usage.as_ref().unwrap().images,
                    Some(delivered as i64)
                );
                assert_eq!(outcome.actual_vendor_cost, None, "{reported_cost}");
                // This is the workflow's settlement path. Passing the engine's
                // result must use frozen per-request plus delivered-image prices,
                // including when the provider reports zero or the entire hold.
                let settlement = snapshot
                    .settle_precise(
                        outcome.usage.as_ref().unwrap(),
                        outcome.actual_vendor_cost.as_ref(),
                        snapshot.estimated_units,
                    )
                    .unwrap();
                assert_eq!(
                    settlement.budget_units,
                    if delivered == 1 { 61_000 } else { 121_000 },
                    "reported {reported_cost}, delivered {delivered}"
                );
            }
        }
    }

    #[test]
    fn nested_template_header_request_id_and_filtered_inline_images() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(PNG);
        let body = json!({
            "id": "body-id",
            "steps": [
                {"type": "thought", "content": [{"type": "image", "data": "ignored"}]},
                {"type": "model_output", "content": [
                    {"type": "text", "text": "here you go"},
                    {"type": "image", "mime_type": "image/png", "data": encoded}
                ]}
            ]
        });
        let mut transport = FakeTransport::ok(body);
        if let Ok(response) = &mut transport.response {
            response
                .headers
                .insert("x-goog-request-id".into(), "goog-123".into());
        }
        let posts = transport.posts.clone();
        let engine = engine(INLINE_PACK, "inline-image-v1", transport);
        let input = json!({"prompt": "a lighthouse", "image_size": "2k"});
        let request = engine
            .pack
            .prepare("inline-image-v1", &input)
            .unwrap()
            .request;
        assert_eq!(request.image_count(), Some(1));
        let outcome = engine
            .invoke(&request, &input, &secret_for_test("goog-key"), None)
            .unwrap();
        let posts = posts.lock().unwrap();
        assert_eq!(
            posts[0].0,
            "https://api.inline-image.test/v1beta/interactions"
        );
        assert_eq!(
            posts[0].1,
            ("x-goog-api-key".to_owned(), "goog-key".to_owned())
        );
        assert_eq!(
            posts[0].2,
            json!({
                "model": "inline-image-v1",
                "input": [{"type": "text", "text": "a lighthouse"}],
                "response_format": {"type": "image", "image_size": "2K"}
            })
        );
        assert_eq!(outcome.provider_request_id.as_deref(), Some("goog-123"));
        assert_eq!(outcome.artifacts.len(), 1);
        assert_eq!(outcome.artifacts[0].bytes, PNG);
        assert_eq!(
            outcome.usage,
            Some(NormalizedUsage {
                images: Some(1),
                ..Default::default()
            })
        );
    }

    #[test]
    fn server_errors_and_unreadable_successes_go_to_reconciliation_with_evidence() {
        let mut failing = FakeTransport::status(503, Value::Null);
        if let Ok(response) = &mut failing.response {
            response
                .headers
                .insert("x-request-id".into(), "req-503".into());
        }
        let engine = engine(URL_PACK, "image-v1", failing);
        let error = engine
            .invoke(
                &url_request(&engine),
                &url_input(),
                &secret_for_test("k"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "provider_failure");
        assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);

        let engine = super::tests::engine(
            URL_PACK,
            "image-v1",
            FakeTransport::ok(json!({"id": "req-1", "data": []})),
        );
        let error = engine
            .invoke(
                &url_request(&engine),
                &url_input(),
                &secret_for_test("k"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "pack_response_artifacts_missing");
        assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
        assert_eq!(error.evidence.request_id.as_deref(), Some("req-1"));

        let engine = super::tests::engine(
            URL_PACK,
            "image-v1",
            FakeTransport {
                response: Err(TransportFailure::BeforeSend),
                ..FakeTransport::ok(Value::Null)
            },
        );
        let error = engine
            .invoke(
                &url_request(&engine),
                &url_input(),
                &secret_for_test("k"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.spend_disposition, SpendDisposition::Release);
    }

    #[test]
    fn engine_binds_only_sync_fixed_packs_and_known_models() {
        let pack = Arc::new(Pack::from_toml_str(URL_PACK).unwrap());
        assert!(SyncFixedEngine::new(pack.clone(), "image-v2", ReqwestSyncTransport, 1).is_err());
        assert!(SyncFixedEngine::new(pack.clone(), "image-v1", ReqwestSyncTransport, 0).is_err());
        let async_pack = Pack::from_toml_str(
            &URL_PACK.replace("archetype = \"sync_fixed\"", "archetype = \"async_poll\""),
        )
        .unwrap();
        assert!(
            SyncFixedEngine::new(Arc::new(async_pack), "image-v1", ReqwestSyncTransport, 1)
                .is_err()
        );
    }
}
