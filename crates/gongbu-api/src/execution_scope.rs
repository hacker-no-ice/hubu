use serde::{Deserialize, Serialize};

pub const EXECUTION_SCOPE_SCHEMA_VERSION: u32 = 1;

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
}
