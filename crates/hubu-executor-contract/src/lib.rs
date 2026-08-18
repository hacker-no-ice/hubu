//! Versioned, transport-neutral contract shared across the Hubu/Gongbu boundary.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const EXECUTOR_CONTRACT: &str = "hubu-spend-executor-v4";
pub const EXECUTION_SCOPE_SCHEMA_VERSION: u32 = 1;
pub const AUTHORIZATION_SCOPE_SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationAmount {
    pub amount_minor: i64,
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationTask {
    pub task_id: String,
    pub reason: String,
    pub semantics: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationWorkload {
    pub workload_type: String,
    pub profile: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationExpiryGuidance {
    pub authorization_ttl_seconds: i64,
    pub claim_ttl_seconds: i64,
    pub guidance: String,
}

/// Exact operator-owned scope that must be used to issue a Hubu token for one
/// planned Gongbu execution. Display names are descriptive; stable IDs carry
/// authority through [`ExecutionScope`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationScope {
    pub schema_version: u32,
    pub executor_contract: String,
    pub account_id: String,
    pub agent_id: String,
    pub operation_key: String,
    pub authorization: AuthorizationAmount,
    pub execution_scope: ExecutionScope,
    pub task: AuthorizationTask,
    pub workload: AuthorizationWorkload,
    pub expiry: AuthorizationExpiryGuidance,
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

    #[test]
    fn complete_authorization_fixture_is_versioned_and_strict() {
        let fixture: AuthorizationScope = serde_json::from_str(include_str!(
            "../../../fixtures/hubu-authorization-scope-v1.json"
        ))
        .unwrap();
        assert_eq!(fixture.schema_version, AUTHORIZATION_SCOPE_SCHEMA_VERSION);
        assert_eq!(fixture.executor_contract, EXECUTOR_CONTRACT);
        assert_eq!(
            fixture.execution_scope.schema_version,
            EXECUTION_SCOPE_SCHEMA_VERSION
        );

        let mut value = serde_json::to_value(&fixture).unwrap();
        value.as_object_mut().unwrap().remove("task");
        assert!(serde_json::from_value::<AuthorizationScope>(value).is_err());
    }
}
