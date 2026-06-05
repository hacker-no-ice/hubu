use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    path::Path,
    process::Command,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";
const FINGERPRINT_PREFIX: &str = "sha256:";

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
        "init" => init(args),
        "register" => register(&base_url, args),
        "registration" => registration(&base_url, args),
        "policy" => policy(&base_url, args),
        "agent" => agent(&base_url, args),
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

fn init(mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_init_help();
        return Ok(());
    }

    let policy_path =
        take_value(&mut args, "--policy").unwrap_or_else(|| "policy.yaml".to_string());
    let force = take_flag(&mut args, "--force");
    ensure_no_args(args)?;

    let path = Path::new(&policy_path);
    if path.exists() && !force {
        bail!("policy file `{policy_path}` already exists; pass --force to overwrite");
    }

    fs::write(path, default_policy_template())
        .with_context(|| format!("write default policy template to `{policy_path}`"))?;
    println!("Hubu policy template created");
    println!("  path: {policy_path}");
    println!(
        "  next: edit the file, then run hubu policy add --agent-id AGENT_ID --path {policy_path}"
    );
    Ok(())
}

fn registration(base_url: &str, args: Vec<String>) -> Result<()> {
    match args.as_slice() {
        [] => {
            print_registration_help();
            Ok(())
        }
        [command] if command == "help" || command == "-h" || command == "--help" => {
            print_registration_help();
            Ok(())
        }
        [command] if command == "guidance" => registration_guidance(base_url),
        _ => bail!("usage: hubu registration guidance"),
    }
}

fn registration_guidance(base_url: &str) -> Result<()> {
    let response = get_json(base_url, "/registration/guidance")?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn register(base_url: &str, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_register_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "human" => register_human(base_url, args),
        "agent" => register_agent(base_url, args),
        "-h" | "--help" | "help" => {
            print_register_help();
            Ok(())
        }
        _ => bail!("unknown register command `{command}`"),
    }
}

fn register_human(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_register_human_help();
        return Ok(());
    }

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

    println!("Human registered");
    println!("  user_id: {}", string_at(&response, "user_id")?);
    println!("  display_name: {}", string_at(&response, "display_name")?);
    Ok(())
}

fn register_agent(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_register_agent_help();
        return Ok(());
    }

    let dry_run = take_flag(&mut args, "--dry-run");
    let name = take_value(&mut args, "--name");
    let version = take_value(&mut args, "--version").unwrap_or_else(default_version_label);
    ensure_no_args(args)?;

    let prepared = build_registration_envelope(base_url, name, &version)?;
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&prepared.envelope)?);
        return Ok(());
    }

    print_registration_review(&prepared);
    let response = post_json(base_url, "/agents/register", prepared.envelope)?;

    println!("Agent registered");
    println!("  agent_id: {}", string_at(&response, "agent_id")?);
    println!("  version_id: {}", string_at(&response, "version_id")?);
    println!("  account_id: {}", string_at(&response, "account_id")?);
    println!("  session_id: {}", string_at(&response, "session_id")?);
    Ok(())
}

struct PreparedRegistration {
    envelope: Value,
    review: Vec<(String, String)>,
}

fn build_registration_envelope(
    base_url: &str,
    name: Option<String>,
    version: &str,
) -> Result<PreparedRegistration> {
    let guidance = get_json(base_url, "/registration/guidance")?;
    let protocol_version = string_at(&guidance, "protocol_version")?;
    let user = get_json(base_url, "/user")?;
    let user_id = string_at(&user, "user_id")?;
    let user_name = string_at(&user, "display_name").unwrap_or("Hubu User");
    let client_filled = guidance
        .get("client_filled")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("registration guidance missing `client_filled`"))?;
    let agent_kind = client_filled
        .get("agent_kind")
        .and_then(Value::as_str)
        .unwrap_or("codex_agent");
    let runtime_provider = client_filled
        .get("runtime.provider")
        .and_then(Value::as_str)
        .unwrap_or("codex");
    let runtime_environment = client_filled
        .get("runtime.environment")
        .and_then(Value::as_str)
        .unwrap_or("development");
    let name = name.unwrap_or_else(|| default_agent_name(client_filled));

    let identity_payload = json!({
        "protocol_version": protocol_version,
        "owner": {
            "type": "hubu_user",
            "pub_id": user_id
        },
        "agent_name": name.as_str(),
        "agent_kind": agent_kind
    });
    let identity_fingerprint = fingerprint_payload(&identity_payload);
    let version_payload = json!({
        "protocol_version": protocol_version,
        "identity_fingerprint": identity_fingerprint,
        "version_label": version,
        "runtime": {
            "provider": runtime_provider,
            "environment": runtime_environment
        },
        "hubu_client": {
            "name": "hubu-cli",
            "version": env!("CARGO_PKG_VERSION")
        }
    });
    let version_fingerprint = fingerprint_payload(&version_payload);

    let envelope = json!({
        "protocol_version": protocol_version,
        "identity": {
            "payload": identity_payload,
            "fingerprint": identity_fingerprint
        },
        "version": {
            "payload": version_payload,
            "fingerprint": version_fingerprint
        },
        "review": {
            "display_name": name.as_str(),
            "description": "Registered through the Hubu CLI"
        },
        "signature": null
    });
    let review = vec![
        ("agent_name".to_string(), name.clone()),
        ("owner".to_string(), format!("{user_name} ({user_id})")),
        ("agent_kind".to_string(), agent_kind.to_string()),
        ("version_label".to_string(), version.to_string()),
        ("runtime.provider".to_string(), runtime_provider.to_string()),
        (
            "runtime.environment".to_string(),
            runtime_environment.to_string(),
        ),
        (
            "identity_fingerprint".to_string(),
            compact_fingerprint(&identity_fingerprint),
        ),
        (
            "version_fingerprint".to_string(),
            compact_fingerprint(&version_fingerprint),
        ),
    ];

    Ok(PreparedRegistration { envelope, review })
}

fn print_registration_review(prepared: &PreparedRegistration) {
    println!("Registration review");
    for (label, value) in &prepared.review {
        println!("  {label}: {value}");
    }
}

fn compact_fingerprint(fingerprint: &str) -> String {
    let Some((prefix, digest)) = fingerprint.split_once(':') else {
        return fingerprint.to_string();
    };
    if digest.len() <= 16 {
        fingerprint.to_string()
    } else {
        format!("{prefix}:{}...", &digest[..16])
    }
}

fn default_agent_name(client_filled: &serde_json::Map<String, Value>) -> String {
    let vendor = client_filled
        .get("agent_identity.vendor")
        .and_then(Value::as_str)
        .or_else(|| {
            client_filled
                .get("runtime.provider")
                .and_then(Value::as_str)
        })
        .unwrap_or("agent");
    let template = client_filled
        .get("agent_name.default_template")
        .and_then(Value::as_str)
        .unwrap_or("{vendor}-{workspace}");
    let workspace_name = env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "agent".to_string());
    template
        .replace("{vendor}", vendor)
        .replace("{workspace}", &workspace_name)
}

fn default_version_label() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dev".to_string())
}

fn policy(base_url: &str, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_policy_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "add" => add_policy(base_url, args),
        "-h" | "--help" | "help" => {
            print_policy_help();
            Ok(())
        }
        _ => bail!("unknown policy command `{command}`"),
    }
}

fn add_policy(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_policy_add_help();
        return Ok(());
    }

    let agent_id = take_required(&mut args, "--agent-id")?;
    let path = take_required(&mut args, "--path")?;
    ensure_no_args(args)?;

    let body = json!({
        "agent_id": agent_id,
        "policy_yaml": fs::read_to_string(&path)
            .with_context(|| format!("read policy file `{path}`"))?,
    });

    let response = post_json(base_url, "/policies", body)?;

    println!("Policy added");
    println!("  agent_id: {}", string_at(&response, "agent_id")?);
    println!("  policy_id: {}", string_at(&response, "policy_id")?);
    println!(
        "  policy_version: {}",
        string_at(&response, "policy_version")?
    );
    println!(
        "  default_decision: {}",
        string_at(&response, "default_decision")?
    );
    Ok(())
}

fn agent(base_url: &str, args: Vec<String>) -> Result<()> {
    match args.as_slice() {
        [] => {
            print_agent_help();
            Ok(())
        }
        [command] if command == "help" || command == "-h" || command == "--help" => {
            print_agent_help();
            Ok(())
        }
        [command] if command == "list" => agent_list(base_url),
        _ => bail!("usage: hubu agent list"),
    }
}

fn agent_list(base_url: &str) -> Result<()> {
    let response = get_json(base_url, "/agents")?;
    let agents = response
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `agents`"))?;

    if agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }

    for agent in agents {
        println!(
            "{}  {}  account: {}  status: {}",
            string_at(agent, "agent_id")?,
            string_at(agent, "display_name")?,
            string_at(agent, "account_id")?,
            string_at(agent, "status")?
        );
    }
    Ok(())
}

fn budget(base_url: &str, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_budget_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "create" => budget_create(base_url, args),
        "create-recurring" => budget_create_recurring(base_url, args),
        "list" => budget_list(base_url, args),
        "-h" | "--help" | "help" => {
            print_budget_help();
            Ok(())
        }
        _ => bail!("unknown budget command `{command}`"),
    }
}

fn budget_create(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_create_help();
        return Ok(());
    }

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
    if take_help(&mut args) {
        print_budget_create_recurring_help();
        return Ok(());
    }

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
    let mut args = args;
    if take_help(&mut args) {
        print_budget_list_help();
        return Ok(());
    }
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
    if take_help(&mut args) {
        print_spend_help();
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("authorize") {
        args.remove(0);
        return spend_authorize(base_url, args);
    }

    let account_id = take_value(&mut args, "--account-id");
    let agent_id = take_value(&mut args, "--agent-id");
    let amount = take_required(&mut args, "--amount")?;
    let reason = take_required(&mut args, "--reason")?;
    let merchant =
        take_value(&mut args, "--merchant").unwrap_or_else(|| "demo-merchant".to_string());
    ensure_no_args(args)?;

    if account_id.is_some() == agent_id.is_some() {
        return Err(anyhow!(
            "provide exactly one of --account-id or --agent-id for spend"
        ));
    }

    let mut body = json!({
        "amount_cents": amount_to_cents(&amount)?,
        "reason": reason,
        "merchant": merchant,
    });
    if let Some(account_id) = account_id {
        body["account_id"] = json!(account_id);
    }
    if let Some(agent_id) = agent_id {
        body["agent_id"] = json!(agent_id);
    }

    let response = post_json(base_url, "/spend", body)?;
    print_spend_response(&response)
}

fn spend_authorize(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_spend_authorize_help();
        return Ok(());
    }

    let account_id = take_value(&mut args, "--account-id");
    let agent_id = take_value(&mut args, "--agent-id");
    let amount = take_required(&mut args, "--amount")?;
    let reason = take_required(&mut args, "--reason")?;
    let merchant =
        take_value(&mut args, "--merchant").unwrap_or_else(|| "demo-merchant".to_string());
    ensure_no_args(args)?;

    if account_id.is_some() == agent_id.is_some() {
        return Err(anyhow!(
            "provide exactly one of --account-id or --agent-id for spend"
        ));
    }

    let mut body = json!({
        "amount_cents": amount_to_cents(&amount)?,
        "reason": reason,
        "merchant": merchant,
    });
    if let Some(account_id) = account_id {
        body["account_id"] = json!(account_id);
    }
    if let Some(agent_id) = agent_id {
        body["agent_id"] = json!(agent_id);
    }

    let response = post_json(base_url, "/spend/authorize", body)?;
    print_spend_response(&response)
}

fn print_spend_response(response: &Value) -> Result<()> {
    println!("Spend evaluated");
    println!("  account_id: {}", string_at(response, "account_id")?);
    println!("  agent_id: {}", string_at(response, "agent_id")?);
    println!("  decision: {}", string_at(response, "decision")?);
    println!("  decision_id: {}", string_at(response, "decision_id")?);
    if let Some(token_id) = response.get("auth_token_id").and_then(Value::as_str) {
        println!("  auth_token_id: {token_id}");
    }
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
        println!("  account_id: {}", string_at(payment, "account_id")?);
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
        [] => {
            print_ledger_help();
            Ok(())
        }
        [command] if command == "help" || command == "-h" || command == "--help" => {
            print_ledger_help();
            Ok(())
        }
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
    let authorization_header = auth_token()?
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to Hubu server at {base_url}"))?;

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{authorization_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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

fn auth_token() -> Result<Option<String>> {
    if let Ok(token) = env::var(AUTH_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{AUTH_TOKEN_ENV} cannot be empty"));
        }
        return Ok(Some(token));
    }

    let path =
        env::var(AUTH_TOKEN_FILE_ENV).unwrap_or_else(|_| DEFAULT_AUTH_TOKEN_FILE.to_string());
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Err(anyhow!("Hubu auth token file `{path}` is empty"))
            } else {
                Ok(Some(token))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read Hubu auth token file `{path}`")),
    }
}

fn fingerprint_payload(payload: &Value) -> String {
    let canonical = canonical_json(payload);
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{FINGERPRINT_PREFIX}{}", hex_encode(&digest))
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(&canonical_value(value)).expect("canonical JSON should serialize")
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        _ => value.clone(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to string should not fail");
    }
    encoded
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

fn take_help(args: &mut Vec<String>) -> bool {
    take_flag(args, "-h") || take_flag(args, "--help") || take_flag(args, "help")
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        true
    } else {
        false
    }
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

fn default_policy_template() -> &'static str {
    r#"id: demo_spending_policy
version: demo-1
# The demo server rewrites owner_user_id to the active registered human user.
owner_user_id: 00000000-0000-4000-8000-000000000000
default_effect: needs_approval
rules:
  - id: deny_blocked_demo_merchant
    effect: deny
    reason: merchant is blocked by the demo policy
    when:
      op: eq
      field: merchant
      value:
        string: blocked-merchant

  - id: allow_small_demo_spend
    effect: allow
    reason: amount is at or below the configured demo limit of 10000 cents
    when:
      op: lte
      field: amount
      value:
        money_cents: 10000
"#
}

fn print_help() {
    println!(
        "Hubu demo CLI

Usage:
  hubu [--url URL] <command>

Commands:
  register   Register human users and agents
  registration
             Read agent registration protocol guidance
  policy     Manage spending policies
  init       Generate local Hubu starter files
  agent      Read registered agents
  budget     Create and list budgets
  spend      Submit an agent spend request
  ledger     Read ledger transactions
  health     Check the Hubu server

Global options:
  --url URL   Hubu server URL (default: http://127.0.0.1:8787)

Run `hubu <command> --help` for command-specific help."
    );
}

fn print_registration_help() {
    println!(
        "Read agent registration protocol guidance

Usage:
  hubu registration guidance"
    );
}

fn print_register_help() {
    println!(
        "Register human users and agents

Usage:
  hubu register human [--display-name NAME] [--email EMAIL]
  hubu register agent [--name NAME] [--version VERSION] [--dry-run]"
    );
}

fn print_register_human_help() {
    println!(
        "Register the active human user

Usage:
  hubu register human [--display-name NAME] [--email EMAIL]"
    );
}

fn print_register_agent_help() {
    println!(
        "Register an agent for the active human user

Usage:
  hubu register agent [--name NAME] [--version VERSION] [--dry-run]

Options:
  --name NAME      Agent name (default: guidance vendor/workspace template)
  --version VALUE  Version label (default: current git short SHA, or dev)
  --dry-run        Print the computed registration envelope without submitting it"
    );
}

fn print_policy_help() {
    println!(
        "Manage spending policies

Usage:
  hubu policy add --agent-id ID --path FILE"
    );
}

fn print_policy_add_help() {
    println!(
        "Attach a spending policy to an agent

Usage:
  hubu policy add --agent-id ID --path FILE

Options:
  --agent-id ID   Public agent ID, such as agt_...
  --path FILE     YAML policy file generated by `hubu init` or written by hand"
    );
}

fn print_init_help() {
    println!(
        "Generate local Hubu starter files

Usage:
  hubu init [--policy FILE] [--force]

Options:
  --policy FILE   Policy template path (default: policy.yaml)
  --force         Overwrite an existing policy file"
    );
}

fn print_agent_help() {
    println!(
        "Read registered agents

Usage:
  hubu agent list"
    );
}

fn print_budget_help() {
    println!(
        "Create and list budgets

Usage:
  hubu budget create --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
  hubu budget create-recurring --amount AMOUNT --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]
  hubu budget list"
    );
}

fn print_budget_create_help() {
    println!(
        "Create a single human-scoped budget

Usage:
  hubu budget create --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]"
    );
}

fn print_budget_create_recurring_help() {
    println!(
        "Create a recurring human-scoped budget series

Usage:
  hubu budget create-recurring --amount AMOUNT --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]"
    );
}

fn print_budget_list_help() {
    println!(
        "List budgets for the active human user

Usage:
  hubu budget list"
    );
}

fn print_spend_help() {
    println!(
        "Submit an agent spend request

Usage:
  hubu spend --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend authorize --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend authorize --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]"
    );
}

fn print_spend_authorize_help() {
    println!(
        "Authorize spend and reserve budget without executing payment

Usage:
  hubu spend authorize --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend authorize --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]"
    );
}

fn print_ledger_help() {
    println!(
        "Read ledger transactions

Usage:
  hubu ledger list"
    );
}
