//! `hubu budget task`: open, close, list and show task-scoped budgets.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{
    ensure_no_args, get_json, local_timestamp, money, money_at, normalize_currency, post_json,
    string_at, take_flag, take_help, take_required, take_value, terminal, terminal_safe_text,
    u64_at, CliContext,
};

pub(crate) fn command(client: &CliContext, mut args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_help();
        return Ok(());
    };
    args.remove(0);
    match command.as_str() {
        "open" => open(client, args),
        "close" => close(client, args),
        "list" => list(client, args),
        "show" => show(client, args),
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        _ => bail!("unknown budget task command `{command}`"),
    }
}

fn open(client: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_help();
        return Ok(());
    }
    let agent_id = take_required(&mut args, "--agent-id")?;
    let task_id = take_required(&mut args, "--task-id")?;
    let amount = take_required(&mut args, "--amount")?;
    let currency = take_value(&mut args, "--currency")
        .map(|currency| normalize_currency(&currency))
        .transpose()?;
    let reason = take_value(&mut args, "--reason");
    ensure_no_args(args)?;

    let mut body = json!({
        "agent_id": agent_id,
        "task_id": task_id,
        "amount": money::parse_positive_amount(&amount, "--amount")?,
        "reason": reason,
        "source": "hubu-cli",
    });
    if let Some(currency) = currency {
        body["currency"] = json!(currency.to_string());
    }
    let response = post_json(client, "/task-budgets", body)?;
    let style = terminal::stdout();
    if response["idempotent_replay"].as_bool() == Some(true) {
        println!("{}", style.muted("Task budget already open"));
    } else {
        println!("{}", style.success("Task budget opened"));
    }
    print_task_budget(task_budget_at(&response)?)
}

fn close(client: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_help();
        return Ok(());
    }
    let task_budget_id = take_required(&mut args, "--task-budget-id")?;
    ensure_no_args(args)?;
    let response = post_json(
        client,
        "/task-budgets/close",
        json!({"task_budget_id": task_budget_id, "source": "hubu-cli"}),
    )?;
    let style = terminal::stdout();
    if response["idempotent_replay"].as_bool() == Some(true) {
        println!("{}", style.muted("Task budget was already closed"));
    } else {
        println!("{}", style.success("Task budget closed"));
    }
    print_task_budget(task_budget_at(&response)?)
}

fn list(client: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_help();
        return Ok(());
    }
    let include_all = take_flag(&mut args, "--all");
    let agent_id = take_value(&mut args, "--agent-id");
    ensure_no_args(args)?;
    let task_budgets = fetch(client, agent_id.as_deref(), include_all)?;
    if task_budgets.is_empty() {
        let message = if include_all {
            "No task budgets."
        } else {
            "No open task budgets."
        };
        println!("{}", terminal::stdout().muted(message));
        return Ok(());
    }
    for task_budget in &task_budgets {
        print_task_budget(task_budget)?;
    }
    Ok(())
}

fn show(client: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_help();
        return Ok(());
    }
    let task_budget_id = take_value(&mut args, "--task-budget-id");
    let agent_id = take_value(&mut args, "--agent-id");
    let task_id = take_value(&mut args, "--task-id");
    ensure_no_args(args)?;
    let found = match (task_budget_id, agent_id, task_id) {
        (Some(id), None, None) => fetch(client, None, true)?
            .into_iter()
            .find(|task_budget| task_budget["task_budget_id"].as_str() == Some(id.as_str())),
        (None, Some(agent_id), Some(task_id)) => fetch(client, Some(&agent_id), false)?
            .into_iter()
            .find(|task_budget| task_budget["task_id"].as_str() == Some(task_id.as_str())),
        _ => bail!("show a task budget with --task-budget-id, or --agent-id and --task-id"),
    };
    match found {
        Some(task_budget) => print_task_budget(&task_budget),
        None => {
            println!("{}", terminal::stdout().muted("No matching task budget."));
            Ok(())
        }
    }
}

/// Task budgets from the server; open ones unless `include_all`.
pub(crate) fn fetch(
    client: &CliContext,
    agent_id: Option<&str>,
    include_all: bool,
) -> Result<Vec<Value>> {
    let mut query = Vec::new();
    if let Some(agent_id) = agent_id {
        if !agent_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            bail!("--agent-id must be a public agent id");
        }
        query.push(format!("agent_id={agent_id}"));
    }
    if include_all {
        query.push("all=true".to_string());
    }
    let path = if query.is_empty() {
        "/task-budgets".to_string()
    } else {
        format!("/task-budgets?{}", query.join("&"))
    };
    let response = get_json(client, &path)?;
    response
        .get("task_budgets")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("server response missing `task_budgets`"))
}

fn task_budget_at(response: &Value) -> Result<&Value> {
    response
        .get("task_budget")
        .ok_or_else(|| anyhow!("server response missing `task_budget`"))
}

pub(crate) fn print_task_budget(task_budget: &Value) -> Result<()> {
    let style = terminal::stdout();
    println!(
        "  task_budget_id: {}  task: {}  agent_id: {}  state: {}",
        string_at(task_budget, "task_budget_id")?,
        terminal_safe_text(string_at(task_budget, "task_id")?),
        string_at(task_budget, "agent_id")?,
        style.semantic(string_at(task_budget, "state")?)
    );
    println!(
        "    limit: {}  spent: {}  frozen: {}  remaining: {}",
        money_at(task_budget, "amount_limit_cents")?,
        money_at(task_budget, "consumed_amount_cents")?,
        money_at(task_budget, "frozen_amount_cents")?,
        money_at(task_budget, "remaining_amount_cents")?
    );
    println!(
        "    drawn from: {}  settled: {}  outstanding: {}",
        string_at(task_budget, "budget_id")?,
        u64_at(task_budget, "settled_authorizations")?,
        u64_at(task_budget, "outstanding_authorizations")?
    );
    let opened_at = local_timestamp(string_at(task_budget, "opened_at")?);
    match task_budget.get("closed_at").and_then(Value::as_str) {
        Some(closed_at) => println!(
            "    opened: {opened_at}  closed: {}  returned: {}",
            local_timestamp(closed_at),
            money_at(task_budget, "returned_amount_cents")?
        ),
        None => println!("    opened: {opened_at}"),
    }
    Ok(())
}

fn print_help() {
    println!(
        "Open and close task-scoped budgets drawn from an agent budget

A task budget reserves part of the agent's budget for one task. Spend that
carries the task's trusted task_id must fit the task budget; other spend
cannot use its unused part. Closing returns the unused part to the agent
budget. Closing is refused while an authorization is outstanding.

Usage:
  hubu budget task open --agent-id ID --task-id TASK --amount DECIMAL [--currency usd|usdc] [--reason TEXT]
  hubu budget task close --task-budget-id ID
  hubu budget task list [--agent-id ID] [--all]
  hubu budget task show (--task-budget-id ID | --agent-id ID --task-id TASK)

Examples:
  hubu budget task open --agent-id AGENT_ID --task-id landing-page --amount 3
  hubu budget task close --task-budget-id tbg_0123456789ab"
    );
}
