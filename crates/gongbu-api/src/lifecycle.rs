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

#[derive(Debug, Serialize, PartialEq, Eq)]
struct LifecycleEvent {
    event: &'static str,
    reason: LifecycleReason,
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
}
