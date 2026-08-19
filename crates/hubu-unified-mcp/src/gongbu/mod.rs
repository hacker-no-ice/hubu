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

pub(crate) fn tool_definitions() -> Vec<Value> {
    catalog::tool_definitions()
}

pub(crate) fn call_tool(client: &BackendClient, name: &str, arguments: Value) -> Value {
    transport::call_tool(client, name, arguments)
}
