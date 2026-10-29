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
const GOOGLE_GEMINI_DEVELOPER_IMAGE_V2: &str =
    include_str!("../../../../packs/verified/google-gemini-developer-image-v2.toml");
const GOOGLE_GEMINI_DEVELOPER_IMAGE_EDIT_V2: &str =
    include_str!("../../../../packs/verified/google-gemini-developer-image-edit-v2.toml");
const BFL_FLUX_2: &str = include_str!("../../../../packs/verified/bfl-flux-2.toml");
const BFL_FLUX_2_V2: &str = include_str!("../../../../packs/verified/bfl-flux-2-v2.toml");
const BFL_FLUX_3_IMAGE: &str = include_str!("../../../../packs/verified/bfl-flux-3-image.toml");

/// Every verified pack, validated.
pub fn verified_packs() -> Vec<Pack> {
    [
        GOOGLE_GEMINI_DEVELOPER_IMAGE,
        GOOGLE_GEMINI_DEVELOPER_IMAGE_EDIT,
        GOOGLE_GEMINI_DEVELOPER_IMAGE_V2,
        GOOGLE_GEMINI_DEVELOPER_IMAGE_EDIT_V2,
        BFL_FLUX_2,
        BFL_FLUX_2_V2,
        BFL_FLUX_3_IMAGE,
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
            let pack = verified_packs()
                .into_iter()
                .find(|pack| {
                    pack.pack_id() == config["pack_id"]
                        && pack.pack_version() == config["pack_version"]
                })
                .unwrap();
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
            // Every advertised preset is a value the pack accepts for this
            // model, at the size it advertises.
            let presets = contract["capability"]["presets"].as_array().unwrap();
            for preset in presets {
                let mut input = json!({"prompt": "x", "image_size": preset["name"]});
                if pack.parameters().iter().any(|p| p.name == "input_images") {
                    input["input_images"] = json!(["0f5c2a64-3f0e-4c9b-9a51-2b7d1c4e8a10"]);
                }
                for (name, value) in preset["when"].as_object().into_iter().flatten() {
                    input[name] = value.clone();
                }
                if pack.parameters().iter().any(|p| p.name == "options.width") {
                    input["options"] =
                        json!({"width": preset["width"], "height": preset["height"]});
                }
                let prepared = pack.prepare(&model.model, &input).unwrap_or_else(|e| {
                    panic!("{preset}: {e}");
                });
                if let Some(size) = prepared.request.output_dimensions {
                    assert_eq!(
                        (json!(size.width), json!(size.height)),
                        (preset["width"].clone(), preset["height"].clone())
                    );
                }
            }
            // A pack that certifies sizes by combination advertises exactly
            // those sizes, and its megapixel rule.
            if pack.has_size_presets() {
                let advertised: Vec<Value> = presets
                    .iter()
                    .map(|p| json!({"when": p["when"], "width": p["width"], "height": p["height"]}))
                    .collect();
                assert_eq!(
                    json!(advertised),
                    serde_json::to_value(pack.output_sizes(&model.model)).unwrap()
                );
            }
            assert_eq!(
                contract["capability"].get("megapixels"),
                pack.megapixel_rule()
                    .map(|rule| serde_json::to_value(rule).unwrap())
                    .as_ref()
            );
            projected += 1;
        }
        assert_eq!(
            projected, 15,
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

    fn flux_v2() -> Pack {
        verified_packs()
            .into_iter()
            .find(|pack| pack.pack_id() == "bfl-flux-2" && pack.pack_version() == "2026-10-21.1")
            .unwrap()
    }

    fn snapshot(pack: &Pack, input: &Value) -> crate::provider::contract::PricingSnapshot {
        snapshot_for(pack, "flux-2-pro", input)
    }

    fn snapshot_for(
        pack: &Pack,
        model: &str,
        input: &Value,
    ) -> crate::provider::contract::PricingSnapshot {
        let catalog = crate::provider::contract::PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": pack.models()[0].pricing_version,
                "rules": pack.pricing_rules(),
            }))
            .unwrap(),
        )
        .unwrap();
        catalog
            .snapshot(&pack.prepare(model, input).unwrap().request)
            .unwrap()
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::GrayImage::new(width, height)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageOutputFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// Every v2 size bills BFL's FLUX.2 [pro] rates (3¢ for the first
    /// megapixel, 1.5¢ for each additional binary megapixel, rounded up), so
    /// each tier keeps the v1 price at every aspect ratio.
    #[test]
    fn flux_v2_prices_every_aspect_ratio_at_bfl_megapixel_rates() {
        let (v1, v2) = (pack("bfl-flux-2"), flux_v2());
        let sizes = v2.output_sizes("flux-2-pro");
        assert_eq!(sizes.len(), 18);
        for size in &sizes {
            let tier = size.when["image_size"].as_str();
            let mut input = json!({"prompt": "x"});
            for (name, value) in &size.when {
                input[name] = json!(value);
            }
            let frozen = snapshot(&v2, &input);
            let expected = match tier {
                "1k" => 30_000,
                "2k" => 45_000,
                _ => 75_000,
            };
            assert_eq!(frozen.estimated_units, expected, "{size:?}");
            let (width, height) = (size.width, size.height);
            assert!(width % 16 == 0 && height % 16 == 0);
            assert!(u64::from(width) * u64::from(height) <= 2048 * 2048);
            // The same tier at v1's certified size costs the same.
            let v1_size = crate::provider::flux_contract::contract_dimensions(tier).unwrap();
            let v1_input = json!({"prompt": "x", "image_size": tier,
                "options": {"width": v1_size.width, "height": v1_size.height}});
            assert_eq!(snapshot(&v1, &v1_input).estimated_units, expected);
        }
        // 2k 16:9 is v1's 2k size; only the size is sent to BFL.
        let header = v2
            .prepare(
                "flux-2-pro",
                &json!({"prompt": "a header", "image_size": "2k", "aspect_ratio": "16:9",
                        "options": {"seed": 7}}),
            )
            .unwrap();
        assert_eq!(
            header.body,
            json!({"prompt": "a header", "width": 1920, "height": 1088, "seed": 7})
        );
        // Size options are optional, but must match the certified size if sent.
        for (options, ok) in [
            (json!({"width": 1920, "height": 1088}), true),
            (json!({"width": 1024, "height": 1024}), false),
        ] {
            let input = json!({"prompt": "x", "image_size": "2k", "aspect_ratio": "16:9",
                               "options": options});
            assert_eq!(v2.prepare("flux-2-pro", &input).is_ok(), ok, "{input}");
        }
        // A ratio the contract does not certify is refused before any spend.
        assert!(v2
            .prepare(
                "flux-2-pro",
                &json!({"prompt": "x", "image_size": "1k", "aspect_ratio": "21:9"})
            )
            .is_err());
    }

    #[test]
    fn flux_v2_settles_the_delivered_megapixels() {
        let v2 = Arc::new(flux_v2());
        let input = json!({"prompt": "a banner", "image_size": "4k", "aspect_ratio": "16:9"});
        let frozen = snapshot(&v2, &input);
        assert_eq!(frozen.estimated_units, 75_000);
        let run = |artifact: Vec<u8>, credits: f64| {
            let request = v2.prepare("flux-2-pro", &input).unwrap().request;
            let transport = FluxTransport {
                submit: Ok(accepted()),
                polls: Arc::new(std::sync::Mutex::new(vec![bfl(
                    200,
                    json!({"id": "job-1", "status": "Ready", "cost": credits,
                           "result": {"sample": "https://delivery.us7.bfl.ai/out.png"}}),
                )])),
                posts: Arc::default(),
                artifact,
            };
            AsyncPollEngine::new(v2.clone(), "flux-2-pro", transport, 1 << 20)
                .unwrap()
                .with_poll_interval_for_tests(Duration::from_millis(1))
                .invoke(&request, &input, &secret_for_test("secret-canary"), None)
                .unwrap()
        };
        // The certified 2688 × 1536 image: 4 billed megapixels, 7.5 credits.
        let outcome = run(png(2688, 1536), 7.5);
        let usage = outcome.usage.clone().unwrap();
        assert_eq!(
            (usage.megapixels, usage.additional_megapixels),
            (Some(4), Some(3))
        );
        let settled = frozen
            .settle_precise(&usage, outcome.actual_vendor_cost.as_ref(), 75_000)
            .unwrap();
        assert_eq!(settled.budget_units, 75_000);
        // An image larger than was held is an overage, never a larger charge.
        let oversized = run(png(4096, 2048), 7.5).usage.unwrap();
        assert_eq!(
            frozen.settle(&oversized, 75_000),
            Err(crate::provider::contract::ContractError::SettlementOverage)
        );
    }

    #[test]
    fn flux_3_sends_tier_and_ratio_without_grounding_and_prices_by_tier() {
        let flux3 = pack("bfl-flux-3-image");
        assert_eq!(
            flux3.request_url().as_str(),
            "https://api.bfl.ai/v1/flux-3-image"
        );
        let prepared = flux3
            .prepare(
                "flux-3-image",
                &json!({"prompt": "a skyline", "image_size": "2k", "aspect_ratio": "21:9",
                        "options": {"safety_tolerance": 2}}),
            )
            .unwrap();
        // Only fields BFL's FLUX 3 schema accepts; web search stays off.
        assert_eq!(
            prepared.body,
            json!({"prompt": "a skyline", "resolution": "2k", "aspect_ratio": "21:9",
                   "safety_tolerance": 2, "grounding": false})
        );
        assert_eq!(prepared.request.output_dimensions, None);
        let square = flux3
            .prepare("flux-3-image", &json!({"prompt": "x", "image_size": "1k"}))
            .unwrap();
        assert_eq!(square.body["aspect_ratio"], "1:1");
        // Every ratio in a tier costs the same: 4.8¢ at 1k, 10¢ at 2k.
        for (tier, units) in [("1k", 48_000), ("2k", 100_000)] {
            for ratio in ["21:9", "1:1", "9:21"] {
                let frozen = snapshot_for(
                    &flux3,
                    "flux-3-image",
                    &json!({"prompt": "x", "image_size": tier, "aspect_ratio": ratio}),
                );
                assert_eq!(frozen.estimated_units, units, "{tier} {ratio}");
            }
        }
        // Not offered: 4k, auto ratio, seed, size options and unknown fields.
        for input in [
            json!({"prompt": "x", "image_size": "4k"}),
            json!({"prompt": "x", "image_size": "1k", "aspect_ratio": "auto"}),
            json!({"prompt": "x", "image_size": "1k", "options": {"seed": 1}}),
            json!({"prompt": "x", "image_size": "1k", "options": {"width": 1024}}),
            json!({"prompt": "x", "image_size": "1k", "grounding": true}),
            json!({"prompt": "x"}),
        ] {
            assert!(flux3.prepare("flux-3-image", &input).is_err(), "{input}");
        }
    }

    #[test]
    fn flux_3_polls_bfl_and_settles_reported_credits() {
        let flux3 = Arc::new(pack("bfl-flux-3-image"));
        let input = json!({"prompt": "a skyline", "image_size": "1k", "aspect_ratio": "16:9"});
        let request = flux3.prepare("flux-3-image", &input).unwrap().request;
        let run = |polls: Vec<TransportResponse>| {
            let transport = FluxTransport {
                submit: Ok(accepted()),
                polls: Arc::new(std::sync::Mutex::new(polls)),
                posts: Arc::default(),
                artifact: PNG.to_vec(),
            };
            let posts = transport.posts.clone();
            let result = AsyncPollEngine::new(flux3.clone(), "flux-3-image", transport, 1 << 20)
                .unwrap()
                .with_poll_interval_for_tests(Duration::from_millis(1))
                .invoke(&request, &input, &secret_for_test("secret-canary"), None);
            let submissions = *posts.lock().unwrap();
            (result, submissions)
        };
        let (outcome, submissions) = run(vec![
            bfl(200, json!({"id": "job-1", "status": "Reasoning"})),
            bfl(
                200,
                json!({"id": "job-1", "status": "Ready", "cost": 4.8,
                       "result": {"sample": "https://delivery.eu2.bfl.ai/out.png"}}),
            ),
        ]);
        let outcome = outcome.unwrap();
        assert_eq!(submissions, 1);
        // 4.8 credits are exactly USD 0.048, the frozen 1k price.
        let frozen = snapshot_for(&flux3, "flux-3-image", &input);
        let settled = frozen
            .settle_precise(
                outcome.usage.as_ref().unwrap(),
                outcome.actual_vendor_cost.as_ref(),
                frozen.estimated_units,
            )
            .unwrap();
        assert_eq!(settled.budget_units, 48_000);
        let (moderated, _) = run(vec![bfl(
            200,
            json!({"id": "job-1", "status": "Content Moderated"}),
        )]);
        let failure = moderated.unwrap_err();
        assert_eq!(failure.code, "provider_content_moderated");
        assert_eq!(failure.spend_disposition, SpendDisposition::Release);
    }

    /// The v2 Gemini packs (HUB-252): aspect ratios and Gemini 3 Pro Image.
    const GEMINI_V2: &str = "2026-10-28.1";

    fn pack_at(id: &str, version: &str) -> Pack {
        verified_packs()
            .into_iter()
            .find(|pack| pack.pack_id() == id && pack.pack_version() == version)
            .unwrap()
    }

    #[derive(Clone)]
    struct CapturingTransport {
        response: TransportResponse,
        sent: Arc<std::sync::Mutex<Vec<Value>>>,
    }
    impl SyncTransport for CapturingTransport {
        fn post_json(
            &self,
            url: &Url,
            auth: (&str, &str),
            _: &BTreeMap<String, String>,
            body: &Value,
            _: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            assert_eq!(
                url.as_str(),
                "https://generativelanguage.googleapis.com/v1beta/interactions"
            );
            assert_eq!(auth, ("x-goog-api-key", "k"));
            self.sent.lock().unwrap().push(body.clone());
            Ok(self.response.clone())
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

    #[test]
    fn gemini_v2_sends_the_aspect_ratio_and_certifies_googles_size_for_it() {
        let gemini = pack_at("google-gemini-developer-image", GEMINI_V2);
        // Sizes from Google's image generation guide, per model and tier.
        for (model, tier, ratio, size) in [
            ("gemini-3-pro-image", "1k", "16:9", (1376, 768)),
            ("gemini-3-pro-image", "2k", "21:9", (3168, 1344)),
            ("gemini-3-pro-image", "4k", "9:16", (3072, 5504)),
            ("gemini-3.1-flash-image", "2k", "4:5", (1856, 2304)),
            ("gemini-3.1-flash-image", "4k", "3:2", (5056, 3392)),
            ("gemini-nano-banana-2.1", "2k", "16:9", (2752, 1536)),
            ("gemini-nano-banana-2.1", "4k", "2:3", (3392, 5056)),
            ("gemini-3.1-flash-lite-image", "1k", "3:4", (896, 1200)),
        ] {
            let prepared = gemini
                .prepare(
                    model,
                    &json!({"prompt": "a poster", "image_size": tier, "aspect_ratio": ratio}),
                )
                .unwrap();
            assert_eq!(
                prepared.body,
                json!({"model": model, "input": [{"type": "text", "text": "a poster"}],
                       "response_format": {"type": "image", "image_size": tier.to_uppercase(),
                                           "aspect_ratio": ratio}}),
                "{model} {tier} {ratio}"
            );
            let dimensions = prepared.request.output_dimensions.unwrap();
            assert_eq!((dimensions.width, dimensions.height), size, "{model}");
        }
        // Without an aspect ratio, Google's text-to-image default: a square.
        let square = gemini
            .prepare(
                "gemini-3-pro-image",
                &json!({"prompt": "x", "image_size": "2k"}),
            )
            .unwrap();
        assert_eq!(square.body["response_format"]["aspect_ratio"], "1:1");
        let dimensions = square.request.output_dimensions.unwrap();
        assert_eq!((dimensions.width, dimensions.height), (2048, 2048));
        assert_eq!(gemini.output_sizes("gemini-3-pro-image").len(), 30);
        assert_eq!(gemini.output_sizes("gemini-nano-banana-2.1").len(), 30);
        assert_eq!(gemini.output_sizes("gemini-3.1-flash-lite-image").len(), 10);
        // Not offered: Lite above 1k, ratios outside the shared published
        // set, 0.5k, more than one image, or no tier.
        for (model, input) in [
            (
                "gemini-3.1-flash-lite-image",
                json!({"prompt": "x", "image_size": "2k"}),
            ),
            (
                "gemini-3-pro-image",
                json!({"prompt": "x", "image_size": "1k", "aspect_ratio": "1:4"}),
            ),
            (
                "gemini-3-pro-image",
                json!({"prompt": "x", "image_size": "1k", "aspect_ratio": "auto"}),
            ),
            (
                "gemini-3.1-flash-image",
                json!({"prompt": "x", "image_size": "0.5k"}),
            ),
            (
                "gemini-nano-banana-2.1",
                json!({"prompt": "x", "image_size": "1k", "aspect_ratio": "8:1"}),
            ),
            (
                "gemini-3-pro-image",
                json!({"prompt": "x", "image_size": "1k", "image_count": 2}),
            ),
            ("gemini-3-pro-image", json!({"prompt": "x"})),
        ] {
            assert!(gemini.prepare(model, &input).is_err(), "{model} {input}");
        }
        // The edit pack never guesses a shape: the agent names the ratio.
        let edit = pack_at("google-gemini-developer-image-edit", GEMINI_V2);
        let mut input = json!({"prompt": "x", "image_size": "1k",
            "input_images": ["0f5c2a64-3f0e-4c9b-9a51-2b7d1c4e8a10"]});
        assert!(edit.prepare("gemini-3-pro-image", &input).is_err());
        input["aspect_ratio"] = json!("4:3");
        let dimensions = edit
            .prepare("gemini-3-pro-image", &input)
            .unwrap()
            .request
            .output_dimensions
            .unwrap();
        assert_eq!((dimensions.width, dimensions.height), (1200, 896));
    }

    #[test]
    fn gemini_v2_prices_per_image_by_tier_at_every_aspect_ratio() {
        let gemini = pack_at("google-gemini-developer-image", GEMINI_V2);
        // 10,000 units are one cent: Pro is 13.4¢ at 1k and 2k, 24¢ at 4k;
        // Flash and Lite keep their v1 rates.
        for (model, tier, units) in [
            ("gemini-3-pro-image", "1k", 134_000),
            ("gemini-3-pro-image", "2k", 134_000),
            ("gemini-3-pro-image", "4k", 240_000),
            ("gemini-3.1-flash-image", "1k", 67_000),
            ("gemini-3.1-flash-image", "2k", 101_000),
            ("gemini-3.1-flash-image", "4k", 151_000),
            ("gemini-3.1-flash-lite-image", "1k", 33_600),
            ("gemini-nano-banana-2.1", "1k", 33_600),
            ("gemini-nano-banana-2.1", "2k", 50_400),
            ("gemini-nano-banana-2.1", "4k", 113_000),
        ] {
            for ratio in ["1:1", "21:9", "9:16"] {
                let frozen = snapshot_for(
                    &gemini,
                    model,
                    &json!({"prompt": "x", "image_size": tier, "aspect_ratio": ratio}),
                );
                assert_eq!(frozen.estimated_units, units, "{model} {tier} {ratio}");
            }
        }
        // A Pro edit adds 0.112¢ per input image: two inputs at 4k are
        // 24 + 2 × 0.112 = 24.224¢. Flash keeps 0.056¢, Lite 0.028¢, and
        // Nano Banana 2.1 bills 1,120 tokens at $1.50 per 1M: 0.168¢.
        let edit = pack_at("google-gemini-developer-image-edit", GEMINI_V2);
        let inputs = json!([
            "0f5c2a64-3f0e-4c9b-9a51-2b7d1c4e8a10",
            "1a6d3b75-4f1f-4dad-8b62-3c8e2d5f9b21"
        ]);
        for (model, tier, units) in [
            ("gemini-3-pro-image", "4k", 242_240),
            ("gemini-3-pro-image", "1k", 136_240),
            ("gemini-3.1-flash-image", "1k", 68_120),
            ("gemini-3.1-flash-lite-image", "1k", 34_160),
            ("gemini-nano-banana-2.1", "2k", 53_760),
        ] {
            let frozen = snapshot_for(
                &edit,
                model,
                &json!({"prompt": "x", "image_size": tier, "aspect_ratio": "16:9",
                        "input_images": inputs}),
            );
            assert_eq!(frozen.estimated_units, units, "{model} {tier}");
        }
    }

    #[test]
    fn gemini_pro_edit_sends_its_inputs_and_settles_the_frozen_price() {
        struct Drafts;
        impl crate::provider::contract::InputImageSource for Drafts {
            fn load(
                &self,
                _: &str,
            ) -> crate::provider::contract::Result<crate::provider::contract::InputImage>
            {
                Ok(crate::provider::contract::InputImage {
                    media_type: "image/png".into(),
                    bytes: PNG.to_vec(),
                })
            }
        }
        let edit = Arc::new(pack_at("google-gemini-developer-image-edit", GEMINI_V2));
        let encoded = base64::engine::general_purpose::STANDARD.encode(PNG);
        let input = json!({"prompt": "make it dusk", "image_size": "2k", "aspect_ratio": "16:9",
            "input_images": ["0f5c2a64-3f0e-4c9b-9a51-2b7d1c4e8a10"]});
        let request = edit.prepare("gemini-3-pro-image", &input).unwrap().request;
        let transport = CapturingTransport {
            response: TransportResponse {
                status: 200,
                headers: BTreeMap::from([("x-goog-request-id".into(), "req-pro".into())]),
                body: json!({
                    "id": "interaction-1",
                    "steps": [{"type": "model_output", "content": [
                        {"type": "image", "mime_type": "image/png", "data": encoded}
                    ]}],
                    "usage": {"total_input_tokens": 580, "total_output_tokens": 1400,
                              "total_thought_tokens": 260}
                }),
            },
            sent: Arc::default(),
        };
        let sent = transport.sent.clone();
        let engine =
            SyncFixedEngine::new(edit.clone(), "gemini-3-pro-image", transport, 1 << 20).unwrap();
        let submission = engine
            .submit_with_inputs(
                &request,
                &input,
                &Drafts,
                &secret_for_test("k"),
                None,
                &crate::provider::contract::NoopProviderTransportObserver,
            )
            .unwrap();
        let crate::provider::contract::AdapterSubmission::Complete(outcome) = submission else {
            panic!("a sync pack completes in one call");
        };
        let sent = sent.lock().unwrap();
        assert_eq!(
            *sent,
            [json!({
                "model": "gemini-3-pro-image",
                "input": [{"type": "text", "text": "make it dusk"},
                          {"type": "image", "mime_type": "image/png", "data": encoded}],
                "response_format": {"type": "image", "image_size": "2K", "aspect_ratio": "16:9"}
            })]
        );
        assert_eq!(outcome.provider_request_id.as_deref(), Some("req-pro"));
        let usage = outcome.usage.as_ref().unwrap();
        assert_eq!(usage.images, Some(1));
        // Recorded as settlement evidence; the rules do not price them.
        assert_eq!(
            (usage.input_tokens, usage.output_tokens),
            (Some(580), Some(1400))
        );
        // 13.4¢ for the 2k image plus 0.112¢ for the input image, exactly.
        let frozen = snapshot_for(&edit, "gemini-3-pro-image", &input);
        assert_eq!(frozen.estimated_units, 135_120);
        let settled = frozen
            .settle_precise(
                usage,
                outcome.actual_vendor_cost.as_ref(),
                frozen.estimated_units,
            )
            .unwrap();
        assert_eq!(settled.budget_units, 135_120);
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
        // The v1 or v2 pack revision the operator bound for this model.
        let (pack, target) = verified_packs()
            .into_iter()
            .filter(|pack| {
                pack.pack_id() == "google-gemini-developer-image" && pack.model(&model).is_some()
            })
            .find_map(|pack| {
                let target = targets.revisions().find(|target| {
                    target.provider == pack.provider()
                        && target.adapter == pack.adapter_id()
                        && target.model == model
                        && target.is_active()
                        && target
                            .pack()
                            .is_some_and(|config| config.pack_digest == pack.digest())
                })?;
                Some((Arc::new(pack.clone()), target))
            })
            .expect("the selected enabled Gemini pack target is required");
        let prompt =
            env::var("GONGBU_LIVE_GEMINI_DEVELOPER_PROMPT").expect("explicit prompt is required");
        let image_size = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_IMAGE_SIZE")
            .expect("explicit image size is required");
        let mut input = json!({"prompt": prompt, "image_size": image_size});
        // v2 only; without it a v2 target renders its default square.
        if let Ok(aspect_ratio) = env::var("GONGBU_LIVE_GEMINI_DEVELOPER_ASPECT_RATIO") {
            input["aspect_ratio"] = json!(aspect_ratio);
        }
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
        let decoded = image::load_from_memory(&outcome.artifacts[0].bytes)
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
        // A v2 target certifies the exact size; the image is kept either way.
        if let Some(size) = request.output_dimensions {
            assert_eq!(
                (decoded.width(), decoded.height()),
                (size.width, size.height),
                "Google returned a size other than the certified one"
            );
        }
        assert_eq!(
            snapshot
                .settle(outcome.usage.as_ref().unwrap(), max_spend)
                .unwrap(),
            snapshot.estimated_units
        );
    }
}
