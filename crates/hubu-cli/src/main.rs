use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    io::{IsTerminal, Read, Write},
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Local};
use hubu_common::build::build_info;
use hubu_common::money::{Currency, DecimalMajorAmount};
use hubu_core::policy::{Effect, Policy};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod codex_mcp;
mod stack;
mod terminal;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";
const APPROVAL_TOKEN_ENV: &str = "HUBU_APPROVAL_TOKEN";
const APPROVAL_TOKEN_FILE_ENV: &str = "HUBU_APPROVAL_TOKEN_FILE";
const DEFAULT_APPROVAL_TOKEN_FILE: &str = "hubu.approval-token";
const APPROVAL_CAPABILITY_HEADER: &str = "X-Hubu-Approval-Capability";
const RECONCILIATION_TOKEN_ENV: &str = "HUBU_RECONCILIATION_TOKEN";
const RECONCILIATION_TOKEN_FILE_ENV: &str = "HUBU_RECONCILIATION_TOKEN_FILE";
const DEFAULT_RECONCILIATION_TOKEN_FILE: &str = "hubu.reconciliation-token";
const RECONCILIATION_CAPABILITY_HEADER: &str = "X-Hubu-Reconciliation-Capability";
const FINGERPRINT_PREFIX: &str = "sha256:";
const DEFAULT_POLICY_TEMPLATE_PATH: &str = "policies/policy.yaml";
const DEFAULT_POLICY_TEMPLATE: &str = include_str!("../../../policies/starter-policy.yaml");
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const HUBU_HOME_ENV: &str = "HUBU_HOME";
const HUBU_UNIFIED_MCP_SERVER_ENV: &str = "HUBU_UNIFIED_MCP_SERVER";
const HUBU_UNIFIED_OPERATION_STATE_PATH_ENV: &str = "HUBU_UNIFIED_OPERATION_STATE_PATH";
const DEFAULT_UNIFIED_OPERATION_STATE_FILE: &str = "hubu-unified-operations.sqlite3";
#[cfg(test)]
const TEST_APPROVAL_TOKEN: &str = "test-human-approval-token";

#[derive(Debug, Default, PartialEq, Eq)]
struct GlobalOptions {
    base_url: Option<String>,
    color: terminal::ColorChoice,
}

#[derive(Debug)]
struct CliContext {
    explicit_base_url: Option<String>,
    legacy_base_url: String,
    hubu_home: PathBuf,
    target: OnceLock<ClientTarget>,
}

#[derive(Debug)]
struct ClientTarget {
    base_url: String,
    credentials: CredentialSources,
}

#[derive(Debug)]
struct HttpApplicationError {
    status: u16,
    body: Value,
}

impl std::fmt::Display for HttpApplicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = self
            .body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        write!(formatter, "server returned HTTP {}", self.status)?;
        if let Some(error_code) = self.body.get("error_code").and_then(Value::as_str) {
            write!(formatter, " ({error_code})")?;
        }
        write!(formatter, ": {message}")?;
        if let Some(details) = self.body.get("details") {
            write!(formatter, "; details={details}")?;
        }
        if let Some(retry_guidance) = self.body.get("retry_guidance") {
            write!(formatter, "; retry_guidance={retry_guidance}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HttpApplicationError {}

#[derive(Debug)]
enum CredentialSources {
    ActiveProfile {
        auth: PathBuf,
        approval: PathBuf,
        reconciliation: PathBuf,
    },
    Legacy,
}

impl CliContext {
    fn new(explicit_base_url: Option<String>, hubu_home: PathBuf) -> Self {
        let legacy_base_url = explicit_base_url
            .clone()
            .or_else(|| env::var("HUBU_URL").ok())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            explicit_base_url,
            legacy_base_url,
            hubu_home,
            target: OnceLock::new(),
        }
    }

    fn legacy_base_url(&self) -> &str {
        &self.legacy_base_url
    }

    fn has_explicit_base_url(&self) -> bool {
        self.explicit_base_url.is_some()
    }

    fn target(&self) -> Result<&ClientTarget> {
        if let Some(target) = self.target.get() {
            return Ok(target);
        }
        let target = self.resolve_target()?;
        let _ = self.target.set(target);
        Ok(self.target.get().expect("CLI target was initialized"))
    }

    fn resolve_target(&self) -> Result<ClientTarget> {
        if let Some(base_url) = &self.explicit_base_url {
            return Ok(ClientTarget {
                base_url: base_url.clone(),
                credentials: CredentialSources::Legacy,
            });
        }
        if let Some(handoff) = stack::active_client_handoff(&self.hubu_home)? {
            return Ok(ClientTarget {
                base_url: handoff.hubu_endpoint,
                credentials: CredentialSources::ActiveProfile {
                    auth: handoff.hubu_token_file,
                    approval: handoff.approval_token_file,
                    reconciliation: handoff.reconciliation_token_file,
                },
            });
        }
        Ok(ClientTarget {
            base_url: self.legacy_base_url.clone(),
            credentials: CredentialSources::Legacy,
        })
    }
}

impl GlobalOptions {
    fn color_choice(args: &[String]) -> Result<terminal::ColorChoice> {
        let mut color = None;
        let mut index = 0;
        while index < args.len() {
            if args[index] != "--color" {
                index += 1;
                continue;
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with('-'))
                .ok_or_else(|| anyhow!("missing value for global option `--color`"))?;
            let parsed = value
                .parse()
                .map_err(|reason| anyhow!("invalid value `{value}` for --color: {reason}"))?;
            if color.replace(parsed).is_some() {
                bail!("global option `--color` may only be provided once");
            }
            index += 2;
        }
        Ok(color.unwrap_or_default())
    }

    fn parse(args: &mut Vec<String>) -> Result<Self> {
        let color = Self::color_choice(args)?;
        let mut options = Self {
            base_url: None,
            color,
        };
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--url" => {
                    let value = take_global_option(args, index, "--url")?;
                    if options.base_url.replace(value).is_some() {
                        bail!("global option `--url` may only be provided once");
                    }
                }
                "--color" => {
                    take_global_option(args, index, "--color")?;
                }
                _ => index += 1,
            }
        }
        Ok(options)
    }
}

fn take_global_option(args: &mut Vec<String>, index: usize, name: &str) -> Result<String> {
    args.remove(index);
    if index >= args.len() || args[index].starts_with('-') {
        bail!("missing value for global option `{name}`");
    }
    Ok(args.remove(index))
}

fn main() {
    if let Err(error) = run() {
        let output = terminal::stderr();
        eprintln!("{}: {error:#}", output.error("error"));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    terminal::configure(GlobalOptions::color_choice(&args)?);
    let global = GlobalOptions::parse(&mut args)?;
    let hubu_home = hubu_home();
    let client = CliContext::new(global.base_url, hubu_home.clone());

    let Some(command) = args.first().cloned() else {
        print_help();
        return Ok(());
    };
    args.remove(0);

    match command.as_str() {
        "init" => init(
            client.legacy_base_url(),
            client.has_explicit_base_url(),
            args,
        ),
        "stack" => stack::command(args, &hubu_home),
        "register" => register(&client, args),
        "protocol" => protocol(&client, args),
        "user" => user(&client, args),
        "policy" => policy(&client, args),
        "agent" => agent(&client, args),
        "budget" => budget(&client, args),
        "spend" => spend(&client, args),
        "ledger" => ledger(&client, args),
        "health" => health(&client),
        "version" | "--version" | "-V" => version(),
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        _ => bail!("unknown command `{command}`"),
    }
}

fn init(base_url: &str, explicit_base_url: bool, mut args: Vec<String>) -> Result<()> {
    if args.first().map(String::as_str) == Some("codex") {
        args.remove(0);
        return init_codex(base_url, explicit_base_url, args);
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

fn init_codex(base_url: &str, explicit_base_url: bool, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_init_codex_help();
        return Ok(());
    }

    let config_path = take_value(&mut args, "--config")
        .map(PathBuf::from)
        .unwrap_or_else(default_codex_config_path);
    let stack_profile = if args.iter().any(|value| value == "--stack-profile") {
        Some(PathBuf::from(
            take_value(&mut args, "--stack-profile")
                .ok_or_else(|| anyhow!("missing value for --stack-profile"))?,
        ))
    } else {
        None
    };
    let manual_mcp_server = take_value(&mut args, "--mcp-server").map(PathBuf::from);
    let manual_gongbu_endpoint = take_value(&mut args, "--gongbu-endpoint");
    let manual_gongbu_token_file = take_value(&mut args, "--gongbu-token-file").map(PathBuf::from);
    let manual_token_file = take_value(&mut args, "--token-file").map(PathBuf::from);
    let manual_approval_token_file =
        take_value(&mut args, "--approval-token-file").map(PathBuf::from);
    let manual_reconciliation_token_file =
        take_value(&mut args, "--reconciliation-token-file").map(PathBuf::from);
    let manual_operation_state_path = take_value(&mut args, "--mcp-state-file").map(PathBuf::from);
    let operation_key_db = take_value(&mut args, "--operation-key-db").map(PathBuf::from);
    if stack_profile.is_some()
        && (manual_mcp_server.is_some()
            || manual_gongbu_endpoint.is_some()
            || manual_gongbu_token_file.is_some()
            || manual_token_file.is_some()
            || manual_approval_token_file.is_some()
            || manual_reconciliation_token_file.is_some()
            || manual_operation_state_path.is_some()
            || explicit_base_url)
    {
        bail!(
            "--stack-profile cannot be combined with manual MCP, endpoint, or token-file options"
        );
    }
    let handoff = stack_profile
        .as_deref()
        .map(|profile| stack::codex_handoff(profile, &hubu_home()))
        .transpose()?;
    let mcp_server = handoff
        .as_ref()
        .map(|value| value.mcp_server.clone())
        .or(manual_mcp_server)
        .unwrap_or_else(default_mcp_server_path);
    let gongbu_endpoint = handoff
        .as_ref()
        .and_then(|value| value.gongbu_endpoint.clone())
        .or(manual_gongbu_endpoint);
    let gongbu_token_file = handoff
        .as_ref()
        .and_then(|value| value.gongbu_token_file.clone())
        .or(manual_gongbu_token_file);
    if gongbu_endpoint.is_some() != gongbu_token_file.is_some() {
        bail!("--gongbu-endpoint and --gongbu-token-file must be provided together");
    }
    let token_file = handoff
        .as_ref()
        .map(|value| value.hubu_token_file.clone())
        .or(manual_token_file)
        .unwrap_or_else(default_codex_token_file_path);
    let approval_token_file = handoff
        .as_ref()
        .map(|value| value.approval_token_file.clone())
        .or(manual_approval_token_file)
        .unwrap_or_else(|| default_codex_approval_token_file_path(&token_file));
    let reconciliation_token_file = handoff
        .as_ref()
        .map(|value| value.reconciliation_token_file.clone())
        .or(manual_reconciliation_token_file)
        .unwrap_or_else(|| default_codex_reconciliation_token_file_path(&token_file));
    let operation_state_path = handoff
        .as_ref()
        .map(|value| value.operation_state_path.clone())
        .or(manual_operation_state_path)
        .unwrap_or_else(|| default_unified_operation_state_path(&token_file));
    let force = take_flag(&mut args, "--force");
    let dry_run = take_flag(&mut args, "--dry-run");
    let trust_client_approval = take_flag(&mut args, "--trust-client-approval");
    ensure_no_args(args)?;

    let mcp_server = absolute_existing_file(&mcp_server, mcp_server_bin_name())
        .with_context(|| format!("resolve MCP server path `{}`", mcp_server.display()))?;

    let token_file = if dry_run {
        absolute_path(&token_file)?
    } else if handoff.is_some() {
        absolute_existing_file_path(&token_file)?
    } else {
        ensure_auth_token_file(&token_file)
            .with_context(|| format!("prepare Hubu auth token file `{}`", token_file.display()))?
    };
    let reconciliation_token_file = if dry_run {
        absolute_path(&reconciliation_token_file)?
    } else if handoff.is_some() {
        absolute_existing_file_path(&reconciliation_token_file)?
    } else {
        ensure_reconciliation_token_file(&reconciliation_token_file).with_context(|| {
            format!(
                "prepare Hubu reconciliation token file `{}`",
                reconciliation_token_file.display()
            )
        })?
    };
    let approval_token_file = if dry_run {
        absolute_path(&approval_token_file)?
    } else if handoff.is_some() {
        absolute_existing_file_path(&approval_token_file)?
    } else {
        ensure_approval_token_file(&approval_token_file).with_context(|| {
            format!(
                "prepare Hubu approval token file `{}`",
                approval_token_file.display()
            )
        })?
    };
    let operation_state_path = absolute_path(&operation_state_path)?;
    let operation_key_db = operation_key_db
        .map(|path| {
            if !path.is_absolute() {
                bail!("--operation-key-db requires an absolute private path");
            }
            Ok(path)
        })
        .transpose()
        .context("resolve preallocated operation-key database path")?;
    let gongbu_token_file = gongbu_token_file
        .as_deref()
        .map(|path| {
            if dry_run {
                absolute_path(path)
            } else {
                absolute_existing_file_path(path)
            }
        })
        .transpose()
        .context("resolve Gongbu token file")?;
    let hubu_endpoint = handoff
        .as_ref()
        .map(|value| value.hubu_endpoint.as_str())
        .unwrap_or(base_url);
    let block = codex_mcp::unified_block(codex_mcp::UnifiedConfig {
        mcp_server: &mcp_server,
        hubu_endpoint,
        hubu_token_file: &token_file,
        approval_token_file: &approval_token_file,
        reconciliation_token_file: &reconciliation_token_file,
        operation_state_path: &operation_state_path,
        operation_key_db: operation_key_db.as_deref(),
        gongbu: gongbu_endpoint.as_deref().zip(gongbu_token_file.as_deref()),
        trust_client_approval,
    });

    if dry_run {
        println!("{block}");
        return Ok(());
    }

    codex_mcp::write_config(&config_path, &block, force)
        .with_context(|| format!("update Codex config `{}`", config_path.display()))?;

    println!("Codex MCP configured for Hubu (unified)");
    println!("  config: {}", config_path.display());
    println!("  mcp_server: {}", mcp_server.display());
    println!("  hubu_url: {hubu_endpoint}");
    println!("  token_file: {}", token_file.display());
    println!("  approval_token_file: {}", approval_token_file.display());
    println!(
        "  reconciliation_token_file: {}",
        reconciliation_token_file.display()
    );
    println!("  mcp_state_file: {}", operation_state_path.display());
    if let Some(operation_key_db) = &operation_key_db {
        println!("  operation_key_db: {}", operation_key_db.display());
    }
    if let Some(profile) = &stack_profile {
        println!("  stack_profile: {}", profile.display());
        println!("  backends: keep the rendered profile's Hubu and Gongbu services running");
    }
    println!("  next: restart Codex, then use /mcp or ask Codex to list Hubu tools");
    if stack_profile.is_none() {
        println!(
            "  server: start hubu-server with {AUTH_TOKEN_FILE_ENV}={} {APPROVAL_TOKEN_FILE_ENV}={} {RECONCILIATION_TOKEN_FILE_ENV}={}",
            token_file.display(),
            approval_token_file.display(),
            reconciliation_token_file.display()
        );
    }
    println!(
        "  spend_tools: Codex pre-approves Hubu spend tool calls; Hubu still returns needs_approval without payment when policy requires review"
    );
    println!(
        "  spend_approval: enabled with a native Codex confirmation after the human chooses approve or deny in chat"
    );
    if trust_client_approval {
        println!("  setup_admin_tools: enabled because --trust-client-approval was set");
    } else {
        println!(
            "  setup_admin_tools: disabled; use the CLI for setup, policy, and budget changes"
        );
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

fn default_codex_reconciliation_token_file_path(auth_token_file: &Path) -> PathBuf {
    if let Ok(path) = env::var(RECONCILIATION_TOKEN_FILE_ENV) {
        return PathBuf::from(path);
    }
    auth_token_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_RECONCILIATION_TOKEN_FILE)
}

fn default_codex_approval_token_file_path(auth_token_file: &Path) -> PathBuf {
    if let Ok(path) = env::var(APPROVAL_TOKEN_FILE_ENV) {
        return PathBuf::from(path);
    }
    auth_token_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_APPROVAL_TOKEN_FILE)
}

fn default_unified_operation_state_path(auth_token_file: &Path) -> PathBuf {
    if let Some(path) = env::var_os(HUBU_UNIFIED_OPERATION_STATE_PATH_ENV) {
        return PathBuf::from(path);
    }
    auth_token_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_UNIFIED_OPERATION_STATE_FILE)
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
    if let Ok(path) = env::var(HUBU_UNIFIED_MCP_SERVER_ENV) {
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
    match cfg!(windows) {
        true => "hubu-unified-mcp.exe",
        false => "hubu-unified-mcp",
    }
}

fn find_on_path(bin_name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(bin_name))
        .find(|candidate| candidate.is_file())
}

fn absolute_existing_file(path: &Path, binary_name: &str) -> Result<PathBuf> {
    let path = absolute_path(path)?;
    if !path.is_file() {
        bail!(
            "`{}` is not a file; build or install {binary_name}, or pass --mcp-server PATH",
            path.display(),
        );
    }
    fs::canonicalize(&path).with_context(|| format!("canonicalize `{}`", path.display()))
}

fn absolute_existing_file_path(path: &Path) -> Result<PathBuf> {
    let path = absolute_path(path)?;
    if !path.is_file() {
        bail!("`{}` is not a file", path.display());
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
    ensure_token_file(path, AUTH_TOKEN_ENV, generate_local_auth_token)
}

fn ensure_reconciliation_token_file(path: &Path) -> Result<PathBuf> {
    ensure_token_file(
        path,
        RECONCILIATION_TOKEN_ENV,
        generate_local_reconciliation_token,
    )
}

fn ensure_approval_token_file(path: &Path) -> Result<PathBuf> {
    ensure_token_file(path, APPROVAL_TOKEN_ENV, generate_local_approval_token)
}

fn ensure_token_file(
    path: &Path,
    token_env: &str,
    generate_token: fn() -> String,
) -> Result<PathBuf> {
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

    let token = env::var(token_env)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(generate_token);
    fs::write(&path, format!("{token}\n"))
        .with_context(|| format!("write token file `{}`", path.display()))?;
    restrict_token_permissions(&path)?;
    fs::canonicalize(&path).with_context(|| format!("canonicalize token file `{}`", path.display()))
}

fn generate_local_auth_token() -> String {
    format!("hubu_{}", Uuid::new_v4().simple())
}

fn generate_local_reconciliation_token() -> String {
    format!("hubu_reconcile_{}", Uuid::new_v4().simple())
}

fn generate_local_approval_token() -> String {
    format!("hubu_approve_{}", Uuid::new_v4().simple())
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

fn protocol(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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

fn agent_registration_protocol(base_url: &CliContext) -> Result<()> {
    let response = get_json(base_url, "/registration/guidance")?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn register(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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

fn register_human(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
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

    println!("{}", terminal::stdout().success("Human registered"));
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

fn user(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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
        Some((command, rest)) if command == "spending-target" => {
            user_spending_target(base_url, rest.to_vec())
        }
        _ => bail!("usage: hubu user list | hubu user spending-target set|show|revoke"),
    }
}

fn user_spending_target(base_url: &CliContext, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_user_spending_target_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "set" => user_spending_target_set(base_url, args),
        "show" => user_spending_target_show(base_url, args),
        "revoke" => user_spending_target_revoke(base_url, args),
        "-h" | "--help" | "help" => {
            print_user_spending_target_help();
            Ok(())
        }
        _ => bail!("unknown user spending-target command `{command}`"),
    }
}

fn user_spending_target_set(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_user_spending_target_set_help();
        return Ok(());
    }

    let amount = take_required(&mut args, "--amount")?;
    let starting_at = take_value(&mut args, "--starting-at");
    let ending_before = take_value(&mut args, "--ending-before");
    ensure_no_args(args)?;

    let response = post_json(
        base_url,
        "/user/spending-target",
        json!({
            "amount_cents": amount_to_cents(&amount)?,
            "starting_at": starting_at,
            "ending_before": ending_before,
        }),
    )?;

    println!(
        "{}",
        terminal::stdout().success("Spending target set (advisory)")
    );
    print_spending_target(
        response
            .get("target")
            .ok_or_else(|| anyhow!("server response missing `target`"))?,
    )?;
    Ok(())
}

fn user_spending_target_show(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_user_spending_target_show_help();
        return Ok(());
    }

    let include_all = take_flag(&mut args, "--all");
    ensure_no_args(args)?;
    let path = if include_all {
        "/user/spending-target?all=true"
    } else {
        "/user/spending-target"
    };
    let response = get_json(base_url, path)?;
    let targets = response
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `targets`"))?;

    if targets.is_empty() {
        println!(
            "{}",
            terminal::stdout().muted("No spending targets configured.")
        );
        return Ok(());
    }

    for target in targets {
        print_spending_target(target)?;
    }
    Ok(())
}

fn user_spending_target_revoke(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_user_spending_target_revoke_help();
        return Ok(());
    }

    let target_id = take_required(&mut args, "--target-id")?;
    ensure_no_args(args)?;
    let response = post_json(
        base_url,
        "/user/spending-target/revoke",
        json!({
            "target_id": target_id,
        }),
    )?;

    println!("{}", terminal::stdout().warning("Spending target revoked"));
    print_spending_target(
        response
            .get("target")
            .ok_or_else(|| anyhow!("server response missing `target`"))?,
    )?;
    Ok(())
}

fn user_list(base_url: &CliContext) -> Result<()> {
    let response = get_json(base_url, "/users")?;
    let users = response
        .get("users")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `users`"))?;

    if users.is_empty() {
        println!("{}", terminal::stdout().muted("No human users registered."));
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
    print!("{}", render_table(headers, rows, terminal::stdout()));
}

fn render_table(headers: &[&str], rows: &[Vec<String>], style: terminal::TerminalStyle) -> String {
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

    let mut output = String::new();
    write_table_header(&mut output, headers.iter().copied(), &widths, style);
    write_table_separator(&mut output, &widths, style);
    for row in rows {
        write_table_row(&mut output, row.iter().map(String::as_str), &widths);
    }
    output
}

fn write_table_header<'a>(
    output: &mut String,
    values: impl Iterator<Item = &'a str>,
    widths: &[usize],
    style: terminal::TerminalStyle,
) {
    let values = values.collect::<Vec<_>>();
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            write!(output, "  ").unwrap();
        }
        let value = values.get(index).copied().unwrap_or("");
        write!(output, "{}", style.heading(format!("{value:<width$}"))).unwrap();
    }
    writeln!(output).unwrap();
}

fn write_table_row<'a>(
    output: &mut String,
    values: impl Iterator<Item = &'a str>,
    widths: &[usize],
) {
    let values = values.collect::<Vec<_>>();
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            write!(output, "  ").unwrap();
        }
        let value = values.get(index).copied().unwrap_or("");
        write!(output, "{value:<width$}").unwrap();
    }
    writeln!(output).unwrap();
}

fn write_table_separator(output: &mut String, widths: &[usize], style: terminal::TerminalStyle) {
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            write!(output, "  ").unwrap();
        }
        write!(output, "{}", style.muted("-".repeat(*width))).unwrap();
    }
    writeln!(output).unwrap();
}

fn local_timestamp(timestamp: &str) -> String {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|timestamp| timestamp.with_timezone(&Local).to_rfc3339())
        .unwrap_or_else(|_| timestamp.to_string())
}

fn register_agent(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
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
    println!("{}", terminal::stdout().success("Agent registered"));
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
    base_url: &CliContext,
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
    let style = terminal::stdout();
    println!("{}", style.heading("Registration review"));
    for (label, value) in &prepared.review {
        println!("  {}: {value}", style.label(label));
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

fn policy(base_url: &CliContext, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_policy_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);

    match command.as_str() {
        "add" => apply_policy(base_url, args, true),
        "apply" => apply_policy(base_url, args, false),
        "show" => show_policy(base_url, args, false),
        "export" => show_policy(base_url, args, true),
        "history" => policy_history(base_url, args),
        "diff" => policy_diff(base_url, args),
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
    println!("{}", terminal::stdout().success("Policy valid"));
    println!("  path: {path}");
    println!("  policy_id: {}", policy.id);
    println!("  policy_version: {}", policy.version);
    let style = terminal::stdout();
    println!(
        "  default_decision: {}",
        style.semantic(policy_effect_name(policy.default_effect))
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
    println!(
        "{}",
        terminal::stdout().success("Hubu policy template created")
    );
    println!("  path: {policy_path}");
    println!("  next: edit the file, then run hubu policy apply --path {policy_path}");
    Ok(())
}

fn policy_effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::Allow => "allow",
        Effect::Deny => "deny",
        Effect::NeedsApproval => "needs_approval",
    }
}

fn apply_policy(base_url: &CliContext, mut args: Vec<String>, legacy_add: bool) -> Result<()> {
    if take_help(&mut args) {
        print_policy_add_help();
        return Ok(());
    }

    let agent_id = take_value(&mut args, "--agent-id");
    let display_name = take_value(&mut args, "--name");
    let declarative_key = take_value(&mut args, "--key");
    let expected_revision = take_value(&mut args, "--expected-revision")
        .map(|value| value.parse::<u64>().context("invalid --expected-revision"))
        .transpose()?;
    let expected_hash = take_value(&mut args, "--expected-hash");
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
    if let Some(display_name) = display_name {
        body["display_name"] = json!(display_name);
    }
    if let Some(declarative_key) = declarative_key {
        body["declarative_key"] = json!(declarative_key);
    }
    if let Some(expected_revision) = expected_revision {
        body["expected_revision"] = json!(expected_revision);
    }
    if let Some(expected_hash) = expected_hash {
        body["expected_hash"] = json!(expected_hash);
    }
    body["source"] = json!("cli");

    let response = post_json(base_url, "/policies", body)?;

    let style = terminal::stdout();
    let outcome = if legacy_add {
        "added"
    } else if response.get("changed").and_then(Value::as_bool) == Some(false) {
        "unchanged"
    } else {
        "applied"
    };
    println!(
        "Policy {}",
        if outcome == "unchanged" {
            style.muted(outcome)
        } else {
            style.success(outcome)
        }
    );
    println!("  scope: {}", string_at(&response, "scope")?);
    if let Some(agent_id) = response.get("agent_id").and_then(Value::as_str) {
        println!("  agent_id: {agent_id}");
    }
    println!("  policy_id: {}", string_at(&response, "policy_id")?);
    println!("  key: {}", string_at(&response, "declarative_key")?);
    println!("  name: {}", string_at(&response, "display_name")?);
    println!("  revision: {}", response["revision"]);
    println!("  payload_hash: {}", string_at(&response, "payload_hash")?);
    println!(
        "  policy_version: {}",
        string_at(&response, "policy_version")?
    );
    println!(
        "  default_decision: {}",
        style.semantic(string_at(&response, "default_decision")?)
    );
    Ok(())
}

fn policy_query(mut args: Vec<String>) -> Result<String> {
    let policy_id = take_value(&mut args, "--policy-id");
    let agent_id = take_value(&mut args, "--agent-id");
    ensure_no_args(args)?;
    match (policy_id, agent_id) {
        (Some(policy_id), None) => Ok(format!("policy_id={policy_id}")),
        (None, Some(agent_id)) => Ok(format!("agent_id={agent_id}")),
        (None, None) => Ok(String::new()),
        _ => bail!("pass only one of --policy-id or --agent-id"),
    }
}

fn show_policy(base_url: &CliContext, args: Vec<String>, export: bool) -> Result<()> {
    let query = policy_query(args)?;
    let path = format!(
        "/policies/{}{}{}",
        if export { "export" } else { "show" },
        if query.is_empty() { "" } else { "?" },
        query
    );
    let response = get_json(base_url, &path)?;
    if export {
        print!("{}", string_at(&response, "policy_yaml")?);
    } else {
        println!("{}", serde_json::to_string_pretty(&response)?);
    }
    Ok(())
}

fn policy_history(base_url: &CliContext, args: Vec<String>) -> Result<()> {
    let query = policy_query(args)?;
    let path = format!(
        "/policies/history{}{}",
        if query.is_empty() { "" } else { "?" },
        query
    );
    let response = get_json(base_url, &path)?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn policy_diff(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    let from_revision = take_required(&mut args, "--from-revision")?;
    let to_revision = take_value(&mut args, "--to-revision");
    let query = policy_query(args)?;
    let mut parameters = Vec::new();
    if !query.is_empty() {
        parameters.push(query);
    }
    parameters.push(format!("from_revision={from_revision}"));
    if let Some(to_revision) = to_revision {
        parameters.push(format!("to_revision={to_revision}"));
    }
    let response = get_json(
        base_url,
        &format!("/policies/diff?{}", parameters.join("&")),
    )?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn policy_list(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
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
        println!("{}", terminal::stdout().muted("No policies attached."));
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
                string_at(policy, "declarative_key")?.to_string(),
                string_at(policy, "display_name")?.to_string(),
                policy
                    .get("revision")
                    .and_then(Value::as_u64)
                    .map(|revision| revision.to_string())
                    .unwrap_or_else(|| "-".to_string()),
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
            "KEY",
            "NAME",
            "REV",
            "AUTHORED VERSION",
            "DEFAULT",
            "RULES",
            "ATTACHED AT",
            "UPDATED AT",
        ],
        &rows,
    );
    Ok(())
}

fn agent(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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

fn agent_list(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
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
        println!("{}", terminal::stdout().muted("No agents registered."));
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

fn budget(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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
        "update" => budget_update(base_url, args),
        "history" => budget_history(base_url, args),
        "revoke" => budget_revoke(base_url, args),
        "-h" | "--help" | "help" => {
            print_budget_help();
            Ok(())
        }
        _ => bail!("unknown budget command `{command}`"),
    }
}

fn budget_create(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_create_help();
        return Ok(());
    }

    let amount = take_required(&mut args, "--amount")?;
    let agent_id = take_required(&mut args, "--agent-id")
        .with_context(|| "budget create requires --agent-id")?;
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

    println!("{}", terminal::stdout().success("Budget created"));
    print_budget(
        response
            .get("budget")
            .ok_or_else(|| anyhow!("server response missing `budget`"))?,
    )?;
    print_spending_target_warnings(&response)?;
    Ok(())
}

fn budget_create_recurring(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_create_recurring_help();
        return Ok(());
    }

    let amount = take_required(&mut args, "--amount")?;
    let agent_id = take_required(&mut args, "--agent-id")
        .with_context(|| "budget create-recurring requires --agent-id")?;
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

    println!("{}", terminal::stdout().success("Budget series created"));
    for budget in response
        .get("budgets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `budgets`"))?
    {
        print_budget(budget)?;
    }
    print_spending_target_warnings(&response)?;
    Ok(())
}

fn budget_list(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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
        println!("{}", terminal::stdout().muted("No budgets configured."));
        return Ok(());
    }

    for budget in budgets {
        print_budget(budget)?;
    }
    Ok(())
}

fn budget_revoke(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
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

    println!("{}", terminal::stdout().warning("Budget revoked"));
    print_budget(
        response
            .get("budget")
            .ok_or_else(|| anyhow!("server response missing `budget`"))?,
    )?;
    Ok(())
}

fn budget_update(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_update_help();
        return Ok(());
    }

    let budget_id = take_required(&mut args, "--budget-id")?;
    validate_public_budget_id(&budget_id)?;
    let amount = take_required(&mut args, "--amount")?;
    let amount_limit_cents = amount_to_cents(&amount)?;
    if amount_limit_cents <= 0 {
        bail!("budget total limit must be greater than zero");
    }
    let expected_revision = take_value(&mut args, "--expected-revision")
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| "--expected-revision must be a positive integer")
        })
        .transpose()?;
    if expected_revision == Some(0) {
        bail!("--expected-revision must be at least 1");
    }
    let reason = take_value(&mut args, "--reason");
    let confirmed = take_flag(&mut args, "--yes");
    ensure_no_args(args)?;

    let path = budget_versions_path(&budget_id)?;
    let history = get_json(base_url, &path)?;
    let current = history
        .get("current_budget")
        .ok_or_else(|| anyhow!("server response missing `current_budget`"))?;
    if string_at(current, "budget_id")? != budget_id {
        bail!("server returned a different logical budget than requested");
    }
    let current_revision = u64_at(current, "current_revision")?;
    let pinned_revision = expected_revision.unwrap_or(current_revision);
    let current_total = i64_at(current, "amount_limit_cents")?;
    let consumed = i64_at(current, "consumed_amount_cents")?;
    let frozen = i64_at(current, "frozen_amount_cents")?;
    let projected_remaining = amount_limit_cents
        .checked_sub(consumed)
        .and_then(|remaining| remaining.checked_sub(frozen))
        .ok_or_else(|| anyhow!("projected remaining amount exceeds the supported range"))?;

    print_budget_update_review(
        &budget_id,
        pinned_revision,
        current_revision,
        current_total,
        consumed,
        frozen,
        amount_limit_cents,
        projected_remaining,
        reason.as_deref(),
    );

    if pinned_revision == current_revision {
        let status = string_at(current, "status")?;
        if status == "revoked" {
            bail!("current budget is revoked and cannot be updated");
        }
        if status == "expired" {
            bail!("current budget is expired and cannot be updated");
        }
        if projected_remaining < 0 {
            let committed = consumed
                .checked_add(frozen)
                .ok_or_else(|| anyhow!("committed budget usage exceeds the supported range"))?;
            bail!(
                "proposed total {} is below committed usage {}",
                format_cents(amount_limit_cents, "$"),
                format_cents(committed, "$")
            );
        }
    }

    if !confirmed {
        confirm_budget_update()?;
    }

    let mut body = json!({
        "amount_limit_cents": amount_limit_cents,
        "expected_revision": pinned_revision,
    });
    if let Some(reason) = &reason {
        body["reason"] = json!(reason);
    }
    let response = post_budget_update_with_retry(base_url, &path, &budget_id, &body)?;
    let replay = response
        .get("idempotent_replay")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("server response missing `idempotent_replay`"))?;

    let outcome = if replay {
        "Budget update replayed"
    } else {
        "Budget update applied"
    };
    println!("{}", terminal::stdout().success(outcome));
    println!(
        "{}",
        terminal::stdout().heading("Applied immutable version")
    );
    print_budget_version(
        response
            .get("applied_version")
            .ok_or_else(|| anyhow!("server response missing `applied_version`"))?,
        None,
    )?;
    println!("  idempotent_replay: {replay}");
    println!(
        "{}",
        terminal::stdout().heading("Authoritative current budget")
    );
    print_budget(
        response
            .get("current_budget")
            .ok_or_else(|| anyhow!("server response missing `current_budget`"))?,
    )?;
    print_spending_target_warnings(&response)?;
    Ok(())
}

fn budget_history(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_budget_history_help();
        return Ok(());
    }

    let budget_id = take_required(&mut args, "--budget-id")?;
    validate_public_budget_id(&budget_id)?;
    ensure_no_args(args)?;
    let response = get_json(base_url, &budget_versions_path(&budget_id)?)?;
    let current = response
        .get("current_budget")
        .ok_or_else(|| anyhow!("server response missing `current_budget`"))?;
    if string_at(current, "budget_id")? != budget_id {
        bail!("server returned a different logical budget than requested");
    }
    let current_version_id = string_at(current, "current_version_id")?;
    let current_revision = u64_at(current, "current_revision")?;

    println!("{}", terminal::stdout().heading("Current logical budget"));
    print_budget(current)?;
    println!(
        "{}",
        terminal::stdout().heading("Immutable version history")
    );
    let versions = response
        .get("versions")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `versions`"))?;
    if versions.is_empty() {
        bail!("server returned an empty budget version history");
    }
    let mut previous_revision = 0;
    let mut current_markers = 0;
    for version in versions {
        let revision = u64_at(version, "revision")?;
        if revision <= previous_revision {
            bail!("server returned budget versions outside ascending revision order");
        }
        previous_revision = revision;
        let marker = (string_at(version, "version_id")? == current_version_id).then(|| {
            current_markers += 1;
            "current"
        });
        print_budget_version(version, marker)?;
    }
    if previous_revision != current_revision || current_markers != 1 {
        bail!("server returned a budget history that does not contain exactly one current head");
    }
    Ok(())
}

fn spend(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_spend_help();
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("authorize") {
        args.remove(0);
        return spend_authorize(base_url, args);
    }
    if args.first().map(String::as_str) == Some("claim") {
        args.remove(0);
        return spend_claim_status(base_url, args);
    }
    if args.first().map(String::as_str) == Some("approval") {
        args.remove(0);
        return spend_approval(base_url, args);
    }
    if args.first().map(String::as_str) == Some("reconcile") {
        args.remove(0);
        return spend_reconcile(base_url, args);
    }

    let account_id = take_value(&mut args, "--account-id");
    let agent_id = take_value(&mut args, "--agent-id");
    let account_id = require_spend_account_id("hubu spend", account_id, agent_id)?;
    let operation_key = take_required(&mut args, "--operation-key")?;
    let amount = take_required(&mut args, "--amount")?;
    let currency_arg = take_value(&mut args, "--currency");
    let currency = normalize_currency(currency_arg.as_deref().unwrap_or("usd"))?;
    let reason = take_required(&mut args, "--reason")?;
    let task_id = take_value(&mut args, "--task-id");
    let (merchant, execution_scope) = take_execution_scope(&mut args)?;
    let lease_profile = take_value(&mut args, "--lease-profile");
    ensure_no_args(args)?;

    let amount_cents = amount_to_cents(&amount)?;
    print_spend_submission(
        amount_cents,
        currency,
        currency_arg.is_some(),
        merchant.as_deref(),
        execution_scope.as_ref(),
    );

    let mut body = json!({
        "operation_key": operation_key,
        "amount_cents": amount_cents,
        "currency": currency_arg.map(|_| currency.to_string()),
        "reason": reason,
        "merchant": merchant,
        "execution_scope": execution_scope,
        "lease_profile": lease_profile,
    });
    body["account_id"] = json!(account_id);
    if let Some(task_id) = task_id {
        body["task_id"] = json!(task_id);
    }

    let response = post_json(base_url, "/spend", body)?;
    print_spend_response(&response)
}

fn spend_authorize(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_spend_authorize_help();
        return Ok(());
    }

    let account_id = take_value(&mut args, "--account-id");
    let agent_id = take_value(&mut args, "--agent-id");
    let account_id = require_spend_account_id("hubu spend authorize", account_id, agent_id)?;
    let operation_key = take_required(&mut args, "--operation-key")?;
    let amount = take_required(&mut args, "--amount")?;
    let currency_arg = take_value(&mut args, "--currency");
    let currency = normalize_currency(currency_arg.as_deref().unwrap_or("usd"))?;
    let reason = take_required(&mut args, "--reason")?;
    let task_id = take_value(&mut args, "--task-id");
    let (merchant, execution_scope) = take_execution_scope(&mut args)?;
    let lease_profile = take_value(&mut args, "--lease-profile");
    ensure_no_args(args)?;

    let amount_cents = amount_to_cents(&amount)?;
    print_spend_submission(
        amount_cents,
        currency,
        currency_arg.is_some(),
        merchant.as_deref(),
        execution_scope.as_ref(),
    );

    let mut body = json!({
        "operation_key": operation_key,
        "amount_cents": amount_cents,
        "currency": currency_arg.map(|_| currency.to_string()),
        "reason": reason,
        "merchant": merchant,
        "execution_scope": execution_scope,
        "lease_profile": lease_profile,
    });
    body["account_id"] = json!(account_id);
    if let Some(task_id) = task_id {
        body["task_id"] = json!(task_id);
    }

    let response = post_json(base_url, "/spend/authorize", body)?;
    print_spend_response(&response)
}

fn spend_claim_status(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    let claim_id = take_required(&mut args, "--claim-id")?;
    ensure_no_args(args)?;
    let response = get_json(
        base_url,
        &format!("/spend/executor/claim?claim_id={claim_id}"),
    )?;
    print_executor_claim(&response)
}

fn spend_approval(base_url: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        print_spend_approval_help();
        return Ok(());
    }
    let action = args
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("hubu spend approval requires get, approve, or deny"))?;
    args.remove(0);
    let approval_request_id = take_required(&mut args, "--approval-request-id")?;
    ensure_no_args(args)?;

    match action.as_str() {
        "get" => {
            let response = get_json(
                base_url,
                &format!("/spend/approval?approval_request_id={approval_request_id}"),
            )?;
            print_spend_approval_response(&response)
        }
        "approve" | "deny" => {
            let response = post_approval_json(
                base_url,
                "/spend/approval/resolve",
                json!({
                    "approval_request_id": approval_request_id,
                    "decision": action,
                }),
            )?;
            print_spend_response(&response)
        }
        _ => bail!("unknown spend approval action `{action}`; use get, approve, or deny"),
    }
}

fn take_execution_scope(args: &mut Vec<String>) -> Result<(Option<String>, Option<Value>)> {
    let merchant = take_value(args, "--merchant");
    let provider = take_value(args, "--provider");
    let executor = take_value(args, "--executor");
    let capability = take_value(args, "--capability");
    let billing_merchant = take_value(args, "--billing-merchant");
    let typed_count = [
        provider.as_ref(),
        executor.as_ref(),
        capability.as_ref(),
        billing_merchant.as_ref(),
    ]
    .into_iter()
    .flatten()
    .count();
    if merchant.is_some() && typed_count > 0 {
        return Err(anyhow!(
            "--merchant cannot be combined with typed execution-scope flags"
        ));
    }
    if typed_count == 0 {
        return Ok((merchant, None));
    }
    if typed_count != 4 {
        return Err(anyhow!("--provider, --executor, --capability, and --billing-merchant must be supplied together"));
    }
    Ok((
        None,
        Some(json!({
            "schema_version": 1,
            "provider": provider,
            "executor": executor,
            "capability": capability,
            "billing_merchant": billing_merchant,
        })),
    ))
}

fn normalize_currency(value: &str) -> Result<Currency> {
    value
        .trim()
        .to_ascii_lowercase()
        .parse()
        .map_err(Into::into)
}

fn print_spend_submission(
    amount_cents: i64,
    currency: Currency,
    currency_supplied: bool,
    merchant: Option<&str>,
    execution_scope: Option<&Value>,
) {
    println!("{}", terminal::stdout().heading("Spend request inputs"));
    println!(
        "  amount: {} {} major units (supplied; {} minor units)",
        currency.to_string().to_ascii_uppercase(),
        format_major_amount(amount_cents),
        amount_cents
    );
    println!(
        "  currency: {currency} ({}; no currency conversion is performed)",
        if currency_supplied {
            "supplied"
        } else {
            "inferred CLI default"
        }
    );
    if let Some(merchant) = merchant {
        println!("  merchant: {merchant} (supplied)");
        println!("  typed execution scope: inferred by the API from legacy merchant");
    } else if let Some(scope) = execution_scope.and_then(Value::as_object) {
        println!("  merchant: omitted (typed billing_merchant is evaluated instead)");
        for field in ["provider", "executor", "capability", "billing_merchant"] {
            let value = scope
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or("<missing>");
            println!("  {field}: {value} (supplied)");
        }
    } else {
        println!("  merchant: omitted (merchant policy conditions cannot match)");
        println!("  provider: omitted");
        println!("  executor: omitted");
        println!("  capability: omitted");
        println!("  billing_merchant: omitted");
    }
}

fn format_major_amount(amount_cents: i64) -> String {
    format_cents(amount_cents, "")
}

fn format_cents(amount_cents: i64, unit_prefix: &str) -> String {
    let sign = if amount_cents < 0 { "-" } else { "" };
    let magnitude = amount_cents.unsigned_abs();
    format!(
        "{sign}{unit_prefix}{}.{:02}",
        magnitude / 100,
        magnitude % 100
    )
}

fn spend_reconcile(base_url: &CliContext, args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().cloned() else {
        print_spend_reconcile_help();
        return Ok(());
    };
    let mut args = args;
    args.remove(0);
    match command.as_str() {
        "list" => spend_reconcile_list(base_url, args),
        "billed" => spend_reconcile_resolve(base_url, args, true),
        "not-billed" => spend_reconcile_resolve(base_url, args, false),
        "-h" | "--help" | "help" => {
            print_spend_reconcile_help();
            Ok(())
        }
        _ => bail!("unknown spend reconcile command `{command}`"),
    }
}

fn spend_reconcile_list(base_url: &CliContext, args: Vec<String>) -> Result<()> {
    ensure_no_args(args)?;
    let response = get_json(base_url, "/spend/executor/reconciliation")?;
    let claims = response
        .get("claims")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `claims`"))?;
    if claims.is_empty() {
        println!(
            "{}",
            terminal::stdout().muted("No executor claims require reconciliation.")
        );
        return Ok(());
    }
    for claim in claims {
        print_executor_claim(claim)?;
    }
    Ok(())
}

fn spend_reconcile_resolve(
    base_url: &CliContext,
    mut args: Vec<String>,
    vendor_billed: bool,
) -> Result<()> {
    let claim_id = take_required(&mut args, "--claim-id")?;
    let provider_reference = take_required(&mut args, "--provider-reference")?;
    let evidence = take_required(&mut args, "--evidence")?;
    let receipt = if vendor_billed {
        let provider_request_id = take_required(&mut args, "--provider-request-id")?;
        let artifact_reference = take_required(&mut args, "--artifact-reference")?;
        if let Some(amount) = take_value(&mut args, "--actual-vendor-cost-amount") {
            let amount = amount
                .parse::<i64>()
                .context("--actual-vendor-cost-amount must be an integer")?;
            let scale = take_required(&mut args, "--actual-vendor-cost-scale")?
                .parse::<u32>()
                .context("--actual-vendor-cost-scale must be a non-negative integer")?;
            let currency = take_value(&mut args, "--currency").unwrap_or_else(|| "usd".to_string());
            let price_model_snapshot: Value =
                serde_json::from_str(&take_required(&mut args, "--price-model-snapshot-json")?)
                    .context("--price-model-snapshot-json must be valid JSON")?;
            if !price_model_snapshot.is_object() {
                bail!("--price-model-snapshot-json must contain a JSON object");
            }
            Some(json!({
                "actual_vendor_cost": {
                    "amount": amount,
                    "scale": scale,
                    "currency": currency.to_ascii_lowercase(),
                },
                "provider_request_id": provider_request_id,
                "price_model_snapshot": price_model_snapshot,
                "artifact_reference": artifact_reference,
            }))
        } else {
            let actual_vendor_cost_cents = take_required(&mut args, "--actual-vendor-cost-cents")?
                .parse::<i64>()
                .context("--actual-vendor-cost-cents must be an integer")?;
            let provider = take_required(&mut args, "--provider")?;
            let model = take_required(&mut args, "--model")?;
            let unit_price_cents = take_required(&mut args, "--unit-price-cents")?
                .parse::<i64>()
                .context("--unit-price-cents must be an integer")?;
            let pricing_unit = take_required(&mut args, "--pricing-unit")?;
            Some(json!({
                "actual_vendor_cost_cents": actual_vendor_cost_cents,
                "provider_request_id": provider_request_id,
                "price_model_snapshot": {
                    "provider": provider,
                    "model": model,
                    "unit_price_cents": unit_price_cents,
                    "pricing_unit": pricing_unit,
                    "currency": "usd",
                },
                "artifact_reference": artifact_reference,
            }))
        }
    } else {
        None
    };
    ensure_no_args(args)?;
    let action = if vendor_billed { "settle" } else { "release" };
    let mut body = json!({
        "claim_id": claim_id,
        "provider_reference": provider_reference,
        "evidence": evidence,
    });
    if let Some(receipt) = receipt {
        body["receipt"] = receipt;
    }
    let response = post_reconciliation_json(base_url, &format!("/spend/executor/{action}"), body)?;
    let style = terminal::stdout();
    println!(
        "{}: {}",
        style.success("Claim reconciled"),
        style.semantic(if vendor_billed {
            "vendor billed; hold settled"
        } else {
            "vendor did not bill; hold released"
        })
    );
    print_executor_claim(&response)
}

fn print_executor_claim(claim: &Value) -> Result<()> {
    let style = terminal::stdout();
    println!("{}", style.heading("Executor claim"));
    println!("  claim_id: {}", string_at(claim, "claim_id")?);
    println!("  status: {}", style.semantic(string_at(claim, "status")?));
    let reconciliation_required = claim
        .get("reconciliation_required")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("server response missing `reconciliation_required`"))?;
    println!(
        "  reconciliation_required: {}",
        if reconciliation_required {
            style.warning("true")
        } else {
            style.success("false")
        }
    );
    println!("  operation_key: {}", string_at(claim, "operation_key")?);
    println!(
        "  claim_expires_at: {}",
        local_timestamp(string_at(claim, "claim_expires_at")?)
    );
    for field in [
        "settlement_id",
        "reconciliation_outcome",
        "provider_reference",
        "evidence",
        "reconciled_at",
        "reconciled_by_user_id",
    ] {
        if let Some(value) = claim.get(field).and_then(Value::as_str) {
            println!("  {field}: {value}");
        }
    }
    let spend = claim
        .get("spend")
        .ok_or_else(|| anyhow!("server response missing `spend`"))?;
    println!("  account_id: {}", string_at(spend, "account_id")?);
    println!("  agent_id: {}", string_at(spend, "agent_id")?);
    println!("  amount: {}", money_at(spend, "amount_cents")?);
    if let Some(merchant) = spend.get("merchant").and_then(Value::as_str) {
        println!("  merchant: {merchant}");
    }
    print_execution_scope(spend);
    let hold = spend
        .get("budget_hold")
        .ok_or_else(|| anyhow!("server response missing `budget_hold`"))?;
    println!(
        "  hold_status: {}",
        style.semantic(string_at(hold, "status")?)
    );
    println!(
        "  frozen_amount: {}",
        money_at(hold, "frozen_amount_cents")?
    );
    Ok(())
}

fn require_spend_account_id(
    command: &str,
    account_id: Option<String>,
    agent_id: Option<String>,
) -> Result<String> {
    if agent_id.is_some() {
        return Err(anyhow!(
            "{command} no longer accepts --agent-id; provide --account-id with the agent account id"
        ));
    }
    account_id.ok_or_else(|| anyhow!("{command} requires --account-id with the agent account id"))
}

fn print_spend_response(response: &Value) -> Result<()> {
    let style = terminal::stdout();
    println!("{}", style.heading("Spend evaluated"));
    println!("  operation_key: {}", string_at(response, "operation_key")?);
    println!("  account_id: {}", string_at(response, "account_id")?);
    println!("  agent_id: {}", string_at(response, "agent_id")?);
    println!(
        "  decision: {}",
        style.semantic(string_at(response, "decision")?)
    );
    println!("  decision_id: {}", string_at(response, "decision_id")?);
    if let Some(approval) = response.get("approval").filter(|value| value.is_object()) {
        print_spend_approval_response(approval)?;
    }
    if let Some(inputs) = response.get("scope_inputs").and_then(Value::as_object) {
        println!("  scope_inputs:");
        for field in [
            "amount_minor",
            "currency",
            "merchant",
            "provider",
            "executor",
            "capability",
            "billing_merchant",
        ] {
            let Some(input) = inputs.get(field).and_then(Value::as_object) else {
                continue;
            };
            let source = input
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let value = input
                .get("value")
                .filter(|value| !value.is_null())
                .map(Value::to_string)
                .unwrap_or_else(|| "<none>".to_string());
            println!("    {field}: {value} ({source})");
        }
    }
    if let Some(token_id) = response.get("auth_token_id").and_then(Value::as_str) {
        println!("  auth_token_id: {token_id}");
    }
    print_execution_scope(response);
    println!("  lease_profile: {}", string_at(response, "lease_profile")?);
    if let Some(expires_at) = response
        .get("authorization_expires_at")
        .and_then(Value::as_str)
    {
        println!(
            "  authorization_expires_at: {}",
            local_timestamp(expires_at)
        );
    }
    if let Some(reasons) = response.get("reasons").and_then(Value::as_array) {
        for reason in reasons {
            println!(
                "  reason: {}",
                reason.as_str().unwrap_or("<non-string reason>")
            );
        }
    }
    if let Some(policy_decision) = response.get("policy_decision") {
        if let Some(summary) = policy_decision.get("summary").and_then(Value::as_str) {
            println!("  policy_decision: {summary}");
        }
        if let Some(conditions) = policy_decision
            .get("decisive_conditions")
            .and_then(Value::as_array)
        {
            for condition in conditions {
                println!(
                    "  policy_condition: {} [{}; matched={}] {}",
                    condition
                        .get("rule_id")
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>"),
                    condition
                        .get("effect")
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>"),
                    condition
                        .get("matched")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    condition
                        .get("condition")
                        .and_then(Value::as_str)
                        .unwrap_or("<unavailable>")
                );
            }
        }
    }

    if let Some(payment) = response
        .get("payment")
        .filter(|payment| payment.is_object())
    {
        println!("{}", style.heading("Payment"));
        println!(
            "  status: {}",
            style.semantic(string_at(payment, "status")?)
        );
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
        println!("{}", style.heading("Budget hold"));
        println!("  status: {}", style.semantic(string_at(hold, "status")?));
        println!("  hold_id: {}", string_at(hold, "hold_id")?);
        println!("  budget_id: {}", string_at(hold, "budget_id")?);
        println!("  amount: {}", money_at(hold, "amount_cents")?);
        println!("  consumed: {}", money_at(hold, "consumed_amount_cents")?);
        println!("  frozen: {}", money_at(hold, "frozen_amount_cents")?);
        println!("  remaining: {}", money_at(hold, "remaining_amount_cents")?);
    }
    Ok(())
}

fn print_spend_approval_response(approval: &Value) -> Result<()> {
    let style = terminal::stdout();
    println!("{}", style.heading("Spend approval"));
    println!(
        "  approval_request_id: {}",
        string_at(approval, "approval_request_id")?
    );
    println!(
        "  status: {}",
        style.semantic(string_at(approval, "status")?)
    );
    let review = approval
        .get("review")
        .ok_or_else(|| anyhow!("server response missing `review`"))?;
    println!("  operation_key: {}", string_at(review, "operation_key")?);
    println!("  account_id: {}", string_at(review, "account_id")?);
    println!("  agent_id: {}", string_at(review, "agent_id")?);
    println!("  amount: {}", money_at(review, "amount_cents")?);
    println!("  lease_profile: {}", string_at(review, "lease_profile")?);
    println!("  reason: {}", string_at(review, "reason")?);
    println!("  policy_summary: {}", string_at(review, "policy_summary")?);
    if let Some(merchant) = review.get("merchant").and_then(Value::as_str) {
        println!("  merchant: {merchant}");
    }
    print_execution_scope(review);
    if let Some(reasons) = review.get("policy_reasons").and_then(Value::as_array) {
        for reason in reasons.iter().filter_map(Value::as_str) {
            println!("  policy_reason: {reason}");
        }
    }
    Ok(())
}

fn print_execution_scope(value: &Value) {
    let Some(scope) = value.get("execution_scope").and_then(Value::as_object) else {
        return;
    };
    println!(
        "  execution_scope_schema: {}",
        scope
            .get("schema_version")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    for field in ["provider", "executor", "capability", "billing_merchant"] {
        let Some(identity) = scope.get(field).and_then(Value::as_object) else {
            continue;
        };
        let name = identity
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");
        let id = identity
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<missing-id>");
        println!("  {field}: {name} [{id}]");
    }
}

fn ledger(base_url: &CliContext, args: Vec<String>) -> Result<()> {
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
                println!(
                    "{}",
                    terminal::stdout().muted("No ledger transactions recorded.")
                );
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
    let style = terminal::stdout();
    println!(
        "  budget_id: {}  agent_id: {}  status: {}",
        string_at(budget, "budget_id")?,
        string_at(budget, "agent_id")?,
        style.semantic(string_at(budget, "status")?)
    );
    println!(
        "    limit: {}  consumed: {}  frozen: {}  remaining: {}",
        money_at(budget, "amount_limit_cents")?,
        money_at(budget, "consumed_amount_cents")?,
        money_at(budget, "frozen_amount_cents")?,
        money_at(budget, "remaining_amount_cents")?
    );
    println!(
        "    current_version_id: {}  current_revision: {}",
        string_at(budget, "current_version_id")?,
        u64_at(budget, "current_revision")?
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

#[allow(clippy::too_many_arguments)]
fn print_budget_update_review(
    budget_id: &str,
    pinned_revision: u64,
    current_revision: u64,
    current_total: i64,
    consumed: i64,
    frozen: i64,
    proposed_total: i64,
    projected_remaining: i64,
    reason: Option<&str>,
) {
    println!("{}", terminal::stdout().heading("Budget update review"));
    println!("  budget_id: {budget_id}");
    println!("  pinned_revision: {pinned_revision}  current_revision: {current_revision}");
    println!(
        "  current_total: {}  consumed: {}  frozen: {}",
        format_cents(current_total, "$"),
        format_cents(consumed, "$"),
        format_cents(frozen, "$")
    );
    println!(
        "  proposed_total: {}  projected_remaining: {}",
        format_cents(proposed_total, "$"),
        format_cents(projected_remaining, "$")
    );
    println!(
        "  reason: {}",
        reason
            .map(terminal_safe_text)
            .unwrap_or_else(|| "(omitted)".to_string())
    );
}

fn confirm_budget_update() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("budget update requires interactive confirmation; rerun with --yes in noninteractive environments");
    }
    print!("Apply this budget update? [y/N] ");
    std::io::stdout().flush()?;
    let mut response = String::new();
    std::io::stdin().read_line(&mut response)?;
    if matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!("budget update cancelled")
    }
}

fn print_budget_version(version: &Value, marker: Option<&str>) -> Result<()> {
    let revision = u64_at(version, "revision")?;
    let version_id = string_at(version, "version_id")?;
    let marker = marker
        .map(|value| format!("  [{value}]"))
        .unwrap_or_default();
    println!("  revision: {revision}  version_id: {version_id}{marker}");
    let predecessor_revision = version
        .get("predecessor_revision")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let predecessor_version_id = version
        .get("predecessor_version_id")
        .and_then(Value::as_str)
        .unwrap_or("-");
    println!(
        "    predecessor_revision: {predecessor_revision}  predecessor_version_id: {predecessor_version_id}"
    );
    println!(
        "    total_limit: {}  effective_at: {}  created_at: {}",
        money_at(version, "amount_limit_cents")?,
        local_timestamp(string_at(version, "effective_at")?),
        local_timestamp(string_at(version, "created_at")?)
    );
    println!(
        "    actor: {}  source: {}  reason: {}",
        string_at(version, "actor")?,
        string_at(version, "source")?,
        version
            .get("reason")
            .and_then(Value::as_str)
            .map(terminal_safe_text)
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "    request_fingerprint: {}",
        string_at(version, "request_fingerprint")?
    );
    Ok(())
}

fn print_spending_target(target: &Value) -> Result<()> {
    let style = terminal::stdout();
    println!(
        "  target_id: {}  status: {}",
        string_at(target, "target_id")?,
        style.semantic(string_at(target, "status")?)
    );
    println!(
        "    target: {}  allocated: {}  exceeded by: {}",
        money_at(target, "target_amount_cents")?,
        money_at(target, "allocated_amount_cents")?,
        money_at(target, "exceeded_by_cents")?
    );
    let starting_at = local_timestamp(string_at(target, "starting_at")?);
    let ending_before = target
        .get("ending_before")
        .and_then(Value::as_str)
        .map(local_timestamp)
        .unwrap_or_else(|| "open-ended".to_string());
    println!("    period: {starting_at} -> {ending_before}");
    println!("    enforcement: advisory only");
    Ok(())
}

fn print_spending_target_warnings(response: &Value) -> Result<()> {
    let warnings = response
        .get("spending_target_warnings")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `spending_target_warnings`"))?;
    for warning in warnings {
        println!(
            "{}",
            terminal::stdout().warning("Spending target warning (advisory)")
        );
        println!("  target_id: {}", string_at(warning, "target_id")?);
        println!(
            "  target: {}  allocated: {}  exceeded by: {}",
            money_at(warning, "target_amount_cents")?,
            money_at(warning, "allocated_amount_cents")?,
            money_at(warning, "exceeded_by_cents")?
        );
        println!("  {}", string_at(warning, "message")?);
    }
    Ok(())
}

fn health(base_url: &CliContext) -> Result<()> {
    let response = get_json(base_url, "/health")?;
    let style = terminal::stdout();
    println!(
        "{}: {}",
        style.label("Hubu server"),
        style.semantic(string_at(&response, "status")?)
    );
    Ok(())
}

fn version() -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&build_info())?);
    Ok(())
}

fn request_json(
    client: &CliContext,
    method: &str,
    path: &str,
    body: Option<Value>,
    include_approval_capability: bool,
    include_reconciliation_capability: bool,
) -> Result<Value> {
    let target = client.target()?;
    let (host, port) = parse_base_url(&target.base_url)?;
    let body_text = body.map(|body| body.to_string()).unwrap_or_default();
    let authorization_header = target
        .auth_token()?
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let reconciliation_header = if include_reconciliation_capability {
        let token = target.reconciliation_token()?.ok_or_else(|| {
            anyhow!(
                "human reconciliation requires {RECONCILIATION_TOKEN_ENV} or {RECONCILIATION_TOKEN_FILE_ENV}"
            )
        })?;
        format!("{RECONCILIATION_CAPABILITY_HEADER}: {token}\r\n")
    } else {
        String::new()
    };
    let approval_header = if include_approval_capability {
        let token = target.approval_token()?.ok_or_else(|| {
            anyhow!("human approval requires {APPROVAL_TOKEN_ENV} or {APPROVAL_TOKEN_FILE_ENV}")
        })?;
        format!("{APPROVAL_CAPABILITY_HEADER}: {token}\r\n")
    } else {
        String::new()
    };
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to Hubu server at {}", target.base_url))?;

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{authorization_header}{approval_header}{reconciliation_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
        return Err(HttpApplicationError { status, body: json }.into());
    }

    Ok(json)
}

fn get_json(client: &CliContext, path: &str) -> Result<Value> {
    request_json(client, "GET", path, None, false, false)
}

fn post_json(client: &CliContext, path: &str, body: Value) -> Result<Value> {
    request_json(client, "POST", path, Some(body), false, false)
}

fn post_budget_update_with_retry(
    client: &CliContext,
    path: &str,
    budget_id: &str,
    body: &Value,
) -> Result<Value> {
    let send = || {
        let response = post_json(client, path, body.clone())?;
        validate_budget_update_response(&response)?;
        let applied = &response["applied_version"];
        let requested_reason = body
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty());
        let applied_reason = applied
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty());
        if string_at(&response["current_budget"], "budget_id")? != budget_id
            || applied.get("predecessor_revision").and_then(Value::as_u64)
                != Some(u64_at(body, "expected_revision")?)
            || i64_at(applied, "amount_limit_cents")? != i64_at(body, "amount_limit_cents")?
            || applied_reason != requested_reason
        {
            bail!("server returned a budget update that does not match the pinned intent");
        }
        Ok(response)
    };
    match send() {
        Ok(response) => Ok(response),
        Err(error) if !is_ambiguous_budget_update_error(&error) => Err(error),
        Err(first_error) => {
            eprintln!(
                "{}",
                terminal::stderr().warning(
                    "Budget update outcome was ambiguous; retrying once with the exact same intent."
                )
            );
            match send() {
                Ok(response) => Ok(response),
                Err(error) if !is_ambiguous_budget_update_error(&error) => Err(error),
                Err(second_error) => {
                    let amount_limit_cents = i64_at(body, "amount_limit_cents")?;
                    let expected_revision = u64_at(body, "expected_revision")?;
                    let reason = body
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_else(|| "<omitted>".to_string());
                    bail!(
                        "budget update outcome remains ambiguous after one exact retry: {second_error}. Retry manually without changing or omitting any field: budget_id={budget_id}, expected_revision={expected_revision}, amount_limit_cents={amount_limit_cents}, reason={reason}. First ambiguous error: {first_error}"
                    )
                }
            }
        }
    }
}

fn is_ambiguous_budget_update_error(error: &anyhow::Error) -> bool {
    match error.downcast_ref::<HttpApplicationError>() {
        None => true,
        Some(application) => {
            application.status == 500
                && application.body.get("error_code").and_then(Value::as_str)
                    == Some("budget_update_storage_error")
        }
    }
}

fn validate_budget_update_response(response: &Value) -> Result<()> {
    let applied = response
        .get("applied_version")
        .ok_or_else(|| anyhow!("server response missing `applied_version`"))?;
    let _ = string_at(applied, "version_id")?;
    let _ = u64_at(applied, "revision")?;
    let _ = i64_at(applied, "amount_limit_cents")?;
    let _ = string_at(applied, "effective_at")?;
    let _ = string_at(applied, "actor")?;
    let _ = string_at(applied, "source")?;
    let _ = string_at(applied, "request_fingerprint")?;
    let _ = string_at(applied, "created_at")?;
    for optional_string in ["predecessor_version_id", "reason"] {
        if applied
            .get(optional_string)
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            bail!("server response field `{optional_string}` has the wrong type");
        }
    }
    if applied
        .get("predecessor_revision")
        .is_none_or(|value| !value.is_null() && !value.is_u64())
    {
        bail!("server response field `predecessor_revision` has the wrong type");
    }

    let current = response
        .get("current_budget")
        .ok_or_else(|| anyhow!("server response missing `current_budget`"))?;
    for field in [
        "budget_id",
        "agent_id",
        "status",
        "current_version_id",
        "starting_at",
    ] {
        let _ = string_at(current, field)?;
    }
    for field in [
        "amount_limit_cents",
        "consumed_amount_cents",
        "frozen_amount_cents",
        "remaining_amount_cents",
    ] {
        let _ = i64_at(current, field)?;
    }
    let _ = u64_at(current, "current_revision")?;
    if current
        .get("ending_before")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        bail!("server response field `ending_before` has the wrong type");
    }
    if !response
        .get("idempotent_replay")
        .is_some_and(Value::is_boolean)
    {
        bail!("server response missing `idempotent_replay`");
    }
    let warnings = response
        .get("spending_target_warnings")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server response missing `spending_target_warnings`"))?;
    for warning in warnings {
        let _ = string_at(warning, "target_id")?;
        let _ = string_at(warning, "message")?;
        for field in [
            "target_amount_cents",
            "allocated_amount_cents",
            "exceeded_by_cents",
        ] {
            let _ = i64_at(warning, field)?;
        }
    }
    Ok(())
}

fn post_approval_json(client: &CliContext, path: &str, body: Value) -> Result<Value> {
    request_json(client, "POST", path, Some(body), true, false)
}

fn post_reconciliation_json(client: &CliContext, path: &str, body: Value) -> Result<Value> {
    request_json(client, "POST", path, Some(body), false, true)
}

impl ClientTarget {
    fn auth_token(&self) -> Result<Option<String>> {
        match &self.credentials {
            CredentialSources::ActiveProfile { auth, .. } => {
                read_active_profile_token(auth, "authentication").map(Some)
            }
            CredentialSources::Legacy => legacy_auth_token(),
        }
    }

    fn reconciliation_token(&self) -> Result<Option<String>> {
        match &self.credentials {
            CredentialSources::ActiveProfile { reconciliation, .. } => {
                read_active_profile_token(reconciliation, "reconciliation").map(Some)
            }
            CredentialSources::Legacy => legacy_reconciliation_token(),
        }
    }

    fn approval_token(&self) -> Result<Option<String>> {
        match &self.credentials {
            CredentialSources::ActiveProfile { approval, .. } => {
                read_active_profile_token(approval, "approval").map(Some)
            }
            CredentialSources::Legacy => legacy_approval_token(),
        }
    }
}

fn read_active_profile_token(path: &Path, capability: &str) -> Result<String> {
    let contents = fs::read_to_string(path).with_context(|| {
        format!(
            "active stack profile {capability} credential is unavailable; run `hubu stack start` for the active profile or pass an explicit `--url` for manual mode"
        )
    })?;
    let token = contents.trim().to_string();
    if token.is_empty() {
        bail!(
            "active stack profile {capability} credential is empty; run `hubu stack start` for the active profile or pass an explicit `--url` for manual mode"
        );
    }
    Ok(token)
}

fn legacy_auth_token() -> Result<Option<String>> {
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

fn legacy_reconciliation_token() -> Result<Option<String>> {
    if let Ok(token) = env::var(RECONCILIATION_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{RECONCILIATION_TOKEN_ENV} cannot be empty"));
        }
        return Ok(Some(token));
    }

    let path = env::var(RECONCILIATION_TOKEN_FILE_ENV)
        .unwrap_or_else(|_| DEFAULT_RECONCILIATION_TOKEN_FILE.to_string());
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Err(anyhow!("Hubu reconciliation token file `{path}` is empty"))
            } else {
                Ok(Some(token))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read Hubu reconciliation token file `{path}`"))
        }
    }
}

fn legacy_approval_token() -> Result<Option<String>> {
    #[cfg(test)]
    if env::var(APPROVAL_TOKEN_ENV).is_err() && env::var(APPROVAL_TOKEN_FILE_ENV).is_err() {
        return Ok(Some(TEST_APPROVAL_TOKEN.to_string()));
    }

    if let Ok(token) = env::var(APPROVAL_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{APPROVAL_TOKEN_ENV} cannot be empty"));
        }
        return Ok(Some(token));
    }

    let path = env::var(APPROVAL_TOKEN_FILE_ENV)
        .unwrap_or_else(|_| DEFAULT_APPROVAL_TOKEN_FILE.to_string());
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Err(anyhow!("Hubu approval token file `{path}` is empty"))
            } else {
                Ok(Some(token))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read Hubu approval token file `{path}`")),
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

fn validate_public_budget_id(budget_id: &str) -> Result<()> {
    let Some(suffix) = budget_id.strip_prefix("bgt_") else {
        bail!("budget id must use the public `bgt_` form");
    };
    const PUBLIC_SUFFIX_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";
    if suffix.len() != 12
        || !suffix
            .chars()
            .all(|character| PUBLIC_SUFFIX_ALPHABET.contains(character))
    {
        bail!("budget id must be a safe public `bgt_` identifier");
    }
    Ok(())
}

fn budget_versions_path(budget_id: &str) -> Result<String> {
    validate_public_budget_id(budget_id)?;
    Ok(format!("/budgets/{budget_id}/versions"))
}

fn terminal_safe_text(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization should not fail")
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

fn i64_at(value: &Value, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("server response missing `{key}`"))
}

fn u64_at(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("server response missing `{key}`"))
}

fn money_at(value: &Value, key: &str) -> Result<String> {
    let cents = value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("server response missing `{key}`"))?;
    Ok(format_cents(cents, "$"))
}

fn amount_to_cents(value: &str) -> Result<i64> {
    Ok(value.parse::<DecimalMajorAmount>()?.minor_units())
}

fn default_policy_template() -> &'static str {
    DEFAULT_POLICY_TEMPLATE
}

fn print_help() {
    println!(
        "Hubu CLI

Usage:
  hubu [--url URL] [--color auto|always|never] <command>

Commands:
  stack      Configure and operate a unified local stack profile
  register   Register human users and agents
  protocol   Read Hubu protocol payloads
  user       List human users and manage advisory spending targets
  policy     Manage spending policies
  init       Generate starter files and configure clients
  agent      Read registered agents
  budget     Create, review, update, and list agent budgets
  spend      Test spend and reconcile uncertain executor claims
  ledger     Read ledger transactions
  health     Check the Hubu server
  version    Print product, source, and executor-contract versions

Global options:
  --url URL                            Manual Hubu URL; bypasses the active stack profile
  --color auto|always|never            Terminal color policy (default: auto)

Connection precedence:
  explicit --url and legacy credential environment/files; otherwise the selected
  profile's active client handoff; otherwise the active default profile; otherwise
  HUBU_URL and legacy credential environment/files (default URL: http://127.0.0.1:8787)

Examples:
  hubu stack init
  hubu stack doctor --json
  hubu init codex --token-file ~/.hubu/hubu.auth-token
  hubu register human --username alice-example --display-name \"Alice Example\"
  hubu register agent --name local-agent --version local-dev
  hubu policy new-template --path policies/policy.yaml
  hubu user spending-target set --amount 100
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
  hubu user spending-target set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
  hubu user spending-target show [--all]
  hubu user spending-target revoke --target-id ID"
    );
}

fn print_user_spending_target_help() {
    println!(
        "Manage the current user's advisory spending target

Usage:
  hubu user spending-target set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
  hubu user spending-target show [--all]
  hubu user spending-target revoke --target-id ID

Spending targets warn when concurrent agent budget allocations exceed the target. They never block budget creation or spend."
    );
}

fn print_user_spending_target_set_help() {
    println!(
        "Set an advisory spending target for the current user

Usage:
  hubu user spending-target set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]"
    );
}

fn print_user_spending_target_show_help() {
    println!(
        "Show advisory spending targets for the current user

Usage:
  hubu user spending-target show [--all]

Options:
  --all  Include expired and revoked targets"
    );
}

fn print_user_spending_target_revoke_help() {
    println!(
        "Revoke an advisory spending target for the current user

Usage:
  hubu user spending-target revoke --target-id ID"
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
  hubu policy apply --path FILE [--key KEY] [--name NAME] [--agent-id AGENT_ID]
  hubu policy add --path FILE                 Compatibility alias for apply
  hubu policy list
  hubu policy show [--policy-id ID | --agent-id AGENT_ID]
  hubu policy export [--policy-id ID | --agent-id AGENT_ID]
  hubu policy history [--policy-id ID | --agent-id AGENT_ID]
  hubu policy diff --from-revision N [--to-revision N] [--policy-id ID | --agent-id AGENT_ID]

Examples:
  hubu policy new-template --path policies/policy.yaml
  hubu policy validate --path policies/policy.yaml
  hubu policy apply --path policies/policy.yaml
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
        "Declaratively apply and assign a spending policy to the current user

Usage:
  hubu policy apply --path FILE [--key KEY] [--name NAME] [--agent-id AGENT_ID]
      [--expected-revision N] [--expected-hash SHA256]
  hubu policy add --path FILE [OPTIONS]

Options:
  --path FILE            YAML policy file generated by `hubu policy new-template` or written by hand
  --key KEY              Immutable owner-scoped declarative key (defaults to authored id)
  --name NAME            Mutable display name (defaults to key)
  --agent-id AGENT_ID    Assign as an agent override instead of the user default
  --expected-revision N  Apply only if the current server revision matches
  --expected-hash HASH   Apply only if the current payload hash matches"
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
  hubu init codex [--stack-profile ABSOLUTE_DIR] [--config FILE] [--mcp-server FILE] [--token-file FILE] [--approval-token-file FILE] [--reconciliation-token-file FILE] [--mcp-state-file FILE] [--operation-key-db FILE] [--gongbu-endpoint URL --gongbu-token-file FILE] [--force] [--dry-run]

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
  hubu init codex [--stack-profile ABSOLUTE_DIR] [--config FILE] [--mcp-server FILE] [--token-file FILE] [--approval-token-file FILE] [--reconciliation-token-file FILE] [--mcp-state-file FILE] [--operation-key-db FILE] [--gongbu-endpoint URL --gongbu-token-file FILE] [--force] [--dry-run] [--trust-client-approval]

Options:
  --config FILE             Codex config path (default: $CODEX_HOME/config.toml or ~/.codex/config.toml)
  --stack-profile DIR       Read MCP binary, backend endpoints, and token files from an active rendered stack profile
  --mcp-server FILE         Selected MCP executable (default: hubu-unified-mcp sibling, then PATH)
  --token-file FILE         Hubu auth token file (default: $HUBU_AUTH_TOKEN_FILE, ./hubu.auth-token, or ~/.hubu/hubu.auth-token)
  --approval-token-file FILE
                             Separate human approval capability file (default: beside --token-file)
  --reconciliation-token-file FILE
                             Separate human reconciliation capability file (default: beside --token-file)
  --mcp-state-file FILE      Durable unified MCP operation registry (default: $HUBU_UNIFIED_OPERATION_STATE_PATH or beside --token-file)
  --operation-key-db FILE    Absolute private helper database for explicitly authorized, preallocated billable operation keys
  --gongbu-endpoint URL     Optional Gongbu backend URL for the unified entry
  --gongbu-token-file FILE  Gongbu bearer token file; required with --gongbu-endpoint
  --force                   Replace an existing unmanaged [mcp_servers.hubu] config block
  --dry-run                 Print the managed Codex config block without writing files
  --trust-client-approval   Enable MCP setup/admin tools when the Codex client prompts for destructive tool approval

Notes:
  Hubu spend tools are pre-approved in Codex; Hubu policy still controls needs_approval outcomes.
  Omit --operation-key-db until a guarded workflow requires the key-redacted preallocation bridge. The option configures only a non-secret path and never creates or reads a key.
  The command writes the only supported agent-facing surface, hubu-unified-mcp.
  Keep --trust-client-approval off for normal agent spend workflows.
  Use --trust-client-approval only when you want to ask Codex to perform setup/admin actions behind a human approval prompt.
  Start hubu-server with the same HUBU_AUTH_TOKEN_FILE, HUBU_APPROVAL_TOKEN_FILE, and HUBU_RECONCILIATION_TOKEN_FILE shown by this command.

Examples:
  hubu init codex --token-file ~/.hubu/hubu.auth-token
  hubu init codex --operation-key-db /absolute/private/operation-keys.sqlite3 --dry-run
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
        "Create, review, update, and list agent budgets

Usage:
  hubu budget create --amount AMOUNT --agent-id ID [--starting-at RFC3339] [--ending-before RFC3339]
  hubu budget create-recurring --amount AMOUNT --agent-id ID --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]
  hubu budget revoke --budget-id ID
  hubu budget update --budget-id ID --amount AMOUNT [--expected-revision N] [--reason TEXT] [--yes]
  hubu budget history --budget-id ID
  hubu budget list [--all]

Examples:
  hubu budget create --agent-id AGENT_ID --amount 25
  hubu budget create-recurring --agent-id AGENT_ID --amount 25 --recurrence monthly --period-count 3
  hubu budget update --budget-id BUDGET_ID --amount 50 --reason \"Raise total cap\"
  hubu budget history --budget-id BUDGET_ID
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
        "List scheduled, active, and exhausted agent budgets for the active human user

Usage:
  hubu budget list [--all]

Options:
  --all  Also include expired and revoked budgets

Examples:
  hubu budget list
  hubu budget list --all"
    );
}

fn print_budget_revoke_help() {
    println!(
        "Administratively revoke a budget so it cannot reserve future spend

Usage:
  hubu budget revoke --budget-id ID"
    );
}

fn print_budget_update_help() {
    println!(
        "Update the total cap of one stable logical budget

Usage:
  hubu budget update --budget-id ID --amount AMOUNT [--expected-revision N] [--reason TEXT] [--yes]

Options:
  --expected-revision N  Pin the immutable predecessor revision; defaults to the reviewed current revision
  --reason TEXT          Optional provenance recorded on the immutable successor
  --yes                  Skip the prompt after printing the required review (needed for noninteractive use)

The amount is the new total budget cap, not an added allowance. Hubu preserves consumed and frozen usage. After confirmation the CLI never refreshes or rebases the pinned intent. Create a new budget to change the agent, currency, or period."
    );
}

fn print_budget_history_help() {
    println!(
        "Show one logical budget and its immutable version history

Usage:
  hubu budget history --budget-id ID"
    );
}

fn print_spend_help() {
    println!(
        "Test an agent spend request

Usage:
  hubu spend --operation-key KEY --account-id ID --amount DECIMAL [--currency USD] --reason TEXT [--task-id ID] [--merchant NAME | --provider ID --executor ID --capability ID --billing-merchant ID] [--lease-profile NAME]
  hubu spend authorize --operation-key KEY --account-id ID --amount DECIMAL [--currency USD] --reason TEXT [--task-id ID] [--merchant NAME | --provider ID --executor ID --capability ID --billing-merchant ID] [--lease-profile NAME]
  hubu spend approval get --approval-request-id ID
  hubu spend approval approve --approval-request-id ID
  hubu spend approval deny --approval-request-id ID
  hubu spend claim --claim-id ID
  hubu spend reconcile list
  hubu spend reconcile billed --claim-id ID --provider-reference REF --evidence TEXT --actual-vendor-cost-amount INTEGER --actual-vendor-cost-scale SCALE [--currency USD] --price-model-snapshot-json JSON --provider-request-id ID --artifact-reference REF
  hubu spend reconcile not-billed --claim-id ID --provider-reference REF --evidence TEXT

Note:
  Spend commands require the agent account id because the account is the spending source. CLI spend commands are for local testing and debugging. Operational spend should normally originate from agents through MCP.
  --amount is a decimal major-unit amount: 5 means USD 5.00 and 0.05 means USD 0.05. USD is the only supported currency; Hubu performs no currency conversion.
  If merchant and typed execution scope are omitted, the CLI shows that omission before submission. Merchant policy conditions then cannot match and the policy may require approval.
  When authorization returns needs_approval, show the approval review to the human, then use exactly one approval approve or approval deny command with its approval_request_id.
  The client harness must supply one immutable agent-scoped operation key before the first request, then reuse it for authorization, claim, finalization, and every retry.
  --task-id is an optional external business correlation. It is independent of the operation key and descriptive --reason. Omitting it preserves the legacy reason-to-task mapping for retry compatibility.

Examples:
  hubu spend authorize --operation-key OPERATION_KEY --account-id ACCOUNT_ID --amount 5 --currency USD --merchant example-model-provider --reason \"Reserve model API credits\"
  hubu spend --operation-key OPERATION_KEY --account-id ACCOUNT_ID --amount 0.05 --currency USD --merchant example-model-provider --reason \"Purchase API credits\""
    );
}

fn print_spend_approval_help() {
    println!(
        "Resolve a pending spend request after explicit human review

Usage:
  hubu spend approval get --approval-request-id ID
  hubu spend approval approve --approval-request-id ID
  hubu spend approval deny --approval-request-id ID

The review is immutable. Approving reserves budget and returns an authorization token; it does not invoke a provider. Denial is terminal for that request. Repeating the same decision is idempotent, while a conflicting decision is rejected."
    );
}

fn print_spend_authorize_help() {
    println!(
        "Authorize spend and reserve budget without executing payment

Usage:
  hubu spend authorize --operation-key KEY --account-id ID --amount DECIMAL [--currency USD] --reason TEXT [--task-id ID] [--merchant NAME | --provider ID --executor ID --capability ID --billing-merchant ID] [--lease-profile NAME]

Note:
  Supply one immutable agent-scoped operation key before the first request; do not generate a new key on retry.
  --amount is a decimal major-unit amount: 5 means USD 5.00 and 0.05 means USD 0.05. USD is the only supported currency; Hubu performs no currency conversion.
  Omitted merchant/scope fields are shown before submission because policies that evaluate those fields will not match them.
  --task-id is an optional external business correlation. It is independent of the operation key and descriptive --reason. Omitting it preserves the legacy reason-to-task mapping for retry compatibility.

Example:
  hubu spend authorize --operation-key OPERATION_KEY --account-id ACCOUNT_ID --amount 5 --currency USD --merchant example-model-provider --reason \"Reserve model API credits\""
    );
}

fn print_spend_reconcile_help() {
    println!(
        "Resolve expired executor claims after a human reviews vendor billing

Usage:
  hubu spend reconcile list
  hubu spend reconcile billed --claim-id ID --provider-reference REF --evidence TEXT --actual-vendor-cost-amount INTEGER --actual-vendor-cost-scale SCALE [--currency USD] --price-model-snapshot-json JSON --provider-request-id ID --artifact-reference REF
  hubu spend reconcile billed --claim-id ID --provider-reference REF --evidence TEXT --actual-vendor-cost-cents CENTS --provider-request-id ID --provider NAME --model NAME --unit-price-cents CENTS --pricing-unit UNIT --artifact-reference REF
  hubu spend reconcile not-billed --claim-id ID --provider-reference REF --evidence TEXT

The provider reference and evidence are stored with the atomic settlement or release. A billed resolution also records the exact integer vendor-cost coefficient, decimal scale, ISO currency, full frozen pricing snapshot, provider request id, and artifact reference. Hubu conservatively rounds budget consumption up to cents; a human-confirmed overrun is recorded rather than discarded. The cents form remains available for v4.3 receipt compatibility. Do not include vendor credentials or sensitive payloads."
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

    fn strip_ansi(value: &str) -> String {
        let mut output = String::new();
        let mut chars = value.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '\u{1b}' && chars.next_if_eq(&'[').is_some() {
                for code in chars.by_ref() {
                    if code == 'm' {
                        break;
                    }
                }
            } else {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn global_options_are_removed_from_any_argument_position() {
        let mut args = [
            "stack",
            "status",
            "--color",
            "always",
            "--profile",
            "/tmp/demo",
            "--url",
            "http://127.0.0.1:9999",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();

        let options = GlobalOptions::parse(&mut args).unwrap();

        assert_eq!(options.color, terminal::ColorChoice::Always);
        assert_eq!(options.base_url.as_deref(), Some("http://127.0.0.1:9999"));
        assert_eq!(
            args,
            ["stack", "status", "--profile", "/tmp/demo"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn global_options_reject_missing_invalid_and_duplicate_values() {
        for values in [
            vec!["stack", "status", "--color"],
            vec!["stack", "status", "--color", "--json"],
            vec!["--color", "sometimes", "stack", "status"],
            vec!["--color", "auto", "--color", "never", "health"],
            vec![
                "--url",
                "http://127.0.0.1:1",
                "--url",
                "http://127.0.0.1:2",
                "health",
            ],
        ] {
            let mut args = values.into_iter().map(str::to_string).collect();
            assert!(GlobalOptions::parse(&mut args).is_err());
        }
    }

    #[test]
    fn colored_table_preserves_plain_alignment_and_text() {
        let rows = vec![
            vec!["alpha".to_string(), "running_ready".to_string()],
            vec!["longer-name".to_string(), "stopped".to_string()],
        ];
        let plain = render_table(
            &["COMPONENT", "STATUS"],
            &rows,
            terminal::TerminalStyle::plain(),
        );
        let colored = render_table(
            &["COMPONENT", "STATUS"],
            &rows,
            terminal::TerminalStyle::colored(),
        );

        assert!(colored.contains("\u{1b}["));
        assert_eq!(strip_ansi(&colored), plain);
        assert!(plain.contains("COMPONENT    STATUS"));
        assert!(plain.contains("alpha        running_ready"));
    }

    fn legacy_spend_args() -> Vec<String> {
        [
            "--operation-key",
            "legacy-operation",
            "--account-id",
            "aga_example",
            "--amount",
            "5",
            "--reason",
            "Legacy CLI reason",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn capture_cli_request(run: impl FnOnce(&CliContext) -> Result<()>) -> (String, Value) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = String::new();
            stream.read_to_string(&mut raw).unwrap();
            let response_body = r#"{"error":"captured"}"#;
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .unwrap();
            let (head, body) = raw.split_once("\r\n\r\n").unwrap();
            let path = head
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            (path, serde_json::from_str(body).unwrap())
        });

        let client = CliContext::new(
            Some(format!("http://{address}")),
            std::env::temp_dir().join("hubu-cli-unit-test-home"),
        );
        let error = run(&client).unwrap_err();
        assert!(error.to_string().contains("captured"));
        server.join().unwrap()
    }

    #[test]
    fn both_spend_helpers_omit_task_id_for_legacy_retry_compatibility() {
        let (path, body) = capture_cli_request(|base_url| spend(base_url, legacy_spend_args()));
        assert_eq!(path, "/spend");
        assert!(body.get("task_id").is_none());

        let (path, body) =
            capture_cli_request(|base_url| spend_authorize(base_url, legacy_spend_args()));
        assert_eq!(path, "/spend/authorize");
        assert!(body.get("task_id").is_none());
    }

    #[test]
    fn spend_approval_cli_submits_the_explicit_human_decision() {
        let (path, body) = capture_cli_request(|base_url| {
            spend_approval(
                base_url,
                ["approve", "--approval-request-id", "decision-123"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )
        });
        assert_eq!(path, "/spend/approval/resolve");
        assert_eq!(body["approval_request_id"], "decision-123");
        assert_eq!(body["decision"], "approve");
    }

    #[test]
    fn typed_scope_flags_are_complete_and_stable() {
        let mut args = [
            "--provider",
            "provider:google:gemini-developer",
            "--executor",
            "executor:gongbu:image",
            "--capability",
            "capability:image:generate",
            "--billing-merchant",
            "merchant:google",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let (merchant, scope) = take_execution_scope(&mut args).unwrap();
        assert!(merchant.is_none());
        assert_eq!(
            scope.unwrap()["provider"],
            "provider:google:gemini-developer"
        );
    }

    #[test]
    fn typed_scope_flags_cannot_mix_with_legacy_merchant() {
        let mut args = [
            "--merchant",
            "google",
            "--provider",
            "provider:google:gemini-developer",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert!(take_execution_scope(&mut args)
            .unwrap_err()
            .to_string()
            .contains("cannot be combined"));
    }

    #[test]
    fn decimal_major_unit_amounts_distinguish_five_from_five_cents() {
        assert_eq!(amount_to_cents("5").unwrap(), 500);
        assert_eq!(amount_to_cents("0.05").unwrap(), 5);
    }

    #[test]
    fn budget_version_paths_require_exact_safe_public_ids() {
        assert_eq!(
            budget_versions_path("bgt_0123456789ab").unwrap(),
            "/budgets/bgt_0123456789ab/versions"
        );
        for unsafe_id in [
            "0123456789ab",
            "bgt_short",
            "bgt_0123456789ai",
            "bgt_0123456789a/versions",
            "bgt_0123456789AB",
        ] {
            assert!(budget_versions_path(unsafe_id).is_err(), "{unsafe_id}");
        }
    }

    #[test]
    fn retired_budget_replace_command_is_rejected_before_network_access() {
        let client = CliContext::new(
            Some("http://127.0.0.1:1".to_string()),
            std::env::temp_dir().join("hubu-cli-retired-budget-replace-test-home"),
        );
        let error = budget(
            &client,
            vec![
                "replace".to_string(),
                "--budget-id".to_string(),
                "bgt_0123456789ab".to_string(),
                "--amount".to_string(),
                "10".to_string(),
            ],
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("unknown budget command `replace`"));
    }

    #[test]
    fn budget_reason_rendering_escapes_terminal_control_characters() {
        let rendered = terminal_safe_text("raise\n  proposed_total: $999.00\r\u{1b}[31m");
        assert!(rendered.contains("\\n"));
        assert!(rendered.contains("\\r"));
        assert!(rendered.contains("\\u001b"));
        assert!(!rendered.contains('\n'));
        assert!(!rendered.contains('\r'));
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn explicit_historical_revision_is_submitted_without_local_head_rejection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let responses = [
                json!({
                    "current_budget": sample_budget("revoked", 3, 1_000, 900, 100),
                    "versions": []
                }),
                json!({
                    "applied_version": sample_budget_version(2, 1, 500),
                    "current_budget": sample_budget("revoked", 3, 1_000, 900, 100),
                    "idempotent_replay": true,
                    "spending_target_warnings": []
                }),
            ];
            let mut captured = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut raw = String::new();
                stream.read_to_string(&mut raw).unwrap();
                let (head, body) = raw.split_once("\r\n\r\n").unwrap();
                let request_line = head.lines().next().unwrap().to_string();
                let body: Option<Value> =
                    (!body.is_empty()).then(|| serde_json::from_str(body).unwrap());
                captured.push((request_line, body));
                let response_body = response.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                )
                .unwrap();
            }
            captured
        });
        let client = CliContext::new(
            Some(format!("http://{address}")),
            std::env::temp_dir().join("hubu-cli-budget-update-test-home"),
        );

        budget_update(
            &client,
            [
                "--budget-id",
                "bgt_0123456789ab",
                "--amount",
                "5",
                "--expected-revision",
                "1",
                "--reason",
                "historical retry",
                "--yes",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        )
        .unwrap();

        let captured = server.join().unwrap();
        assert_eq!(
            captured[0].0,
            "GET /budgets/bgt_0123456789ab/versions HTTP/1.1"
        );
        assert!(captured[0].1.is_none());
        assert_eq!(
            captured[1].0,
            "POST /budgets/bgt_0123456789ab/versions HTTP/1.1"
        );
        assert_eq!(
            captured[1].1.as_ref().unwrap(),
            &json!({
                "amount_limit_cents": 500,
                "expected_revision": 1,
                "reason": "historical retry"
            })
        );
    }

    #[test]
    fn ambiguous_budget_post_retries_once_with_byte_equivalent_json_intent() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let success_response = json!({
            "applied_version": sample_budget_version(8, 7, 5_000),
            "current_budget": sample_budget("active", 8, 5_000, 0, 0),
            "idempotent_replay": false,
            "spending_target_warnings": []
        });
        let server_success = success_response.clone();
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut raw = String::new();
                stream.read_to_string(&mut raw).unwrap();
                bodies.push(raw.split_once("\r\n\r\n").unwrap().1.to_string());
                let response_body = if attempt == 0 {
                    json!({"ok": true}).to_string()
                } else {
                    server_success.to_string()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                )
                .unwrap();
            }
            bodies
        });
        let client = CliContext::new(
            Some(format!("http://{address}")),
            std::env::temp_dir().join("hubu-cli-budget-retry-test-home"),
        );
        let body = json!({
            "amount_limit_cents": 5_000,
            "expected_revision": 7,
            "reason": "historical retry"
        });

        let response = post_budget_update_with_retry(
            &client,
            "/budgets/bgt_0123456789ab/versions",
            "bgt_0123456789ab",
            &body,
        )
        .unwrap();
        assert_eq!(response, success_response);
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
    }

    #[test]
    fn typed_storage_outcome_retries_once_and_recovers_exact_replay() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let success_response = json!({
            "applied_version": sample_budget_version(8, 7, 5_000),
            "current_budget": sample_budget("active", 8, 5_000, 0, 0),
            "idempotent_replay": true,
            "spending_target_warnings": []
        });
        let server_success = success_response.clone();
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut raw = String::new();
                stream.read_to_string(&mut raw).unwrap();
                bodies.push(raw.split_once("\r\n\r\n").unwrap().1.to_string());
                let (status, response_body) = if attempt == 0 {
                    (
                        "500 Internal Server Error",
                        json!({
                            "error": "budget update could not be completed",
                            "error_code": "budget_update_storage_error",
                            "retry_guidance": {
                                "action": "retry_exactly",
                                "message": "reuse the pinned intent"
                            }
                        })
                        .to_string(),
                    )
                } else {
                    ("200 OK", server_success.to_string())
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                )
                .unwrap();
            }
            bodies
        });
        let client = CliContext::new(
            Some(format!("http://{address}")),
            std::env::temp_dir().join("hubu-cli-budget-storage-retry-test-home"),
        );
        let body = json!({
            "amount_limit_cents": 5_000,
            "expected_revision": 7,
            "reason": "historical retry"
        });

        let response = post_budget_update_with_retry(
            &client,
            "/budgets/bgt_0123456789ab/versions",
            "bgt_0123456789ab",
            &body,
        )
        .unwrap();
        assert_eq!(response, success_response);
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
    }

    #[test]
    fn structured_budget_rejection_is_never_retried() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = String::new();
            stream.read_to_string(&mut raw).unwrap();
            let response_body =
                r#"{"error":"refresh first","error_code":"budget_revision_conflict"}"#;
            write!(
                stream,
                "HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .unwrap();
            drop(stream);
            std::thread::sleep(std::time::Duration::from_millis(50));
            listener.set_nonblocking(true).unwrap();
            matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            )
        });
        let client = CliContext::new(
            Some(format!("http://{address}")),
            std::env::temp_dir().join("hubu-cli-budget-rejection-test-home"),
        );
        let body = json!({
            "amount_limit_cents": 5_000,
            "expected_revision": 7
        });

        let error = post_budget_update_with_retry(
            &client,
            "/budgets/bgt_0123456789ab/versions",
            "bgt_0123456789ab",
            &body,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<HttpApplicationError>().is_some());
        assert!(error.to_string().contains("HTTP 409"));
        assert!(server.join().unwrap(), "structured rejection was retried");
    }

    fn sample_budget(status: &str, revision: u64, total: i64, consumed: i64, frozen: i64) -> Value {
        json!({
            "budget_id": "bgt_0123456789ab",
            "agent_id": "agt_0123456789ab",
            "status": status,
            "amount_limit_cents": total,
            "consumed_amount_cents": consumed,
            "frozen_amount_cents": frozen,
            "remaining_amount_cents": total - consumed - frozen,
            "current_version_id": format!("bgv_{revision:012}"),
            "current_revision": revision,
            "starting_at": "2026-08-28T00:00:00+00:00",
            "ending_before": null
        })
    }

    fn sample_budget_version(revision: u64, predecessor_revision: u64, total: i64) -> Value {
        json!({
            "version_id": format!("bgv_{revision:012}"),
            "revision": revision,
            "predecessor_version_id": format!("bgv_{predecessor_revision:012}"),
            "predecessor_revision": predecessor_revision,
            "amount_limit_cents": total,
            "effective_at": "2026-08-28T00:00:00+00:00",
            "actor": "usr_0123456789ab",
            "source": "hubu-api:update-budget-limit",
            "reason": "historical retry",
            "request_fingerprint": "sha256:test",
            "created_at": "2026-08-28T00:00:00+00:00"
        })
    }

    #[test]
    fn cents_formatting_preserves_negative_sign_and_i64_boundaries() {
        assert_eq!(format_major_amount(0), "0.00");
        assert_eq!(format_major_amount(-1), "-0.01");
        assert_eq!(format_major_amount(i64::MIN), "-92233720368547758.08");
        assert_eq!(format_major_amount(i64::MAX), "92233720368547758.07");

        assert_eq!(
            money_at(
                &json!({ "remaining_amount_cents": -1 }),
                "remaining_amount_cents"
            )
            .unwrap(),
            "-$0.01"
        );
        assert_eq!(
            money_at(
                &json!({ "remaining_amount_cents": i64::MIN }),
                "remaining_amount_cents"
            )
            .unwrap(),
            "-$92233720368547758.08"
        );
    }

    #[test]
    fn missing_merchant_remains_explicitly_omitted() {
        let mut args = Vec::new();
        let (merchant, scope) = take_execution_scope(&mut args).unwrap();
        assert!(merchant.is_none());
        assert!(scope.is_none());
    }

    #[test]
    fn local_timestamp_converts_rfc3339_to_the_system_timezone() {
        let timestamp = "2026-08-14T21:55:43.393178+00:00";
        let expected = DateTime::parse_from_rfc3339(timestamp)
            .unwrap()
            .with_timezone(&Local)
            .to_rfc3339();

        assert_eq!(local_timestamp(timestamp), expected);
    }

    #[test]
    fn local_timestamp_preserves_an_unparseable_value() {
        assert_eq!(local_timestamp("unknown"), "unknown");
    }
}
