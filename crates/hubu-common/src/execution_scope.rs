use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const EXECUTION_SCOPE_SCHEMA_VERSION: u32 = 1;
/// Schema version 2 adds the execution target's `model` and requested
/// `image_size`. The executor derives both from the target it runs and
/// rejects an authorization whose values differ, so policies may rely on them.
pub const EXECUTION_SCOPE_TARGET_SCHEMA_VERSION: u32 = 2;

const MAX_MODEL_LEN: usize = 128;
const MAX_IMAGE_SIZE_LEN: usize = 32;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScopeSelector {
    #[serde(default = "scope_schema_version")]
    pub schema_version: u32,
    pub provider: String,
    pub executor: String,
    pub capability: String,
    pub billing_merchant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,
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
    /// Execution target model id (schema version 2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Requested output size selector such as `1k` (schema version 2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,
}

impl ExecutionScope {
    /// Whether both scopes name the same trusted catalog entry, ignoring the
    /// schema-v2 target fields that a catalog entry never carries.
    pub fn same_catalog_entry(&self, other: &ExecutionScope) -> bool {
        self.provider == other.provider
            && self.executor == other.executor
            && self.capability == other.capability
            && self.billing_merchant == other.billing_merchant
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScopeResolutionError {
    #[error("unsupported execution scope schema version {0}")]
    UnsupportedSchema(u32),
    #[error("unknown execution scope identifier")]
    Unknown,
    #[error("ambiguous execution scope identifier")]
    Ambiguous,
    #[error("execution scope model and image_size require schema version 2")]
    TargetFieldsRequireSchemaV2,
    #[error("execution scope schema version 2 requires model")]
    MissingModel,
    #[error("invalid execution scope model")]
    InvalidModel,
    #[error("invalid execution scope image_size")]
    InvalidImageSize,
}

pub fn resolve_execution_scope(
    selector: &ExecutionScopeSelector,
    catalog: &[ExecutionScope],
) -> Result<ExecutionScope, ScopeResolutionError> {
    match selector.schema_version {
        EXECUTION_SCOPE_SCHEMA_VERSION => {
            if selector.model.is_some() || selector.image_size.is_some() {
                return Err(ScopeResolutionError::TargetFieldsRequireSchemaV2);
            }
        }
        EXECUTION_SCOPE_TARGET_SCHEMA_VERSION => {
            let model = selector
                .model
                .as_deref()
                .ok_or(ScopeResolutionError::MissingModel)?;
            if !valid_model(model) {
                return Err(ScopeResolutionError::InvalidModel);
            }
            if selector
                .image_size
                .as_deref()
                .is_some_and(|size| !valid_image_size(size))
            {
                return Err(ScopeResolutionError::InvalidImageSize);
            }
        }
        version => return Err(ScopeResolutionError::UnsupportedSchema(version)),
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
        [scope] => Ok(ExecutionScope {
            schema_version: selector.schema_version,
            model: selector.model.clone(),
            image_size: selector.image_size.clone(),
            ..(*scope).clone()
        }),
        [] => Err(ScopeResolutionError::Unknown),
        _ => Err(ScopeResolutionError::Ambiguous),
    }
}

/// Target fields are matched exactly against the executor's derived values,
/// so they are never trimmed or case-folded.
fn valid_model(model: &str) -> bool {
    (1..=MAX_MODEL_LEN).contains(&model.len())
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-:/".contains(&byte))
}

fn valid_image_size(size: &str) -> bool {
    (1..=MAX_IMAGE_SIZE_LEN).contains(&size.len())
        && size.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
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
            model: None,
            image_size: None,
        }
    }

    fn selector(schema_version: u32) -> ExecutionScopeSelector {
        ExecutionScopeSelector {
            schema_version,
            provider: "provider:google:gemini".into(),
            executor: "executor:gongbu:image".into(),
            capability: "capability:image:generate".into(),
            billing_merchant: "merchant:google".into(),
            model: None,
            image_size: None,
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
            model: None,
            image_size: None,
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
            model: None,
            image_size: None,
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
                        model: None,
                        image_size: None,
                    },
                    &catalog
                )
                .unwrap(),
                *scope
            );
        }
    }

    #[test]
    fn schema_v2_carries_target_model_and_image_size() {
        let catalog = [scope("provider:google:gemini", "Google Gemini")];
        let mut v2 = selector(EXECUTION_SCOPE_TARGET_SCHEMA_VERSION);
        v2.model = Some("gemini-3.1-flash-image".into());
        v2.image_size = Some("4k".into());
        let resolved = resolve_execution_scope(&v2, &catalog).unwrap();
        assert_eq!(
            resolved.schema_version,
            EXECUTION_SCOPE_TARGET_SCHEMA_VERSION
        );
        assert_eq!(resolved.provider, catalog[0].provider);
        assert_eq!(resolved.model.as_deref(), Some("gemini-3.1-flash-image"));
        assert_eq!(resolved.image_size.as_deref(), Some("4k"));
        assert_eq!(
            serde_json::to_value(&resolved).unwrap()["image_size"],
            serde_json::json!("4k")
        );

        v2.image_size = None;
        assert_eq!(
            resolve_execution_scope(&v2, &catalog).unwrap().image_size,
            None
        );
    }

    #[test]
    fn schema_v1_serialization_is_unchanged() {
        let value = serde_json::to_value(scope("provider:google:gemini", "Google Gemini")).unwrap();
        assert!(value.get("model").is_none());
        assert!(value.get("image_size").is_none());
    }

    #[test]
    fn rejects_malformed_target_fields() {
        let catalog = [scope("provider:google:gemini", "Google Gemini")];
        let mut v1 = selector(EXECUTION_SCOPE_SCHEMA_VERSION);
        v1.model = Some("gemini-3.1-flash-image".into());
        assert_eq!(
            resolve_execution_scope(&v1, &catalog),
            Err(ScopeResolutionError::TargetFieldsRequireSchemaV2)
        );
        let mut v1 = selector(EXECUTION_SCOPE_SCHEMA_VERSION);
        v1.image_size = Some("1k".into());
        assert_eq!(
            resolve_execution_scope(&v1, &catalog),
            Err(ScopeResolutionError::TargetFieldsRequireSchemaV2)
        );

        let mut v2 = selector(EXECUTION_SCOPE_TARGET_SCHEMA_VERSION);
        v2.image_size = Some("1k".into());
        assert_eq!(
            resolve_execution_scope(&v2, &catalog),
            Err(ScopeResolutionError::MissingModel)
        );
        for model in ["", " gemini", "gemini image", &"m".repeat(129)] {
            v2.model = Some(model.into());
            assert_eq!(
                resolve_execution_scope(&v2, &catalog),
                Err(ScopeResolutionError::InvalidModel),
                "{model:?}"
            );
        }
        v2.model = Some("gemini-3.1-flash-image".into());
        for size in ["", "4K", " 1k", "1k ", &"1".repeat(33)] {
            v2.image_size = Some(size.into());
            assert_eq!(
                resolve_execution_scope(&v2, &catalog),
                Err(ScopeResolutionError::InvalidImageSize),
                "{size:?}"
            );
        }
        assert_eq!(
            resolve_execution_scope(&selector(3), &catalog),
            Err(ScopeResolutionError::UnsupportedSchema(3))
        );
    }
}
