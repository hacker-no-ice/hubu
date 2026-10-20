use serde::{Deserialize, Serialize};

pub const EXECUTION_SCOPE_SCHEMA_VERSION: u32 = 1;
/// Schema version 2 also binds the target model and requested image size, so
/// Hubu policy may branch on them. Gongbu derives both from the execution it
/// runs; an authorization asserting different values is rejected.
pub const EXECUTION_SCOPE_TARGET_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeIdentity {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScope {
    pub schema_version: u32,
    pub provider: ScopeIdentity,
    pub executor: ScopeIdentity,
    pub capability: ScopeIdentity,
    pub billing_merchant: ScopeIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,
}

pub fn for_target(provider: &str, adapter: &str) -> Option<ExecutionScope> {
    let (provider_id, provider_name, merchant_id, merchant_name) = match (provider, adapter) {
        ("google", "gemini_developer_image") => (
            "provider:google:gemini-developer",
            "Google Gemini Developer API",
            "merchant:google",
            "Google",
        ),
        ("flux", "flux2_api") => (
            "provider:black-forest-labs:flux",
            "Black Forest Labs FLUX",
            "merchant:black-forest-labs",
            "Black Forest Labs",
        ),
        ("ideogram", "ideogram_image") => (
            "provider:ideogram",
            "Ideogram",
            "merchant:ideogram",
            "Ideogram",
        ),
        ("local-mock", _)
        | ("mock", "deterministic")
        | ("sandbox", "fixture")
        | ("example", "fixture") => (
            "provider:local:fixture",
            "Local fixture provider",
            "merchant:local",
            "Local merchant",
        ),
        _ => return None,
    };
    Some(ExecutionScope {
        schema_version: EXECUTION_SCOPE_SCHEMA_VERSION,
        provider: identity(provider_id, provider_name),
        executor: identity("executor:gongbu:image", "Gongbu image executor"),
        capability: identity("capability:image:generate", "Generate image"),
        billing_merchant: identity(merchant_id, merchant_name),
        model: None,
        image_size: None,
    })
}

/// The scope Gongbu derives for one execution, in the schema version the
/// Hubu authorization was issued with. A version-1 authorization makes no
/// model or size claim, so only the target identities are compared. Any other
/// version yields the version-2 scope, which an authorization in an
/// unsupported version can never equal.
pub fn for_execution(
    authorized_schema_version: Option<u32>,
    provider: &str,
    adapter: &str,
    model: &str,
    image_size: Option<&str>,
) -> Option<ExecutionScope> {
    let scope = for_target(provider, adapter)?;
    if authorized_schema_version == Some(EXECUTION_SCOPE_SCHEMA_VERSION) {
        return Some(scope);
    }
    Some(ExecutionScope {
        schema_version: EXECUTION_SCOPE_TARGET_SCHEMA_VERSION,
        model: Some(model.to_owned()),
        image_size: image_size.map(str::to_owned),
        ..scope
    })
}

fn identity(id: &str, display_name: &str) -> ScopeIdentity {
    ScopeIdentity {
        id: id.into(),
        display_name: display_name.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_fixture_uses_gongbu_schema() {
        let fixture: ExecutionScope =
            serde_json::from_str(include_str!("../../../fixtures/execution-scope-v1.json"))
                .unwrap();
        assert_eq!(
            fixture,
            for_target("google", "gemini_developer_image").unwrap()
        );
    }

    #[test]
    fn every_gongbu_target_matches_the_shared_catalog_fixture() {
        let catalog: Vec<ExecutionScope> = serde_json::from_str(include_str!(
            "../../../fixtures/execution-scope-catalog-v1.json"
        ))
        .unwrap();
        for (provider, adapter) in [
            ("google", "gemini_developer_image"),
            ("flux", "flux2_api"),
            ("ideogram", "ideogram_image"),
            ("mock", "deterministic"),
        ] {
            assert!(catalog.contains(&for_target(provider, adapter).unwrap()));
        }
    }

    #[test]
    fn execution_scope_follows_the_authorized_schema_version() {
        let v1 = for_target("google", "gemini_developer_image").unwrap();
        assert_eq!(
            for_execution(
                Some(EXECUTION_SCOPE_SCHEMA_VERSION),
                "google",
                "gemini_developer_image",
                "gemini-3.1-flash-image",
                Some("4k"),
            ),
            Some(v1.clone())
        );
        let v2 = for_execution(
            Some(EXECUTION_SCOPE_TARGET_SCHEMA_VERSION),
            "google",
            "gemini_developer_image",
            "gemini-3.1-flash-image",
            Some("4k"),
        )
        .unwrap();
        assert_eq!(v2.schema_version, EXECUTION_SCOPE_TARGET_SCHEMA_VERSION);
        assert_eq!(v2.provider, v1.provider);
        assert_eq!(v2.model.as_deref(), Some("gemini-3.1-flash-image"));
        assert_eq!(v2.image_size.as_deref(), Some("4k"));
        // Missing or unknown versions never fall back to the v1 comparison.
        assert_eq!(
            for_execution(
                None,
                "google",
                "gemini_developer_image",
                "gemini-3.1-flash-image",
                None
            )
            .unwrap()
            .schema_version,
            EXECUTION_SCOPE_TARGET_SCHEMA_VERSION
        );
        let v1_json = serde_json::to_value(&v1).unwrap();
        assert!(v1_json.get("model").is_none() && v1_json.get("image_size").is_none());
    }
}
