//! Capability snapshot model and `UnifiedCapabilitiesV1` rendering.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    diagnostics::tool_availability, governed_execution, resume_operation, DOMAIN_TOOLS,
    ROUTING_REVISION, UNIFIED_CONTRACT_VERSION,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BackendState {
    Available,
    Degraded,
    Unavailable,
    Incompatible,
    Unconfigured,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ContractVersions {
    pub(super) executor: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct BackendReport {
    pub(super) state: BackendState,
    pub(super) product_version: Option<String>,
    pub(super) source_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) api_schema_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) mcp_schema_version: Option<u32>,
    pub(super) contract_versions: ContractVersions,
    pub(super) reason_code: Option<&'static str>,
}

impl BackendReport {
    pub(super) fn unconfigured() -> Self {
        Self {
            state: BackendState::Unconfigured,
            product_version: None,
            source_commit: None,
            api_schema_version: None,
            mcp_schema_version: None,
            contract_versions: ContractVersions { executor: None },
            reason_code: Some("configuration_missing"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct CapabilitySnapshot {
    pub(super) generated_at: String,
    pub(super) hubu: BackendReport,
    pub(super) gongbu: BackendReport,
}

pub(super) fn capabilities_value(snapshot: &CapabilitySnapshot) -> Value {
    let mut tools = DOMAIN_TOOLS
        .iter()
        .map(|(name, owner)| {
            let (available, reason_code) = match tool_availability(name, *owner, snapshot) {
                Ok(()) => (true, None),
                Err(rejection) => (false, Some(rejection.reason_code())),
            };
            json!({
                "name": name,
                "owner": owner.as_str(),
                "available": available,
                "reason_code": reason_code
            })
        })
        .collect::<Vec<_>>();
    tools.push(json!({
        "name": "hubu_unified_capabilities",
        "owner": "router",
        "available": true,
        "reason_code": null
    }));
    let governed_availability = governed_execution::backend_availability(snapshot);
    tools.push(json!({
        "name": governed_execution::TOOL_NAME,
        "owner": "router",
        "available": governed_availability.is_ok(),
        "reason_code": governed_availability.err().map(|(_, rejection)| rejection.reason_code())
    }));
    tools.push(json!({
        "name": resume_operation::TOOL_NAME,
        "owner": "router",
        "available": tool_availability("hubu_authorize_spend", crate::BackendOwner::Hubu, snapshot).is_ok(),
        "reason_code": tool_availability("hubu_authorize_spend", crate::BackendOwner::Hubu, snapshot)
            .err()
            .map(|rejection| rejection.reason_code())
    }));
    tools.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));

    json!({
        "contract_version": UNIFIED_CONTRACT_VERSION,
        "routing_revision": ROUTING_REVISION,
        "generated_at": snapshot.generated_at,
        "backends": {
            "hubu": backend_value(&snapshot.hubu, false),
            "gongbu": backend_value(&snapshot.gongbu, true)
        },
        "tools": tools
    })
}

fn backend_value(report: &BackendReport, gongbu: bool) -> Value {
    let mut backend = BTreeMap::from([
        ("state", json!(report.state)),
        ("product_version", json!(report.product_version)),
        ("source_commit", json!(report.source_commit)),
        (
            "contract_versions",
            json!({ "executor": report.contract_versions.executor }),
        ),
        ("reason_code", json!(report.reason_code)),
    ]);
    if gongbu {
        backend.insert("api_schema_version", json!(report.api_schema_version));
        backend.insert("mcp_schema_version", json!(report.mcp_schema_version));
    }
    serde_json::to_value(backend).expect("backend capability serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EXECUTOR_CONTRACT_VERSION;

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
    fn schema_covers_full_partial_unavailable_and_incompatible_states() {
        let cases = [
            (BackendState::Available, BackendState::Available),
            (BackendState::Available, BackendState::Unavailable),
            (BackendState::Unavailable, BackendState::Unavailable),
            (BackendState::Incompatible, BackendState::Available),
        ];
        for (hubu, gongbu) in cases {
            let capability = capabilities_value(&snapshot(hubu, gongbu));
            assert_eq!(capability["backends"]["hubu"]["state"], json!(hubu));
            assert_eq!(capability["backends"]["gongbu"]["state"], json!(gongbu));
            assert_eq!(capability["tools"].as_array().unwrap().len(), 37);
            let names = capability["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn compatible_routed_tools_are_advertised_available() {
        let capability =
            capabilities_value(&snapshot(BackendState::Available, BackendState::Available));
        for tool in capability["tools"].as_array().unwrap() {
            if tool["owner"] == "router" {
                assert_eq!(tool["available"], true);
                assert!(tool["reason_code"].is_null());
            } else {
                assert_eq!(tool["available"], true, "{}", tool["name"]);
                assert!(tool["reason_code"].is_null(), "{}", tool["name"]);
            }
        }
    }
}
