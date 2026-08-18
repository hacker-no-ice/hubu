use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const EXECUTION_SCOPE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScopeSelector {
    #[serde(default = "scope_schema_version")]
    pub schema_version: u32,
    pub provider: String,
    pub executor: String,
    pub capability: String,
    pub billing_merchant: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeIdentity {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScope {
    pub schema_version: u32,
    pub provider: ScopeIdentity,
    pub executor: ScopeIdentity,
    pub capability: ScopeIdentity,
    pub billing_merchant: ScopeIdentity,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScopeResolutionError {
    #[error("unsupported execution scope schema version {0}")]
    UnsupportedSchema(u32),
    #[error("unknown execution scope identifier")]
    Unknown,
    #[error("ambiguous execution scope identifier")]
    Ambiguous,
}

pub fn resolve_execution_scope(
    selector: &ExecutionScopeSelector,
    catalog: &[ExecutionScope],
) -> Result<ExecutionScope, ScopeResolutionError> {
    if selector.schema_version != EXECUTION_SCOPE_SCHEMA_VERSION {
        return Err(ScopeResolutionError::UnsupportedSchema(
            selector.schema_version,
        ));
    }
    let matches = catalog
        .iter()
        .filter(|scope| {
            identity_matches(&scope.provider, &selector.provider)
                && identity_matches(&scope.executor, &selector.executor)
                && identity_matches(&scope.capability, &selector.capability)
                && identity_matches(&scope.billing_merchant, &selector.billing_merchant)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [scope] => Ok((*scope).clone()),
        [] => Err(ScopeResolutionError::Unknown),
        _ => Err(ScopeResolutionError::Ambiguous),
    }
}

fn identity_matches(identity: &ScopeIdentity, selector: &str) -> bool {
    let selector = selector.trim();
    identity.id == selector || identity.display_name.eq_ignore_ascii_case(selector)
}

const fn scope_schema_version() -> u32 {
    EXECUTION_SCOPE_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(provider_id: &str, provider_name: &str) -> ExecutionScope {
        ExecutionScope {
            schema_version: EXECUTION_SCOPE_SCHEMA_VERSION,
            provider: ScopeIdentity {
                id: provider_id.into(),
                display_name: provider_name.into(),
            },
            executor: ScopeIdentity {
                id: "executor:gongbu:image".into(),
                display_name: "Gongbu Image".into(),
            },
            capability: ScopeIdentity {
                id: "capability:image:generate".into(),
                display_name: "Generate image".into(),
            },
            billing_merchant: ScopeIdentity {
                id: "merchant:google".into(),
                display_name: "Google".into(),
            },
        }
    }

    #[test]
    fn resolves_stable_ids_or_friendly_names() {
        let catalog = [scope("provider:google:gemini", "Google Gemini")];
        let selector = ExecutionScopeSelector {
            schema_version: 1,
            provider: "Google Gemini".into(),
            executor: "executor:gongbu:image".into(),
            capability: "Generate image".into(),
            billing_merchant: "merchant:google".into(),
        };
        assert_eq!(
            resolve_execution_scope(&selector, &catalog).unwrap(),
            catalog[0]
        );
    }

    #[test]
    fn rejects_unknown_and_ambiguous_selectors() {
        let scope = scope("provider:google:gemini", "Google Gemini");
        let selector = ExecutionScopeSelector {
            schema_version: 1,
            provider: "missing".into(),
            executor: "executor:gongbu:image".into(),
            capability: "capability:image:generate".into(),
            billing_merchant: "merchant:google".into(),
        };
        assert_eq!(
            resolve_execution_scope(&selector, std::slice::from_ref(&scope)),
            Err(ScopeResolutionError::Unknown)
        );
        let mut ambiguous = selector;
        ambiguous.provider = "Google Gemini".into();
        assert_eq!(
            resolve_execution_scope(&ambiguous, &[scope.clone(), scope]),
            Err(ScopeResolutionError::Ambiguous)
        );
    }

    #[test]
    fn compatibility_fixture_uses_hubu_schema() {
        let fixture: ExecutionScope =
            serde_json::from_str(include_str!("../../../fixtures/execution-scope-v1.json"))
                .unwrap();
        assert_eq!(fixture.schema_version, EXECUTION_SCOPE_SCHEMA_VERSION);
        assert_eq!(fixture.provider.id, "provider:google:gemini-developer");
    }

    #[test]
    fn shared_catalog_fixture_is_versioned_and_unambiguous() {
        let catalog: Vec<ExecutionScope> = serde_json::from_str(include_str!(
            "../../../fixtures/execution-scope-catalog-v1.json"
        ))
        .unwrap();
        assert!(catalog
            .iter()
            .all(|scope| scope.schema_version == EXECUTION_SCOPE_SCHEMA_VERSION));
        for scope in &catalog {
            assert_eq!(
                resolve_execution_scope(
                    &ExecutionScopeSelector {
                        schema_version: scope.schema_version,
                        provider: scope.provider.id.clone(),
                        executor: scope.executor.id.clone(),
                        capability: scope.capability.id.clone(),
                        billing_merchant: scope.billing_merchant.id.clone(),
                    },
                    &catalog
                )
                .unwrap(),
                *scope
            );
        }
    }
}
