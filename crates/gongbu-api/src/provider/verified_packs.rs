//! Verified provider packs shipped with Gongbu.
//!
//! Each pack's pricing is reviewed and its targets are frozen into
//! `contracts/provider-contracts-v1.json` as the pack's checked-in projection.
//! Tests fail if a pack and its contract entries ever disagree.

use super::pack::Pack;

const GOOGLE_GEMINI_DEVELOPER_IMAGE: &str =
    include_str!("../../../../packs/verified/google-gemini-developer-image.toml");
const GOOGLE_GEMINI_DEVELOPER_IMAGE_EDIT: &str =
    include_str!("../../../../packs/verified/google-gemini-developer-image-edit.toml");
const BFL_FLUX_2: &str = include_str!("../../../../packs/verified/bfl-flux-2.toml");

/// Every verified pack, validated.
pub fn verified_packs() -> Vec<Pack> {
    [
        GOOGLE_GEMINI_DEVELOPER_IMAGE,
        GOOGLE_GEMINI_DEVELOPER_IMAGE_EDIT,
        BFL_FLUX_2,
    ]
    .into_iter()
    .map(|source| Pack::from_toml_str(source).expect("verified packs are valid"))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::{
            contract::{ProviderAdapter, SpendDisposition},
            http_kernel::{ArtifactDownloadPolicy, InvocationDeadline},
            pack::{host_allowed, ArtifactFetchError, Pack},
            pack_async::AsyncPollEngine,
            pack_sync::{SyncFixedEngine, SyncTransport, TransportFailure, TransportResponse},
        },
        secrets::secret_for_test,
    };
    use base64::Engine as _;
    use reqwest::Url;
    use serde_json::{json, Value};
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn pack(id: &str) -> Pack {
        verified_packs()
            .into_iter()
            .find(|pack| pack.pack_id() == id)
            .unwrap()
    }

    /// The contract document carries each verified pack's checked-in
    /// projection. Changing a pack without re-projecting it fails here.
    #[test]
    fn contract_entries_are_the_exact_projection_of_their_verified_packs() {
        let document: Value = serde_json::from_str(include_str!(
            "../../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        let mut projected = 0;
        for contract in document["contracts"].as_array().unwrap() {
            let target = &contract["target"];
            if target["settings"]["type"] != "pack" {
                continue;
            }
            let config = &target["settings"]["config"];
            let pack = pack(config["pack_id"].as_str().unwrap());
            assert_eq!(config, &serde_json::to_value(pack.target_config()).unwrap());
            let model = pack.model(target["model"].as_str().unwrap()).unwrap();
            assert_eq!(
                target["provider_config_version"],
                model.provider_config_version
            );
            assert_eq!(target["workload_type"], pack.workload_type());
            assert_eq!(target["provider"], pack.provider());
            assert_eq!(target["adapter"], pack.adapter_id());
            assert_eq!(contract["pricing_version"], model.pricing_version);
            assert_eq!(
                contract["pricing_reviewed_on"].as_str(),
                model.pricing_reviewed_on.as_deref()
            );
            let rules: Vec<Value> = pack
                .pricing_rules()
                .into_iter()
                .filter(|rule| rule.model == model.model)
                .map(|rule| serde_json::to_value(rule).unwrap())
                .collect();
            assert_eq!(contract["pricing_rules"].as_array().unwrap(), &rules);
            // Every advertised preset is a value the pack accepts for this model.
            for preset in contract["capability"]["presets"].as_array().unwrap() {
                let mut input = json!({"prompt": "x", "image_size": preset["name"]});
                if pack.parameters().iter().any(|p| p.name == "input_images") {
                    input["input_images"] = json!(["0f5c2a64-3f0e-4c9b-9a51-2b7d1c4e8a10"]);
                }
                if pack.parameters().iter().any(|p| p.name == "options.width") {
                    input["options"] =
                        json!({"width": preset["width"], "height": preset["height"]});
                }
                assert!(pack.prepare(&model.model, &input).is_ok(), "{preset}");
            }
            projected += 1;
        }
        assert_eq!(
            projected, 5,
            "every shipped contract comes from a verified pack"
        );
    }

    struct Transport(TransportResponse);
    impl SyncTransport for Transport {
        fn post_json(
            &self,
            _: &Url,
            _: (&str, &str),
            _: &BTreeMap<String, String>,
            _: &Value,
            _: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            Ok(self.0.clone())
        }
        fn download(
            &self,
            _: &Url,
            _: &ArtifactDownloadPolicy,
            _: InvocationDeadline,
        ) -> Result<Vec<u8>, ArtifactFetchError> {
            Err(ArtifactFetchError::Failed)
        }
    }

    fn gemini(status: u16, body: Value) -> crate::provider::contract::ProviderFailure {
        let pack = Arc::new(pack("google-gemini-developer-image"));
        let input = json!({"prompt": "a cat", "image_size": "2k"});
        let request = pack
            .prepare("gemini-3.1-flash-image", &input)
            .unwrap()
            .request;
        SyncFixedEngine::new(
            pack,
            "gemini-3.1-flash-image",
            Transport(TransportResponse {
                status,
                headers: BTreeMap::from([("x-goog-request-id".into(), "req-1".into())]),
                body,
            }),
            1024 * 1024,
        )
        .unwrap()
        .invoke(&request, &input, &secret_for_test("k"), None)
        .unwrap_err()
    }

    #[test]
    fn gemini_pack_keeps_the_adapter_failure_behavior() {
        let image = |data: &str| json!({"type": "image", "mime_type": "image/png", "data": data});
        let encoded = base64::engine::general_purpose::STANDARD.encode(PNG);
        // Two images for a one-image request is an artifact policy failure.
        let two = gemini(
            200,
            json!({"steps": [{"type": "model_output", "content": [image(&encoded), image(&encoded)]}]}),
        );
        assert_eq!(two.code, "artifact_policy_failure");
        assert_eq!(two.spend_disposition, SpendDisposition::Reconcile);
        assert_eq!(two.evidence.request_id.as_deref(), Some("req-1"));
        // No image after acceptance must be reconciled, never released.
        let none = gemini(
            200,
            json!({"steps": [{"type": "model_output", "content": []}]}),
        );
        assert_eq!(none.spend_disposition, SpendDisposition::Reconcile);
        // A 4xx is a definitive rejection.
        let rejected = gemini(400, Value::Null);
        assert_eq!(rejected.spend_disposition, SpendDisposition::Release);
        // A server error is ambiguous.
        assert_eq!(
            gemini(500, Value::Null).spend_disposition,
            SpendDisposition::Reconcile
        );
        // Lite advertises only 1k; larger sizes are refused before any spend.
        let lite = pack("google-gemini-developer-image");
        assert!(lite
            .prepare(
                "gemini-3.1-flash-lite-image",
                &json!({"prompt": "x", "image_size": "2k"})
            )
            .is_err());
        assert!(lite
            .prepare("gemini-3.1-flash-lite-image", &json!({"prompt": "x"}))
            .is_err());
    }

    #[test]
    fn flux_pack_keeps_the_adapter_input_contract_and_request_body() {
        let flux = pack("bfl-flux-2");
        let prepared = flux
            .prepare(
                "flux-2-pro",
                &json!({"prompt": "a lighthouse", "image_count": 1, "image_size": "2k",
                        "options": {"width": 1920, "height": 1088, "seed": 7,
                                    "safety_tolerance": 2, "output_format": "jpeg"}}),
            )
            .unwrap();
        assert_eq!(
            prepared.body,
            json!({"prompt": "a lighthouse", "width": 1920, "height": 1088, "seed": 7,
                   "safety_tolerance": 2, "output_format": "jpeg"})
        );
        let dimensions = prepared.request.output_dimensions.unwrap();
        assert_eq!((dimensions.width, dimensions.height), (1920, 1088));
        assert_eq!(
            flux.request_url().as_str(),
            "https://api.bfl.ai/v1/flux-2-pro"
        );
        // Optional options are left out of the body when the agent omits them.
        let minimal = flux
            .prepare(
                "flux-2-pro",
                &json!({"prompt": "x", "image_size": "4k", "options": {"width": 2048, "height": 2048}}),
            )
            .unwrap();
        assert_eq!(
            minimal.body,
            json!({"prompt": "x", "width": 2048, "height": 2048})
        );
        for input in [
            json!({"prompt": "x", "image_size": "1k"}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 1024}}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 2048, "height": 2048}}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 1024, "height": 1024, "safety_tolerance": 6}}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 1024, "height": 1024, "output_format": "webp"}}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 1024, "height": 1024, "steps": 20}}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 1024, "height": 1024}, "steps": 20}),
            json!({"prompt": "x", "image_size": "8k", "options": {"width": 1024, "height": 1024}}),
            json!({"prompt": "x", "image_size": "1k", "options": "not-an-object"}),
        ] {
            assert!(flux.prepare("flux-2-pro", &input).is_err(), "{input}");
        }
    }

    #[test]
    fn flux_pack_polls_and_downloads_only_from_bfl_host_families() {
        let flux = pack("bfl-flux-2");
        for host in [
            "api.bfl.ai",
            "api.eu.bfl.ai",
            "api.us.bfl.ai",
            "api.us1.bfl.ai",
            "api.us7.bfl.ai",
            "api.eu-2.bfl.ai",
        ] {
            assert!(flux.poll_url(host, "job-1").is_some(), "{host}");
        }
        for host in [
            "api.bfl.ai.evil.example",
            "evil.api.us7.bfl.ai",
            "api.us7.bfl.ai.",
            "api.us7-bfl.ai",
            "api.us.east.bfl.ai",
            "api.-us7.bfl.ai",
            "api.us7-.bfl.ai",
            "api.xn--us7-9za.bfl.ai",
            "delivery.us7.bfl.ai",
        ] {
            assert!(flux.poll_url(host, "job-1").is_none(), "{host}");
        }
        let delivery = &flux.artifacts().hosts;
        for host in [
            "delivery.us2.bfl.ai",
            "delivery.us7.bfl.ai",
            "delivery.eu-2.bfl.ai",
        ] {
            assert!(host_allowed(delivery, host), "{host}");
        }
        for host in [
            "delivery.us2.bfl.ai.evil.example",
            "evil.delivery.us2.bfl.ai",
            "delivery.us2-bfl.ai",
            "delivery.us2.bfl.ai.",
            "delivery.xn--us7-9za.bfl.ai",
            "delivery.us.east.bfl.ai",
            "cdn.bfl.ai",
            "api.us2.bfl.ai",
        ] {
            assert!(!host_allowed(delivery, host), "{host}");
        }
    }

    #[derive(Clone)]
    struct FluxTransport {
        submit: Result<TransportResponse, TransportFailure>,
        polls: Arc<std::sync::Mutex<Vec<TransportResponse>>>,
        posts: Arc<std::sync::Mutex<u32>>,
        artifact: Vec<u8>,
    }
    impl SyncTransport for FluxTransport {
        fn post_json(
            &self,
            _: &Url,
            auth: (&str, &str),
            _: &BTreeMap<String, String>,
            _: &Value,
            _: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            assert_eq!(auth.0, "x-key");
            *self.posts.lock().unwrap() += 1;
            self.submit.clone()
        }
        fn get_json(
            &self,
            _: &Url,
            _: (&str, &str),
            _: &BTreeMap<String, String>,
            _: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            let mut polls = self.polls.lock().unwrap();
            if polls.is_empty() {
                return Err(TransportFailure::UnknownOutcome { request_id: None });
            }
            Ok(polls.remove(0))
        }
        fn download(
            &self,
            url: &Url,
            policy: &ArtifactDownloadPolicy,
            _: InvocationDeadline,
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

    fn bfl(status: u16, body: Value) -> TransportResponse {
        TransportResponse {
            status,
            headers: BTreeMap::from([("x-request-id".into(), "bfl-req-1".into())]),
            body,
        }
    }

    fn flux_input() -> Value {
        json!({"prompt": "a lighthouse", "image_count": 1, "image_size": "1k",
               "options": {"width": 1024, "height": 1024}})
    }

    fn flux_run(
        submit: Result<TransportResponse, TransportFailure>,
        polls: Vec<TransportResponse>,
        artifact: Vec<u8>,
        mutate: impl FnOnce(&mut crate::provider::contract::NormalizedRequest),
        key: Option<&str>,
    ) -> (
        Result<
            crate::provider::contract::AdapterOutcome,
            crate::provider::contract::ProviderFailure,
        >,
        u32,
    ) {
        let flux = Arc::new(pack("bfl-flux-2"));
        let mut request = flux.prepare("flux-2-pro", &flux_input()).unwrap().request;
        mutate(&mut request);
        let transport = FluxTransport {
            submit,
            polls: Arc::new(std::sync::Mutex::new(polls)),
            posts: Arc::default(),
            artifact,
        };
        let posts = transport.posts.clone();
        let mut engine = AsyncPollEngine::new(flux, "flux-2-pro", transport, 1024).unwrap();
        engine = engine.with_poll_interval_for_tests(Duration::from_millis(1));
        let result = engine.invoke(
            &request,
            &flux_input(),
            &secret_for_test("secret-canary"),
            key,
        );
        let submissions = *posts.lock().unwrap();
        (result, submissions)
    }

    fn accepted() -> TransportResponse {
        bfl(
            200,
            json!({"id": "job-1", "polling_url": "https://api.us7.bfl.ai/v1/get_result?id=job-1"}),
        )
    }

    fn ready(sample: &str) -> TransportResponse {
        bfl(
            200,
            json!({"id": "job-1", "status": "Ready", "cost": 3,
                         "result": {"sample": sample}}),
        )
    }

    #[test]
    fn flux_pack_passes_executor_conformance_on_the_async_engine() {
        use crate::provider::conformance::{assert_adapter_conformance, Case, Observation};
        let good = "https://delivery.us7.bfl.ai/out.png";
        assert_adapter_conformance(|case| {
            let (result, submissions) = match case {
                Case::Rejection => flux_run(
                    Ok(bfl(400, Value::Null)),
                    vec![],
                    PNG.to_vec(),
                    |_| {},
                    None,
                ),
                Case::AmbiguousPostSend => flux_run(
                    Err(TransportFailure::UnknownOutcome { request_id: None }),
                    vec![],
                    PNG.to_vec(),
                    |_| {},
                    None,
                ),
                Case::EvidenceRetention => flux_run(
                    Ok(accepted()),
                    vec![ready(good)],
                    b"nope".to_vec(),
                    |_| {},
                    None,
                ),
                Case::HostPolicy => flux_run(
                    Ok(accepted()),
                    vec![ready("https://evil.example/out.png")],
                    PNG.to_vec(),
                    |_| {},
                    None,
                ),
                Case::ArtifactBound => flux_run(
                    Ok(accepted()),
                    vec![ready(good)],
                    vec![0; 1025],
                    |_| {},
                    None,
                ),
                Case::UnsafeRetry => flux_run(
                    Ok(accepted()),
                    vec![ready(good)],
                    PNG.to_vec(),
                    |_| {},
                    Some("opaque-key"),
                ),
                Case::InvalidRequest => flux_run(
                    Ok(accepted()),
                    vec![ready(good)],
                    PNG.to_vec(),
                    |request| request.set_quantity("image_count", Some(2)),
                    None,
                ),
            };
            Observation {
                result,
                submissions,
            }
        });
    }

    #[test]
    fn flux_pack_maps_bfl_statuses_and_settles_credits_exactly() {
        let good = "https://delivery.us7.bfl.ai/out.png";
        let (outcome, submissions) = flux_run(
            Ok(accepted()),
            vec![
                bfl(200, json!({"id": "job-1", "status": "Pending"})),
                ready(good),
            ],
            PNG.to_vec(),
            |_| {},
            None,
        );
        let outcome = outcome.unwrap();
        assert_eq!(submissions, 1);
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("job-1"));
        assert_eq!(outcome.provider_request_id.as_deref(), Some("bfl-req-1"));
        // 3 credits are exactly USD 0.03, the frozen 1k price.
        assert_eq!(
            outcome.actual_vendor_cost,
            Some(crate::provider::contract::ActualVendorCost::new(3, 2, "USD").unwrap())
        );
        for (status, code, disposition) in [
            (
                "Request Moderated",
                "provider_request_moderated",
                SpendDisposition::Release,
            ),
            (
                "Content Moderated",
                "provider_content_moderated",
                SpendDisposition::Release,
            ),
            ("Error", "provider_error", SpendDisposition::Release),
            (
                "Task not found",
                "provider_task_not_found",
                SpendDisposition::Reconcile,
            ),
        ] {
            let (result, _) = flux_run(
                Ok(accepted()),
                vec![bfl(200, json!({"id": "job-1", "status": status}))],
                PNG.to_vec(),
                |_| {},
                None,
            );
            let failure = result.unwrap_err();
            assert_eq!(
                (failure.code.as_str(), failure.spend_disposition),
                (code, disposition)
            );
            assert_eq!(failure.evidence.operation_id.as_deref(), Some("job-1"));
        }
    }

    #[test]
    #[ignore = "explicit live Google spend; see docs/operations/live-provider-testing.md"]
    fn live_gemini_pack_e2e_requires_explicit_spend_guard() {
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
        let model = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_MODEL")
            .expect("exact Gemini image model is required");
        let pack = Arc::new(pack("google-gemini-developer-image"));
        assert!(pack.model(&model).is_some(), "unknown Gemini image model");
        let target = targets
            .revisions()
            .find(|target| {
                target.provider == pack.provider()
                    && target.adapter == pack.adapter_id()
                    && target.model == model
                    && target.is_active()
                    && target
                        .pack()
                        .is_some_and(|config| config.pack_digest == pack.digest())
            })
            .expect("the selected enabled Gemini pack target is required");
        let prompt =
            env::var("GONGBU_LIVE_GEMINI_DEVELOPER_PROMPT").expect("explicit prompt is required");
        let image_size = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_IMAGE_SIZE")
            .expect("explicit image size is required");
        let input = json!({"prompt": prompt, "image_size": image_size});
        let request = pack.prepare(&model, &input).unwrap().request;
        let snapshot = PricingCatalog::load(
            env::var("GONGBU_PRICING_CATALOG").expect("pricing catalog is required"),
        )
        .unwrap()
        .snapshot(&request)
        .unwrap();
        let max_spend = max_spend * crate::provider_contract::UNITS_PER_MINOR;
        enforce_cost(&snapshot, max_spend, "USD").unwrap();
        let engine =
            SyncFixedEngine::from_target(target, &pack, crate::artifact::DEFAULT_MAX_ENCODED_BYTES)
                .unwrap();
        let secret = preflight_selected_secret(&engine, &MacOsKeychain, target, &request).unwrap();
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
        let outcome = engine.invoke(&request, &input, &secret, None).unwrap();
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
            snapshot.estimated_units
        );
    }
}
