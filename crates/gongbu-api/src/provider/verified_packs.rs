//! Verified provider packs shipped with Gongbu.
//!
//! Each pack's pricing is reviewed and its targets are frozen into
//! `contracts/provider-contracts-v1.json` as the pack's checked-in projection.
//! Tests fail if a pack and its contract entries ever disagree.

use super::pack::Pack;

const GOOGLE_GEMINI_DEVELOPER_IMAGE: &str =
    include_str!("../../../../packs/verified/google-gemini-developer-image.toml");

/// Every verified pack, validated.
pub fn verified_packs() -> Vec<Pack> {
    [GOOGLE_GEMINI_DEVELOPER_IMAGE]
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
            pack::{ArtifactFetchError, Pack},
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
                let input = json!({"prompt": "x", "image_size": preset["name"]});
                assert!(pack.prepare(&model.model, &input).is_ok(), "{preset}");
            }
            projected += 1;
        }
        assert_eq!(projected, 2, "both Gemini contracts come from the pack");
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
