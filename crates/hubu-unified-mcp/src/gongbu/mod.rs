//! Gongbu-owned routes for the unified MCP adapter.
//!
//! This facade intentionally mirrors Gongbu's public MCP wire contract without
//! depending on a Gongbu crate. Each HTTP attempt uses a fixed relative path
//! with no inline retry; the durable worker separately schedules bounded exact
//! create replay or read-only observation after safe transient failures.

mod catalog;
mod request;
mod response;
mod transport;

#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::BackendClient;

pub(crate) use transport::{CallOutcome, DurableCallError, DurableExecutionObservation};

const TARGET_ID_FIELDS: &[&str] = &["target_id"];
const PRICING_SELECTOR_FIELDS: &[&str] = &["input.image_size"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionDiagnostic {
    TargetIdNotSelectable,
    PricingSelectorNotMatched,
}

impl AdmissionDiagnostic {
    pub(crate) fn reason_code(self) -> &'static str {
        match self {
            Self::TargetIdNotSelectable => "target_not_selectable",
            Self::PricingSelectorNotMatched => "pricing_selector_not_matched",
        }
    }

    pub(crate) fn fields(self) -> &'static [&'static str] {
        match self {
            Self::TargetIdNotSelectable => TARGET_ID_FIELDS,
            Self::PricingSelectorNotMatched => PRICING_SELECTOR_FIELDS,
        }
    }

    pub(crate) fn durable_result_code(self) -> &'static str {
        match self {
            Self::TargetIdNotSelectable => "execution_request_target_id_not_selectable",
            Self::PricingSelectorNotMatched => "execution_request_pricing_selector_not_matched",
        }
    }

    pub(crate) fn from_durable_result_code(code: &str) -> Option<Self> {
        match code {
            "execution_request_target_id_not_selectable" => Some(Self::TargetIdNotSelectable),
            "execution_request_pricing_selector_not_matched" => {
                Some(Self::PricingSelectorNotMatched)
            }
            _ => None,
        }
    }
}

pub(crate) fn tool_definitions() -> Vec<Value> {
    catalog::tool_definitions()
}

pub(crate) fn operation_status_definition() -> Value {
    catalog::operation_status_definition()
}

pub(crate) fn call_tool(
    client: &BackendClient,
    name: &str,
    arguments: Value,
    expected: Option<&crate::operation_registry::GongbuContinuation>,
) -> CallOutcome {
    transport::call_tool(client, name, arguments, expected)
}

pub(crate) fn fetch_artifact_bounded(
    client: &BackendClient,
    artifact_id: &str,
    byte_limit: usize,
) -> Value {
    serde_json::to_value(transport::fetch_artifact_bounded(
        client,
        artifact_id,
        byte_limit,
    ))
    .expect("Gongbu MCP artifact result serializes")
}

pub(crate) fn create_continuation_id(arguments: &Value) -> Result<String, Value> {
    request::create_continuation_id(arguments).map_err(|error| {
        serde_json::to_value(error.into_result()).expect("Gongbu MCP error serializes")
    })
}

pub(crate) fn governed_execution_arguments(
    intent: &Value,
    spend_auth_token_id: &str,
) -> Result<Value, Value> {
    let Some(mut arguments) = intent.as_object().cloned() else {
        return Err(request_error_result());
    };
    if arguments.contains_key("spend_auth_token_id") {
        return Err(request_error_result());
    }
    arguments.insert(
        "spend_auth_token_id".into(),
        Value::String(spend_auth_token_id.to_owned()),
    );
    let arguments = Value::Object(arguments);
    create_continuation_id(&arguments)?;
    Ok(arguments)
}

fn request_error_result() -> Value {
    serde_json::to_value(response::ToolError::invalid().into_result())
        .expect("Gongbu MCP error serializes")
}

pub(crate) fn status_execution_id(arguments: &Value) -> Result<String, Value> {
    request::status_execution_id(arguments).map_err(|error| {
        serde_json::to_value(error.into_result()).expect("Gongbu MCP error serializes")
    })
}

pub(crate) fn create_durable_execution(
    client: &BackendClient,
    arguments: Value,
    expected: &crate::operation_registry::GongbuContinuation,
) -> Result<crate::operation_registry::GongbuLifecycle, DurableCallError> {
    transport::create_durable_execution(client, arguments, expected)
}

pub(crate) fn observe_durable_execution(
    client: &BackendClient,
    execution_id: &str,
    expected: &crate::operation_registry::GongbuContinuation,
) -> Result<crate::operation_registry::GongbuLifecycle, DurableCallError> {
    transport::observe_durable_execution(client, execution_id, expected)
}

pub(crate) fn fetch_durable_execution_observation(
    client: &BackendClient,
    execution_id: &str,
    expected: &crate::operation_registry::GongbuContinuation,
) -> Result<DurableExecutionObservation, DurableCallError> {
    transport::fetch_durable_execution_observation(client, execution_id, expected)
}
