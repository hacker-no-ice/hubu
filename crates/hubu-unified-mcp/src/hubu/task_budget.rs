//! Task budgets bound to this MCP session.
//!
//! Codex and Claude Code calls carry no trusted task correlation. Proposing a
//! task budget through this server binds its task to the session: later spend
//! operations allocated here carry its `task_id`. Hubu denies them until a
//! human approves the proposal, then caps them by the task budget. Asking to
//! close keeps the binding, so Hubu refuses new spend while the request is
//! pending and resumes it if the human rejects the close. The binding ends
//! when this server sees the task budget closed or its proposal denied or
//! expired, or when another task is proposed. A platform invocation envelope
//! keeps its own trusted `task_id`.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{Server, ToolCall};

pub(crate) const OPEN_TOOL: &str = "hubu_open_task_budget";
pub(crate) const CLOSE_TOOL: &str = "hubu_close_task_budget";
pub(crate) const LIST_TOOL: &str = "hubu_list_task_budgets";
pub(crate) const PROPOSAL_PATH: &str = "/task-budgets/proposals";
pub(crate) const CLOSE_REQUEST_PATH: &str = "/task-budgets/close-requests";
const SOURCE: &str = "hubu-unified-mcp";

/// The task this session's spend operations carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveTaskBudget {
    pub(crate) task_id: String,
}

fn object<'a>(arguments: &'a Value, tool: &str) -> Result<&'a serde_json::Map<String, Value>> {
    arguments
        .as_object()
        .ok_or_else(|| anyhow!("{tool} arguments must be an object"))
}

fn reject_unknown(arguments: &Value, tool: &str, allowed: &[&str]) -> Result<()> {
    if let Some(field) = object(arguments, tool)?
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        bail!("{tool} does not accept `{field}`");
    }
    Ok(())
}

/// Public agent ids are `[A-Za-z0-9_]`; anything else cannot reach a query.
fn public_id(arguments: &Value, field: &str, tool: &str) -> Result<Option<String>> {
    match arguments.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value))
            if !value.is_empty()
                && value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_') =>
        {
            Ok(Some(value.clone()))
        }
        Some(_) => bail!("{tool} {field} must be a public id"),
    }
}

pub(super) fn open_body(arguments: Value) -> Result<Value> {
    reject_unknown(
        &arguments,
        OPEN_TOOL,
        &["agent_id", "task_id", "amount", "currency", "reason"],
    )?;
    for required in ["agent_id", "task_id", "amount"] {
        if !arguments.get(required).is_some_and(Value::is_string) {
            bail!("{OPEN_TOOL} requires {required}");
        }
    }
    let mut body = arguments;
    body["source"] = json!(SOURCE);
    Ok(body)
}

pub(super) fn close_body(arguments: Value) -> Result<Value> {
    reject_unknown(&arguments, CLOSE_TOOL, &["task_budget_id", "reason"])?;
    let task_budget_id = public_id(&arguments, "task_budget_id", CLOSE_TOOL)?
        .ok_or_else(|| anyhow!("{CLOSE_TOOL} requires task_budget_id"))?;
    let reason = match arguments.get("reason") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(reason)) => json!(reason),
        Some(_) => bail!("{CLOSE_TOOL} reason must be a string"),
    };
    Ok(json!({"task_budget_id": task_budget_id, "reason": reason, "source": SOURCE}))
}

pub(super) fn list_path(arguments: &Value) -> Result<String> {
    reject_unknown(arguments, LIST_TOOL, &["agent_id", "include_all"])?;
    let mut query = Vec::new();
    if let Some(agent_id) = public_id(arguments, "agent_id", LIST_TOOL)? {
        query.push(format!("agent_id={agent_id}"));
    }
    match arguments.get("include_all") {
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => query.push("all=true".into()),
        Some(_) => bail!("{LIST_TOOL} include_all must be a boolean"),
    }
    Ok(if query.is_empty() {
        "/task-budgets".into()
    } else {
        format!("/task-budgets?{}", query.join("&"))
    })
}

/// Update this session's binding from a successful proposal or close
/// request result and say what the binding now is.
pub(super) fn observe_result(server: &Server, name: &str, mut result: Value) -> Value {
    let Some(content) = result.get("structuredContent") else {
        return result;
    };
    let approval_status = content.pointer("/approval/status").and_then(Value::as_str);
    let task_budget_state = content
        .pointer("/task_budget/state")
        .and_then(Value::as_str);
    let Some(task_id) = content
        .pointer("/approval/review/task_id")
        .or_else(|| content.pointer("/task_budget/task_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return result;
    };
    let binding = match (name, approval_status, task_budget_state) {
        (OPEN_TOOL, Some("pending"), _) => {
            server.bind_task_budget(ActiveTaskBudget { task_id });
            "A human must approve this proposal (approval_request_id) before the task budget opens. Until then it reserves nothing and spend tools in this session are denied."
        }
        (OPEN_TOOL, _, Some("open")) => {
            server.bind_task_budget(ActiveTaskBudget { task_id });
            "Spend tools in this session now draw from this task budget until it closes. Each request still goes through the normal approval policy."
        }
        (CLOSE_TOOL, Some("pending"), _) => {
            "A human must approve closing this task budget (approval_request_id) after checking the task is done. Until then new spend in this session is refused; if the human rejects the close, spending resumes."
        }
        (CLOSE_TOOL, _, Some("closed")) => {
            server.unbind_task_budget(&task_id);
            "This task budget is closed; its unused amount returned to the agent budget. Spend tools in this session draw from the agent budget again."
        }
        _ => return result,
    };
    set_session_binding(&mut result, binding);
    result
}

/// Update this session's binding from a task budget approval read or
/// resolution: a closed task budget, or a denied or expired proposal, ends it.
pub(crate) fn observe_approval(server: &Server, approval: &Value) {
    let Some(task_id) = approval.pointer("/review/task_id").and_then(Value::as_str) else {
        return;
    };
    let kind = approval.get("kind").and_then(Value::as_str);
    let status = approval.get("status").and_then(Value::as_str);
    let closed = approval
        .pointer("/task_budget/state")
        .and_then(Value::as_str)
        == Some("closed");
    let ended = match kind {
        Some("task_budget_open") => matches!(status, Some("denied" | "expired")),
        Some("task_budget_close") => closed,
        _ => false,
    };
    if ended {
        server.unbind_task_budget(task_id);
    }
}

fn set_session_binding(result: &mut Value, binding: &str) {
    if let Some(content) = result
        .get_mut("structuredContent")
        .and_then(Value::as_object_mut)
    {
        content.insert("session_binding".into(), json!(binding));
    }
    if let Some(structured) = result.get("structuredContent").cloned() {
        result["content"] = json!([{
            "type": "text",
            "text": serde_json::to_string_pretty(&structured).expect("tool response should serialize"),
        }]);
    }
}

/// Spend so far against the task budget with this exact public id.
pub(crate) fn status_task_budget(server: &Server, task_budget_id: &str) -> Option<Value> {
    let response = super::call_tool(
        server,
        Value::Null,
        ToolCall {
            name: LIST_TOOL.into(),
            arguments: json!({"include_all": true}),
            meta: None,
        },
    );
    response
        .get("result")
        .filter(|result| result.get("isError") != Some(&Value::Bool(true)))
        .and_then(|result| result.pointer("/structuredContent/task_budgets"))
        .and_then(Value::as_array)?
        .iter()
        .find(|candidate| candidate["task_budget_id"].as_str() == Some(task_budget_id))
        .map(|task_budget| {
            json!({
                "task_budget_id": task_budget["task_budget_id"],
                "task_id": task_budget["task_id"],
                "state": task_budget["state"],
                "amount_limit": task_budget["amount_limit"],
                "spent_amount": task_budget["consumed_amount"],
                "frozen_amount": task_budget["frozen_amount"],
                "remaining_amount": task_budget["remaining_amount"],
            })
        })
}
