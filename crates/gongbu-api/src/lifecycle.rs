//! Stable, redacted process lifecycle events for Gongbu operators.

use serde::{de::IgnoredAny, Deserialize, Serialize};
use std::{
    io::{self, Write},
    sync::atomic::{AtomicU8, Ordering},
};

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

/// Static admission routes that may emit a bounded rejection diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionRoute {
    CreateExecutionV1,
    CreateExecutionV2,
}

impl AdmissionRoute {
    fn version(self) -> u32 {
        match self {
            Self::CreateExecutionV1 => 1,
            Self::CreateExecutionV2 => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdmissionErrorCode {
    InvalidRequest,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdmissionReasonCode {
    TargetNotSelectable,
    PricingSelectorNotMatched,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum AdmissionField {
    #[serde(rename = "workload_type")]
    WorkloadType,
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "adapter")]
    Adapter,
    #[serde(rename = "model")]
    Model,
    #[serde(rename = "input.image_size")]
    InputImageSize,
}

const TARGET_FIELDS: &[AdmissionField] = &[
    AdmissionField::WorkloadType,
    AdmissionField::Provider,
    AdmissionField::Adapter,
    AdmissionField::Model,
];
const PRICING_SELECTOR_FIELDS: &[AdmissionField] = &[AdmissionField::InputImageSize];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionErrorEnvelope {
    schema_version: u32,
    error: AdmissionErrorBody,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionErrorBody {
    code: AdmissionErrorCode,
    #[serde(rename = "message")]
    _message: IgnoredAny,
    reason_code: AdmissionReasonCode,
    fields: Vec<AdmissionField>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AdmissionRejectionEvent {
    event: &'static str,
    route: &'static str,
    route_version: u32,
    status: u16,
    code: AdmissionErrorCode,
    reason_code: AdmissionReasonCode,
    fields: &'static [AdmissionField],
}

struct AdmissionLogGate {
    emitted: AtomicU8,
}

impl AdmissionLogGate {
    const fn new() -> Self {
        Self {
            emitted: AtomicU8::new(0),
        }
    }

    fn first_occurrence(&self, event: &AdmissionRejectionEvent) -> bool {
        let route_offset = match event.route_version {
            1 => 0,
            2 => 2,
            _ => return false,
        };
        let reason_offset = match event.reason_code {
            AdmissionReasonCode::TargetNotSelectable => 0,
            AdmissionReasonCode::PricingSelectorNotMatched => 1,
        };
        let bit = 1 << (route_offset + reason_offset);
        self.emitted.fetch_or(bit, Ordering::Relaxed) & bit == 0
    }
}

static ADMISSION_LOG_GATE: AdmissionLogGate = AdmissionLogGate::new();

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

/// Emit one bounded admission rejection only when the internal API response
/// contains an exact allowlisted reason/field pair. Request bodies, field
/// values, identifiers, target values, raw errors, and unknown diagnostics are
/// never copied into the event.
pub(crate) fn log_admission_rejection(route: AdmissionRoute, status: u16, body: &[u8]) {
    let Some(event) = admission_rejection_event(route, status, body) else {
        return;
    };
    if !ADMISSION_LOG_GATE.first_occurrence(&event) {
        return;
    }
    write_admission_rejection(&mut io::stderr().lock(), &event);
}

fn write_admission_rejection(writer: &mut impl Write, event: &AdmissionRejectionEvent) {
    if serde_json::to_writer(&mut *writer, event).is_ok() {
        let _ = writer.write_all(b"\n");
    }
}

fn admission_rejection_event(
    route: AdmissionRoute,
    status: u16,
    body: &[u8],
) -> Option<AdmissionRejectionEvent> {
    if status != 400 {
        return None;
    }
    let envelope: AdmissionErrorEnvelope = serde_json::from_slice(body).ok()?;
    if envelope.schema_version != route.version() {
        return None;
    }
    let fields = match envelope.error.reason_code {
        AdmissionReasonCode::TargetNotSelectable if envelope.error.fields == TARGET_FIELDS => {
            TARGET_FIELDS
        }
        AdmissionReasonCode::PricingSelectorNotMatched
            if envelope.error.fields == PRICING_SELECTOR_FIELDS =>
        {
            PRICING_SELECTOR_FIELDS
        }
        _ => return None,
    };
    Some(AdmissionRejectionEvent {
        event: "gongbu_admission_rejected",
        route: "create_execution",
        route_version: route.version(),
        status: 400,
        code: envelope.error.code,
        reason_code: envelope.error.reason_code,
        fields,
    })
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

    #[test]
    fn admission_rejection_event_serializes_only_static_allowlisted_data() {
        let event = admission_rejection_event(
            AdmissionRoute::CreateExecutionV2,
            400,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"request validation failed","reason_code":"target_not_selectable","fields":["workload_type","provider","adapter","model"]}}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "event": "gongbu_admission_rejected",
                "route": "create_execution",
                "route_version": 2,
                "status": 400,
                "code": "invalid_request",
                "reason_code": "target_not_selectable",
                "fields": ["workload_type", "provider", "adapter", "model"]
            })
        );

        let selector = admission_rejection_event(
            AdmissionRoute::CreateExecutionV1,
            400,
            br#"{"schema_version":1,"error":{"code":"invalid_request","message":"request validation failed","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(selector).unwrap(),
            serde_json::json!({
                "event": "gongbu_admission_rejected",
                "route": "create_execution",
                "route_version": 1,
                "status": 400,
                "code": "invalid_request",
                "reason_code": "pricing_selector_not_matched",
                "fields": ["input.image_size"]
            })
        );
    }

    #[test]
    fn admission_rejection_event_rejects_every_non_allowlisted_diagnostic() {
        let rejected: &[&[u8]] = &[
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic"}}"#,
            br#"{"schema_version":2,"error":{"code":"other_error","message":"generic","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"unknown_reason","fields":["model"]}}"#,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"target_not_selectable","fields":["input.image_size"]}}"#,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"target_not_selectable","fields":["provider","workload_type","adapter","model"]}}"#,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"target_not_selectable","fields":["workload_type","provider","adapter","model","secret"]}}"#,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"pricing_selector_not_matched","fields":["input.image_size"],"target_value":"secret-canary"}}"#,
            br#"{"schema_version":1,"error":{"code":"invalid_request","message":"generic","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#,
        ];
        for body in rejected {
            assert!(
                admission_rejection_event(AdmissionRoute::CreateExecutionV2, 400, body).is_none()
            );
        }

        let valid = br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#;
        assert!(admission_rejection_event(AdmissionRoute::CreateExecutionV2, 500, valid).is_none());
    }

    #[test]
    fn admission_logging_is_deduplicated_per_route_and_reason() {
        let gate = AdmissionLogGate::new();
        let target = admission_rejection_event(
            AdmissionRoute::CreateExecutionV2,
            400,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"target_not_selectable","fields":["workload_type","provider","adapter","model"]}}"#,
        )
        .unwrap();
        let selector = admission_rejection_event(
            AdmissionRoute::CreateExecutionV2,
            400,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#,
        )
        .unwrap();
        let v1_target = admission_rejection_event(
            AdmissionRoute::CreateExecutionV1,
            400,
            br#"{"schema_version":1,"error":{"code":"invalid_request","message":"generic","reason_code":"target_not_selectable","fields":["workload_type","provider","adapter","model"]}}"#,
        )
        .unwrap();

        assert!(gate.first_occurrence(&target));
        assert!(!gate.first_occurrence(&target));
        assert!(gate.first_occurrence(&selector));
        assert!(!gate.first_occurrence(&selector));
        assert!(gate.first_occurrence(&v1_target));
        assert!(!gate.first_occurrence(&v1_target));
    }

    #[test]
    fn admission_logging_ignores_writer_failures() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("closed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("closed"))
            }
        }

        let event = admission_rejection_event(
            AdmissionRoute::CreateExecutionV2,
            400,
            br#"{"schema_version":2,"error":{"code":"invalid_request","message":"generic","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#,
        )
        .unwrap();
        write_admission_rejection(&mut FailingWriter, &event);
    }
}
