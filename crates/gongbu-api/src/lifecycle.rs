//! Stable, redacted process lifecycle events for Gongbu operators.

use serde::{Deserialize, Serialize};

/// Machine-readable reasons that may affect, or be mistaken for affecting, the
/// persistent server lifecycle. These values are an operator-facing contract;
/// do not include request bodies, credentials, identifiers, or raw errors.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleReason {
    ConfigurationStartupFailure,
    DependencyHealthShutdown,
    OperatorSignal,
    WorkerUnavailable,
    ExecutionFailure,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyName {
    Temporal,
    Hubu,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyProbeOutcome {
    Unhealthy,
    Indeterminate,
    Recovered,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct LifecycleEvent {
    event: &'static str,
    reason: LifecycleReason,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct DependencyProbeEvent<'a> {
    event: &'static str,
    dependency: DependencyName,
    outcome: DependencyProbeOutcome,
    consecutive_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    grpc_code: Option<&'a str>,
}

/// Emit one stable JSON lifecycle event without request-scoped or secret data.
pub fn log(reason: LifecycleReason) {
    let event = LifecycleEvent {
        event: "gongbu_server_lifecycle",
        reason,
    };
    eprintln!(
        "{}",
        serde_json::to_string(&event)
            .expect("the static Gongbu lifecycle event is always serializable")
    );
}

/// Emit a bounded, redacted dependency transition without raw transport text.
pub fn log_dependency_probe(
    dependency: DependencyName,
    outcome: DependencyProbeOutcome,
    consecutive_failures: u32,
    grpc_code: Option<&str>,
) {
    let event = DependencyProbeEvent {
        event: "gongbu_dependency_probe",
        dependency,
        outcome,
        consecutive_failures,
        grpc_code,
    };
    eprintln!(
        "{}",
        serde_json::to_string(&event)
            .expect("the bounded Gongbu dependency event is always serializable")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_event_is_stable_and_contains_no_dynamic_data() {
        let event = serde_json::to_value(LifecycleEvent {
            event: "gongbu_server_lifecycle",
            reason: LifecycleReason::ExecutionFailure,
        })
        .unwrap();
        assert_eq!(
            event,
            serde_json::json!({
                "event": "gongbu_server_lifecycle",
                "reason": "execution_failure"
            })
        );
    }

    #[test]
    fn dependency_probe_event_is_redacted_and_stable() {
        let event = serde_json::to_value(DependencyProbeEvent {
            event: "gongbu_dependency_probe",
            dependency: DependencyName::Temporal,
            outcome: DependencyProbeOutcome::Indeterminate,
            consecutive_failures: 1,
            grpc_code: Some("unavailable"),
        })
        .unwrap();
        assert_eq!(
            event,
            serde_json::json!({
                "event": "gongbu_dependency_probe",
                "dependency": "temporal",
                "outcome": "indeterminate",
                "consecutive_failures": 1,
                "grpc_code": "unavailable"
            })
        );
    }
}
