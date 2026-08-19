use serde_json::{json, Value};

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

pub(super) fn approval_profile() -> Value {
    let mut profile = hubu_mcp::approval_profile();
    let definitions = tool_definitions();
    let names_matching = |client_mode: &str, runtime_approval: Option<&str>| {
        definitions
            .iter()
            .filter(|tool| {
                tool["annotations"]["x_hubu_client_approval_mode"] == client_mode
                    && runtime_approval.is_none_or(|runtime| {
                        tool["annotations"]["x_hubu_runtime_approval"] == runtime
                    })
            })
            .map(|tool| tool["name"].clone())
            .collect::<Vec<_>>()
    };
    profile["client_policy"]["auto_approve_tools"] = json!(names_matching("auto", None));
    profile["client_policy"]["prompt_before_call_tools"] =
        json!(names_matching("prompt_before_call", None));
    profile["client_policy"]["hubu_policy_conditional_tools"] =
        json!(names_matching("auto", Some("hubu_policy_needs_approval")));
    profile["tools"][0]["names"] = json!(names_matching("auto", Some("none")));
    profile["tools"][1]["names"] =
        json!(names_matching("auto", Some("hubu_policy_needs_approval")));
    profile["tools"][2]["names"] = json!(names_matching(
        "prompt_before_call",
        Some("client_human_approval_required")
    ));
    profile["response_contract"]["agent_action"] = json!(
        "Stop the spend workflow and surface approval_reason plus the structured response to the human."
    );
    profile
}
