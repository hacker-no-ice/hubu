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

pub(crate) use transport::{CallOutcome, DurableCallError};

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

pub(crate) fn create_continuation_id(arguments: &Value) -> Result<String, Value> {
    request::create_continuation_id(arguments).map_err(|error| {
        serde_json::to_value(error.into_result()).expect("Gongbu MCP error serializes")
    })
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
