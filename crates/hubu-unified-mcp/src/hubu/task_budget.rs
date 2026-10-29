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
pub(crate) const GET_APPROVAL_TOOL: &str = "hubu_get_task_budget_approval";
pub(crate) const RESOLVE_APPROVAL_TOOL: &str = "hubu_resolve_task_budget_approval";
pub(crate) const PROPOSAL_PATH: &str = "/task-budgets/proposals";
pub(crate) const CLOSE_REQUEST_PATH: &str = "/task-budgets/close-requests";
pub(crate) const APPROVAL_PATH: &str = "/task-budgets/approvals";
pub(crate) const APPROVAL_RESOLVE_PATH: &str = "/task-budgets/approvals/resolve";
/// Task budget approval ids carry this prefix; spend approval tools refuse it.
pub(crate) const APPROVAL_ID_PREFIX: &str = "tba_";
const SOURCE: &str = "hubu-unified-mcp";

/// The task this session's spend operations carry, and the proposal and task
/// budget that bound it. Only results about this exact proposal or task budget
/// end the binding, so replaying an older proposal or close request for the
/// same task cannot unbind a newer one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveTaskBudget {
    pub(crate) task_id: String,
    pub(crate) approval_request_id: Option<String>,
    pub(crate) task_budget_id: Option<String>,
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

/// Whether a tool reviews or resolves a task budget approval request.
pub(crate) fn is_approval_tool(name: &str) -> bool {
    matches!(name, GET_APPROVAL_TOOL | RESOLVE_APPROVAL_TOOL)
}

/// The `tba_` id a task budget approval tool names.
pub(super) fn approval_request_id(
    arguments: &Value,
    tool: &str,
    fields: &[&str],
) -> Result<String> {
    reject_unknown(arguments, tool, fields)?;
    match arguments.get("approval_request_id") {
        Some(Value::String(id))
            if id.len() > APPROVAL_ID_PREFIX.len()
                && id.len() <= 64
                && id.starts_with(APPROVAL_ID_PREFIX)
                && id[APPROVAL_ID_PREFIX.len()..]
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()) =>
        {
            Ok(id.clone())
        }
        _ => bail!(
            "{tool} requires a task budget approval_request_id ({APPROVAL_ID_PREFIX}...); review spend approvals with hubu_get_spend_approval"
        ),
    }
}

pub(super) fn approval_path(arguments: &Value) -> Result<String> {
    let approval_request_id =
        approval_request_id(arguments, GET_APPROVAL_TOOL, &["approval_request_id"])?;
    Ok(format!(
        "{APPROVAL_PATH}?approval_request_id={approval_request_id}"
    ))
}

pub(super) fn resolve_approval_body(arguments: &Value) -> Result<Value> {
    let approval_request_id = approval_request_id(
        arguments,
        RESOLVE_APPROVAL_TOOL,
        &["approval_request_id", "decision"],
    )?;
    let decision = arguments
        .get("decision")
        .and_then(Value::as_str)
        .filter(|decision| matches!(*decision, "approve" | "deny"))
        .ok_or_else(|| anyhow!("{RESOLVE_APPROVAL_TOOL} requires decision approve or deny"))?;
    Ok(json!({"approval_request_id": approval_request_id, "decision": decision}))
}

/// Spend approval tools cover spend only.
pub(super) fn reject_on_spend_tool(approval_request_id: &str, tool: &str) -> Result<()> {
    if approval_request_id.starts_with(APPROVAL_ID_PREFIX) {
        let replacement = if tool == "hubu_resolve_spend_approval" {
            RESOLVE_APPROVAL_TOOL
        } else {
            GET_APPROVAL_TOOL
        };
        bail!(
            "{approval_request_id} is a task budget approval request, not a spend approval; use {replacement}"
        );
    }
    Ok(())
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
    let approval_request_id = string_at(content, "/approval/approval_request_id");
    let task_budget_id = string_at(content, "/task_budget/task_budget_id");
    let task_budget_state = content
        .pointer("/task_budget/state")
        .and_then(Value::as_str);
    let Some(task_id) = string_at(content, "/approval/review/task_id")
        .or_else(|| string_at(content, "/task_budget/task_id"))
    else {
        return result;
    };
    let binding = match (name, approval_status, task_budget_state) {
        (OPEN_TOOL, Some("pending"), _) => {
            server.bind_task_budget(ActiveTaskBudget {
                task_id,
                approval_request_id,
                task_budget_id: None,
            });
            "A human must approve this proposal (approval_request_id) before the task budget opens. Until then it reserves nothing and spend tools in this session are denied."
        }
        (OPEN_TOOL, _, Some("open")) => {
            server.bind_task_budget(ActiveTaskBudget {
                task_id,
                approval_request_id,
                task_budget_id,
            });
            "Spend tools in this session now draw from this task budget until it closes. Each request still goes through the normal approval policy."
        }
        (CLOSE_TOOL, Some("pending"), _) => {
            // Hubu accepts a close request only for an open task budget, so a
            // binding still waiting on its proposal learns its task budget here.
            if let Some(task_budget_id) = task_budget_id {
                server.update_task_budget(|active| {
                    if let Some(active) = active.as_mut().filter(|active| {
                        active.task_id == task_id && active.task_budget_id.is_none()
                    }) {
                        active.task_budget_id = Some(task_budget_id);
                    }
                });
            }
            "A human must approve closing this task budget (approval_request_id) after checking the task is done. Until then new spend in this session is refused; if the human rejects the close, spending resumes."
        }
        (CLOSE_TOOL, _, Some("closed")) => {
            unbind_task_budget(server, task_budget_id.as_deref());
            "This task budget is closed; its unused amount returned to the agent budget. Spend tools in this session draw from the agent budget again."
        }
        _ => return result,
    };
    set_session_binding(&mut result, binding);
    result
}

/// Update this session's binding from a task budget approval read or
/// resolution. Approving the bound proposal records its task budget; denying
/// or expiring the bound proposal, or closing the bound task budget, ends it.
pub(crate) fn observe_approval(server: &Server, approval: &Value) {
    let approval_request_id = string_at(approval, "/approval_request_id");
    let task_budget_id = string_at(approval, "/task_budget/task_budget_id");
    let kind = approval.get("kind").and_then(Value::as_str);
    let status = approval.get("status").and_then(Value::as_str);
    let state = approval
        .pointer("/task_budget/state")
        .and_then(Value::as_str);
    server.update_task_budget(|active| {
        let Some(bound) = active.as_mut() else {
            return;
        };
        let bound_proposal =
            approval_request_id.is_some() && bound.approval_request_id == approval_request_id;
        match (kind, status, state) {
            (Some("task_budget_open"), Some("denied" | "expired"), _) if bound_proposal => {
                *active = None;
            }
            (Some("task_budget_open"), Some("approved"), _) if bound_proposal => {
                bound.task_budget_id = task_budget_id;
            }
            (Some("task_budget_close"), _, Some("closed"))
                if task_budget_id.is_some() && bound.task_budget_id == task_budget_id =>
            {
                *active = None;
            }
            _ => {}
        }
    });
}

fn unbind_task_budget(server: &Server, task_budget_id: Option<&str>) {
    let Some(task_budget_id) = task_budget_id else {
        return;
    };
    server.update_task_budget(|active| {
        if active
            .as_ref()
            .is_some_and(|active| active.task_budget_id.as_deref() == Some(task_budget_id))
        {
            *active = None;
        }
    });
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
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
