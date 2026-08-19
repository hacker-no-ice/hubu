use serde_json::Value;

use crate::{BackendOwner, DOMAIN_TOOLS};

pub(crate) fn is_approved_tool(name: &str) -> bool {
    DOMAIN_TOOLS
        .iter()
        .any(|(candidate, owner)| *owner == BackendOwner::Hubu && *candidate == name)
}

pub(crate) fn tool_definitions() -> Vec<Value> {
    hubu_mcp::tool_definitions()
        .into_iter()
        .filter(|tool| tool["name"].as_str().is_some_and(is_approved_tool))
        .collect()
}
