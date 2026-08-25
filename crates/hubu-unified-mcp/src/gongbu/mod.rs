//! Gongbu-owned routes for the unified MCP adapter.
//!
//! This facade intentionally mirrors Gongbu's public MCP wire contract without
//! depending on a Gongbu crate. Requests use fixed relative paths on the
//! separately configured Gongbu client and are never retried by the router.

mod catalog;
mod request;
mod response;
mod transport;

#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::BackendClient;

pub(crate) use transport::CallOutcome;

pub(crate) fn tool_definitions() -> Vec<Value> {
    catalog::tool_definitions()
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
