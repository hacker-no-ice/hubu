//! Compatibility re-exports for the neutral Hubu/Gongbu executor contract.

pub use hubu_executor_contract::{
    resolve_execution_scope, ExecutionScope, ExecutionScopeSelector, ScopeIdentity,
    ScopeResolutionError, EXECUTION_SCOPE_SCHEMA_VERSION,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_fixture_uses_hubu_schema() {
        let fixture: ExecutionScope =
            serde_json::from_str(include_str!("../../../fixtures/execution-scope-v1.json"))
                .unwrap();
        assert_eq!(fixture.schema_version, EXECUTION_SCOPE_SCHEMA_VERSION);
        assert_eq!(fixture.provider.id, "provider:google:gemini-developer");
    }
}
