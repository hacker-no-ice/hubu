//! Task budgets bound to this MCP session.
//!
//! Codex and Claude Code calls carry no trusted task correlation. Opening a
//! task budget through this server binds it to the session: later spend
//! operations allocated here carry its `task_id`, so Hubu caps them by the
//! task budget. Closing it, or opening another, ends the binding. A platform
//! invocation envelope keeps its own trusted `task_id`.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{Server, ToolCall};

pub(crate) const OPEN_TOOL: &str = "hubu_open_task_budget";
pub(crate) const CLOSE_TOOL: &str = "hubu_close_task_budget";
pub(crate) const LIST_TOOL: &str = "hubu_list_task_budgets";
const SOURCE: &str = "hubu-unified-mcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveTaskBudget {
    pub(crate) task_budget_id: String,
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
    reject_unknown(&arguments, CLOSE_TOOL, &["task_budget_id"])?;
    let task_budget_id = public_id(&arguments, "task_budget_id", CLOSE_TOOL)?
        .ok_or_else(|| anyhow!("{CLOSE_TOOL} requires task_budget_id"))?;
    Ok(json!({"task_budget_id": task_budget_id, "source": SOURCE}))
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

/// Update this session's binding from a successful open or close result and
/// say what the binding now is.
pub(super) fn observe_result(server: &Server, name: &str, mut result: Value) -> Value {
    let Some(task_budget) = result.pointer("/structuredContent/task_budget").cloned() else {
        return result;
    };
    let (Some(task_budget_id), Some(task_id), Some(state)) = (
        task_budget["task_budget_id"].as_str(),
        task_budget["task_id"].as_str(),
        task_budget["state"].as_str(),
    ) else {
        return result;
    };
    let binding = if name == OPEN_TOOL && state == "open" {
        server.bind_task_budget(ActiveTaskBudget {
            task_budget_id: task_budget_id.to_owned(),
            task_id: task_id.to_owned(),
        });
        "Spend tools in this session now draw from this task budget until it closes."
    } else if name == CLOSE_TOOL {
        server.unbind_task_budget(task_budget_id);
        "This task budget is closed; its unused amount returned to the agent budget. Spend tools in this session draw from the agent budget again."
    } else {
        return result;
    };
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
    result
}

/// The task budget an operation's trusted `task_id` draws from: this
/// session's bound budget when it matches, else the open one for that task.
pub(crate) fn status_task_budget(server: &Server, task_id: &str) -> Option<Value> {
    let response = super::call_tool(
        server,
        Value::Null,
        ToolCall {
            name: LIST_TOOL.into(),
            arguments: json!({"include_all": true}),
            meta: None,
        },
    );
    let task_budgets = response
        .get("result")
        .filter(|result| result.get("isError") != Some(&Value::Bool(true)))
        .and_then(|result| result.pointer("/structuredContent/task_budgets"))
        .and_then(Value::as_array)?;
    let bound = server.active_task_budget();
    let matching = |candidate: &&Value| candidate["task_id"].as_str() == Some(task_id);
    task_budgets
        .iter()
        .filter(matching)
        .find(|candidate| {
            bound.as_ref().is_some_and(|bound| {
                candidate["task_budget_id"].as_str() == Some(bound.task_budget_id.as_str())
            })
        })
        .or_else(|| {
            task_budgets
                .iter()
                .filter(matching)
                .find(|candidate| candidate["state"] == "open")
        })
        .or_else(|| task_budgets.iter().find(matching))
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
