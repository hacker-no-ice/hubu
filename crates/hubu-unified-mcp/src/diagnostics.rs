//! Fail-closed tool guards and sanitized backend diagnostics.

use serde_json::{json, Value};

use crate::{
    capability::{BackendState, CapabilitySnapshot},
    BackendOwner,
};

pub(super) const ROUTING_NOT_IMPLEMENTED: &str = "routing_not_implemented";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolRejection {
    Unconfigured,
    Unavailable,
    Incompatible,
    NotReady,
    RequiredBackendUnavailable,
}

impl ToolRejection {
    pub(super) fn reason_code(self) -> &'static str {
        match self {
            Self::Unconfigured => "backend_unconfigured",
            Self::Unavailable | Self::RequiredBackendUnavailable => "backend_unavailable",
            Self::Incompatible => "backend_incompatible",
            Self::NotReady => "backend_not_ready",
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::NotReady | Self::RequiredBackendUnavailable
        )
    }
}

pub(super) fn tool_availability(
    name: &str,
    owner: BackendOwner,
    snapshot: &CapabilitySnapshot,
) -> Result<(), ToolRejection> {
    let report = match owner {
        BackendOwner::Hubu => &snapshot.hubu,
        BackendOwner::Gongbu => &snapshot.gongbu,
    };
    match report.state {
        BackendState::Unconfigured => return Err(ToolRejection::Unconfigured),
        BackendState::Unavailable => return Err(ToolRejection::Unavailable),
        BackendState::Incompatible => return Err(ToolRejection::Incompatible),
        BackendState::Degraded if name == "gongbu_create_execution" => {
            return Err(ToolRejection::NotReady);
        }
        BackendState::Degraded | BackendState::Available => {}
    }
    if name == "gongbu_create_execution" && snapshot.hubu.state != BackendState::Available {
        return Err(match snapshot.hubu.state {
            BackendState::Unconfigured => ToolRejection::Unconfigured,
            BackendState::Incompatible => ToolRejection::Incompatible,
            BackendState::Degraded | BackendState::Unavailable => {
                ToolRejection::RequiredBackendUnavailable
            }
            BackendState::Available => unreachable!(),
        });
    }
    Ok(())
}

pub(super) fn backend_error_response(
    id: Value,
    tool: &str,
    owner: BackendOwner,
    rejection: ToolRejection,
) -> Value {
    let code = rejection.reason_code();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32010,
            "message": format!("{owner} backend cannot safely serve `{tool}` ({code})"),
            "data": {
                "code": code,
                "tool": tool,
                "owner": owner.as_str(),
                "retryable": rejection.retryable(),
                "capabilities_changed": true
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capability::{BackendReport, ContractVersions},
        EXECUTOR_CONTRACT_VERSION,
    };

    fn report(state: BackendState, gongbu: bool) -> BackendReport {
        BackendReport {
            state,
            product_version: Some("1.2.3".into()),
            source_commit: Some("a".repeat(40)),
            api_schema_version: gongbu.then_some(2),
            mcp_schema_version: gongbu.then_some(2),
            contract_versions: ContractVersions {
                executor: Some(EXECUTOR_CONTRACT_VERSION.into()),
            },
            reason_code: (state != BackendState::Available).then_some("test_state"),
        }
    }

    fn snapshot(hubu: BackendState, gongbu: BackendState) -> CapabilitySnapshot {
        CapabilitySnapshot {
            generated_at: "2026-08-18T00:00:00.000Z".into(),
            hubu: report(hubu, false),
            gongbu: report(gongbu, true),
        }
    }

    #[test]
    fn unrelated_healthy_backend_remains_usable() {
        let state = snapshot(BackendState::Available, BackendState::Unavailable);
        assert_eq!(
            tool_availability("hubu_list_budgets", BackendOwner::Hubu, &state),
            Ok(())
        );
        assert_eq!(
            tool_availability("gongbu_get_execution", BackendOwner::Gongbu, &state),
            Err(ToolRejection::Unavailable)
        );
    }

    #[test]
    fn degraded_gongbu_keeps_reads_but_blocks_execution_admission() {
        let state = snapshot(BackendState::Available, BackendState::Degraded);
        assert_eq!(
            tool_availability("gongbu_get_artifact", BackendOwner::Gongbu, &state),
            Ok(())
        );
        assert_eq!(
            tool_availability("gongbu_create_execution", BackendOwner::Gongbu, &state),
            Err(ToolRejection::NotReady)
        );
    }

    #[test]
    fn governed_execution_fails_closed_on_required_hubu_state() {
        for hubu in [
            BackendState::Unconfigured,
            BackendState::Unavailable,
            BackendState::Incompatible,
        ] {
            let state = snapshot(hubu, BackendState::Available);
            assert!(
                tool_availability("gongbu_create_execution", BackendOwner::Gongbu, &state).is_err()
            );
            assert_eq!(
                tool_availability("gongbu_get_execution", BackendOwner::Gongbu, &state),
                Ok(())
            );
        }
    }

    #[test]
    fn unavailable_backend_errors_are_actionable_and_redacted() {
        let response = backend_error_response(
            json!(7),
            "hubu_list_budgets",
            BackendOwner::Hubu,
            ToolRejection::Unavailable,
        );
        assert_eq!(response["error"]["code"], -32010);
        assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
        assert_eq!(response["error"]["data"]["retryable"], true);
        assert_eq!(response["error"]["data"]["capabilities_changed"], true);
        let serialized = response.to_string();
        assert!(!serialized.contains("endpoint"));
        assert!(!serialized.contains("credential"));
        assert!(!serialized.contains("token"));
    }
}
