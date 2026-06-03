use std::{
    env,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let base_url = take_global_value(&mut args, "--url")
        .or_else(|| env::var("HUBU_URL").ok())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    let Some(command) = args.first().cloned() else {
        print_help();
        return Ok(());
    };
    args.remove(0);

    match command.as_str() {
        "init" => init(&base_url, args),
        "register-agent" => register_agent(&base_url, args),
        "add-policy" => add_policy(&base_url, args),
        "budget" => budget(&base_url, args),
        "spend" => spend(&base_url, args),
        "ledger" => ledger(&base_url, args),
        "health" => health(&base_url),
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        _ => bail!("unknown command `{command}`"),
    }
}

fn init(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let display_name =
        take_value(&mut args, "--display-name").unwrap_or_else(|| "Hubu User".to_string());
    let email = take_value(&mut args, "--email");
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/init",
        json!({
            "display_name": display_name,
            "email": email,
        }),
    )?;

    println!("Hubu initialized");
    println!("  user_id: {}", string_at(&response, "user_id")?);
    println!("  display_name: {}", string_at(&response, "display_name")?);
    Ok(())
}

fn register_agent(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let name = take_required(&mut args, "--name")?;
    let version = take_required(&mut args, "--version")?;
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/agents/register",
        json!({
            "name": name,
            "version": version
        }),
    )?;

    println!("Agent registered");
    println!("  agent_id: {}", string_at(&response, "agent_id")?);
    println!("  version_id: {}", string_at(&response, "version_id")?);
    println!("  account_id: {}", string_at(&response, "account_id")?);
    println!("  session_id: {}", string_at(&response, "session_id")?);
    Ok(())
}

fn add_policy(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let agent_id = take_required(&mut args, "--agent-id")?;
    let daily_limit = take_required(&mut args, "--daily-limit")?;
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/policies",
        json!({
            "agent_id": agent_id,
            "daily_limit_cents": amount_to_cents(&daily_limit)?,
        }),
    )?;

    println!("Policy added");
    println!("  agent_id: {}", string_at(&response, "agent_id")?);
    println!("  policy_id: {}", string_at(&response, "policy_id")?);
    println!(
        "  per_request_limit: {}",
        money_at(&response, "daily_limit_cents")?
    );
    println!("  default_decision: needs_approval");
    Ok(())
}

fn budget(base_url: &str, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        bail!("usage: hubu budget create|create-recurring|list");
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "create" => budget_create(base_url, args),
        "create-recurring" => budget_create_recurring(base_url, args),
        "list" => budget_list(base_url, args),
        _ => bail!("unknown budget command `{command}`"),
    }
}

fn budget_create(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let amount = take_required(&mut args, "--amount")?;
    let starting_at = take_value(&mut args, "--starting-at");
    let ending_before = take_value(&mut args, "--ending-before");
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/budgets",
        json!({
            "amount_cents": amount_to_cents(&amount)?,
            "starting_at": starting_at,
            "ending_before": ending_before,
        }),
    )?;

    println!("Budget created");
    print_budget(
        response
            .get("budget")
            .ok_or_else(|| anyhow!("server response missing `budget`"))?,
    )?;
    Ok(())
}

fn budget_create_recurring(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let amount = take_required(&mut args, "--amount")?;
    let recurrence = take_required(&mut args, "--recurrence")?;
    let period_count = take_required(&mut args, "--period-count")?;
    let starting_at = take_value(&mut args, "--starting-at");
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/budgets/series",
        json!({
            "amount_cents": amount_to_cents(&amount)?,
            "starting_at": starting_at,
            "recurrence": recurrence,
            "period_count": period_count.parse::<usize>()?,
        }),
    )?;

    println!("Budget series created");
    for budget in response
        .get("budgets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `budgets`"))?
    {
        print_budget(budget)?;
    }
    Ok(())
}

fn budget_list(base_url: &str, args: Vec<String>) -> Result<()> {
    ensure_no_args(args)?;
    let response = get_json(base_url, "/budgets")?;
    let budgets = response
        .get("budgets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `budgets`"))?;

    if budgets.is_empty() {
        println!("No budgets configured.");
        return Ok(());
    }

    for budget in budgets {
        print_budget(budget)?;
    }
    Ok(())
}

fn spend(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let agent_id = take_required(&mut args, "--agent-id")?;
    let amount = take_required(&mut args, "--amount")?;
    let reason = take_required(&mut args, "--reason")?;
    let merchant =
        take_value(&mut args, "--merchant").unwrap_or_else(|| "demo-merchant".to_string());
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/spend",
        json!({
            "agent_id": agent_id,
            "amount_cents": amount_to_cents(&amount)?,
            "reason": reason,
            "merchant": merchant,
        }),
    )?;

    println!("Spend evaluated");
    println!("  decision: {}", string_at(&response, "decision")?);
    println!("  decision_id: {}", string_at(&response, "decision_id")?);
    if let Some(reasons) = response.get("reasons").and_then(Value::as_array) {
        for reason in reasons {
            println!(
                "  reason: {}",
                reason.as_str().unwrap_or("<non-string reason>")
            );
        }
    }

    if let Some(payment) = response
        .get("payment")
        .filter(|payment| payment.is_object())
    {
        println!("Payment");
        println!("  status: {}", string_at(payment, "status")?);
        println!("  payment_id: {}", string_at(payment, "payment_id")?);
        println!(
            "  owner_user: {} ({})",
            string_at(payment, "owner_user_name")?,
            string_at(payment, "owner_user_id")?
        );
        if let Some(tx_id) = payment.get("ledger_transaction_id").and_then(Value::as_str) {
            println!("  ledger_transaction_id: {tx_id}");
        }
        if let Some(rail_ref) = payment.get("rail_reference").and_then(Value::as_str) {
            println!("  rail_reference: {rail_ref}");
        }
        if let Some(reason) = payment.get("failure_reason").and_then(Value::as_str) {
            println!("  failure_reason: {reason}");
        }
    }

    if let Some(hold) = response.get("budget_hold").filter(|hold| hold.is_object()) {
        println!("Budget hold");
        println!("  status: {}", string_at(hold, "status")?);
        println!("  hold_id: {}", string_at(hold, "hold_id")?);
        println!("  budget_id: {}", string_at(hold, "budget_id")?);
        println!("  amount: {}", money_at(hold, "amount_cents")?);
        println!("  consumed: {}", money_at(hold, "consumed_amount_cents")?);
        println!("  frozen: {}", money_at(hold, "frozen_amount_cents")?);
        println!("  remaining: {}", money_at(hold, "remaining_amount_cents")?);
    }
    Ok(())
}

fn ledger(base_url: &str, args: Vec<String>) -> Result<()> {
    match args.as_slice() {
        [command] if command == "list" => {
            let response = get_json(base_url, "/ledger")?;
            let transactions = response
                .get("transactions")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("server response missing transactions"))?;

            if transactions.is_empty() {
                println!("No ledger transactions recorded.");
                return Ok(());
            }

            for transaction in transactions {
                println!(
                    "{}  {}  {}  owner: {} ({})",
                    string_at(transaction, "created_at")?,
                    string_at(transaction, "id")?,
                    string_at(transaction, "description")?,
                    string_at(transaction, "owner_user_name")?,
                    string_at(transaction, "owner_user_id")?
                );

                for entry in transaction
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("ledger transaction missing entries"))?
                {
                    println!(
                        "  {:<6} {:>10}  {}  owner: {} ({})",
                        string_at(entry, "direction")?,
                        money_at(entry, "amount_cents")?,
                        string_at(entry, "account_id")?,
                        string_at(entry, "owner_user_name")?,
                        string_at(entry, "owner_user_id")?
                    );
                }
            }
            Ok(())
        }
        _ => bail!("usage: hubu ledger list"),
    }
}

fn print_budget(budget: &Value) -> Result<()> {
    println!(
        "  budget_id: {}  scope: {}  status: {}",
        string_at(budget, "budget_id")?,
        string_at(budget, "scope")?,
        string_at(budget, "status")?
    );
    println!(
        "    limit: {}  consumed: {}  frozen: {}  remaining: {}",
        money_at(budget, "amount_limit_cents")?,
        money_at(budget, "consumed_amount_cents")?,
        money_at(budget, "frozen_amount_cents")?,
        money_at(budget, "remaining_amount_cents")?
    );
    println!(
        "    period: {} -> {}",
        string_at(budget, "starting_at")?,
        budget
            .get("ending_before")
            .and_then(Value::as_str)
            .unwrap_or("open-ended")
    );
    Ok(())
}

fn health(base_url: &str) -> Result<()> {
    let response = get_json(base_url, "/health")?;
    println!("Hubu server: {}", string_at(&response, "status")?);
    Ok(())
}

fn request_json(base_url: &str, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let (host, port) = parse_base_url(base_url)?;
    let body_text = body.map(|body| body.to_string()).unwrap_or_default();
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to Hubu server at {base_url}"))?;

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body_text.len(),
        body_text
    )?;
    stream.shutdown(Shutdown::Write)?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (status, response_body) = parse_http_response(&raw)?;
    let json: Value = serde_json::from_str(response_body)
        .with_context(|| format!("parse server response body `{response_body}`"))?;

    if !(200..300).contains(&status) {
        let message = json
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        bail!("server returned HTTP {status}: {message}");
    }

    Ok(json)
}

fn get_json(base_url: &str, path: &str) -> Result<Value> {
    request_json(base_url, "GET", path, None)
}

fn post_json(base_url: &str, path: &str, body: Value) -> Result<Value> {
    request_json(base_url, "POST", path, Some(body))
}

fn parse_base_url(base_url: &str) -> Result<(String, u16)> {
    let trimmed = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// Hubu URLs are supported"))?;
    let host_port = trimmed.trim_end_matches('/');
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("Hubu URL must include a port"))?;
    Ok((host.to_string(), port.parse()?))
}

fn parse_http_response(raw: &str) -> Result<(u16, &str)> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid HTTP status line"))?
        .parse()?;
    Ok((status, body))
}

fn take_global_value(args: &mut Vec<String>, name: &str) -> Option<String> {
    take_value(args, name)
}

fn take_required(args: &mut Vec<String>, name: &str) -> Result<String> {
    take_value(args, name).ok_or_else(|| anyhow!("missing required argument `{name}`"))
}

fn take_value(args: &mut Vec<String>, name: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == name)?;
    args.remove(index);
    if index >= args.len() {
        return None;
    }
    Some(args.remove(index))
}

fn ensure_no_args(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        bail!("unexpected arguments: {}", args.join(" "))
    }
}

fn string_at<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("server response missing `{key}`"))
}

fn money_at(value: &Value, key: &str) -> Result<String> {
    let cents = value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("server response missing `{key}`"))?;
    Ok(format!("${}.{:02}", cents / 100, cents.abs() % 100))
}

fn amount_to_cents(value: &str) -> Result<i64> {
    let (dollars, cents) = value.split_once('.').unwrap_or((value, "0"));
    if cents.len() > 2 {
        bail!("amount `{value}` has more than two decimal places");
    }
    let dollars = dollars.parse::<i64>()?;
    let cents = format!("{cents:0<2}").parse::<i64>()?;
    Ok(dollars * 100 + cents)
}

fn print_help() {
    println!(
        "Hubu demo CLI

Usage:
  hubu [--url http://127.0.0.1:8787] register-agent --name NAME --version VERSION
  hubu [--url http://127.0.0.1:8787] init [--display-name NAME] [--email EMAIL]
  hubu [--url http://127.0.0.1:8787] add-policy --agent-id ID --daily-limit AMOUNT
  hubu [--url http://127.0.0.1:8787] budget create --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
  hubu [--url http://127.0.0.1:8787] budget create-recurring --amount AMOUNT --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]
  hubu [--url http://127.0.0.1:8787] budget list
  hubu [--url http://127.0.0.1:8787] spend --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu [--url http://127.0.0.1:8787] ledger list
  hubu [--url http://127.0.0.1:8787] health"
    );
}
