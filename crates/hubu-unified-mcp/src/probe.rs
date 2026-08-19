//! Independent backend health probes and exact compatibility evaluation.

use std::{
    sync::{Condvar, Mutex},
    thread,
};

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use crate::{
    capability::{BackendReport, BackendState, CapabilitySnapshot, ContractVersions},
    product_version, source_commit, BackendClient, BackendClients, EXECUTOR_CONTRACT_VERSION,
    MCP_PROTOCOL_VERSION,
};

const GONGBU_API_SCHEMA_VERSION: u32 = 2;
const GONGBU_MCP_SCHEMA_VERSION: u32 = 2;

#[derive(Debug)]
struct ProbeResponse {
    status: u16,
    body: Value,
}

#[derive(Debug, Default)]
struct ProbeGateState {
    in_flight: bool,
    last_report: Option<BackendReport>,
}

/// Per-backend single-flight gate. The mutex only protects bookkeeping and is
/// always released before the network probe runs.
#[derive(Debug, Default)]
pub(super) struct ProbeGate {
    state: Mutex<ProbeGateState>,
    completed: Condvar,
}

impl ProbeGate {
    fn run(&self, probe: impl FnOnce() -> BackendReport) -> BackendReport {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.in_flight {
            while state.in_flight {
                state = self
                    .completed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            return state
                .last_report
                .clone()
                .expect("completed probe publishes a report");
        }
        state.in_flight = true;
        drop(state);

        let report = probe();

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.last_report = Some(report.clone());
        state.in_flight = false;
        self.completed.notify_all();
        report
    }
}

impl BackendClient {
    fn probe(&self, path: &str) -> Result<ProbeResponse, ()> {
        let url = self.endpoint().join(path).map_err(|_| ())?;
        let response = self.http_client().get(url).send().map_err(|_| ())?;
        let status = response.status().as_u16();
        let body = response.json().map_err(|_| ())?;
        Ok(ProbeResponse { status, body })
    }
}

impl BackendClients {
    pub(super) fn probe_hubu(&self) -> BackendReport {
        self.hubu_probe_gate.run(|| {
            self.hubu
                .as_ref()
                .map(probe_hubu)
                .unwrap_or_else(BackendReport::unconfigured)
        })
    }

    pub(super) fn probe_gongbu(&self) -> BackendReport {
        self.gongbu_probe_gate.run(|| {
            self.gongbu
                .as_ref()
                .map(probe_gongbu)
                .unwrap_or_else(BackendReport::unconfigured)
        })
    }

    pub(super) fn probe(&self) -> CapabilitySnapshot {
        thread::scope(|scope| {
            let hubu = scope.spawn(|| self.probe_hubu());
            let gongbu = scope.spawn(|| self.probe_gongbu());
            CapabilitySnapshot {
                generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                hubu: hubu.join().expect("Hubu capability probe must not panic"),
                gongbu: gongbu
                    .join()
                    .expect("Gongbu capability probe must not panic"),
            }
        })
    }
}

fn probe_hubu(client: &BackendClient) -> BackendReport {
    let health = client.probe("health");
    let version = client.probe("version");
    classify_hubu(health.as_ref().ok(), version.as_ref().ok())
}

fn classify_hubu(health: Option<&ProbeResponse>, version: Option<&ProbeResponse>) -> BackendReport {
    classify_hubu_for(health, version, product_version(), source_commit())
}

fn classify_hubu_for(
    health: Option<&ProbeResponse>,
    version: Option<&ProbeResponse>,
    expected_product_version: &str,
    expected_source_commit: &str,
) -> BackendReport {
    let metadata = version.and_then(|response| response.body.as_object());
    let reported_product_version = string_value(metadata, "product_version");
    let source = string_value(metadata, "source_commit");
    let executor = string_value(metadata, "executor_contract");
    let mut report = BackendReport {
        state: BackendState::Unavailable,
        product_version: reported_product_version,
        source_commit: source,
        api_schema_version: None,
        mcp_schema_version: None,
        contract_versions: ContractVersions { executor },
        reason_code: Some("health_unavailable"),
    };

    if !health.is_some_and(|response| {
        (200..300).contains(&response.status) && response.body["status"] == "ok"
    }) {
        return report;
    }
    if !version.is_some_and(|response| (200..300).contains(&response.status)) {
        report.reason_code = Some("version_unavailable");
        return report;
    }
    if report.product_version.as_deref() != Some(expected_product_version) {
        return incompatible(report, "product_version_mismatch");
    }
    if !matching_source_commit(report.source_commit.as_deref(), expected_source_commit) {
        return incompatible(report, "source_commit_mismatch");
    }
    if report.contract_versions.executor.as_deref() != Some(EXECUTOR_CONTRACT_VERSION) {
        return incompatible(report, "executor_contract_mismatch");
    }
    report.state = BackendState::Available;
    report.reason_code = None;
    report
}

fn probe_gongbu(client: &BackendClient) -> BackendReport {
    let live = client.probe("livez");
    let ready = client.probe("readyz");
    let version = client.probe("version");
    classify_gongbu(
        live.as_ref().ok(),
        ready.as_ref().ok(),
        version.as_ref().ok(),
    )
}

fn classify_gongbu(
    live: Option<&ProbeResponse>,
    ready: Option<&ProbeResponse>,
    version: Option<&ProbeResponse>,
) -> BackendReport {
    classify_gongbu_for(live, ready, version, product_version(), source_commit())
}

fn classify_gongbu_for(
    live: Option<&ProbeResponse>,
    ready: Option<&ProbeResponse>,
    version: Option<&ProbeResponse>,
    expected_product_version: &str,
    expected_source_commit: &str,
) -> BackendReport {
    let metadata = version.and_then(|response| response.body.as_object());
    let reported_product_version = string_value(metadata, "product_version");
    let source = string_value(metadata, "source_commit");
    let executor = string_value(metadata, "hubu_executor_contract");
    let api_schema_version = integer_value(metadata, "api_schema_version");
    let mcp_schema_version = integer_value(metadata, "mcp_schema_version");
    let mcp_protocol = string_value(metadata, "mcp_protocol_version");
    let mut report = BackendReport {
        state: BackendState::Unavailable,
        product_version: reported_product_version,
        source_commit: source,
        api_schema_version,
        mcp_schema_version,
        contract_versions: ContractVersions { executor },
        reason_code: Some("liveness_unavailable"),
    };

    if !live.is_some_and(|response| {
        (200..300).contains(&response.status) && response.body["status"] == "live"
    }) {
        return report;
    }
    if !version.is_some_and(|response| (200..300).contains(&response.status)) {
        report.reason_code = Some("version_unavailable");
        return report;
    }
    if report.product_version.as_deref() != Some(expected_product_version) {
        return incompatible(report, "product_version_mismatch");
    }
    if !matching_source_commit(report.source_commit.as_deref(), expected_source_commit) {
        return incompatible(report, "source_commit_mismatch");
    }
    if report.contract_versions.executor.as_deref() != Some(EXECUTOR_CONTRACT_VERSION) {
        return incompatible(report, "executor_contract_mismatch");
    }
    if report.api_schema_version != Some(GONGBU_API_SCHEMA_VERSION) {
        return incompatible(report, "api_schema_version_mismatch");
    }
    if report.mcp_schema_version != Some(GONGBU_MCP_SCHEMA_VERSION) {
        return incompatible(report, "mcp_schema_version_mismatch");
    }
    if mcp_protocol.as_deref() != Some(MCP_PROTOCOL_VERSION) {
        return incompatible(report, "mcp_protocol_version_mismatch");
    }
    if !ready.is_some_and(|response| {
        (200..300).contains(&response.status) && response.body["status"] == "ready"
    }) {
        report.state = BackendState::Degraded;
        report.reason_code = Some("backend_not_ready");
        return report;
    }
    report.state = BackendState::Available;
    report.reason_code = None;
    report
}

fn string_value(object: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    object?.get(key)?.as_str().map(str::to_owned)
}

fn integer_value(object: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<u32> {
    object?.get(key)?.as_u64()?.try_into().ok()
}

fn incompatible(mut report: BackendReport, reason: &'static str) -> BackendReport {
    report.state = BackendState::Incompatible;
    report.reason_code = Some(reason);
    report
}

fn matching_source_commit(candidate: Option<&str>, expected: &str) -> bool {
    valid_source_commit(expected) && candidate == Some(expected)
}

fn valid_source_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn response(status: u16, body: Value) -> ProbeResponse {
        ProbeResponse { status, body }
    }

    #[test]
    fn exact_compatibility_matrix_accepts_matching_backends() {
        let commit = "a".repeat(40);
        let health = response(200, json!({"status":"ok"}));
        let hubu_version = response(
            200,
            json!({
                "product_version":"1.2.3",
                "source_commit":commit,
                "executor_contract":EXECUTOR_CONTRACT_VERSION
            }),
        );
        assert_eq!(
            classify_hubu_for(Some(&health), Some(&hubu_version), "1.2.3", &commit).state,
            BackendState::Available
        );

        let live = response(200, json!({"status":"live"}));
        let ready = response(200, json!({"status":"ready"}));
        let gongbu_version = response(
            200,
            json!({
                "product_version":"1.2.3",
                "source_commit":commit,
                "api_schema_version":2,
                "mcp_protocol_version":MCP_PROTOCOL_VERSION,
                "mcp_schema_version":2,
                "hubu_executor_contract":EXECUTOR_CONTRACT_VERSION
            }),
        );
        assert_eq!(
            classify_gongbu_for(
                Some(&live),
                Some(&ready),
                Some(&gongbu_version),
                "1.2.3",
                &commit
            )
            .state,
            BackendState::Available
        );
    }

    #[test]
    fn unknown_source_commit_fails_closed() {
        let health = response(200, json!({"status":"ok"}));
        let version = response(
            200,
            json!({
                "product_version":"1.2.3",
                "source_commit":"unknown",
                "executor_contract":EXECUTOR_CONTRACT_VERSION
            }),
        );
        let report = classify_hubu_for(Some(&health), Some(&version), "1.2.3", &"a".repeat(40));
        assert_eq!(report.state, BackendState::Incompatible);
        assert_eq!(report.reason_code, Some("source_commit_mismatch"));
    }

    #[test]
    fn every_gongbu_compatibility_dimension_fails_closed() {
        let commit = "a".repeat(40);
        let live = response(200, json!({"status":"live"}));
        let ready = response(200, json!({"status":"ready"}));
        let base = json!({
            "product_version":"1.2.3",
            "source_commit":commit,
            "api_schema_version":GONGBU_API_SCHEMA_VERSION,
            "mcp_protocol_version":MCP_PROTOCOL_VERSION,
            "mcp_schema_version":GONGBU_MCP_SCHEMA_VERSION,
            "hubu_executor_contract":EXECUTOR_CONTRACT_VERSION
        });
        let cases = [
            (
                "product_version",
                json!("9.9.9"),
                "product_version_mismatch",
            ),
            (
                "source_commit",
                json!("b".repeat(40)),
                "source_commit_mismatch",
            ),
            (
                "hubu_executor_contract",
                json!("hubu-spend-executor-v0"),
                "executor_contract_mismatch",
            ),
            (
                "api_schema_version",
                json!(1),
                "api_schema_version_mismatch",
            ),
            (
                "mcp_schema_version",
                json!(1),
                "mcp_schema_version_mismatch",
            ),
            (
                "mcp_protocol_version",
                json!("2099-01-01"),
                "mcp_protocol_version_mismatch",
            ),
        ];
        for (field, mismatch, expected_reason) in cases {
            let mut body = base.clone();
            body[field] = mismatch;
            let version = response(200, body);
            let report =
                classify_gongbu_for(Some(&live), Some(&ready), Some(&version), "1.2.3", &commit);
            assert_eq!(report.state, BackendState::Incompatible, "{field}");
            assert_eq!(report.reason_code, Some(expected_reason), "{field}");
        }
    }
}
