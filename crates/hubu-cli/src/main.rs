use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Local};
use hubu_core::policy::{Effect, Policy};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";
const FINGERPRINT_PREFIX: &str = "sha256:";
const DEFAULT_POLICY_TEMPLATE_PATH: &str = "policies/policy.yaml";
const DEFAULT_POLICY_TEMPLATE: &str = include_str!("../../../policies/starter-policy.yaml");
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const HUBU_HOME_ENV: &str = "HUBU_HOME";
const HUBU_MCP_SERVER_ENV: &str = "HUBU_MCP_SERVER";
const HUBU_CODEX_MCP_BEGIN: &str = "# >>> hubu managed codex mcp";
const HUBU_CODEX_MCP_END: &str = "# <<< hubu managed codex mcp";

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
        "register" => register(&base_url, args),
        "protocol" => protocol(&base_url, args),
        "user" => user(&base_url, args),
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

fn init(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if args.first().map(String::as_str) == Some("codex") {
        args.remove(0);
        return init_codex(base_url, args);
    }

    if take_help(&mut args) {
        print_init_help();
        return Ok(());
    }

    let policy_path =
        take_value(&mut args, "--policy").unwrap_or_else(|| "policy.yaml".to_string());
    let force = take_flag(&mut args, "--force");
    ensure_no_args(args)?;

    write_policy_template(&policy_path, force)
}

fn init_codex(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_init_codex_help();
        return Ok(());
    }

    let config_path = take_value(&mut args, "--config")
        .map(PathBuf::from)
        .unwrap_or_else(default_codex_config_path);
    let mcp_server = take_value(&mut args, "--mcp-server")
        .map(PathBuf::from)
        .unwrap_or_else(default_mcp_server_path);
    let token_file = take_value(&mut args, "--token-file")
        .map(PathBuf::from)
        .unwrap_or_else(default_codex_token_file_path);
    let force = take_flag(&mut args, "--force");
    let dry_run = take_flag(&mut args, "--dry-run");
    let trust_client_approval = take_flag(&mut args, "--trust-client-approval");
    ensure_no_args(args)?;

    let mcp_server = absolute_existing_file(&mcp_server)
        .with_context(|| format!("resolve hubu-mcp-server path `{}`", mcp_server.display()))?;

    let token_file = if dry_run {
        absolute_path(&token_file)?
    } else {
        ensure_auth_token_file(&token_file)
            .with_context(|| format!("prepare Hubu auth token file `{}`", token_file.display()))?
    };
    let block = codex_mcp_config_block(&mcp_server, base_url, &token_file, trust_client_approval);

    if dry_run {
        println!("{block}");
        return Ok(());
    }

    write_codex_mcp_config(&config_path, &block, force)
        .with_context(|| format!("update Codex config `{}`", config_path.display()))?;

    println!("Codex MCP configured for Hubu");
    println!("  config: {}", config_path.display());
    println!("  mcp_server: {}", mcp_server.display());
    println!("  hubu_url: {base_url}");
    println!("  token_file: {}", token_file.display());
    println!("  next: restart Codex, then use /mcp or ask Codex to list Hubu tools");
    println!(
        "  server: start hubu-server with {AUTH_TOKEN_FILE_ENV}={}",
        token_file.display()
    );
    println!(
        "  spend_tools: Codex pre-approves Hubu spend tool calls; Hubu still returns needs_approval without payment when policy requires review"
    );
    if trust_client_approval {
        println!("  approval_tools: enabled because --trust-client-approval was set");
    } else {
        println!("  approval_tools: disabled; use the CLI for setup, policy, and budget changes");
    }
    Ok(())
}

fn default_codex_config_path() -> PathBuf {
    codex_home().join("config.toml")
}

fn codex_home() -> PathBuf {
    env::var_os(CODEX_HOME_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

fn default_codex_token_file_path() -> PathBuf {
    if let Ok(path) = env::var(AUTH_TOKEN_FILE_ENV) {
        return PathBuf::from(path);
    }

    let local_token_file = PathBuf::from(DEFAULT_AUTH_TOKEN_FILE);
    if local_token_file.exists() {
        return local_token_file;
    }

    hubu_home().join(DEFAULT_AUTH_TOKEN_FILE)
}

fn hubu_home() -> PathBuf {
    env::var_os(HUBU_HOME_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".hubu"))
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_mcp_server_path() -> PathBuf {
    if let Ok(path) = env::var(HUBU_MCP_SERVER_ENV) {
        return PathBuf::from(path);
    }

    if let Ok(current_exe) = env::current_exe() {
        let sibling = current_exe.with_file_name(mcp_server_bin_name());
        if sibling.exists() {
            return sibling;
        }
    }

    find_on_path(mcp_server_bin_name()).unwrap_or_else(|| PathBuf::from(mcp_server_bin_name()))
}

fn mcp_server_bin_name() -> &'static str {
    if cfg!(windows) {
        "hubu-mcp-server.exe"
    } else {
        "hubu-mcp-server"
    }
}

fn find_on_path(bin_name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(bin_name))
        .find(|candidate| candidate.is_file())
}

fn absolute_existing_file(path: &Path) -> Result<PathBuf> {
    let path = absolute_path(path)?;
    if !path.is_file() {
        bail!(
            "`{}` is not a file; build or install hubu-mcp-server, or pass --mcp-server PATH",
            path.display()
        );
    }
    fs::canonicalize(&path).with_context(|| format!("canonicalize `{}`", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn ensure_auth_token_file(path: &Path) -> Result<PathBuf> {
    let path = absolute_path(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create token directory `{}`", parent.display()))?;
    }

    match fs::read_to_string(&path) {
        Ok(contents) if !contents.trim().is_empty() => {
            return fs::canonicalize(&path)
                .with_context(|| format!("canonicalize token file `{}`", path.display()));
        }
        Ok(_) => bail!("Hubu auth token file `{}` is empty", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("read `{}`", path.display())),
    }

    let token = env::var(AUTH_TOKEN_ENV)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(generate_local_auth_token);
    fs::write(&path, format!("{token}\n"))
        .with_context(|| format!("write token file `{}`", path.display()))?;
    restrict_token_permissions(&path)?;
    fs::canonicalize(&path).with_context(|| format!("canonicalize token file `{}`", path.display()))
}

fn generate_local_auth_token() -> String {
    format!("hubu_{}", Uuid::new_v4().simple())
}

#[cfg(unix)]
fn restrict_token_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_token_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_codex_mcp_config(config_path: &Path, block: &str, force: bool) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Codex config directory `{}`", parent.display()))?;
    }
    let existing = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("read `{}`", config_path.display()))
        }
    };
    let updated = upsert_managed_codex_mcp_block(&existing, block, force)?;
    fs::write(config_path, updated)
        .with_context(|| format!("write Codex config `{}`", config_path.display()))
}

fn codex_mcp_config_block(
    mcp_server: &Path,
    base_url: &str,
    token_file: &Path,
    trust_client_approval: bool,
) -> String {
    let mut block = format!(
        "{HUBU_CODEX_MCP_BEGIN}\n\
         [mcp_servers.hubu]\n\
         command = \"{}\"\n\
         startup_timeout_sec = 10\n\
         tool_timeout_sec = 60\n\n\
         [mcp_servers.hubu.env]\n\
         HUBU_URL = \"{}\"\n\
         {AUTH_TOKEN_FILE_ENV} = \"{}\"\n",
        toml_basic_string(&mcp_server.display().to_string()),
        toml_basic_string(base_url),
        toml_basic_string(&token_file.display().to_string())
    );
    if trust_client_approval {
        let _ = writeln!(block, "HUBU_MCP_TRUST_CLIENT_APPROVAL = \"1\"");
    }
    block.push_str(
        "\n[mcp_servers.hubu.tools.hubu_authorize_spend]\n\
         approval_mode = \"approve\"\n\n\
         [mcp_servers.hubu.tools.hubu_submit_spend]\n\
         approval_mode = \"approve\"\n",
    );
    let _ = writeln!(block, "{HUBU_CODEX_MCP_END}");
    block
}

fn upsert_managed_codex_mcp_block(existing: &str, block: &str, force: bool) -> Result<String> {
    let lines = existing.lines().collect::<Vec<_>>();
    if let Some(start) = lines
        .iter()
        .position(|line| line.trim() == HUBU_CODEX_MCP_BEGIN)
    {
        let end = lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find_map(|(index, line)| (line.trim() == HUBU_CODEX_MCP_END).then_some(index))
            .ok_or_else(|| {
                anyhow!("Codex config has a Hubu managed block without an end marker")
            })?;
        let mut updated = Vec::new();
        updated.extend(lines[..start].iter().copied());
        updated.extend(block.trim_end_matches('\n').lines());
        updated.extend(lines[end + 1..].iter().copied());
        return Ok(join_config_lines(&updated));
    }

    let existing = if contains_hubu_mcp_table(existing) {
        if !force {
            bail!(
                "Codex config already contains an unmanaged [mcp_servers.hubu] table; pass --force to replace it"
            );
        }
        remove_hubu_mcp_tables(existing)
    } else {
        existing.to_string()
    };

    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(block.trim_end_matches('\n'));
    updated.push('\n');
    Ok(updated)
}

fn join_config_lines(lines: &[&str]) -> String {
    let mut value = lines.join("\n");
    value.push('\n');
    value
}

fn contains_hubu_mcp_table(config: &str) -> bool {
    config
        .lines()
        .filter_map(toml_table_name)
        .any(is_hubu_mcp_table)
}

fn remove_hubu_mcp_tables(config: &str) -> String {
    let mut kept = Vec::new();
    let mut skipping = false;
    for line in config.lines() {
        if let Some(table) = toml_table_name(line) {
            skipping = is_hubu_mcp_table(table);
        }
        if !skipping {
            kept.push(line);
        }
    }
    join_config_lines(&kept)
}

fn toml_table_name(line: &str) -> Option<&str> {
    let trimmed = line
        .split_once('#')
        .map(|(before_comment, _)| before_comment)
        .unwrap_or(line)
        .trim();
    if trimmed.starts_with("[[") || !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }
    Some(trimmed.trim_start_matches('[').trim_end_matches(']').trim())
}

fn is_hubu_mcp_table(table: &str) -> bool {
    table == "mcp_servers.hubu" || table.starts_with("mcp_servers.hubu.")
}

fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character => escaped.push(character),
        }
    }
    escaped
}

fn protocol(base_url: &str, args: Vec<String>) -> Result<()> {
    match args.as_slice() {
        [] => {
            print_protocol_help();
            Ok(())
        }
        [command] if command == "help" || command == "-h" || command == "--help" => {
            print_protocol_help();
            Ok(())
        }
        [protocol_name] if protocol_name == "agent-registration" => {
            agent_registration_protocol(base_url)
        }
        _ => bail!("usage: hubu protocol agent-registration"),
    }
}

fn agent_registration_protocol(base_url: &str) -> Result<()> {
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

    let username = take_value(&mut args, "--username").ok_or_else(|| {
        anyhow!("missing --username; use 3-32 lowercase letters, digits, or hyphens")
    })?;
    let display_name = take_value(&mut args, "--display-name")
        .ok_or_else(|| anyhow!("missing --display-name; provide a human-readable display name"))?;
    let email = take_value(&mut args, "--email");
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/init",
        json!({
            "username": username,
            "display_name": display_name,
            "email": email,
        }),
    )?;

    println!("Human registered");
    println!("  user_id: {}", string_at(&response, "user_id")?);
    println!(
        "  username: {}",
        response
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("-")
    );
    println!("  display_name: {}", string_at(&response, "display_name")?);
    Ok(())
}

fn user(base_url: &str, args: Vec<String>) -> Result<()> {
    match args.split_first() {
        None => {
            print_user_help();
            Ok(())
        }
        Some((command, [])) if command == "help" || command == "-h" || command == "--help" => {
            print_user_help();
            Ok(())
        }
        Some((command, [])) if command == "list" => user_list(base_url),
        Some((command, rest)) if command == "cap" => user_cap(base_url, rest.to_vec()),
        _ => bail!("usage: hubu user list | hubu user cap set|show|revoke"),
    }
}

fn user_cap(base_url: &str, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_user_cap_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "set" => user_cap_set(base_url, args),
        "show" => user_cap_show(base_url, args),
        "revoke" => user_cap_revoke(base_url, args),
        "-h" | "--help" | "help" => {
            print_user_cap_help();
            Ok(())
        }
        _ => bail!("unknown user cap command `{command}`"),
    }
}

fn user_cap_set(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_user_cap_set_help();
        return Ok(());
    }

    let amount = take_required(&mut args, "--amount")?;
    let starting_at = take_value(&mut args, "--starting-at");
    let ending_before = take_value(&mut args, "--ending-before");
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/user/cap",
        json!({
            "amount_cents": amount_to_cents(&amount)?,
            "starting_at": starting_at,
            "ending_before": ending_before,
        }),
    )?;

    println!("User cap set");
    print_user_cap(
        response
            .get("cap")
            .ok_or_else(|| anyhow!("server response missing `cap`"))?,
    )?;
    Ok(())
}

fn user_cap_show(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_user_cap_show_help();
        return Ok(());
    }

    let include_all = take_flag(&mut args, "--all");
    ensure_no_args(args)?;
    let path = if include_all {
        "/user/cap?all=true"
    } else {
        "/user/cap"
    };
    let response = get_json(base_url, path)?;
    let caps = response
        .get("caps")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `caps`"))?;

    if caps.is_empty() {
        println!("No user caps configured.");
        return Ok(());
    }

    for cap in caps {
        print_user_cap(cap)?;
    }
    Ok(())
}

fn user_cap_revoke(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_user_cap_revoke_help();
        return Ok(());
    }

    let cap_id = take_required(&mut args, "--cap-id")?;
    ensure_no_args(args)?;
    let response = post_json(
        base_url,
        "/user/cap/revoke",
        json!({
            "cap_id": cap_id,
        }),
    )?;

    println!("User cap revoked");
    print_user_cap(
        response
            .get("cap")
            .ok_or_else(|| anyhow!("server response missing `cap`"))?,
    )?;
    Ok(())
}

fn user_list(base_url: &str) -> Result<()> {
    let response = get_json(base_url, "/users")?;
    let users = response
        .get("users")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `users`"))?;

    if users.is_empty() {
        println!("No human users registered.");
        return Ok(());
    }

    let rows = users
        .iter()
        .map(|user| {
            Ok(vec![
                string_at(user, "user_id")?.to_string(),
                user.get("username")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                string_at(user, "display_name")?.to_string(),
                user.get("email")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                string_at(user, "status")?.to_string(),
                if user.get("current").and_then(Value::as_bool) == Some(true) {
                    "*".to_string()
                } else {
                    "-".to_string()
                },
                local_timestamp(string_at(user, "created_at")?),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    print_table(
        &[
            "USER ID",
            "USERNAME",
            "DISPLAY NAME",
            "EMAIL",
            "STATUS",
            "CURRENT",
            "CREATED AT",
        ],
        &rows,
    );
    Ok(())
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .filter_map(|row| row.get(index))
                .map(|value| value.len())
                .max()
                .unwrap_or(0)
                .max(header.len())
        })
        .collect::<Vec<_>>();

    print_table_row(headers.iter().copied(), &widths);
    print_table_separator(&widths);
    for row in rows {
        print_table_row(row.iter().map(String::as_str), &widths);
    }
}

fn print_table_row<'a>(values: impl Iterator<Item = &'a str>, widths: &[usize]) {
    let values = values.collect::<Vec<_>>();
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            print!("  ");
        }
        let value = values.get(index).copied().unwrap_or("");
        print!("{value:<width$}");
    }
    println!();
}

fn print_table_separator(widths: &[usize]) {
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            print!("  ");
        }
        print!("{}", "-".repeat(*width));
    }
    println!();
}

fn local_timestamp(timestamp: &str) -> String {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|timestamp| timestamp.with_timezone(&Local).to_rfc3339())
        .unwrap_or_else(|_| timestamp.to_string())
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

    let response = post_json(base_url, "/agents/register", prepared.envelope.clone())?;

    print_registration_review(&prepared);
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
        "list" => policy_list(base_url, args),
        "new-template" => new_policy_template(args),
        "validate" => validate_policy_file(args),
        "-h" | "--help" | "help" => {
            print_policy_help();
            Ok(())
        }
        _ => bail!("unknown policy command `{command}`"),
    }
}

fn validate_policy_file(mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_policy_validate_help();
        return Ok(());
    }

    let path = take_required(&mut args, "--path")?;
    ensure_no_args(args)?;

    let policy =
        Policy::from_yaml_file(&path).with_context(|| format!("validate policy file `{path}`"))?;
    println!("Policy valid");
    println!("  path: {path}");
    println!("  policy_id: {}", policy.id);
    println!("  policy_version: {}", policy.version);
    println!(
        "  default_decision: {}",
        policy_effect_name(policy.default_effect)
    );
    println!("  rules: {}", policy.rules.len());
    Ok(())
}

fn new_policy_template(mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_policy_new_template_help();
        return Ok(());
    }

    let policy_path =
        take_value(&mut args, "--path").unwrap_or_else(|| DEFAULT_POLICY_TEMPLATE_PATH.to_string());
    let force = take_flag(&mut args, "--force");
    ensure_no_args(args)?;

    write_policy_template(&policy_path, force)
}

fn write_policy_template(policy_path: &str, force: bool) -> Result<()> {
    let path = Path::new(policy_path);
    if path.exists() && !force {
        bail!("policy file `{policy_path}` already exists; pass --force to overwrite");
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create policy directory `{}`", parent.display()))?;
    }

    fs::write(path, default_policy_template())
        .with_context(|| format!("write default policy template to `{policy_path}`"))?;
    println!("Hubu policy template created");
    println!("  path: {policy_path}");
    println!("  next: edit the file, then run hubu policy add --path {policy_path}");
    Ok(())
}

fn policy_effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::Allow => "allow",
        Effect::Deny => "deny",
        Effect::NeedsApproval => "needs_approval",
    }
}

fn add_policy(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_policy_add_help();
        return Ok(());
    }

    let agent_id = take_value(&mut args, "--agent-id");
    let path = take_required(&mut args, "--path")?;
    ensure_no_args(args)?;
    Policy::from_yaml_file(&path).with_context(|| format!("validate policy file `{path}`"))?;

    let mut body = json!({
        "policy_yaml": fs::read_to_string(&path)
            .with_context(|| format!("read policy file `{path}`"))?,
    });
    if let Some(agent_id) = agent_id {
        body["agent_id"] = json!(agent_id);
    }

    let response = post_json(base_url, "/policies", body)?;

    println!("Policy added");
    println!("  scope: {}", string_at(&response, "scope")?);
    if let Some(agent_id) = response.get("agent_id").and_then(Value::as_str) {
        println!("  agent_id: {agent_id}");
    }
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

fn policy_list(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_policy_list_help();
        return Ok(());
    }
    ensure_no_args(args)?;

    let response = get_json(base_url, "/policies")?;
    let policies = response
        .get("policies")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `policies`"))?;

    if policies.is_empty() {
        println!("No policies attached.");
        return Ok(());
    }

    let rows = policies
        .iter()
        .map(|policy| {
            Ok(vec![
                string_at(policy, "scope")?.to_string(),
                policy
                    .get("agent_id")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                string_at(policy, "policy_id")?.to_string(),
                string_at(policy, "policy_version")?.to_string(),
                string_at(policy, "default_decision")?.to_string(),
                policy
                    .get("rules")
                    .and_then(Value::as_u64)
                    .map(|rules| rules.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                local_timestamp(string_at(policy, "attached_at")?),
                local_timestamp(string_at(policy, "updated_at")?),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    print_table(
        &[
            "SCOPE",
            "AGENT ID",
            "POLICY ID",
            "VERSION",
            "DEFAULT",
            "RULES",
            "ATTACHED AT",
            "UPDATED AT",
        ],
        &rows,
    );
    Ok(())
}

fn agent(base_url: &str, args: Vec<String>) -> Result<()> {
    match args.split_first() {
        None => {
            print_agent_help();
            Ok(())
        }
        Some((command, [])) if command == "help" || command == "-h" || command == "--help" => {
            print_agent_help();
            Ok(())
        }
        Some((command, rest)) if command == "list" => agent_list(base_url, rest.to_vec()),
        _ => bail!("usage: hubu agent list [--all]"),
    }
}

fn agent_list(base_url: &str, mut args: Vec<String>) -> Result<()> {
    let include_all = take_flag(&mut args, "--all");
    ensure_no_args(args)?;
    let path = if include_all {
        "/agents?all=true"
    } else {
        "/agents"
    };
    let response = get_json(base_url, path)?;
    let agents = response
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `agents`"))?;

    if agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }

    let rows = agents
        .iter()
        .map(|agent| {
            Ok(vec![
                string_at(agent, "agent_id")?.to_string(),
                string_at(agent, "display_name")?.to_string(),
                string_at(agent, "owner_user_id")?.to_string(),
                agent
                    .get("owner_username")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                string_at(agent, "account_id")?.to_string(),
                string_at(agent, "status")?.to_string(),
                local_timestamp(string_at(agent, "created_at")?),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    print_table(
        &[
            "AGENT ID",
            "NAME",
            "OWNER USER ID",
            "OWNER USERNAME",
            "ACCOUNT ID",
            "STATUS",
            "CREATED AT",
        ],
        &rows,
    );
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
        "replace" => budget_replace(base_url, args),
        "revoke" => budget_revoke(base_url, args),
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
    let agent_id = take_required(&mut args, "--agent-id").with_context(|| {
        "budget create requires --agent-id; use `hubu user cap set` for a user-level max spend cap"
    })?;
    let starting_at = take_value(&mut args, "--starting-at");
    let ending_before = take_value(&mut args, "--ending-before");
    ensure_no_args(args)?;

    let mut body = json!({
        "amount_cents": amount_to_cents(&amount)?,
        "starting_at": starting_at,
        "ending_before": ending_before,
    });
    body["agent_id"] = json!(agent_id);

    let response = post_json(base_url, "/budgets", body)?;

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
    let agent_id = take_required(&mut args, "--agent-id")
        .with_context(|| "budget create-recurring requires --agent-id; use `hubu user cap set` for a user-level max spend cap")?;
    let recurrence = take_required(&mut args, "--recurrence")?;
    let period_count = take_required(&mut args, "--period-count")?;
    let starting_at = take_value(&mut args, "--starting-at");
    ensure_no_args(args)?;

    let mut body = json!({
        "amount_cents": amount_to_cents(&amount)?,
        "starting_at": starting_at,
        "recurrence": recurrence,
        "period_count": period_count.parse::<usize>()?,
    });
    body["agent_id"] = json!(agent_id);

    let response = post_json(base_url, "/budgets/series", body)?;

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
    let include_all = take_flag(&mut args, "--all");
    ensure_no_args(args)?;
    let path = if include_all {
        "/budgets?all=true"
    } else {
        "/budgets"
    };
    let response = get_json(base_url, path)?;
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

fn budget_revoke(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_revoke_help();
        return Ok(());
    }

    let budget_id = take_required(&mut args, "--budget-id")?;
    ensure_no_args(args)?;
    let response = post_json(
        base_url,
        "/budgets/revoke",
        json!({
            "budget_id": budget_id,
        }),
    )?;

    println!("Budget revoked");
    print_budget(
        response
            .get("budget")
            .ok_or_else(|| anyhow!("server response missing `budget`"))?,
    )?;
    Ok(())
}

fn budget_replace(base_url: &str, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_replace_help();
        return Ok(());
    }

    let budget_id = take_required(&mut args, "--budget-id")?;
    let amount = take_required(&mut args, "--amount")?;
    ensure_no_args(args)?;
    let response = post_json(
        base_url,
        "/budgets/replace",
        json!({
            "budget_id": budget_id,
            "amount_cents": amount_to_cents(&amount)?,
        }),
    )?;

    println!("Budget replaced");
    println!("Revoked budget");
    print_budget(
        response
            .get("revoked_budget")
            .ok_or_else(|| anyhow!("server response missing `revoked_budget`"))?,
    )?;
    println!("Replacement budget");
    print_budget(
        response
            .get("budget")
            .ok_or_else(|| anyhow!("server response missing `budget`"))?,
    )?;
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
        take_value(&mut args, "--merchant").unwrap_or_else(|| "local-merchant".to_string());
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
        take_value(&mut args, "--merchant").unwrap_or_else(|| "local-merchant".to_string());
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
    if let Some(hold) = response.get("cap_hold").filter(|hold| hold.is_object()) {
        println!("Cap hold");
        println!("  status: {}", string_at(hold, "status")?);
        println!("  hold_id: {}", string_at(hold, "hold_id")?);
        println!("  cap_id: {}", string_at(hold, "budget_id")?);
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
        "  budget_id: {}  scope: {} ({})  status: {}",
        string_at(budget, "budget_id")?,
        string_at(budget, "scope")?,
        string_at(budget, "scope_id")?,
        string_at(budget, "status")?
    );
    println!(
        "    limit: {}  consumed: {}  frozen: {}  remaining: {}",
        money_at(budget, "amount_limit_cents")?,
        money_at(budget, "consumed_amount_cents")?,
        money_at(budget, "frozen_amount_cents")?,
        money_at(budget, "remaining_amount_cents")?
    );
    let starting_at = local_timestamp(string_at(budget, "starting_at")?);
    let ending_before = budget
        .get("ending_before")
        .and_then(Value::as_str)
        .map(local_timestamp)
        .unwrap_or_else(|| "open-ended".to_string());
    println!("    period: {starting_at} -> {ending_before}");
    Ok(())
}

fn print_user_cap(cap: &Value) -> Result<()> {
    println!(
        "  cap_id: {}  status: {}",
        string_at(cap, "cap_id")?,
        string_at(cap, "status")?
    );
    println!(
        "    limit: {}  consumed: {}  frozen: {}  remaining: {}",
        money_at(cap, "amount_limit_cents")?,
        money_at(cap, "consumed_amount_cents")?,
        money_at(cap, "frozen_amount_cents")?,
        money_at(cap, "remaining_amount_cents")?
    );
    let starting_at = local_timestamp(string_at(cap, "starting_at")?);
    let ending_before = cap
        .get("ending_before")
        .and_then(Value::as_str)
        .map(local_timestamp)
        .unwrap_or_else(|| "open-ended".to_string());
    println!("    period: {starting_at} -> {ending_before}");
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
    DEFAULT_POLICY_TEMPLATE
}

fn print_help() {
    println!(
        "Hubu CLI

Usage:
  hubu [--url URL] <command>

Commands:
  register   Register human users and agents
  protocol   Read Hubu protocol payloads
  user       List human users and manage user caps
  policy     Manage spending policies
  init       Generate starter files and configure clients
  agent      Read registered agents
  budget     Create and list agent budgets
  spend      Test the agent spend path
  ledger     Read ledger transactions
  health     Check the Hubu server

Global options:
  --url URL   Hubu server URL (default: http://127.0.0.1:8787)

Examples:
  hubu init codex --token-file ~/.hubu/hubu.auth-token
  hubu register human --username alice-example --display-name \"Alice Example\"
  hubu register agent --name local-agent --version local-dev
  hubu policy new-template --path policies/policy.yaml
  hubu user cap set --amount 100
  hubu budget create --agent-id AGENT_ID --amount 25
  hubu spend --help

Run `hubu <command> --help` for command-specific help."
    );
}

fn print_protocol_help() {
    println!(
        "Read Hubu protocol payloads

Usage:
  hubu protocol agent-registration"
    );
}

fn print_register_help() {
    println!(
        "Register human users and agents

Usage:
  hubu register human --username USERNAME --display-name NAME [--email EMAIL]
  hubu register agent [--name NAME] [--version VERSION] [--dry-run]

Examples:
  hubu register human --username alice-example --display-name \"Alice Example\" --email alice@example.com
  hubu register agent
  hubu register agent --name local-agent --version local-dev
  hubu register agent --dry-run"
    );
}

fn print_register_human_help() {
    println!(
        "Register the active human user

Usage:
  hubu register human --username USERNAME --display-name NAME [--email EMAIL]

Options:
  --username USERNAME    Stable handle: 3-32 lowercase letters, digits, or hyphens
  --display-name NAME    Human-readable display name
  --email EMAIL          Optional email address

Example:
  hubu register human --username alice-example --display-name \"Alice Example\" --email alice@example.com"
    );
}

fn print_user_help() {
    println!(
        "Manage human users

Usage:
  hubu user list
  hubu user cap set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
  hubu user cap show [--all]
  hubu user cap revoke --cap-id ID"
    );
}

fn print_user_cap_help() {
    println!(
        "Manage the current user's global spend cap

Usage:
  hubu user cap set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
  hubu user cap show [--all]
  hubu user cap revoke --cap-id ID"
    );
}

fn print_user_cap_set_help() {
    println!(
        "Set a global spend cap for the current user

Usage:
  hubu user cap set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]"
    );
}

fn print_user_cap_show_help() {
    println!(
        "Show global spend caps for the current user

Usage:
  hubu user cap show [--all]

Options:
  --all  Include revoked caps"
    );
}

fn print_user_cap_revoke_help() {
    println!(
        "Revoke a global spend cap for the current user

Usage:
  hubu user cap revoke --cap-id ID"
    );
}

fn print_register_agent_help() {
    println!(
        "Register an agent for the current human user

Usage:
  hubu register agent [--name NAME] [--version VERSION] [--dry-run]

Options:
  --name NAME      Agent name (default: guidance vendor/workspace template)
  --version VALUE  Version label (default: current git short SHA, or dev)
  --dry-run        Print the computed registration envelope without submitting it

Examples:
  hubu register agent
  hubu register agent --name local-agent --version local-dev
  hubu register agent --dry-run"
    );
}

fn print_policy_help() {
    println!(
        "Manage spending policies

Usage:
  hubu policy new-template [--path FILE] [--force]
  hubu policy validate --path FILE
  hubu policy add --path FILE
  hubu policy list

Examples:
  hubu policy new-template --path policies/policy.yaml
  hubu policy validate --path policies/policy.yaml
  hubu policy add --path policies/policy.yaml
  hubu policy list"
    );
}

fn print_policy_new_template_help() {
    println!(
        "Create an editable policy template

Usage:
  hubu policy new-template [--path FILE] [--force]

Options:
  --path FILE  Policy template path (default: policies/policy.yaml)
  --force      Overwrite an existing policy file"
    );
}

fn print_policy_validate_help() {
    println!(
        "Validate a policy file

Usage:
  hubu policy validate --path FILE

Options:
  --path FILE  YAML policy file to validate"
    );
}

fn print_policy_add_help() {
    println!(
        "Attach a spending policy to the current user

Usage:
  hubu policy add --path FILE

Options:
  --path FILE  YAML policy file generated by `hubu policy new-template` or written by hand"
    );
}

fn print_policy_list_help() {
    println!(
        "List policies attached to the current user

Usage:
  hubu policy list"
    );
}

fn print_init_help() {
    println!(
        "Generate local Hubu starter files and client config

Usage:
  hubu init [--policy FILE] [--force]
  hubu init codex [--config FILE] [--mcp-server FILE] [--token-file FILE] [--force] [--dry-run]

Options:
  --policy FILE   Policy template path (default: policy.yaml)
  --force         Overwrite an existing policy file

Note:
  Prefer `hubu policy new-template` for new policy files.
  Use `hubu init codex` to expose Hubu MCP tools to Codex across projects.

Examples:
  hubu init --policy policies/policy.yaml
  hubu init codex --token-file ~/.hubu/hubu.auth-token"
    );
}

fn print_init_codex_help() {
    println!(
        "Configure Codex to discover Hubu MCP tools

Usage:
  hubu init codex [--config FILE] [--mcp-server FILE] [--token-file FILE] [--force] [--dry-run] [--trust-client-approval]

Options:
  --config FILE             Codex config path (default: $CODEX_HOME/config.toml or ~/.codex/config.toml)
  --mcp-server FILE         hubu-mcp-server executable (default: sibling of hubu, then PATH)
  --token-file FILE         Hubu auth token file (default: $HUBU_AUTH_TOKEN_FILE, ./hubu.auth-token, or ~/.hubu/hubu.auth-token)
  --force                  Replace an existing unmanaged [mcp_servers.hubu] config block
  --dry-run                Print the managed Codex config block without writing files
  --trust-client-approval  Enable MCP setup/admin tools when the Codex client prompts for destructive tool approval

Notes:
  Hubu spend tools are pre-approved in Codex; Hubu policy still controls needs_approval outcomes.
  Keep --trust-client-approval off for normal agent spend workflows.
  Use --trust-client-approval only when you want to ask Codex to perform setup/admin actions behind a human approval prompt.
  Start hubu-server with the same HUBU_AUTH_TOKEN_FILE shown by this command.

Examples:
  hubu init codex --token-file ~/.hubu/hubu.auth-token
  hubu init codex --trust-client-approval
  hubu init codex --dry-run"
    );
}

fn print_agent_help() {
    println!(
        "Read registered agents

Usage:
  hubu agent list [--all]

Options:
  --all  Show agents for all local users instead of only the current user

Examples:
  hubu agent list
  hubu agent list --all"
    );
}

fn print_budget_help() {
    println!(
        "Create and list agent budgets

Usage:
  hubu budget create --amount AMOUNT --agent-id ID [--starting-at RFC3339] [--ending-before RFC3339]
  hubu budget create-recurring --amount AMOUNT --agent-id ID --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]
  hubu budget revoke --budget-id ID
  hubu budget replace --budget-id ID --amount AMOUNT
  hubu budget list [--all]

Examples:
  hubu budget create --agent-id AGENT_ID --amount 25
  hubu budget create-recurring --agent-id AGENT_ID --amount 25 --recurrence monthly --period-count 3
  hubu budget replace --budget-id BUDGET_ID --amount 50
  hubu budget revoke --budget-id BUDGET_ID
  hubu budget list"
    );
}

fn print_budget_create_help() {
    println!(
        "Create a single agent budget

Usage:
  hubu budget create --amount AMOUNT --agent-id ID [--starting-at RFC3339] [--ending-before RFC3339]

Options:
  --agent-id ID  Agent this budget applies to

Examples:
  hubu budget create --agent-id AGENT_ID --amount 25"
    );
}

fn print_budget_create_recurring_help() {
    println!(
        "Create a recurring agent budget series

Usage:
  hubu budget create-recurring --amount AMOUNT --agent-id ID --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]

Options:
  --agent-id ID  Agent this budget series applies to

Examples:
  hubu budget create-recurring --agent-id AGENT_ID --amount 25 --recurrence monthly --period-count 3"
    );
}

fn print_budget_list_help() {
    println!(
        "List caps and budgets for the active human user

Usage:
  hubu budget list [--all]

Options:
  --all  Include revoked budgets

Examples:
  hubu budget list
  hubu budget list --all"
    );
}

fn print_budget_revoke_help() {
    println!(
        "Revoke an active budget so it cannot reserve future spend

Usage:
  hubu budget revoke --budget-id ID"
    );
}

fn print_budget_replace_help() {
    println!(
        "Replace an active budget with a new forward-looking allowance

Usage:
  hubu budget replace --budget-id ID --amount AMOUNT"
    );
}

fn print_spend_help() {
    println!(
        "Test an agent spend request

Usage:
  hubu spend --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend authorize --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend authorize --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]

Note:
  CLI spend commands are for local testing and debugging. Operational spend should normally originate from agents through MCP.

Examples:
  hubu spend authorize --agent-id AGENT_ID --amount 5 --reason \"Reserve model API credits\"
  hubu spend --agent-id AGENT_ID --amount 20 --reason \"Purchase API credits\""
    );
}

fn print_spend_authorize_help() {
    println!(
        "Authorize spend and reserve budget without executing payment

Usage:
  hubu spend authorize --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
  hubu spend authorize --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]

Example:
  hubu spend authorize --agent-id AGENT_ID --amount 5 --reason \"Reserve model API credits\""
    );
}

fn print_ledger_help() {
    println!(
        "Read ledger transactions

Usage:
  hubu ledger list

Example:
  hubu ledger list"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_mcp_block_escapes_toml_strings() {
        let block = codex_mcp_config_block(
            Path::new("/tmp/hubu \"dev\"/hubu-mcp-server"),
            "http://127.0.0.1:8787",
            Path::new("/tmp/hubu\\token"),
            false,
        );

        assert!(block.contains(HUBU_CODEX_MCP_BEGIN));
        assert!(block.contains("command = \"/tmp/hubu \\\"dev\\\"/hubu-mcp-server\""));
        assert!(block.contains("HUBU_AUTH_TOKEN_FILE = \"/tmp/hubu\\\\token\""));
        assert!(block.contains("[mcp_servers.hubu.tools.hubu_authorize_spend]"));
        assert!(block.contains("[mcp_servers.hubu.tools.hubu_submit_spend]"));
        assert!(block.contains("approval_mode = \"approve\""));
        assert!(!block.contains("HUBU_MCP_TRUST_CLIENT_APPROVAL"));
        assert!(block.contains(HUBU_CODEX_MCP_END));
    }

    #[test]
    fn codex_mcp_block_can_enable_trusted_client_approval() {
        let block = codex_mcp_config_block(
            Path::new("/tmp/hubu-mcp-server"),
            "http://127.0.0.1:8787",
            Path::new("/tmp/hubu.auth-token"),
            true,
        );

        assert!(block.contains("HUBU_MCP_TRUST_CLIENT_APPROVAL = \"1\""));
        let env_index = block.find("[mcp_servers.hubu.env]").unwrap();
        let trust_index = block.find("HUBU_MCP_TRUST_CLIENT_APPROVAL").unwrap();
        let tool_index = block
            .find("[mcp_servers.hubu.tools.hubu_authorize_spend]")
            .unwrap();
        assert!(env_index < trust_index);
        assert!(trust_index < tool_index);
    }

    #[test]
    fn upsert_managed_block_appends_without_touching_existing_config() {
        let existing = "model = \"gpt-5.5\"\n";
        let block = "# >>> hubu managed codex mcp\n[mcp_servers.hubu]\ncommand = \"/tmp/hubu-mcp-server\"\n# <<< hubu managed codex mcp\n";

        let updated = upsert_managed_codex_mcp_block(existing, block, false).unwrap();

        assert!(updated.starts_with(existing));
        assert!(updated.contains("[mcp_servers.hubu]"));
        assert!(updated.ends_with('\n'));
    }

    #[test]
    fn upsert_managed_block_replaces_prior_managed_block() {
        let existing = "model = \"gpt-5.5\"\n\n# >>> hubu managed codex mcp\n[mcp_servers.hubu]\ncommand = \"old\"\n# <<< hubu managed codex mcp\n\nsandbox_mode = \"workspace-write\"\n";
        let block = "# >>> hubu managed codex mcp\n[mcp_servers.hubu]\ncommand = \"new\"\n# <<< hubu managed codex mcp\n";

        let updated = upsert_managed_codex_mcp_block(existing, block, false).unwrap();

        assert!(updated.contains("command = \"new\""));
        assert!(!updated.contains("command = \"old\""));
        assert!(updated.contains("sandbox_mode = \"workspace-write\""));
    }

    #[test]
    fn upsert_rejects_unmanaged_hubu_config_without_force() {
        let existing = "[mcp_servers.hubu]\ncommand = \"custom\"\n";
        let block = "# >>> hubu managed codex mcp\n[mcp_servers.hubu]\ncommand = \"new\"\n# <<< hubu managed codex mcp\n";

        let error = upsert_managed_codex_mcp_block(existing, block, false).unwrap_err();

        assert!(error.to_string().contains("unmanaged [mcp_servers.hubu]"));
    }

    #[test]
    fn upsert_force_replaces_unmanaged_hubu_tables_only() {
        let existing = "[mcp_servers.other]\ncommand = \"keep\"\n\n[mcp_servers.hubu] # old Hubu config\ncommand = \"old\"\n\n[mcp_servers.hubu.env]\nHUBU_URL = \"old\"\n\n[features]\nhooks = true\n";
        let block = "# >>> hubu managed codex mcp\n[mcp_servers.hubu]\ncommand = \"new\"\n# <<< hubu managed codex mcp\n";

        let updated = upsert_managed_codex_mcp_block(existing, block, true).unwrap();

        assert!(updated.contains("[mcp_servers.other]"));
        assert!(updated.contains("[features]"));
        assert!(updated.contains("command = \"new\""));
        assert!(!updated.contains("command = \"old\""));
        assert!(!updated.contains("HUBU_URL = \"old\""));
    }
}
