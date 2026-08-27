use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::OpenOptions,
    io::Write,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

mod doctor;
mod lifecycle;

const SOURCE_SCHEMA_VERSION: u32 = 1;
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const PRINCIPAL_NEUTRAL_GONGBU_SCHEMA_VERSION: u32 = 3;
const DEFAULT_PROFILE: &str = "default";
const PROFILE_REGISTRY_SCHEMA_VERSION: u32 = 1;
const PROFILE_LIST_SCHEMA_VERSION: u32 = 1;
const PROFILE_REGISTRY_FILE: &str = "stack-profiles.json";
const LIVE_SPEND_ACKNOWLEDGEMENT: &str = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND";
const MANAGED_GONGBU_SECRET_SERVICE: &str = "hubu.managed-stack.v1";
const MANAGED_GONGBU_HUBU_ACCOUNT: &str = "hubu-executor";
const MANAGED_GONGBU_CALLER_ACCOUNT: &str = "gongbu-caller";
const MANAGED_CREDENTIAL_IGNORE: &[u8] =
    b"# Hubu-managed credentials. Do not edit or remove.\n*\n!.gitignore\n";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProfileRegistry {
    schema_version: u32,
    selected: Option<PathBuf>,
    profiles: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct ProfileListReport {
    schema_version: u32,
    profiles: Vec<ProfileListEntry>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct ProfileListEntry {
    path: PathBuf,
    selected: bool,
    #[serde(rename = "default")]
    is_default: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StackSource {
    schema_version: u32,
    #[serde(default)]
    allow_development_builds: bool,
    binaries: Option<BinarySource>,
    // Accepted only for source schema v1 compatibility; remove when that
    // compatibility window closes. Rendering never consumes these values.
    #[serde(rename = "identity")]
    _legacy_identity: Option<LegacyIdentitySource>,
    hubu: Option<ServiceSource>,
    gongbu: Option<GongbuSource>,
    temporal: Option<TemporalSource>,
    #[serde(default)]
    runtime: RuntimePolicy,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BinarySource {
    hubu: Option<PathBuf>,
    hubu_server: Option<PathBuf>,
    gongbu_server: Option<PathBuf>,
    hubu_unified_mcp: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyIdentitySource {
    #[serde(rename = "account_id")]
    _account_id: Option<String>,
    #[serde(rename = "agent_id")]
    _agent_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Ownership {
    Managed,
    External,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceSource {
    ownership: Option<Ownership>,
    endpoint: Option<String>,
    listen: Option<SocketAddr>,
    database_path: Option<PathBuf>,
    log_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GongbuSource {
    ownership: Option<Ownership>,
    endpoint: Option<String>,
    listen: Option<SocketAddr>,
    database_path: Option<PathBuf>,
    artifact_root: Option<PathBuf>,
    log_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TemporalMode {
    ManagedLocal,
    External,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemporalSource {
    mode: Option<TemporalMode>,
    binary_path: Option<PathBuf>,
    expected_cli_version: Option<String>,
    data_path: Option<PathBuf>,
    rpc_port: Option<u16>,
    ui_port: Option<u16>,
    address: Option<String>,
    namespace: Option<String>,
    task_queue: Option<String>,
    ui_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RuntimePolicy {
    hubu_startup_policy: String,
    hubu_startup_timeout_ms: u64,
    recovery_delays_seconds: Vec<u64>,
    temporal_startup_timeout_ms: u64,
    dependency_check_interval_ms: u64,
    worker_drain_timeout_ms: u64,
    max_artifacts_per_execution: u64,
    max_encoded_bytes: u64,
    max_decoded_bytes: u64,
    max_width: u32,
    max_height: u32,
    log_level: String,
    log_format: String,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            hubu_startup_policy: "wait".into(),
            hubu_startup_timeout_ms: 30_000,
            recovery_delays_seconds: vec![30, 120, 600],
            temporal_startup_timeout_ms: 30_000,
            dependency_check_interval_ms: 5_000,
            worker_drain_timeout_ms: 30_000,
            max_artifacts_per_execution: 4,
            max_encoded_bytes: 20_971_520,
            max_decoded_bytes: 104_857_600,
            max_width: 16_384,
            max_height: 16_384,
            log_level: "info".into(),
            log_format: "text".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialsSource {
    schema_version: u32,
    files: Option<CredentialFiles>,
    #[serde(default)]
    opaque: BTreeMap<String, OpaqueReference>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFiles {
    hubu_auth: Option<PathBuf>,
    hubu_approval: Option<PathBuf>,
    hubu_reconciliation: Option<PathBuf>,
    gongbu_caller: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct ResolvedCredentialPaths {
    hubu_auth: PathBuf,
    hubu_approval: PathBuf,
    hubu_reconciliation: PathBuf,
    gongbu_caller: PathBuf,
    gongbu_secret_dir: PathBuf,
    managed_gongbu_handoff: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OpaqueReference {
    service: Option<String>,
    account: Option<String>,
}

fn managed_credential_root(profile: &Path) -> PathBuf {
    profile.join("state/credentials")
}

fn managed_hubu_credential_root(profile: &Path) -> PathBuf {
    managed_credential_root(profile).join("hubu")
}

fn managed_gongbu_credential_root(profile: &Path) -> PathBuf {
    managed_credential_root(profile).join("gongbu")
}

fn uses_managed_gongbu_handoff(stack: &StackSource, credentials: &CredentialsSource) -> bool {
    stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed)
        && !credentials.opaque.contains_key("gongbu_hubu")
        && !credentials.opaque.contains_key("gongbu_caller")
}

fn managed_gongbu_reference(account: &str) -> OpaqueReference {
    OpaqueReference {
        service: Some(MANAGED_GONGBU_SECRET_SERVICE.into()),
        account: Some(account.into()),
    }
}

fn gongbu_bootstrap_references(
    stack: &StackSource,
    credentials: &CredentialsSource,
) -> Result<(OpaqueReference, OpaqueReference)> {
    if uses_managed_gongbu_handoff(stack, credentials) {
        return Ok((
            managed_gongbu_reference(MANAGED_GONGBU_HUBU_ACCOUNT),
            managed_gongbu_reference(MANAGED_GONGBU_CALLER_ACCOUNT),
        ));
    }
    let hubu = credentials
        .opaque
        .get("gongbu_hubu")
        .cloned()
        .ok_or_else(|| anyhow!("credentials.toml:opaque.gongbu_hubu is required"))?;
    let caller = credentials
        .opaque
        .get("gongbu_caller")
        .cloned()
        .ok_or_else(|| anyhow!("credentials.toml:opaque.gongbu_caller is required"))?;
    if [&hubu, &caller]
        .iter()
        .any(|reference| reference.service.as_deref() == Some(MANAGED_GONGBU_SECRET_SERVICE))
    {
        bail!(
            "credentials.toml explicit Gongbu overrides cannot use the reserved managed-stack secret service"
        );
    }
    Ok((hubu, caller))
}

fn resolve_credential_paths(
    profile: &Path,
    stack: &StackSource,
    credentials: &CredentialsSource,
) -> Result<ResolvedCredentialPaths> {
    let files = credentials.files.as_ref();
    let hubu_managed =
        stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed);
    let gongbu_handoff_managed = uses_managed_gongbu_handoff(stack, credentials);
    let hubu_root = managed_hubu_credential_root(profile);
    let gongbu_root = managed_gongbu_credential_root(profile);
    let hubu_auth = resolve_credential_path(
        files.and_then(|value| value.hubu_auth.as_deref()),
        hubu_managed,
        &hubu_root.join("auth"),
        "credentials.toml:files.hubu_auth",
    )?;
    let hubu_approval = resolve_credential_path(
        files.and_then(|value| value.hubu_approval.as_deref()),
        hubu_managed,
        &hubu_root.join("approval"),
        "credentials.toml:files.hubu_approval",
    )?;
    let hubu_reconciliation = resolve_credential_path(
        files.and_then(|value| value.hubu_reconciliation.as_deref()),
        hubu_managed,
        &hubu_root.join("reconciliation"),
        "credentials.toml:files.hubu_reconciliation",
    )?;
    let gongbu_caller = resolve_credential_path(
        files.and_then(|value| value.gongbu_caller.as_deref()),
        gongbu_handoff_managed,
        &gongbu_root.join("caller"),
        "credentials.toml:files.gongbu_caller",
    )?;
    let gongbu_secret_dir = resolve_existing_prefix(&gongbu_root)?;
    let gongbu_hubu = resolve_existing_prefix(&gongbu_root.join("hubu-executor"))?;
    if gongbu_handoff_managed && gongbu_caller != gongbu_secret_dir.join("caller") {
        bail!("managed Gongbu caller file overrides require explicit opaque bootstrap references");
    }
    let paths = [
        &hubu_auth,
        &hubu_approval,
        &hubu_reconciliation,
        &gongbu_caller,
        &gongbu_hubu,
    ];
    for (index, path) in paths.iter().enumerate() {
        for prior in &paths[..index] {
            if paths_resolve_to_same_resource(path, prior)? {
                bail!("managed credential references must identify distinct resources");
            }
        }
    }
    Ok(ResolvedCredentialPaths {
        hubu_auth,
        hubu_approval,
        hubu_reconciliation,
        gongbu_caller,
        gongbu_secret_dir,
        managed_gongbu_handoff: gongbu_handoff_managed,
    })
}

fn resolve_credential_path(
    configured: Option<&Path>,
    provisioned_by_managed_stack: bool,
    managed_default: &Path,
    field: &str,
) -> Result<PathBuf> {
    match configured {
        Some(path) => {
            validate_safe_absolute(path, field)?;
            existing_absolute(path, field)
        }
        None if provisioned_by_managed_stack => {
            validate_safe_absolute(managed_default, field)?;
            if managed_default.exists() && !managed_default.is_file() {
                bail!("{field} must name a regular file when it already exists");
            }
            resolve_existing_prefix(managed_default)
        }
        None => bail!("{field} is required for an external credential owner"),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProviderMode {
    Disabled,
    Live,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvidersSource {
    schema_version: u32,
    mode: Option<ProviderMode>,
    catalog_version: Option<String>,
    maximum_spend_minor: Option<i64>,
    live_spend_acknowledgement: Option<String>,
    #[serde(default)]
    targets: Vec<ProviderTargetSource>,
    #[serde(default)]
    pricing_rules: Vec<toml::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderTargetSource {
    provider_config_version: Option<String>,
    workload_type: Option<String>,
    provider: Option<String>,
    adapter: Option<String>,
    model: Option<String>,
    credential: Option<String>,
    active: Option<bool>,
    execution_enabled: Option<bool>,
    settings: Option<toml::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodexHandoff {
    pub schema_version: u32,
    pub mcp_server: PathBuf,
    pub hubu_endpoint: String,
    pub hubu_token_file: PathBuf,
    pub approval_token_file: PathBuf,
    pub reconciliation_token_file: PathBuf,
    pub operation_state_path: PathBuf,
    pub gongbu_endpoint: String,
    pub gongbu_token_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct BinaryProvenance {
    component: String,
    path: PathBuf,
    product_version: String,
    source_commit: String,
    executor_contract: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_config_schema_version: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActiveManifest {
    schema_version: u32,
    generation_id: String,
    generation: String,
    #[serde(default)]
    source_schema_versions: BTreeMap<String, u32>,
    source_digests: BTreeMap<String, String>,
    generated_file_digests: BTreeMap<String, String>,
    binary_provenance: Vec<BinaryProvenance>,
    process_log_files: BTreeMap<String, Option<PathBuf>>,
    #[serde(default)]
    restart_impact: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RenderOutcome {
    generation_id: String,
    activated: bool,
    changed_source_files: Vec<String>,
    affected_components: Vec<String>,
}

pub(crate) fn command(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    let Some(subcommand) = args.first().cloned() else {
        print_help();
        return Ok(());
    };
    args.remove(0);
    match subcommand.as_str() {
        "init" => init(args, hubu_home),
        "select" => select_profile(args, hubu_home),
        "profiles" => profiles(args, hubu_home),
        "doctor" => doctor::command(args, hubu_home),
        "render" => render(args, hubu_home),
        "activate" => activate(args, hubu_home),
        "rollback" => rollback(args, hubu_home),
        "generations" => generations(args, hubu_home),
        "start" => lifecycle::start(args, hubu_home),
        "status" => lifecycle::status(args, hubu_home),
        "logs" => lifecycle::logs(args, hubu_home),
        "stop" => lifecycle::stop(args, hubu_home),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        _ => bail!("unknown stack command `{subcommand}`"),
    }
}

pub(crate) fn codex_handoff(profile: &Path, hubu_home: &Path) -> Result<CodexHandoff> {
    let root = resolve_profile(profile, hubu_home)?;
    let manifest_path = root.join("generated/active-manifest.json");
    let manifest: ActiveManifest = read_json(&manifest_path)?;
    let generation = active_generation_path(&root.join("generated"), &manifest)?;
    verify_generated_file(&generation, &manifest, "client-handoff.json")?;
    let handoff: CodexHandoff = read_json(&generation.join("client-handoff.json"))?;
    if handoff.schema_version != 2 {
        bail!("active client handoff has an unsupported schema_version; run `hubu stack render`, activate the schema-v2 generation, and reinitialize the client");
    }
    Ok(handoff)
}

fn active_credential_paths(
    profile: &Path,
    manifest: &ActiveManifest,
) -> Result<(ResolvedCredentialPaths, u64)> {
    let generation = active_generation_path(&profile.join("generated"), manifest)?;
    verify_generated_file(&generation, manifest, "client-handoff.json")?;
    let handoff: CodexHandoff = read_json(&generation.join("client-handoff.json"))?;
    if handoff.schema_version != 2 {
        bail!("active client handoff has an unsupported schema_version");
    }

    let gongbu_root = managed_gongbu_credential_root(profile);
    let gongbu_secret_dir = resolve_existing_prefix(&gongbu_root)?;
    let mut managed_gongbu_handoff = false;
    let mut startup_timeout_ms = 30_000;
    if manifest
        .generated_file_digests
        .contains_key("gongbu-server.json")
    {
        verify_generated_file(&generation, manifest, "gongbu-server.json")?;
        let config: Value = read_json(&generation.join("gongbu-server.json"))?;
        let hubu_reference = &config["hubu"]["credential_reference"];
        let caller_reference = &config["authentication"]["bearer_credential_reference"];
        managed_gongbu_handoff = hubu_reference["service"].as_str()
            == Some(MANAGED_GONGBU_SECRET_SERVICE)
            && hubu_reference["account"].as_str() == Some(MANAGED_GONGBU_HUBU_ACCOUNT)
            && caller_reference["service"].as_str() == Some(MANAGED_GONGBU_SECRET_SERVICE)
            && caller_reference["account"].as_str() == Some(MANAGED_GONGBU_CALLER_ACCOUNT);
        startup_timeout_ms = config["hubu"]["startup_timeout_ms"]
            .as_u64()
            .ok_or_else(|| anyhow!("active Gongbu config has no valid Hubu startup timeout"))?;
    }
    Ok((
        ResolvedCredentialPaths {
            hubu_auth: handoff.hubu_token_file,
            hubu_approval: handoff.approval_token_file,
            hubu_reconciliation: handoff.reconciliation_token_file,
            gongbu_caller: handoff.gongbu_token_file,
            gongbu_secret_dir,
            managed_gongbu_handoff,
        },
        startup_timeout_ms,
    ))
}

fn init(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_init_help();
        return Ok(());
    }
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let style = crate::terminal::stdout();
    create_secure_dir(&profile)?;
    create_secure_dir(&profile.join("generated"))?;
    let credential_ignore_created = ensure_managed_credential_ignore(&profile)?;

    let files = [
        ("README.md", readme_template()),
        ("stack.toml", stack_template(&profile)?),
        ("credentials.toml", credentials_template()),
        ("providers.toml", providers_template()),
    ];
    for (name, contents) in files {
        let path = profile.join(name);
        if write_new_secure(&path, contents.as_bytes())? {
            println!(
                "{}: {}",
                style.success("created"),
                style.accent(path.display())
            );
        } else {
            println!(
                "{}: {}",
                style.muted("preserved"),
                style.accent(path.display())
            );
        }
    }
    register_profile(&profile, hubu_home, false)?;
    let credential_ignore = managed_credential_root(&profile).join(".gitignore");
    if credential_ignore_created {
        println!(
            "{}: {}",
            style.success("created"),
            style.accent(credential_ignore.display())
        );
    } else {
        println!(
            "{}: {}",
            style.muted("preserved"),
            style.accent(credential_ignore.display())
        );
    }
    println!(
        "{}: {}",
        style.label("profile"),
        style.accent(profile.display())
    );
    let input_files = files_needing_input(&profile);
    if input_files.is_empty() {
        println!("{}: {}", style.label("input needed"), style.success("none"));
    } else {
        println!("{}:", style.warning("input needed"));
        for path in input_files {
            println!("  - {}", style.accent(path.display()));
        }
    }
    println!(
        "{}: edit the annotated files, then run {}",
        style.label("next"),
        style.command(format!(
            "`hubu stack doctor --profile {}`",
            profile.display()
        ))
    );
    Ok(())
}

fn select_profile(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_select_help();
        return Ok(());
    }
    let profile = take_required_profile(&mut args)?;
    ensure_no_args(args)?;
    let profile = initialized_profile_path(&profile, "--profile")?;
    register_profile(&profile, hubu_home, true)?;
    let style = crate::terminal::stdout();
    println!(
        "{}: {}",
        style.success("selected profile"),
        style.accent(profile.display())
    );
    Ok(())
}

fn profiles(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_profiles_help();
        return Ok(());
    }
    let json_output = crate::take_flag(&mut args, "--json");
    ensure_no_args(args)?;
    let report = profile_list_report(hubu_home)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.profiles.is_empty() {
        println!(
            "{}",
            crate::terminal::stdout().muted("No initialized stack profiles found.")
        );
    } else {
        let style = crate::terminal::stdout();
        println!("{}", style.heading("SELECTED  DEFAULT  PATH"));
        for profile in report.profiles {
            println!(
                "{}  {}  {}",
                if profile.selected {
                    style.success(format!("{:<8}", "*"))
                } else {
                    format!("{:<8}", "")
                },
                if profile.is_default {
                    style.accent(format!("{:<7}", "yes"))
                } else {
                    format!("{:<7}", "")
                },
                style.accent(profile.path.display())
            );
        }
    }
    Ok(())
}

fn files_needing_input(profile: &Path) -> Vec<PathBuf> {
    let paths = [
        profile.join("stack.toml"),
        profile.join("credentials.toml"),
        profile.join("providers.toml"),
    ];
    let stack = fs::read(&paths[0])
        .ok()
        .and_then(|bytes| parse_toml::<StackSource>(&paths[0], &bytes).ok());
    let credentials = fs::read(&paths[1])
        .ok()
        .and_then(|bytes| parse_toml::<CredentialsSource>(&paths[1], &bytes).ok());
    let providers = fs::read(&paths[2])
        .ok()
        .and_then(|bytes| parse_toml::<ProvidersSource>(&paths[2], &bytes).ok());
    let mut needed = Vec::new();
    if stack.is_none() {
        needed.push(paths[0].clone());
    }
    if credentials.is_none() {
        needed.push(paths[1].clone());
    }
    if providers.is_none() {
        needed.push(paths[2].clone());
    }
    let (Some(stack), Some(credentials), Some(providers)) = (stack, credentials, providers) else {
        return needed;
    };
    if stack.schema_version != SOURCE_SCHEMA_VERSION {
        needed.push(paths[0].clone());
    }
    if credentials.schema_version != SOURCE_SCHEMA_VERSION {
        needed.push(paths[1].clone());
    }
    if providers.schema_version != SOURCE_SCHEMA_VERSION {
        needed.push(paths[2].clone());
    }
    for field in missing_fields(&stack, &credentials, &providers) {
        let path = if field.starts_with("stack.toml:") {
            &paths[0]
        } else if field.starts_with("credentials.toml:") {
            &paths[1]
        } else {
            &paths[2]
        };
        if !needed.contains(path) {
            needed.push(path.clone());
        }
    }
    needed
}

fn render(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_render_help();
        return Ok(());
    }
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let _lock = lifecycle::acquire_lifecycle_lock(&profile)?;
    render_profile(&profile).map(|_| ())
}

fn activate(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_activate_help();
        return Ok(());
    }
    let generation = required_generation(&mut args)?;
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let _lock = lifecycle::acquire_lifecycle_lock(&profile)?;
    activate_selected_generation(&profile, &generation, false)
}

fn rollback(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_rollback_help();
        return Ok(());
    }
    let generation = required_generation(&mut args)?;
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let _lock = lifecycle::acquire_lifecycle_lock(&profile)?;
    activate_selected_generation(&profile, &generation, true)
}

fn generations(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_generations_help();
        return Ok(());
    }
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let generated = profile.join("generated");
    let active_path = generated.join("active-manifest.json");
    let active = if active_path.exists() {
        Some(read_json::<ActiveManifest>(&active_path).context("read active generation manifest")?)
    } else {
        None
    };
    let generations_dir = generated.join("generations");
    if !generations_dir.exists() {
        println!(
            "{}",
            crate::terminal::stdout().muted("no recoverable generations")
        );
        return Ok(());
    }
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&generations_dir)
        .with_context(|| format!("list generations for `{}`", profile.display()))?
    {
        let entry = entry.context("read generation directory entry")?;
        if !entry.file_type()?.is_dir() {
            bail!(
                "unexpected non-directory entry in generation store: `{}`",
                entry.path().display()
            );
        }
        let manifest_path = entry.path().join("manifest.json");
        let manifest: ActiveManifest = if manifest_path.exists() {
            read_json(&manifest_path).with_context(|| {
                format!("read recoverable generation `{}`", entry.path().display())
            })?
        } else if active
            .as_ref()
            .is_some_and(|value| generated.join(&value.generation) == entry.path())
        {
            active.clone().expect("checked active generation")
        } else {
            bail!(
                "recoverable generation `{}` has no manifest",
                entry.path().display()
            );
        };
        let resolved = active_generation_path(&generated, &manifest)?;
        if resolved != entry.path() {
            bail!(
                "generation directory `{}` does not match its manifest",
                entry.path().display()
            );
        }
        validate_manifest_integrity(&generated, &manifest)?;
        manifests.push(manifest);
    }
    manifests.sort_by(|left, right| left.generation_id.cmp(&right.generation_id));
    if manifests.is_empty() {
        println!(
            "{}",
            crate::terminal::stdout().muted("no recoverable generations")
        );
        return Ok(());
    }
    let style = crate::terminal::stdout();
    for manifest in manifests {
        let is_active = active
            .as_ref()
            .is_some_and(|value| value.generation_id == manifest.generation_id);
        println!(
            "{}{}",
            style.accent(&manifest.generation_id),
            if is_active {
                format!(" {}", style.success("active"))
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

fn required_generation(args: &mut Vec<String>) -> Result<String> {
    let value = take_option_value(args, "--generation")?
        .ok_or_else(|| anyhow!("--generation is required"))?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("--generation must be a lowercase 64-character SHA-256 identifier");
    }
    Ok(value)
}

fn activate_selected_generation(profile: &Path, requested: &str, rollback: bool) -> Result<()> {
    let renderer = env::current_exe()
        .and_then(fs::canonicalize)
        .context("resolve the running hubu executable")?;
    activate_selected_generation_with_renderer(profile, requested, rollback, &renderer)
}

fn activate_selected_generation_with_renderer(
    profile: &Path,
    requested: &str,
    rollback: bool,
    renderer: &Path,
) -> Result<()> {
    let outcome = render_profile_with_renderer_outcome(profile, renderer)?;
    lifecycle::ensure_profile_stopped(profile)?;
    if outcome.generation_id != requested {
        bail!(
            "generation `{requested}` does not match the current operator-owned source files and selected binary provenance; restore stack.toml, credentials.toml, providers.toml, and the compatible binaries recorded for that generation before retrying"
        );
    }
    let generated = profile.join("generated");
    let active_path = generated.join("active-manifest.json");
    let previous = if active_path.exists() {
        Some(read_json::<ActiveManifest>(&active_path).context("read active generation manifest")?)
    } else {
        None
    };
    if rollback
        && previous
            .as_ref()
            .is_none_or(|value| value.generation_id == requested)
    {
        bail!("rollback requires a different currently active generation");
    }
    if !rollback
        && previous
            .as_ref()
            .is_some_and(|value| value.generation_id == requested)
    {
        let style = crate::terminal::stdout();
        println!(
            "{}: {}",
            style.muted("activation unchanged"),
            style.accent(requested)
        );
        return Ok(());
    }
    let path = generated.join("generations").join(requested);
    let mut target: ActiveManifest = read_json(&path.join("manifest.json"))?;
    active_generation_path(&generated, &target)?;
    target.restart_impact = restart_impact_between(previous.as_ref(), &target);
    let plan = render_outcome(previous.as_ref(), &target, true);
    let style = crate::terminal::stdout();
    println!(
        "{}: {} -> {}",
        style.heading(if rollback {
            "rollback plan"
        } else {
            "activation plan"
        }),
        previous
            .as_ref()
            .map(|value| value.generation_id.as_str())
            .map(|value| style.accent(value))
            .unwrap_or_else(|| style.muted("none")),
        style.accent(requested)
    );
    println!(
        "{}: {}",
        style.label("affected components"),
        if plan.affected_components.is_empty() {
            style.muted("none")
        } else {
            style.warning(plan.affected_components.join(", "))
        }
    );
    activate_manifest(&generated, &target)?;
    println!(
        "{}: {}",
        if rollback {
            style.success("rolled back to generation")
        } else {
            style.success("activated generation")
        },
        style.accent(requested)
    );
    println!(
        "{}: {}",
        style.label("next"),
        style.command(format!("hubu stack start --profile {}", profile.display()))
    );
    Ok(())
}

fn render_profile(profile: &Path) -> Result<RenderOutcome> {
    let renderer = env::current_exe()
        .and_then(fs::canonicalize)
        .context("resolve the running hubu executable")?;
    render_profile_with_renderer_outcome(profile, &renderer)
}

#[cfg(test)]
fn render_profile_with_renderer(profile: &Path, renderer: &Path) -> Result<()> {
    render_profile_with_renderer_outcome(profile, renderer).map(|_| ())
}

fn render_profile_with_renderer_outcome(profile: &Path, renderer: &Path) -> Result<RenderOutcome> {
    let stack_path = profile.join("stack.toml");
    let credentials_path = profile.join("credentials.toml");
    let providers_path = profile.join("providers.toml");
    let stack_bytes =
        fs::read(&stack_path).with_context(|| format!("read `{}`", stack_path.display()))?;
    let credential_bytes = fs::read(&credentials_path)
        .with_context(|| format!("read `{}`", credentials_path.display()))?;
    let provider_bytes = fs::read(&providers_path)
        .with_context(|| format!("read `{}`", providers_path.display()))?;
    let stack: StackSource = parse_toml(&stack_path, &stack_bytes)?;
    let credentials: CredentialsSource = parse_toml(&credentials_path, &credential_bytes)?;
    let providers: ProvidersSource = parse_toml(&providers_path, &provider_bytes)?;
    validate_schema_versions(&stack, &credentials, &providers)?;
    let missing = missing_fields(&stack, &credentials, &providers);
    if !missing.is_empty() {
        bail!("incomplete stack profile:\n  - {}", missing.join("\n  - "));
    }
    validate_provider_source(&providers)?;
    validate_topology(&stack)?;
    let credential_paths = resolve_credential_paths(profile, &stack, &credentials)?;
    validate_credential_resource_separation(profile, &stack, &credential_paths)?;

    let binaries = stack.binaries.as_ref().expect("checked");
    let hubu_bin = existing_absolute(binaries.hubu.as_deref().expect("checked"), "binaries.hubu")?;
    validate_renderer_identity(renderer, &hubu_bin)?;
    let hubu_server =
        if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
            Some(existing_absolute(
                binaries.hubu_server.as_deref().expect("checked"),
                "binaries.hubu_server",
            )?)
        } else {
            None
        };
    let gongbu_server =
        if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
            Some(existing_absolute(
                binaries.gongbu_server.as_deref().expect("checked"),
                "binaries.gongbu_server",
            )?)
        } else {
            None
        };
    let unified_mcp = existing_absolute(
        binaries.hubu_unified_mcp.as_deref().expect("checked"),
        "binaries.hubu_unified_mcp",
    )?;
    let mut provenances = vec![binary_provenance("hubu", &hubu_bin)?];
    if let Some(path) = &hubu_server {
        provenances.push(binary_provenance("hubu-server", path)?);
    }
    if let Some(path) = &gongbu_server {
        provenances.push(binary_provenance("gongbu-server", path)?);
    }
    provenances.push(binary_provenance("hubu-unified-mcp", &unified_mcp)?);
    validate_release_lineage(&provenances, stack.allow_development_builds)?;
    if let Some(gongbu) = provenances
        .iter()
        .find(|provenance| provenance.component == "gongbu-server")
    {
        negotiate_principal_neutral_gongbu_schema(gongbu)?;
    }

    let source_digests = BTreeMap::from([
        ("credentials.toml".to_string(), digest(&credential_bytes)),
        ("providers.toml".to_string(), digest(&provider_bytes)),
        ("stack.toml".to_string(), digest(&stack_bytes)),
    ]);
    let generation_seed = serde_json::to_vec(&json!({
        "sources": source_digests,
        "binaries": provenances,
    }))?;
    let generation_id = digest(&generation_seed)
        .trim_start_matches("sha256:")
        .to_string();
    let generated = profile.join("generated");
    create_secure_dir(&generated)?;
    let active_path = generated.join("active-manifest.json");
    let previous_active = if active_path.exists() {
        Some(read_json::<ActiveManifest>(&active_path).context("read active generation manifest")?)
    } else {
        None
    };
    if let Some(active) = &previous_active {
        validate_manifest_integrity(&generated, active)?;
    }
    let relative_generation = PathBuf::from("generations").join(&generation_id);
    let generation = generated.join(&relative_generation);
    if let Some(active) = &previous_active {
        if active.generation_id == generation_id {
            let expected = rederive_generation(
                &generated,
                &generation,
                &stack,
                &credentials,
                &providers,
                &provenances,
                hubu_server.as_deref(),
                gongbu_server.as_deref(),
                &unified_mcp,
                &credential_paths.hubu_auth,
                &credential_paths.hubu_approval,
                &credential_paths.hubu_reconciliation,
                &credential_paths.gongbu_caller,
                &source_digests,
                &generation_id,
                &relative_generation,
                previous_active.as_ref(),
            )?;
            validate_rederived_generation(active, &expected, true)?;
            validate_generation(
                &generated,
                active,
                &stack,
                &providers,
                hubu_server.as_deref(),
                gongbu_server.as_deref(),
            )?;
            persist_generation_manifest(&generated, active)?;
            let style = crate::terminal::stdout();
            println!(
                "{}: {}",
                style.muted("render unchanged"),
                style.accent(&generation_id)
            );
            println!(
                "{}: {}",
                style.label("active manifest"),
                style.accent(active_path.display())
            );
            return Ok(RenderOutcome {
                generation_id,
                activated: true,
                changed_source_files: Vec::new(),
                affected_components: Vec::new(),
            });
        }
    }
    if generation.exists() {
        if !fs::symlink_metadata(&generation)?.file_type().is_dir() {
            bail!(
                "inactive generation path `{}` is not a real directory",
                generation.display()
            );
        }
        let manifest_path = generation.join("manifest.json");
        let staged = if manifest_path.exists() {
            let bytes = fs::read(&manifest_path)?;
            match serde_json::from_slice::<Value>(&bytes) {
                Err(_) => None,
                Ok(value) => {
                    if value
                        .get("schema_version")
                        .and_then(Value::as_u64)
                        .is_some_and(|version| version != u64::from(MANIFEST_SCHEMA_VERSION))
                    {
                        bail!("staged generation uses an unsupported manifest schema");
                    }
                    serde_json::from_slice::<ActiveManifest>(&bytes).ok()
                }
            }
        } else {
            None
        };
        if let Some(staged) = staged {
            let expected = rederive_generation(
                &generated,
                &generation,
                &stack,
                &credentials,
                &providers,
                &provenances,
                hubu_server.as_deref(),
                gongbu_server.as_deref(),
                &unified_mcp,
                &credential_paths.hubu_auth,
                &credential_paths.hubu_approval,
                &credential_paths.hubu_reconciliation,
                &credential_paths.gongbu_caller,
                &source_digests,
                &generation_id,
                &relative_generation,
                previous_active.as_ref(),
            )?;
            validate_rederived_generation(&staged, &expected, false)?;
            validate_generation(
                &generated,
                &staged,
                &stack,
                &providers,
                hubu_server.as_deref(),
                gongbu_server.as_deref(),
            )?;
            let outcome = render_outcome(previous_active.as_ref(), &staged, false);
            print_staged_plan(profile, &outcome);
            return Ok(outcome);
        }
        fs::remove_dir_all(&generation)
            .with_context(|| format!("discard incomplete generation `{generation_id}`"))?;
        let style = crate::terminal::stderr();
        eprintln!(
            "{}: {}",
            style.warning("discarded incomplete staged generation"),
            style.accent(&generation_id)
        );
    }
    create_secure_dir(&generated.join("generations"))?;
    create_secure_dir(&generation)?;

    let result = render_generation(
        &generation,
        &generation,
        &stack,
        &credentials,
        &providers,
        &provenances,
        hubu_server.as_deref(),
        gongbu_server.as_deref(),
        &unified_mcp,
        &credential_paths.hubu_auth,
        &credential_paths.hubu_approval,
        &credential_paths.hubu_reconciliation,
        &credential_paths.gongbu_caller,
        &source_digests,
        &generation_id,
        &relative_generation,
        previous_active
            .as_ref()
            .filter(|manifest| manifest.schema_version == MANIFEST_SCHEMA_VERSION),
    );
    let manifest = match result {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = fs::remove_dir_all(&generation);
            return Err(error);
        }
    };
    if let Err(error) = write_json_secure(
        &generation.join("manifest.json"),
        &serde_json::to_value(&manifest)?,
    ) {
        let _ = fs::remove_dir_all(&generation);
        return Err(error).context("persist staged generation manifest");
    }
    if previous_active.is_none() {
        lifecycle::ensure_profile_stopped(profile)?;
        activate_manifest(&generated, &manifest)?;
        let style = crate::terminal::stdout();
        println!(
            "{}: {}",
            style.success("rendered generation"),
            style.accent(&generation_id)
        );
        println!(
            "{}: {}",
            style.label("active manifest"),
            style.accent(active_path.display())
        );
        println!(
            "{}: {}",
            style.label("next"),
            style.command(format!("hubu stack doctor --profile {}", profile.display()))
        );
        return Ok(render_outcome(None, &manifest, true));
    }
    let outcome = render_outcome(previous_active.as_ref(), &manifest, false);
    print_staged_plan(profile, &outcome);
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn render_generation(
    generation: &Path,
    runtime_generation: &Path,
    stack: &StackSource,
    credentials: &CredentialsSource,
    providers: &ProvidersSource,
    provenances: &[BinaryProvenance],
    hubu_server: Option<&Path>,
    gongbu_server: Option<&Path>,
    unified_mcp: &Path,
    hubu_auth: &Path,
    hubu_approval: &Path,
    hubu_reconciliation: &Path,
    gongbu_caller_file: &Path,
    source_digests: &BTreeMap<String, String>,
    generation_id: &str,
    relative_generation: &Path,
    previous_active: Option<&ActiveManifest>,
) -> Result<ActiveManifest> {
    let hubu = stack.hubu.as_ref().expect("checked");
    let gongbu = stack.gongbu.as_ref().expect("checked");
    let hubu_endpoint = hubu.endpoint.as_ref().expect("checked");
    let gongbu_endpoint = gongbu.endpoint.as_ref().expect("checked");
    let mut generated_files = BTreeMap::<String, String>::new();

    if hubu.ownership == Some(Ownership::Managed) {
        let hubu_server = hubu_server.expect("selected managed binary");
        let credential_files = credentials.files.as_ref();
        let launch = json!({
            "schema_version": 1,
            "listen": hubu.listen.expect("checked"),
            "database_path": hubu.database_path.as_ref().expect("checked"),
            "log_file": hubu.log_file,
            "auth_token_file": hubu_auth,
            "auth_token_file_generated": credential_files.and_then(|files| files.hubu_auth.as_ref()).is_none(),
            "approval_token_file": hubu_approval,
            "approval_token_file_generated": credential_files.and_then(|files| files.hubu_approval.as_ref()).is_none(),
            "reconciliation_token_file": hubu_reconciliation,
            "reconciliation_token_file_generated": credential_files.and_then(|files| files.hubu_reconciliation.as_ref()).is_none(),
        });
        write_generated_json(
            generation,
            "hubu-launch.json",
            &launch,
            &mut generated_files,
        )?;
        validate_with_binary(hubu_server, &generation.join("hubu-launch.json"))?;
    }

    let provider_mode = providers.mode.expect("checked");
    if provider_mode == ProviderMode::Live {
        let targets = render_targets(providers, credentials)?;
        let pricing = render_pricing_catalog(providers)?;
        write_generated_json(
            generation,
            "provider-targets.json",
            &targets,
            &mut generated_files,
        )?;
        write_generated_json(
            generation,
            "pricing-catalog.json",
            &pricing,
            &mut generated_files,
        )?;
    }

    if gongbu.ownership == Some(Ownership::Managed) {
        let gongbu_server = gongbu_server.expect("selected managed binary");
        let (gongbu_hubu, gongbu_caller) = gongbu_bootstrap_references(stack, credentials)?;
        let temporal = render_temporal(stack.temporal.as_ref().expect("checked"))?;
        let gongbu_version = provenances
            .iter()
            .find(|item| item.component == "gongbu-server")
            .expect("probed");
        let gongbu_schema_version = negotiate_principal_neutral_gongbu_schema(gongbu_version)?;
        let provider_json =
            render_provider_config(providers, gongbu_schema_version, runtime_generation)?;
        let config = json!({
            "schema_version": gongbu_schema_version,
            "http": {"listen": gongbu.listen.expect("checked")},
            "state": {
                "database_path": gongbu.database_path.as_ref().expect("checked"),
                "artifact_root": gongbu.artifact_root.as_ref().expect("checked"),
            },
            "temporal": temporal,
            "hubu": {
                "endpoint": hubu_endpoint,
                "allowlisted_hosts": [],
                "expected_product_version": gongbu_version.product_version,
                "expected_executor_contract": gongbu_version.executor_contract,
                "credential_reference": &gongbu_hubu,
                "startup_policy": stack.runtime.hubu_startup_policy,
                "startup_timeout_ms": stack.runtime.hubu_startup_timeout_ms,
            },
            "authentication": {
                "bearer_credential_reference": &gongbu_caller,
            },
            "providers": provider_json,
            "artifacts": {
                "max_artifacts_per_execution": stack.runtime.max_artifacts_per_execution,
                "max_encoded_bytes": stack.runtime.max_encoded_bytes,
                "max_decoded_bytes": stack.runtime.max_decoded_bytes,
                "max_width": stack.runtime.max_width,
                "max_height": stack.runtime.max_height,
            },
            "execution": {
                "recovery_delays_seconds": stack.runtime.recovery_delays_seconds,
                "temporal_startup_timeout_ms": stack.runtime.temporal_startup_timeout_ms,
                "dependency_check_interval_ms": stack.runtime.dependency_check_interval_ms,
            },
            "logging": {"level": stack.runtime.log_level, "format": stack.runtime.log_format},
            "shutdown": {"worker_drain_timeout_ms": stack.runtime.worker_drain_timeout_ms},
        });
        write_generated_json(
            generation,
            "gongbu-server.json",
            &config,
            &mut generated_files,
        )?;
        validate_with_binary(gongbu_server, &generation.join("gongbu-server.json"))?;
    }

    let operation_state_path = runtime_generation
        .ancestors()
        .nth(3)
        .ok_or_else(|| anyhow!("generated stack path has no profile root"))?
        .join("state/hubu-unified-operations.sqlite3");
    validate_operation_state_separation(stack, &operation_state_path)?;
    let handoff = CodexHandoff {
        schema_version: 2,
        mcp_server: unified_mcp.to_path_buf(),
        hubu_endpoint: hubu_endpoint.clone(),
        hubu_token_file: hubu_auth.to_path_buf(),
        approval_token_file: hubu_approval.to_path_buf(),
        reconciliation_token_file: hubu_reconciliation.to_path_buf(),
        operation_state_path,
        gongbu_endpoint: gongbu_endpoint.clone(),
        gongbu_token_file: gongbu_caller_file.to_path_buf(),
    };
    write_generated_json(
        generation,
        "client-handoff.json",
        &serde_json::to_value(&handoff)?,
        &mut generated_files,
    )?;

    let process_log_files = expected_process_log_files(stack);
    let hubu_lifecycle_relevant = hubu.ownership == Some(Ownership::Managed)
        || previous_active.is_some_and(|manifest| {
            manifest
                .generated_file_digests
                .contains_key("hubu-launch.json")
        });
    let gongbu_lifecycle_relevant = gongbu.ownership == Some(Ownership::Managed)
        || previous_active.is_some_and(|manifest| {
            manifest
                .generated_file_digests
                .contains_key("gongbu-server.json")
        });
    let mut restart_impact = Vec::new();
    if hubu_lifecycle_relevant
        && (generated_digest_changed(previous_active, &generated_files, &["hubu-launch.json"])
            || binary_provenance_changed(previous_active, provenances, "hubu-server")
            || process_log_file_changed(previous_active, &process_log_files, "hubu-server"))
    {
        restart_impact.push("hubu-server".to_owned());
    }
    if gongbu_lifecycle_relevant
        && (generated_digest_changed(
            previous_active,
            &generated_files,
            &[
                "gongbu-server.json",
                "provider-targets.json",
                "pricing-catalog.json",
            ],
        ) || binary_provenance_changed(previous_active, provenances, "gongbu-server")
            || process_log_file_changed(previous_active, &process_log_files, "gongbu-server"))
    {
        restart_impact.push("gongbu-server".to_owned());
    }
    if generated_digest_changed(previous_active, &generated_files, &["client-handoff.json"])
        || binary_provenance_changed(previous_active, provenances, "hubu-unified-mcp")
    {
        restart_impact.push("hubu-unified-mcp-client-config".to_owned());
    }
    Ok(ActiveManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        generation_id: generation_id.to_owned(),
        generation: relative_generation.display().to_string(),
        source_schema_versions: BTreeMap::from([
            ("stack".into(), SOURCE_SCHEMA_VERSION),
            ("credentials".into(), SOURCE_SCHEMA_VERSION),
            ("providers".into(), SOURCE_SCHEMA_VERSION),
        ]),
        source_digests: source_digests.clone(),
        binary_provenance: provenances.to_vec(),
        generated_file_digests: generated_files,
        restart_impact,
        process_log_files,
    })
}

#[allow(clippy::too_many_arguments)]
fn rederive_generation(
    generated: &Path,
    generation: &Path,
    stack: &StackSource,
    credentials: &CredentialsSource,
    providers: &ProvidersSource,
    provenances: &[BinaryProvenance],
    hubu_server: Option<&Path>,
    gongbu_server: Option<&Path>,
    unified_mcp: &Path,
    hubu_auth: &Path,
    hubu_approval: &Path,
    hubu_reconciliation: &Path,
    gongbu_caller_file: &Path,
    source_digests: &BTreeMap<String, String>,
    generation_id: &str,
    relative_generation: &Path,
    previous_active: Option<&ActiveManifest>,
) -> Result<ActiveManifest> {
    let candidate = generated.join(format!("verification-{}", Uuid::new_v4()));
    create_secure_dir(&candidate)?;
    let result = render_generation(
        &candidate,
        generation,
        stack,
        credentials,
        providers,
        provenances,
        hubu_server,
        gongbu_server,
        unified_mcp,
        hubu_auth,
        hubu_approval,
        hubu_reconciliation,
        gongbu_caller_file,
        source_digests,
        generation_id,
        relative_generation,
        previous_active,
    );
    let cleanup = fs::remove_dir_all(&candidate)
        .with_context(|| format!("remove verification candidate `{}`", candidate.display()));
    match (result, cleanup) {
        (Ok(manifest), Ok(())) => Ok(manifest),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn validate_rederived_generation(
    stored: &ActiveManifest,
    expected: &ActiveManifest,
    allow_legacy_source_schema_metadata: bool,
) -> Result<()> {
    let source_schemas_match = stored.source_schema_versions == expected.source_schema_versions
        || (allow_legacy_source_schema_metadata && stored.source_schema_versions.is_empty());
    if stored.schema_version != expected.schema_version
        || stored.generation_id != expected.generation_id
        || stored.generation != expected.generation
        || !source_schemas_match
        || stored.source_digests != expected.source_digests
        || stored.binary_provenance != expected.binary_provenance
        || stored.generated_file_digests != expected.generated_file_digests
        || stored.process_log_files != expected.process_log_files
    {
        bail!("stored generation does not match artifacts re-derived from current inputs");
    }
    Ok(())
}

fn validate_generation(
    generated: &Path,
    manifest: &ActiveManifest,
    stack: &StackSource,
    providers: &ProvidersSource,
    hubu_server: Option<&Path>,
    gongbu_server: Option<&Path>,
) -> Result<()> {
    validate_manifest_integrity(generated, manifest)?;
    let expected_source_schemas = BTreeMap::from([
        ("stack".into(), SOURCE_SCHEMA_VERSION),
        ("credentials".into(), SOURCE_SCHEMA_VERSION),
        ("providers".into(), SOURCE_SCHEMA_VERSION),
    ]);
    if !manifest.source_schema_versions.is_empty()
        && manifest.source_schema_versions != expected_source_schemas
    {
        bail!("generation manifest has incompatible source schema metadata");
    }
    if manifest.process_log_files != expected_process_log_files(stack) {
        bail!("generation manifest does not match the configured process log paths");
    }
    let generation = active_generation_path(generated, manifest)?;
    let expected = expected_generated_files(stack, providers);
    if manifest.generated_file_digests.len() != expected.len() {
        bail!("generation manifest does not describe the complete generated artifact set");
    }
    for name in expected {
        verify_generated_file(&generation, manifest, name)?;
    }
    if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        validate_with_binary(
            hubu_server.ok_or_else(|| anyhow!("managed Hubu binary is unavailable"))?,
            &generation.join("hubu-launch.json"),
        )?;
    }
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        validate_with_binary(
            gongbu_server.ok_or_else(|| anyhow!("managed Gongbu binary is unavailable"))?,
            &generation.join("gongbu-server.json"),
        )?;
    }
    Ok(())
}

fn expected_process_log_files(stack: &StackSource) -> BTreeMap<String, Option<PathBuf>> {
    let mut values = BTreeMap::new();
    if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        values.insert(
            "hubu-server".to_string(),
            stack.hubu.as_ref().and_then(|value| value.log_file.clone()),
        );
    }
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        values.insert(
            "gongbu-server".to_string(),
            stack
                .gongbu
                .as_ref()
                .and_then(|value| value.log_file.clone()),
        );
    }
    values
}

fn validate_manifest_integrity(generated: &Path, manifest: &ActiveManifest) -> Result<()> {
    let generation = active_generation_path(generated, manifest)?;
    if !manifest
        .generated_file_digests
        .contains_key("client-handoff.json")
    {
        bail!("generation manifest does not authenticate its client handoff");
    }
    for name in manifest.generated_file_digests.keys() {
        verify_generated_file(&generation, manifest, name)?;
    }
    Ok(())
}

fn persist_generation_manifest(generated: &Path, manifest: &ActiveManifest) -> Result<()> {
    let generation = active_generation_path(generated, manifest)?;
    let path = generation.join("manifest.json");
    if path.exists() {
        let stored: ActiveManifest = read_json(&path)?;
        validate_rederived_generation(&stored, manifest, true)
            .context("stored generation manifest does not match the authenticated generation")?;
        return Ok(());
    }
    write_json_secure(&path, &serde_json::to_value(manifest)?)
}

fn render_outcome(
    previous: Option<&ActiveManifest>,
    target: &ActiveManifest,
    activated: bool,
) -> RenderOutcome {
    let changed_source_files = target
        .source_digests
        .iter()
        .filter(|(name, value)| {
            previous.and_then(|manifest| manifest.source_digests.get(*name)) != Some(*value)
        })
        .map(|(name, _)| name.clone())
        .collect();
    RenderOutcome {
        generation_id: target.generation_id.clone(),
        activated,
        changed_source_files,
        affected_components: restart_impact_between(previous, target),
    }
}

fn restart_impact_between(
    previous: Option<&ActiveManifest>,
    target: &ActiveManifest,
) -> Vec<String> {
    let hubu_relevant = target
        .generated_file_digests
        .contains_key("hubu-launch.json")
        || previous.is_some_and(|value| {
            value
                .generated_file_digests
                .contains_key("hubu-launch.json")
        });
    let gongbu_relevant = target
        .generated_file_digests
        .contains_key("gongbu-server.json")
        || previous.is_some_and(|value| {
            value
                .generated_file_digests
                .contains_key("gongbu-server.json")
        });
    let mut impact = Vec::new();
    if hubu_relevant
        && (generated_digest_changed(
            previous,
            &target.generated_file_digests,
            &["hubu-launch.json"],
        ) || binary_provenance_changed(previous, &target.binary_provenance, "hubu-server")
            || process_log_file_changed(previous, &target.process_log_files, "hubu-server"))
    {
        impact.push("hubu-server".into());
    }
    if gongbu_relevant
        && (generated_digest_changed(
            previous,
            &target.generated_file_digests,
            &[
                "gongbu-server.json",
                "provider-targets.json",
                "pricing-catalog.json",
            ],
        ) || binary_provenance_changed(previous, &target.binary_provenance, "gongbu-server")
            || process_log_file_changed(previous, &target.process_log_files, "gongbu-server"))
    {
        impact.push("gongbu-server".into());
    }
    if generated_digest_changed(
        previous,
        &target.generated_file_digests,
        &["client-handoff.json"],
    ) || binary_provenance_changed(previous, &target.binary_provenance, "hubu-unified-mcp")
    {
        impact.push("hubu-unified-mcp-client-config".into());
    }
    impact
}

fn print_staged_plan(profile: &Path, outcome: &RenderOutcome) {
    let style = crate::terminal::stdout();
    println!(
        "{}: {}",
        style.success("validated staged generation"),
        style.accent(&outcome.generation_id)
    );
    println!(
        "{}: {}",
        style.label("changed source files"),
        if outcome.changed_source_files.is_empty() {
            style.muted("none")
        } else {
            style.warning(outcome.changed_source_files.join(", "))
        }
    );
    println!(
        "{}: {}",
        style.label("affected components"),
        if outcome.affected_components.is_empty() {
            style.muted("none")
        } else {
            style.warning(outcome.affected_components.join(", "))
        }
    );
    println!(
        "{}: {}",
        style.warning("activation required"),
        style.command(format!(
            "hubu stack activate --generation {} --profile {}",
            outcome.generation_id,
            profile.display()
        ))
    );
}

fn activate_manifest(generated: &Path, manifest: &ActiveManifest) -> Result<()> {
    persist_generation_manifest(generated, manifest)?;
    let active_path = generated.join("active-manifest.json");
    if active_path.exists() {
        let previous = read_json::<ActiveManifest>(&active_path)
            .context("read active generation manifest before activation")?;
        persist_generation_manifest(generated, &previous)?;
    }
    let temp = active_path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    write_json_secure(&temp, &serde_json::to_value(manifest)?)?;
    if let Err(error) = fs::rename(&temp, &active_path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("activate `{}`", active_path.display()));
    }
    Ok(())
}

fn render_provider_config(
    providers: &ProvidersSource,
    gongbu_schema_version: u32,
    generation: &Path,
) -> Result<Value> {
    match (providers.mode.expect("checked"), gongbu_schema_version) {
        (ProviderMode::Disabled, version) if version < 2 => {
            bail!("disabled provider mode requires a Gongbu binary with server config schema version 2")
        }
        (ProviderMode::Disabled, _) => Ok(json!({"mode": "disabled"})),
        (ProviderMode::Live, 1) => Ok(json!({
            "target_catalog_path": generation.join("provider-targets.json"),
            "pricing_catalog_path": generation.join("pricing-catalog.json"),
            "maximum_spend_minor": providers.maximum_spend_minor.expect("checked"),
            "live_spend_acknowledgement": providers.live_spend_acknowledgement.as_ref().expect("checked"),
        })),
        (ProviderMode::Live, _) => Ok(json!({
            "mode": "live",
            "target_catalog_path": generation.join("provider-targets.json"),
            "pricing_catalog_path": generation.join("pricing-catalog.json"),
            "maximum_spend_minor": providers.maximum_spend_minor.expect("checked"),
            "live_spend_acknowledgement": providers.live_spend_acknowledgement.as_ref().expect("checked"),
        })),
    }
}

fn render_pricing_catalog(providers: &ProvidersSource) -> Result<Value> {
    Ok(json!({
        "schema_version": 2,
        "catalog_version": providers.catalog_version.as_ref().expect("checked"),
        "rules": providers.pricing_rules.iter().map(toml_to_json).collect::<Result<Vec<_>>>()?,
    }))
}

fn generated_digest_changed(
    previous: Option<&ActiveManifest>,
    current: &BTreeMap<String, String>,
    names: &[&str],
) -> bool {
    let Some(previous) = previous else {
        return names.iter().any(|name| current.contains_key(*name));
    };
    names
        .iter()
        .any(|name| previous.generated_file_digests.get(*name) != current.get(*name))
}

fn binary_provenance_changed(
    previous: Option<&ActiveManifest>,
    current: &[BinaryProvenance],
    component: &str,
) -> bool {
    let current = current.iter().find(|item| item.component == component);
    let previous = previous.and_then(|manifest| {
        manifest
            .binary_provenance
            .iter()
            .find(|item| item.component == component)
    });
    previous != current
}

fn process_log_file_changed(
    previous: Option<&ActiveManifest>,
    current: &BTreeMap<String, Option<PathBuf>>,
    component: &str,
) -> bool {
    previous.and_then(|manifest| manifest.process_log_files.get(component))
        != current.get(component)
}

fn expected_generated_files(stack: &StackSource, providers: &ProvidersSource) -> Vec<&'static str> {
    let mut files = vec!["client-handoff.json"];
    if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        files.push("hubu-launch.json");
    }
    if providers.mode == Some(ProviderMode::Live) {
        files.extend(["provider-targets.json", "pricing-catalog.json"]);
    }
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        files.push("gongbu-server.json");
    }
    files
}

fn render_targets(providers: &ProvidersSource, credentials: &CredentialsSource) -> Result<Value> {
    let configs = providers
        .targets
        .iter()
        .map(|target| {
            let credential = target.credential.as_ref().expect("checked");
            let secret = credentials.opaque.get(credential).ok_or_else(|| {
                anyhow!(
                    "providers.toml target references unknown credentials.toml opaque key `{}`",
                    credential
                )
            })?;
            Ok(json!({
                "provider_config_version": target.provider_config_version.as_ref().expect("checked"),
                "workload_type": target.workload_type.as_ref().expect("checked"),
                "provider": target.provider.as_ref().expect("checked"),
                "adapter": target.adapter.as_ref().expect("checked"),
                "model": target.model.as_ref().expect("checked"),
                "secret_service": secret.service.as_ref().expect("checked"),
                "secret_account": secret.account.as_ref().expect("checked"),
                "active": target.active.expect("checked"),
                "execution_enabled": target.execution_enabled.expect("checked"),
                "settings": toml_to_json(target.settings.as_ref().expect("checked"))?,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"schema_version": 2, "provider_configs": configs}))
}

fn render_temporal(source: &TemporalSource) -> Result<Value> {
    match source.mode.expect("checked") {
        TemporalMode::ManagedLocal => Ok(json!({
            "mode": "managed_local",
            "binary_path": source.binary_path.as_ref().expect("checked"),
            "expected_cli_version": source.expected_cli_version.as_ref().expect("checked"),
            "data_path": source.data_path.as_ref().expect("checked"),
            "rpc_port": source.rpc_port.expect("checked"),
            "ui_port": source.ui_port.expect("checked"),
            "namespace": source.namespace.as_ref().expect("checked"),
            "task_queue": source.task_queue.as_ref().expect("checked"),
            "ui_url": source.ui_url,
        })),
        TemporalMode::External => Ok(json!({
            "mode": "external",
            "address": source.address.as_ref().expect("checked"),
            "namespace": source.namespace.as_ref().expect("checked"),
            "task_queue": source.task_queue.as_ref().expect("checked"),
            "ui_url": source.ui_url,
        })),
    }
}

fn validate_provider_source(source: &ProvidersSource) -> Result<()> {
    match source.mode.expect("checked") {
        ProviderMode::Disabled => {
            if source.catalog_version.is_some()
                || source.maximum_spend_minor.is_some()
                || source.live_spend_acknowledgement.is_some()
                || !source.targets.is_empty()
                || !source.pricing_rules.is_empty()
            {
                bail!("providers.toml disabled mode must omit catalogs, targets, prices, and live-spend fields");
            }
        }
        ProviderMode::Live => {
            if source.maximum_spend_minor.is_none_or(|value| value <= 0)
                || source.live_spend_acknowledgement.as_deref() != Some(LIVE_SPEND_ACKNOWLEDGEMENT)
            {
                bail!("providers.toml live mode requires a positive maximum_spend_minor and the exact Gongbu live-spend acknowledgement");
            }
        }
    }
    Ok(())
}

fn validate_topology(stack: &StackSource) -> Result<()> {
    let hubu = stack.hubu.as_ref().expect("checked");
    let gongbu = stack.gongbu.as_ref().expect("checked");
    validate_loopback_endpoint(hubu.endpoint.as_deref().expect("checked"), "hubu.endpoint")?;
    validate_loopback_endpoint(
        gongbu.endpoint.as_deref().expect("checked"),
        "gongbu.endpoint",
    )?;
    if hubu.ownership == Some(Ownership::Managed) {
        validate_endpoint_matches_listen(
            hubu.endpoint.as_deref().expect("checked"),
            hubu.listen.expect("checked"),
            "hubu",
        )?;
    }
    if gongbu.ownership == Some(Ownership::Managed) {
        validate_endpoint_matches_listen(
            gongbu.endpoint.as_deref().expect("checked"),
            gongbu.listen.expect("checked"),
            "gongbu",
        )?;
    }
    validate_managed_ports(stack)?;
    validate_managed_resources(stack)?;
    Ok(())
}

fn validate_managed_ports(stack: &StackSource) -> Result<()> {
    let mut sockets = Vec::new();
    if let Some(hubu) = stack
        .hubu
        .as_ref()
        .filter(|service| service.ownership == Some(Ownership::Managed))
    {
        if let Some(listen) = hubu.listen {
            sockets.push(("stack.toml:hubu.listen", listen));
        }
    }
    if let Some(gongbu) = stack
        .gongbu
        .as_ref()
        .filter(|service| service.ownership == Some(Ownership::Managed))
    {
        if let Some(listen) = gongbu.listen {
            sockets.push(("stack.toml:gongbu.listen", listen));
        }
    }
    for index in 0..sockets.len() {
        for other in &sockets[index + 1..] {
            if sockets[index].1 == other.1 {
                bail!(
                    "{} and {} must be distinct managed sockets",
                    sockets[index].0,
                    other.0
                );
            }
        }
    }

    let Some(temporal) = stack
        .temporal
        .as_ref()
        .filter(|temporal| temporal.mode == Some(TemporalMode::ManagedLocal))
    else {
        return Ok(());
    };
    let temporal_ports = [
        ("stack.toml:temporal.rpc_port", temporal.rpc_port),
        ("stack.toml:temporal.ui_port", temporal.ui_port),
    ];
    if temporal_ports[0].1.is_some() && temporal_ports[0].1 == temporal_ports[1].1 {
        bail!("stack.toml:temporal.rpc_port and temporal.ui_port must be distinct managed ports");
    }
    for (field, port) in temporal_ports {
        let Some(port) = port else { continue };
        for (socket_field, socket) in &sockets {
            if socket.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                && socket.port() == port
            {
                bail!("{field} conflicts with managed socket {socket_field}");
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ManagedResourceKind {
    File,
    Directory,
}

fn validate_managed_resources(stack: &StackSource) -> Result<()> {
    let mut resources = Vec::new();
    if let Some(hubu) = stack
        .hubu
        .as_ref()
        .filter(|service| service.ownership == Some(Ownership::Managed))
    {
        push_resource(
            &mut resources,
            "stack.toml:hubu.database_path",
            hubu.database_path.as_deref(),
            ManagedResourceKind::File,
        );
        push_resource(
            &mut resources,
            "stack.toml:hubu.log_file",
            hubu.log_file.as_deref(),
            ManagedResourceKind::File,
        );
    }
    if let Some(gongbu) = stack
        .gongbu
        .as_ref()
        .filter(|service| service.ownership == Some(Ownership::Managed))
    {
        push_resource(
            &mut resources,
            "stack.toml:gongbu.database_path",
            gongbu.database_path.as_deref(),
            ManagedResourceKind::File,
        );
        push_resource(
            &mut resources,
            "stack.toml:gongbu.artifact_root",
            gongbu.artifact_root.as_deref(),
            ManagedResourceKind::Directory,
        );
        push_resource(
            &mut resources,
            "stack.toml:gongbu.log_file",
            gongbu.log_file.as_deref(),
            ManagedResourceKind::File,
        );
    }
    if let Some(temporal) = stack
        .temporal
        .as_ref()
        .filter(|temporal| temporal.mode == Some(TemporalMode::ManagedLocal))
    {
        push_resource(
            &mut resources,
            "stack.toml:temporal.data_path",
            temporal.data_path.as_deref(),
            ManagedResourceKind::Directory,
        );
    }

    for index in 0..resources.len() {
        for other in &resources[index + 1..] {
            if managed_resources_overlap(&resources[index], other)? {
                bail!(
                    "{} and {} must not overlap managed resources",
                    resources[index].0,
                    other.0
                );
            }
        }
    }
    Ok(())
}

fn validate_credential_resource_separation(
    profile: &Path,
    stack: &StackSource,
    paths: &ResolvedCredentialPaths,
) -> Result<()> {
    let mut backend_resources = Vec::new();
    if let Some(hubu) = stack
        .hubu
        .as_ref()
        .filter(|service| service.ownership == Some(Ownership::Managed))
    {
        push_resource(
            &mut backend_resources,
            "stack.toml:hubu.database_path",
            hubu.database_path.as_deref(),
            ManagedResourceKind::File,
        );
        push_resource(
            &mut backend_resources,
            "stack.toml:hubu.log_file",
            hubu.log_file.as_deref(),
            ManagedResourceKind::File,
        );
    }
    if let Some(gongbu) = stack
        .gongbu
        .as_ref()
        .filter(|service| service.ownership == Some(Ownership::Managed))
    {
        push_resource(
            &mut backend_resources,
            "stack.toml:gongbu.database_path",
            gongbu.database_path.as_deref(),
            ManagedResourceKind::File,
        );
        push_resource(
            &mut backend_resources,
            "stack.toml:gongbu.artifact_root",
            gongbu.artifact_root.as_deref(),
            ManagedResourceKind::Directory,
        );
        push_resource(
            &mut backend_resources,
            "stack.toml:gongbu.log_file",
            gongbu.log_file.as_deref(),
            ManagedResourceKind::File,
        );
    }
    if let Some(temporal) = stack
        .temporal
        .as_ref()
        .filter(|temporal| temporal.mode == Some(TemporalMode::ManagedLocal))
    {
        push_resource(
            &mut backend_resources,
            "stack.toml:temporal.data_path",
            temporal.data_path.as_deref(),
            ManagedResourceKind::Directory,
        );
    }

    let credential_root = managed_credential_root(profile);
    let mut credential_resources = vec![
        (
            "Hubu authentication credential",
            paths.hubu_auth.as_path(),
            ManagedResourceKind::File,
        ),
        (
            "Hubu approval credential",
            paths.hubu_approval.as_path(),
            ManagedResourceKind::File,
        ),
        (
            "Hubu reconciliation credential",
            paths.hubu_reconciliation.as_path(),
            ManagedResourceKind::File,
        ),
        (
            "Gongbu caller credential",
            paths.gongbu_caller.as_path(),
            ManagedResourceKind::File,
        ),
    ];
    let has_managed_credentials = stack.hubu.as_ref().and_then(|value| value.ownership)
        == Some(Ownership::Managed)
        || paths.managed_gongbu_handoff;
    if has_managed_credentials {
        credential_resources.push((
            "managed credential state",
            credential_root.as_path(),
            ManagedResourceKind::Directory,
        ));
    }

    for credential in &credential_resources {
        for backend in &backend_resources {
            if managed_resources_overlap(credential, backend)? {
                bail!(
                    "{} and {} must not overlap credential and managed service state",
                    credential.0,
                    backend.0
                );
            }
        }
    }
    let operation_state = profile.join("state/hubu-unified-operations.sqlite3");
    let operation = (
        "generated client handoff operation_state_path",
        operation_state.as_path(),
        ManagedResourceKind::File,
    );
    for credential in &credential_resources {
        if managed_resources_overlap(credential, &operation)? {
            bail!(
                "{} and {} must not overlap credential and unified MCP state",
                credential.0,
                operation.0
            );
        }
    }
    Ok(())
}

fn validate_operation_state_separation(stack: &StackSource, operation_state: &Path) -> Result<()> {
    let operation = (
        "generated client handoff operation_state_path",
        operation_state,
        ManagedResourceKind::File,
    );
    let mut backend_resources = Vec::new();
    if let Some(hubu) = stack.hubu.as_ref() {
        push_resource(
            &mut backend_resources,
            "stack.toml:hubu.database_path",
            hubu.database_path.as_deref(),
            ManagedResourceKind::File,
        );
    }
    if let Some(gongbu) = stack.gongbu.as_ref() {
        push_resource(
            &mut backend_resources,
            "stack.toml:gongbu.database_path",
            gongbu.database_path.as_deref(),
            ManagedResourceKind::File,
        );
        push_resource(
            &mut backend_resources,
            "stack.toml:gongbu.artifact_root",
            gongbu.artifact_root.as_deref(),
            ManagedResourceKind::Directory,
        );
    }
    for backend in backend_resources {
        if managed_resources_overlap(&operation, &backend)? {
            bail!(
                "{} and {} must not overlap; unified MCP, Hubu, and Gongbu state remain separate",
                operation.0,
                backend.0
            );
        }
    }
    Ok(())
}

fn push_resource<'a>(
    resources: &mut Vec<(&'static str, &'a Path, ManagedResourceKind)>,
    field: &'static str,
    path: Option<&'a Path>,
    kind: ManagedResourceKind,
) {
    if let Some(path) = path {
        resources.push((field, path, kind));
    }
}

fn managed_resources_overlap(
    left: &(&str, &Path, ManagedResourceKind),
    right: &(&str, &Path, ManagedResourceKind),
) -> Result<bool> {
    if paths_resolve_to_same_resource(left.1, right.1)? {
        return Ok(true);
    }
    if matches!(left.2, ManagedResourceKind::Directory)
        || matches!(right.2, ManagedResourceKind::Directory)
    {
        let left = resolve_existing_prefix(left.1)?;
        let right = resolve_existing_prefix(right.1)?;
        return Ok(left.starts_with(&right) || right.starts_with(&left));
    }
    Ok(false)
}

fn validate_renderer_identity(renderer: &Path, configured_hubu: &Path) -> Result<()> {
    let renderer = fs::canonicalize(renderer).with_context(|| {
        format!(
            "canonicalize running hubu executable `{}`",
            renderer.display()
        )
    })?;
    let configured_hubu = fs::canonicalize(configured_hubu).with_context(|| {
        format!(
            "canonicalize configured hubu executable `{}`",
            configured_hubu.display()
        )
    })?;
    if renderer != configured_hubu {
        bail!(
            "stack.toml:binaries.hubu must identify the running hubu executable `{}`",
            renderer.display()
        );
    }
    Ok(())
}

fn paths_resolve_to_same_resource(left: &Path, right: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        if left.exists() && right.exists() {
            let left_metadata = fs::metadata(left)
                .with_context(|| format!("read metadata for `{}`", left.display()))?;
            let right_metadata = fs::metadata(right)
                .with_context(|| format!("read metadata for `{}`", right.display()))?;
            use std::os::unix::fs::MetadataExt;
            if left_metadata.dev() == right_metadata.dev()
                && left_metadata.ino() == right_metadata.ino()
            {
                return Ok(true);
            }
        }
    }
    Ok(resolve_existing_prefix(left)? == resolve_existing_prefix(right)?)
}

fn resolve_existing_prefix(path: &Path) -> Result<PathBuf> {
    validate_safe_absolute(path, "managed resource path")?;
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| anyhow!("managed resource path has no existing ancestor"))?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| anyhow!("managed resource path has no existing ancestor"))?;
    }
    let mut resolved = fs::canonicalize(existing)
        .with_context(|| format!("canonicalize `{}`", existing.display()))?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn validate_loopback_endpoint(endpoint: &str, field: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(endpoint).with_context(|| format!("invalid {field}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("{field} has no host"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "http"
        || !loopback
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        bail!("{field} must be an explicit loopback http:// origin");
    }
    Ok(url)
}

fn validate_endpoint_matches_listen(endpoint: &str, listen: SocketAddr, name: &str) -> Result<()> {
    let url = validate_loopback_endpoint(endpoint, &format!("{name}.endpoint"))?;
    let host_matches = match url.host_str().expect("validated URL host") {
        host if host.eq_ignore_ascii_case("localhost") => {
            listen.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        host => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address == listen.ip()),
    };
    if !host_matches || url.port_or_known_default() != Some(listen.port()) {
        bail!("stack.toml:{name}.endpoint host and port must match {name}.listen");
    }
    Ok(())
}

fn validate_schema_versions(
    stack: &StackSource,
    credentials: &CredentialsSource,
    providers: &ProvidersSource,
) -> Result<()> {
    if stack.schema_version != SOURCE_SCHEMA_VERSION
        || credentials.schema_version != SOURCE_SCHEMA_VERSION
        || providers.schema_version != SOURCE_SCHEMA_VERSION
    {
        bail!("unsupported stack source schema_version");
    }
    Ok(())
}

fn missing_fields(
    stack: &StackSource,
    credentials: &CredentialsSource,
    providers: &ProvidersSource,
) -> Vec<String> {
    let mut missing = Vec::new();
    if let Some(binaries) = stack.binaries.as_ref() {
        for (value, path, required) in [
            (binaries.hubu.as_ref(), "stack.toml:binaries.hubu", true),
            (
                binaries.hubu_server.as_ref(),
                "stack.toml:binaries.hubu_server",
                stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed),
            ),
            (
                binaries.gongbu_server.as_ref(),
                "stack.toml:binaries.gongbu_server",
                stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed),
            ),
            (
                binaries.hubu_unified_mcp.as_ref(),
                "stack.toml:binaries.hubu_unified_mcp",
                true,
            ),
        ] {
            if required && value.is_none() {
                missing.push(path.into());
            }
        }
    } else {
        missing.push("stack.toml:binaries".into());
    }
    check_service_missing(stack.hubu.as_ref(), "hubu", &mut missing);
    check_gongbu_missing(stack.gongbu.as_ref(), &mut missing);
    if stack.gongbu.as_ref().and_then(|v| v.ownership) == Some(Ownership::Managed) {
        check_temporal_missing(stack.temporal.as_ref(), &mut missing);
    }
    let files = credentials.files.as_ref();
    let hubu_external =
        stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::External);
    for (value, path) in [
        (
            files.and_then(|value| value.hubu_auth.as_ref()),
            "credentials.toml:files.hubu_auth",
        ),
        (
            files.and_then(|value| value.hubu_approval.as_ref()),
            "credentials.toml:files.hubu_approval",
        ),
        (
            files.and_then(|value| value.hubu_reconciliation.as_ref()),
            "credentials.toml:files.hubu_reconciliation",
        ),
    ] {
        if hubu_external && value.is_none() {
            missing.push(path.into());
        }
    }
    let gongbu_ownership = stack.gongbu.as_ref().and_then(|value| value.ownership);
    let has_gongbu_hubu_reference = credentials.opaque.contains_key("gongbu_hubu");
    let has_gongbu_caller_reference = credentials.opaque.contains_key("gongbu_caller");
    let explicit_gongbu_bootstrap = has_gongbu_hubu_reference || has_gongbu_caller_reference;
    if (gongbu_ownership == Some(Ownership::External)
        || (gongbu_ownership == Some(Ownership::Managed) && explicit_gongbu_bootstrap))
        && files
            .and_then(|value| value.gongbu_caller.as_ref())
            .is_none()
    {
        missing.push("credentials.toml:files.gongbu_caller".into());
    }
    if gongbu_ownership == Some(Ownership::Managed) && explicit_gongbu_bootstrap {
        for (key, present) in [
            ("gongbu_hubu", has_gongbu_hubu_reference),
            ("gongbu_caller", has_gongbu_caller_reference),
        ] {
            if !present {
                missing.push(format!("credentials.toml:opaque.{key}"));
            }
        }
    }
    for (key, reference) in &credentials.opaque {
        if reference.service.as_deref().is_none_or(str::is_empty) {
            missing.push(format!("credentials.toml:opaque.{key}.service"));
        }
        if reference.account.as_deref().is_none_or(str::is_empty) {
            missing.push(format!("credentials.toml:opaque.{key}.account"));
        }
    }
    match providers.mode {
        None => missing.push("providers.toml:mode".into()),
        Some(ProviderMode::Disabled) => {}
        Some(ProviderMode::Live) => {
            if providers
                .catalog_version
                .as_deref()
                .is_none_or(str::is_empty)
            {
                missing.push("providers.toml:catalog_version".into());
            }
            if providers.maximum_spend_minor.is_none() {
                missing.push("providers.toml:maximum_spend_minor".into());
            }
            if providers.live_spend_acknowledgement.is_none() {
                missing.push("providers.toml:live_spend_acknowledgement".into());
            }
            if providers.targets.is_empty() {
                missing.push("providers.toml:targets".into());
            }
            for (index, target) in providers.targets.iter().enumerate() {
                for (value, field) in [
                    (
                        target.provider_config_version.as_ref(),
                        "provider_config_version",
                    ),
                    (target.workload_type.as_ref(), "workload_type"),
                    (target.provider.as_ref(), "provider"),
                    (target.adapter.as_ref(), "adapter"),
                    (target.model.as_ref(), "model"),
                    (target.credential.as_ref(), "credential"),
                ] {
                    if value.is_none_or(|value| value.is_empty()) {
                        missing.push(format!("providers.toml:targets[{index}].{field}"));
                    }
                }
                if target.active.is_none() {
                    missing.push(format!("providers.toml:targets[{index}].active"));
                }
                if target.execution_enabled.is_none() {
                    missing.push(format!("providers.toml:targets[{index}].execution_enabled"));
                }
                if target.settings.is_none() {
                    missing.push(format!("providers.toml:targets[{index}].settings"));
                }
                if let Some(key) = target.credential.as_deref().filter(|key| !key.is_empty()) {
                    let field = format!("credentials.toml:opaque.{key}");
                    if !credentials.opaque.contains_key(key) && !missing.contains(&field) {
                        missing.push(field);
                    }
                }
            }
            if providers.pricing_rules.is_empty() {
                missing.push("providers.toml:pricing_rules".into());
            }
        }
    }
    missing
}

fn check_service_missing(source: Option<&ServiceSource>, name: &str, missing: &mut Vec<String>) {
    let Some(source) = source else {
        missing.push(format!("stack.toml:{name}"));
        return;
    };
    if source.ownership.is_none() {
        missing.push(format!("stack.toml:{name}.ownership"));
    }
    if source.endpoint.is_none() {
        missing.push(format!("stack.toml:{name}.endpoint"));
    }
    if source.ownership == Some(Ownership::Managed) {
        if source.listen.is_none() {
            missing.push(format!("stack.toml:{name}.listen"));
        }
        if source.database_path.is_none() {
            missing.push(format!("stack.toml:{name}.database_path"));
        }
    }
}

fn check_gongbu_missing(source: Option<&GongbuSource>, missing: &mut Vec<String>) {
    let Some(source) = source else {
        missing.push("stack.toml:gongbu".into());
        return;
    };
    if source.ownership.is_none() {
        missing.push("stack.toml:gongbu.ownership".into());
    }
    if source.endpoint.is_none() {
        missing.push("stack.toml:gongbu.endpoint".into());
    }
    if source.ownership == Some(Ownership::Managed) {
        if source.listen.is_none() {
            missing.push("stack.toml:gongbu.listen".into());
        }
        if source.database_path.is_none() {
            missing.push("stack.toml:gongbu.database_path".into());
        }
        if source.artifact_root.is_none() {
            missing.push("stack.toml:gongbu.artifact_root".into());
        }
    }
}

fn check_temporal_missing(source: Option<&TemporalSource>, missing: &mut Vec<String>) {
    let Some(source) = source else {
        missing.push("stack.toml:temporal".into());
        return;
    };
    if source.mode.is_none() {
        missing.push("stack.toml:temporal.mode".into());
        return;
    }
    for (value, path) in [
        (source.namespace.as_ref(), "stack.toml:temporal.namespace"),
        (source.task_queue.as_ref(), "stack.toml:temporal.task_queue"),
    ] {
        if value.is_none() {
            missing.push(path.into());
        }
    }
    match source.mode {
        Some(TemporalMode::ManagedLocal) => {
            if source.binary_path.is_none() {
                missing.push("stack.toml:temporal.binary_path".into());
            }
            if source.expected_cli_version.is_none() {
                missing.push("stack.toml:temporal.expected_cli_version".into());
            }
            if source.data_path.is_none() {
                missing.push("stack.toml:temporal.data_path".into());
            }
            if source.rpc_port.is_none() {
                missing.push("stack.toml:temporal.rpc_port".into());
            }
            if source.ui_port.is_none() {
                missing.push("stack.toml:temporal.ui_port".into());
            }
        }
        Some(TemporalMode::External) if source.address.is_none() => {
            missing.push("stack.toml:temporal.address".into());
        }
        _ => {}
    }
}

fn binary_provenance(component: &str, path: &Path) -> Result<BinaryProvenance> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("run `{}` --version", path.display()))?;
    if !output.status.success() {
        bail!("`{}` --version failed", path.display());
    }
    binary_provenance_from_output(component, path, &output.stdout)
}

fn binary_provenance_from_output(
    component: &str,
    path: &Path,
    stdout: &[u8],
) -> Result<BinaryProvenance> {
    let value: Value = serde_json::from_slice(stdout)
        .with_context(|| format!("parse safe version output from `{}`", path.display()))?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("`{}` version output is missing `{name}`", path.display()))
    };
    Ok(BinaryProvenance {
        component: component.to_owned(),
        path: path.to_path_buf(),
        product_version: field("product_version")?,
        source_commit: field("source_commit")?,
        executor_contract: value
            .get("executor_contract")
            .or_else(|| value.get("hubu_executor_contract"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                anyhow!(
                    "`{}` version output has no executor contract",
                    path.display()
                )
            })?,
        server_config_schema_version: value
            .get("server_config_schema_version")
            .and_then(Value::as_u64)
            .map(|value| value as u32),
    })
}

fn validate_release_lineage(items: &[BinaryProvenance], allow_development: bool) -> Result<()> {
    let first = items
        .first()
        .ok_or_else(|| anyhow!("no binaries selected"))?;
    if items.iter().any(|item| {
        item.product_version != first.product_version
            || item.source_commit != first.source_commit
            || item.executor_contract != first.executor_contract
    }) {
        bail!("selected stack binaries do not share product, source, and executor-contract provenance");
    }
    if first.source_commit == "unknown" && !allow_development {
        bail!("selected binaries are unstamped development builds; set allow_development_builds = true only for an explicit local development profile");
    }
    if first.source_commit == "unknown" {
        let style = crate::terminal::stderr();
        eprintln!(
            "{}: rendering an explicit development profile with unstamped binaries",
            style.warning("warning")
        );
    }
    Ok(())
}

fn negotiate_principal_neutral_gongbu_schema(provenance: &BinaryProvenance) -> Result<u32> {
    match provenance.server_config_schema_version {
        Some(version) if version >= PRINCIPAL_NEUTRAL_GONGBU_SCHEMA_VERSION => Ok(version),
        Some(version) => bail!(
            "selected managed Gongbu binary supports only static-principal server config schema version {version}; upgrade Gongbu to a build supporting principal-neutral server config schema version {PRINCIPAL_NEUTRAL_GONGBU_SCHEMA_VERSION} or newer"
        ),
        None => bail!(
            "selected managed Gongbu binary does not report a server config schema version and cannot negotiate principal-neutral rendering; upgrade Gongbu to a build supporting principal-neutral server config schema version {PRINCIPAL_NEUTRAL_GONGBU_SCHEMA_VERSION} or newer"
        ),
    }
}

fn validate_with_binary(binary: &Path, config: &Path) -> Result<()> {
    let output = Command::new(binary)
        .args(["validate-config", "--config"])
        .arg(config)
        .output()
        .with_context(|| format!("run production validator `{}`", binary.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`{}` rejected generated configuration: {}",
            binary.display(),
            stderr.trim()
        );
    }
    Ok(())
}

fn write_generated_json(
    generation: &Path,
    name: &str,
    value: &Value,
    digests: &mut BTreeMap<String, String>,
) -> Result<()> {
    let path = generation.join(name);
    let bytes = serde_json::to_vec_pretty(value)?;
    write_secure(&path, &bytes)?;
    digests.insert(name.into(), digest(&bytes));
    Ok(())
}

fn write_json_secure(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_secure(path, &bytes)
}

fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("create `{}`", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write `{}`", path.display()))
}

fn write_new_secure(path: &Path, bytes: &[u8]) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    match write_secure(path, bytes) {
        Ok(()) => Ok(true),
        Err(_error) if path.exists() => Ok(false),
        Err(error) => Err(error),
    }
}

fn create_secure_dir(path: &Path) -> Result<()> {
    let created = !path.exists();
    fs::create_dir_all(path).with_context(|| format!("create `{}`", path.display()))?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn ensure_managed_credential_ignore(profile: &Path) -> Result<bool> {
    let root = managed_credential_root(profile);
    create_secure_dir(&root)?;
    let metadata =
        fs::symlink_metadata(&root).with_context(|| format!("inspect `{}`", root.display()))?;
    if !metadata.file_type().is_dir() {
        bail!("managed credential root must be a real directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("managed credential root must exclude group and other access");
        }
    }

    let ignore = root.join(".gitignore");
    let created = write_new_secure(&ignore, MANAGED_CREDENTIAL_IGNORE)?;
    if !created {
        let metadata = fs::symlink_metadata(&ignore)
            .with_context(|| format!("inspect `{}`", ignore.display()))?;
        if !metadata.file_type().is_file() || fs::read(&ignore)? != MANAGED_CREDENTIAL_IGNORE {
            bail!("managed credential ignore guard is missing or modified");
        }
    }
    Ok(created)
}

fn existing_absolute(path: &Path, field: &str) -> Result<PathBuf> {
    validate_safe_absolute(path, field)?;
    if !path.is_file() {
        bail!("{field} must name an existing regular file");
    }
    fs::canonicalize(path).with_context(|| format!("canonicalize `{}`", path.display()))
}

fn validate_safe_absolute(path: &Path, field: &str) -> Result<()> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        bail!("{field} must be a safe absolute path");
    }
    Ok(())
}

fn toml_to_json(value: &toml::Value) -> Result<Value> {
    serde_json::to_value(value).context("convert provider TOML value to JSON")
}

fn parse_toml<T: for<'de> Deserialize<'de>>(path: &Path, bytes: &[u8]) -> Result<T> {
    let text =
        std::str::from_utf8(bytes).with_context(|| format!("decode `{}`", path.display()))?;
    toml::from_str(text).with_context(|| format!("parse `{}`", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read `{}`", path.display()))?)
        .with_context(|| format!("parse `{}`", path.display()))
}

fn active_generation_path(generated: &Path, manifest: &ActiveManifest) -> Result<PathBuf> {
    let generation = Path::new(&manifest.generation);
    let valid_id = manifest.generation_id.len() == 64
        && manifest
            .generation_id
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value));
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION
        || !valid_id
        || generation.is_absolute()
        || generation.parent() != Some(Path::new("generations"))
        || generation.file_name().and_then(|value| value.to_str())
            != Some(manifest.generation_id.as_str())
        || generation
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        bail!("active manifest contains an unsafe or unsupported generation");
    }
    Ok(generated.join(generation))
}

fn verify_generated_file(generation: &Path, manifest: &ActiveManifest, name: &str) -> Result<()> {
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        bail!("active manifest contains an unsafe generated filename");
    }
    let expected = manifest
        .generated_file_digests
        .get(name)
        .ok_or_else(|| anyhow!("active manifest does not authenticate `{name}`"))?;
    let path = generation.join(name);
    let bytes = fs::read(&path).with_context(|| format!("read active `{}`", path.display()))?;
    if digest(&bytes) != *expected {
        bail!("active generated file `{name}` does not match its manifest digest");
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn resolve_profile(path: &Path, hubu_home: &Path) -> Result<PathBuf> {
    let profile = if path.as_os_str().is_empty() {
        default_stack_home(hubu_home)
            .join("stacks")
            .join(DEFAULT_PROFILE)
    } else {
        path.to_path_buf()
    };
    validate_safe_absolute(&profile, "--profile")?;
    Ok(profile)
}

fn take_profile(args: &mut Vec<String>, hubu_home: &Path) -> Result<PathBuf> {
    let path = if args.iter().any(|arg| arg == "--profile") {
        take_required_profile(args)?
    } else if let Some(selected) = read_profile_registry(hubu_home)?.selected {
        initialized_profile_path(&selected, "selected profile").with_context(|| {
            format!(
                "selected stack profile `{}` is unavailable; select an initialized profile with `hubu stack select --profile ABSOLUTE_DIR`",
                selected.display()
            )
        })?
    } else {
        default_stack_home(hubu_home)
            .join("stacks")
            .join(DEFAULT_PROFILE)
    };
    resolve_profile(&path, hubu_home)
}

fn take_required_profile(args: &mut Vec<String>) -> Result<PathBuf> {
    let Some(index) = args.iter().position(|arg| arg == "--profile") else {
        bail!("--profile is required");
    };
    args.remove(index);
    if index >= args.len() || args[index].starts_with('-') {
        bail!("missing value for --profile");
    }
    Ok(PathBuf::from(args.remove(index)))
}

fn profile_registry_path(hubu_home: &Path) -> PathBuf {
    default_stack_home(hubu_home).join(PROFILE_REGISTRY_FILE)
}

fn empty_profile_registry() -> ProfileRegistry {
    ProfileRegistry {
        schema_version: PROFILE_REGISTRY_SCHEMA_VERSION,
        selected: None,
        profiles: Vec::new(),
    }
}

fn read_profile_registry(hubu_home: &Path) -> Result<ProfileRegistry> {
    let path = profile_registry_path(hubu_home);
    if !path.exists() {
        return Ok(empty_profile_registry());
    }
    let recovery = format!(
        "inspect or move `{}` aside after review, then register a profile again with `hubu stack select --profile ABSOLUTE_DIR`",
        path.display()
    );
    let mut registry: ProfileRegistry =
        read_json(&path).with_context(|| format!("read stack profile registry; {recovery}"))?;
    if registry.schema_version != PROFILE_REGISTRY_SCHEMA_VERSION {
        bail!(
            "stack profile registry has unsupported schema_version {}; upgrade Hubu or {recovery}",
            registry.schema_version,
        );
    }
    for profile in &registry.profiles {
        validate_safe_absolute(profile, "registered profile")
            .with_context(|| format!("invalid stack profile registry; {recovery}"))?;
    }
    if let Some(selected) = &registry.selected {
        validate_safe_absolute(selected, "selected profile")
            .with_context(|| format!("invalid stack profile registry; {recovery}"))?;
        if !registry.profiles.contains(selected) {
            bail!("stack profile registry selects an unregistered profile; {recovery}");
        }
    }
    registry.profiles.sort();
    registry.profiles.dedup();
    Ok(registry)
}

fn write_profile_registry(hubu_home: &Path, registry: &ProfileRegistry) -> Result<()> {
    let path = profile_registry_path(hubu_home);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("profile registry path has no parent"))?;
    create_secure_dir(parent)?;
    let temp = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let mut bytes = serde_json::to_vec_pretty(registry)?;
    bytes.push(b'\n');
    write_secure(&temp, &bytes)?;
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("update `{}`", path.display()));
    }
    Ok(())
}

fn initialized_profile_path(path: &Path, field: &str) -> Result<PathBuf> {
    validate_safe_absolute(path, field)?;
    if !path.is_dir()
        || ["stack.toml", "credentials.toml", "providers.toml"]
            .iter()
            .any(|name| !path.join(name).is_file())
    {
        bail!(
            "{field} must identify an initialized Hubu stack profile containing stack.toml, credentials.toml, and providers.toml"
        );
    }
    fs::canonicalize(path).with_context(|| format!("canonicalize `{}`", path.display()))
}

fn register_profile(profile: &Path, hubu_home: &Path, select: bool) -> Result<()> {
    let profile = initialized_profile_path(profile, "profile")?;
    let mut registry = read_profile_registry(hubu_home)?;
    let previous = registry.clone();
    if !registry.profiles.contains(&profile) {
        registry.profiles.push(profile.clone());
        registry.profiles.sort();
    }
    if select {
        registry.selected = Some(profile);
    }
    if registry != previous {
        write_profile_registry(hubu_home, &registry)?;
    }
    Ok(())
}

fn profile_list_report(hubu_home: &Path) -> Result<ProfileListReport> {
    let mut registry = read_profile_registry(hubu_home)?;
    if let Some(selected) = &registry.selected {
        initialized_profile_path(selected, "selected profile").with_context(|| {
            format!(
                "selected stack profile `{}` is unavailable; select an initialized profile with `hubu stack select --profile ABSOLUTE_DIR`",
                selected.display()
            )
        })?;
    }

    let previous = registry.clone();
    registry
        .profiles
        .retain(|profile| initialized_profile_path(profile, "registered profile").is_ok());
    if registry != previous {
        write_profile_registry(hubu_home, &registry)?;
    }

    let stack_root = default_stack_home(hubu_home).join("stacks");
    let mut paths = registry.profiles.iter().cloned().collect::<BTreeSet<_>>();
    if stack_root.is_dir() {
        for entry in
            fs::read_dir(&stack_root).with_context(|| format!("list `{}`", stack_root.display()))?
        {
            let path = entry?.path();
            if let Ok(profile) = initialized_profile_path(&path, "profile") {
                paths.insert(profile);
            }
        }
    }

    let default_path = stack_root.join(DEFAULT_PROFILE);
    let default_path = fs::canonicalize(&default_path).unwrap_or(default_path);
    let profiles = paths
        .into_iter()
        .map(|path| ProfileListEntry {
            selected: registry.selected.as_ref() == Some(&path),
            is_default: path == default_path,
            path,
        })
        .collect();
    Ok(ProfileListReport {
        schema_version: PROFILE_LIST_SCHEMA_VERSION,
        profiles,
    })
}

fn take_option_value(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    args.remove(index);
    if index >= args.len() || args[index].starts_with('-') {
        bail!("missing value for {name}");
    }
    Ok(Some(args.remove(index)))
}

fn default_stack_home(hubu_home: &Path) -> PathBuf {
    let implicit_hubu_home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hubu");
    if env::var_os("HUBU_HOME").is_some() || hubu_home != implicit_hubu_home {
        return hubu_home.to_path_buf();
    }
    dirs::config_dir()
        .map(|root| root.join("hubu"))
        .unwrap_or_else(|| hubu_home.to_path_buf())
}

fn take_help(args: &mut Vec<String>) -> bool {
    ["help", "--help", "-h"].iter().any(|flag| {
        args.iter()
            .position(|arg| arg == flag)
            .map(|i| args.remove(i))
            .is_some()
    })
}

fn ensure_no_args(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        bail!("unexpected arguments: {}", args.join(" "))
    }
}

fn quote(value: impl AsRef<str>) -> String {
    toml::Value::String(value.as_ref().to_owned()).to_string()
}

fn binary_line(key: &str, executable: &str) -> String {
    discover_binary(executable)
        .map(|path| format!("{key} = {}", quote(path.display().to_string())))
        .unwrap_or_else(|| format!("# {key} = \"/absolute/path/to/{executable}\""))
}

fn discover_binary(name: &str) -> Option<PathBuf> {
    if name == "hubu" {
        if let Ok(path) = env::current_exe().and_then(fs::canonicalize) {
            return Some(path);
        }
    }
    if let Ok(current) = env::current_exe() {
        let sibling = current.with_file_name(name);
        if sibling.is_file() {
            return fs::canonicalize(sibling).ok();
        }
    }
    env::var_os("PATH")
        .and_then(|path| {
            env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        })
        .and_then(|path| fs::canonicalize(path).ok())
}

fn stack_template(profile: &Path) -> Result<String> {
    let state = profile.join("state");
    Ok(format!(
        r#"# Hubu local stack source. Comments identify choices that still need input.
# Reference: https://hubustack.dev/configuration/local-stack/v1/stack-toml
schema_version = 1
# Set true only when every selected binary is an unstamped local development build.
allow_development_builds = false

[binaries]
{}
{}
{}
{}

[hubu]
# ownership = "managed" # or "external"
endpoint = "http://127.0.0.1:8787"
listen = "127.0.0.1:8787"
database_path = {}
log_file = {}

[gongbu]
# ownership = "managed" # or "external"
endpoint = "http://127.0.0.1:8788"
listen = "127.0.0.1:8788"
database_path = {}
artifact_root = {}
log_file = {}

[temporal]
# mode = "managed_local" # or "external"
# For managed_local, fill binary_path and expected_cli_version.
# binary_path = "/absolute/path/to/temporal"
# expected_cli_version = "<exact installed version>"
data_path = {}
rpc_port = 7233
ui_port = 8233
# For external mode, fill address instead of the managed-local fields.
# address = "http://127.0.0.1:7233"
namespace = "default"
task_queue = "gongbu-local-executions"
ui_url = "http://127.0.0.1:8233"

[runtime]
hubu_startup_policy = "wait"
hubu_startup_timeout_ms = 30000
recovery_delays_seconds = [30, 120, 600]
temporal_startup_timeout_ms = 30000
dependency_check_interval_ms = 5000
worker_drain_timeout_ms = 30000
max_artifacts_per_execution = 4
max_encoded_bytes = 20971520
max_decoded_bytes = 104857600
max_width = 16384
max_height = 16384
log_level = "info"
log_format = "text"
"#,
        binary_line("hubu", "hubu"),
        binary_line("hubu_server", "hubu-server"),
        binary_line("gongbu_server", "gongbu-server"),
        binary_line("hubu_unified_mcp", "hubu-unified-mcp"),
        quote(state.join("hubu/hubu.sqlite3").display().to_string()),
        quote(state.join("hubu/hubu.jsonl").display().to_string()),
        quote(state.join("gongbu/gongbu.sqlite3").display().to_string()),
        quote(state.join("gongbu/artifacts").display().to_string()),
        quote(state.join("gongbu/gongbu.jsonl").display().to_string()),
        quote(state.join("temporal").display().to_string()),
    ))
}

fn credentials_template() -> String {
    r#"# Managed-local service credentials are provisioned inside this profile by
# `hubu stack start`; no token values or file locations belong in this file.
# Explicit service file and reserved Gongbu references are advanced overrides
# for externally provisioned credentials. Provider opaque references are normal
# live-target inputs. Never paste bearer tokens or provider secrets here.
# Reference: https://hubustack.dev/configuration/local-stack/v1/credentials-toml
schema_version = 1

# External/advanced service example:
# [files]
# hubu_auth = "/absolute/path/to/hubu.auth-token"
# hubu_approval = "/absolute/path/to/hubu.approval-token"
# hubu_reconciliation = "/absolute/path/to/hubu.reconciliation-token"
# gongbu_caller = "/absolute/path/to/gongbu.caller-token"

# Advanced Gongbu bootstrap overrides are resolved by Gongbu, not by rendering.
# [opaque.gongbu_hubu]
# service = "<keychain service>"
# account = "<keychain account>"

# [opaque.gongbu_caller]
# service = "<keychain service>"
# account = "<keychain account>"

# Add one section per live provider credential, then reference its key from
# providers.toml, for example [opaque.provider_image].
"#
    .into()
}

fn providers_template() -> String {
    r#"# Provider and pricing choices are intentionally omitted by initialization.
# Reference: https://hubustack.dev/configuration/local-stack/v1/providers-toml
schema_version = 1

# Choose exactly one mode. Disabled is the no-spend local dependency profile.
# mode = "disabled"
# mode = "live"

# Live mode additionally requires every field and table below.
# catalog_version = "<operator-owned immutable catalog version>"
# Set maximum_spend_minor to a positive operator-approved minor-unit ceiling.
# live_spend_acknowledgement = "<exact acknowledgement required by Gongbu>"

# [[targets]]
# provider_config_version = "<immutable version>"
# workload_type = "image_generation"
# provider = "<provider id>"
# adapter = "<registered adapter id>"
# model = "<exact model id>"
# credential = "<key from credentials.toml [opaque.*]>"
# active = true
# execution_enabled = true
# [targets.settings]
# type = "<adapter settings type>"
# [targets.settings.config]
# endpoint = "https://provider.example"
# api_version = "v1"
# timeout_ms = 30000

# [[pricing_rules]]
# rule_id = "<immutable rule id>"
# provider = "<same provider id>"
# model = "<same model id>"
# currency = "USD"
# selector = { image_size = "1k" }
# components = [
#   { unit = "image", rate_numerator_minor = 1, rate_denominator = 1 },
# ]
# Add separate selector-qualified rules for every enabled image size. Rates use
# exact rational currency-minor-unit values; verify all values with the provider.
"#
    .into()
}

fn readme_template() -> String {
    r#"# Hubu local stack profile

This directory is operator-owned. `hubu stack init` never overwrites these
files and never starts a service.

- `stack.toml`: topology, binaries, state locations, Temporal, and lifecycle.
- `credentials.toml`: provider references and advanced external-service
  credential overrides; never raw secret values. Managed-local service
  credentials and their locations are selected internally during start.
- `providers.toml`: disabled or live provider mode, targets, frozen pricing,
  spend ceiling, and the explicit live-spend gate.
- `generated/`: validated implementation output; do not edit it.

Start with every uncommented example that matches your topology, fill the
commented fields you choose, and leave unrelated examples commented. Then run
`hubu stack select --profile /absolute/path/to/this/profile`, followed by
`hubu stack doctor`. When the profile is `ready_to_render`, run
`hubu stack start`. Start runs doctor and render as needed, starts the final
managed Hubu so it can create its private
capabilities, completes Gongbu's internal handoff, and then starts Gongbu. It
leaves external services and the client-owned unified MCP process untouched.
Use `hubu stack status` and `hubu stack logs` for the combined operator view.
An explicit `--profile` temporarily overrides the selection without changing
it; `hubu stack profiles` lists known profiles.

For later changes, edit the operator-owned TOML and run `hubu stack render`.
The validated generation is staged without replacing the active generation.
Review the reported filenames and affected components, then use the printed
generation ID with `hubu stack stop`, `hubu stack activate`, and `hubu stack
start`. Rotate explicit external file references by changing only their paths
in `credentials.toml`; do not overwrite a secret in place or put secret values
in this profile. Managed-local credentials are lifecycle state, not editable
source. `hubu stack
generations` lists retained generations. Rollback requires restoring the exact
operator-owned TOML for the target generation before `hubu stack rollback`.
There is no stack restart command and no per-component repair path.

Quick start:
https://hubustack.dev/docs/local-stack
Public configuration reference:
https://hubustack.dev/configuration/local-stack/v1/
"#
    .into()
}

fn print_help() {
    println!(
        "Manage local Hubu stack profiles\n\nUsage:\n  hubu stack init [--profile ABSOLUTE_DIR]\n  hubu stack select --profile ABSOLUTE_DIR\n  hubu stack profiles [--json]\n  hubu stack doctor [--profile ABSOLUTE_DIR] [--json]\n  hubu stack render [--profile ABSOLUTE_DIR]\n  hubu stack activate --generation ID [--profile ABSOLUTE_DIR]\n  hubu stack rollback --generation ID [--profile ABSOLUTE_DIR]\n  hubu stack generations [--profile ABSOLUTE_DIR]\n  hubu stack start [--profile ABSOLUTE_DIR]\n  hubu stack status [--profile ABSOLUTE_DIR] [--json]\n  hubu stack logs [--profile ABSOLUTE_DIR] [--component hubu|gongbu|all] [--execution-id ID] [--lines N]\n  hubu stack stop [--profile ABSOLUTE_DIR] [--forget-stale]\n\nProfile precedence:\n  explicit --profile, selected profile, platform default"
    );
}

fn print_init_help() {
    println!(
        "Create annotated local stack starter files without overwriting input or starting services\n\nUsage:\n  hubu stack init [--profile ABSOLUTE_DIR]"
    );
}

fn print_select_help() {
    println!(
        "Register and select an initialized local stack profile\n\nUsage:\n  hubu stack select --profile ABSOLUTE_DIR\n\nThe selected profile is used when a stack command omits --profile. An explicit --profile remains a one-command override."
    );
}

fn print_profiles_help() {
    println!(
        "List known initialized local stack profiles without scanning the machine\n\nUsage:\n  hubu stack profiles [--json]\n\nProfiles are read from the registry and the immediate children of the conventional stacks directory."
    );
}

fn print_render_help() {
    println!(
        "Render and production-validate a complete local stack profile\n\nUsage:\n  hubu stack render [--profile ABSOLUTE_DIR]\n\nThe first valid generation activates automatically. Later changes are staged and require stack activate after review."
    );
}

fn print_activate_help() {
    println!(
        "Activate one validated generation while launcher-owned services are stopped\n\nUsage:\n  hubu stack activate --generation ID [--profile ABSOLUTE_DIR]"
    );
}

fn print_rollback_help() {
    println!(
        "Reactivate a prior validated generation after restoring its operator-owned source files and selected binary provenance\n\nUsage:\n  hubu stack rollback --generation ID [--profile ABSOLUTE_DIR]"
    );
}

fn print_generations_help() {
    println!(
        "List recoverable validated generations\n\nUsage:\n  hubu stack generations [--profile ABSOLUTE_DIR]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn write_fake_binary(path: &Path, reject_validation: bool) {
        use std::os::unix::fs::PermissionsExt;

        let validation = if reject_validation {
            "exit 9"
        } else {
            "exit 0"
        };
        fs::write(
            path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo '{{\"product_version\":\"0.1.0\",\"source_commit\":\"unknown\",\"executor_contract\":\"hubu-executor.v1\",\"server_config_schema_version\":2}}'\n  exit 0\nfi\nif [ \"$1\" = \"validate-config\" ]; then\n  {validation}\nfi\nexit 2\n"
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    fn write_fake_gongbu_v3_binary(path: &Path, reject_validation: bool) {
        use std::os::unix::fs::PermissionsExt;

        let validation = if reject_validation {
            "exit 9"
        } else {
            "if grep -Eq '\"(account_id|agent_id|caller_account_id)\"' \"$3\"; then exit 8; fi\n  exit 0"
        };
        fs::write(
            path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo '{{\"product_version\":\"0.1.0\",\"source_commit\":\"unknown\",\"executor_contract\":\"hubu-executor.v1\",\"server_config_schema_version\":3}}'\n  exit 0\nfi\nif [ \"$1\" = \"validate-config\" ]; then\n  {validation}\nfi\nexit 2\n"
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn init_is_annotated_incomplete_and_byte_preserving() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        init(
            vec!["--profile".into(), profile.display().to_string()],
            root.path(),
        )
        .unwrap();
        let stack_before = fs::read(profile.join("stack.toml")).unwrap();
        fs::write(profile.join("stack.toml"), b"operator-owned\n").unwrap();
        init(
            vec!["--profile".into(), profile.display().to_string()],
            root.path(),
        )
        .unwrap();
        assert_eq!(
            fs::read(profile.join("stack.toml")).unwrap(),
            b"operator-owned\n"
        );
        assert!(!stack_before.is_empty());
        let stack_template = String::from_utf8(stack_before).unwrap();
        assert!(!stack_template.contains("[identity]"));
        assert!(!stack_template.contains("account_id"));
        assert!(!stack_template.contains("agent_id"));
        let providers = fs::read_to_string(profile.join("providers.toml")).unwrap();
        let stack_reference = "https://hubustack.dev/configuration/local-stack/v1/stack-toml";
        assert!(stack_template.contains(stack_reference));
        assert!(fs::read_to_string(profile.join("README.md"))
            .unwrap()
            .contains("https://hubustack.dev/configuration/local-stack/v1/"));
        assert!(!providers.contains(LIVE_SPEND_ACKNOWLEDGEMENT));
        assert!(!providers.contains("REQUIRED"));
        assert!(!providers.contains("maximum_spend_minor ="));
        assert!(providers.contains("selector = { image_size = \"1k\" }"));
        assert!(providers.contains("rate_numerator_minor = 1"));
        assert!(profile.join("generated").is_dir());
        assert_eq!(
            fs::read(managed_credential_root(&profile).join(".gitignore")).unwrap(),
            MANAGED_CREDENTIAL_IGNORE
        );
        assert_eq!(
            fs::read_dir(managed_credential_root(&profile))
                .unwrap()
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(profile.join("providers.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn profile_option_requires_a_safe_absolute_value() {
        let root = tempdir().unwrap();
        assert!(init(vec!["--profile".into()], root.path()).is_err());
        assert!(init(vec!["--profile".into(), "relative".into()], root.path()).is_err());
    }

    fn write_initialized_profile(path: &Path) {
        fs::create_dir_all(path).unwrap();
        for name in ["stack.toml", "credentials.toml", "providers.toml"] {
            fs::write(path.join(name), "schema_version = 1\n").unwrap();
        }
    }

    #[test]
    fn init_registers_and_select_persists_an_external_profile() {
        let root = tempdir().unwrap();
        let initialized = root.path().join("initialized");
        init(
            vec!["--profile".into(), initialized.display().to_string()],
            root.path(),
        )
        .unwrap();
        let initialized = fs::canonicalize(initialized).unwrap();
        assert_eq!(
            read_profile_registry(root.path()).unwrap().profiles,
            vec![initialized]
        );

        let external_root = tempdir().unwrap();
        let external = external_root.path().join("external");
        write_initialized_profile(&external);
        select_profile(
            vec!["--profile".into(), external.display().to_string()],
            root.path(),
        )
        .unwrap();
        let external = fs::canonicalize(external).unwrap();
        let registry = read_profile_registry(root.path()).unwrap();
        assert_eq!(registry.selected.as_ref(), Some(&external));
        assert!(registry.profiles.contains(&external));
        assert_eq!(
            take_profile(&mut Vec::new(), root.path()).unwrap(),
            external
        );
    }

    #[test]
    fn explicit_profile_overrides_selection_without_changing_it() {
        let root = tempdir().unwrap();
        let selected = root.path().join("selected");
        let override_profile = root.path().join("override");
        write_initialized_profile(&selected);
        write_initialized_profile(&override_profile);
        select_profile(
            vec!["--profile".into(), selected.display().to_string()],
            root.path(),
        )
        .unwrap();

        let resolved = take_profile(
            &mut vec!["--profile".into(), override_profile.display().to_string()],
            root.path(),
        )
        .unwrap();
        assert_eq!(resolved, override_profile);
        assert_eq!(
            read_profile_registry(root.path()).unwrap().selected,
            Some(fs::canonicalize(selected).unwrap())
        );
    }

    #[test]
    fn profile_listing_combines_registry_and_conventional_discovery() {
        let root = tempdir().unwrap();
        let default = root.path().join("stacks/default");
        let conventional = root.path().join("stacks/fixture");
        let external_root = tempdir().unwrap();
        let external = external_root.path().join("external");
        write_initialized_profile(&default);
        write_initialized_profile(&conventional);
        write_initialized_profile(&external);
        select_profile(
            vec!["--profile".into(), external.display().to_string()],
            root.path(),
        )
        .unwrap();
        let external = fs::canonicalize(external).unwrap();

        let report = profile_list_report(root.path()).unwrap();
        assert_eq!(report.schema_version, PROFILE_LIST_SCHEMA_VERSION);
        assert_eq!(report.profiles.len(), 3);
        assert!(report
            .profiles
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path));
        assert!(report
            .profiles
            .iter()
            .any(|profile| profile.selected && profile.path == external));
        assert!(report.profiles.iter().any(|profile| profile.is_default));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema_version"], PROFILE_LIST_SCHEMA_VERSION);
        assert!(json["profiles"][0].get("default").is_some());
        assert!(json["profiles"][0].get("selected").is_some());
        assert!(json["profiles"][0].get("path").is_some());
    }

    #[test]
    fn listing_prunes_stale_unselected_entries_but_rejects_a_stale_selection() {
        let root = tempdir().unwrap();
        let stale = root.path().join("stale");
        write_initialized_profile(&stale);
        register_profile(&stale, root.path(), false).unwrap();
        fs::remove_file(stale.join("stack.toml")).unwrap();
        assert!(profile_list_report(root.path())
            .unwrap()
            .profiles
            .is_empty());
        assert!(read_profile_registry(root.path())
            .unwrap()
            .profiles
            .is_empty());

        write_initialized_profile(&stale);
        register_profile(&stale, root.path(), true).unwrap();
        fs::remove_file(stale.join("providers.toml")).unwrap();
        let error = profile_list_report(root.path()).unwrap_err().to_string();
        assert!(error.contains("selected stack profile"));
        assert!(take_profile(&mut Vec::new(), root.path())
            .unwrap_err()
            .to_string()
            .contains("selected stack profile"));
    }

    #[test]
    fn registry_is_versioned_deduplicated_and_scoped_by_hubu_home() {
        let first_root = tempdir().unwrap();
        let second_root = tempdir().unwrap();
        let profile = first_root.path().join("profile");
        write_initialized_profile(&profile);
        register_profile(&profile, first_root.path(), false).unwrap();
        register_profile(&profile, first_root.path(), false).unwrap();
        assert_eq!(
            read_profile_registry(first_root.path())
                .unwrap()
                .profiles
                .len(),
            1
        );
        assert!(read_profile_registry(second_root.path())
            .unwrap()
            .profiles
            .is_empty());

        fs::write(
            profile_registry_path(second_root.path()),
            "{\"schema_version\":99,\"selected\":null,\"profiles\":[]}",
        )
        .unwrap();
        assert!(read_profile_registry(second_root.path())
            .unwrap_err()
            .to_string()
            .contains("unsupported schema_version"));
    }

    #[test]
    fn generation_option_requires_a_lowercase_sha256_identifier() {
        assert!(required_generation(&mut Vec::new()).is_err());
        assert!(required_generation(&mut vec!["--generation".into(), "abc".into()]).is_err());
        assert!(required_generation(&mut vec!["--generation".into(), "A".repeat(64)]).is_err());
        assert_eq!(
            required_generation(&mut vec!["--generation".into(), "a".repeat(64)]).unwrap(),
            "a".repeat(64)
        );
    }

    #[cfg(unix)]
    #[test]
    fn render_serializes_with_lifecycle_mutations() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        create_secure_dir(&profile).unwrap();
        let _lock = lifecycle::acquire_lifecycle_lock(&profile).unwrap();
        let error = render(
            vec!["--profile".into(), profile.display().to_string()],
            root.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("another lifecycle command"));
    }

    #[test]
    fn generation_listing_fails_closed_on_a_corrupt_retained_manifest() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        let generation = profile.join("generated/generations").join("a".repeat(64));
        fs::create_dir_all(&generation).unwrap();
        fs::write(generation.join("manifest.json"), b"{}\n").unwrap();
        let error = generations(
            vec!["--profile".into(), profile.display().to_string()],
            root.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("read recoverable generation"));
    }

    #[test]
    fn incomplete_profile_reports_stable_file_and_field_paths() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        init(
            vec!["--profile".into(), profile.display().to_string()],
            root.path(),
        )
        .unwrap();
        let error = render_profile(&profile).unwrap_err().to_string();
        assert!(error.contains("stack.toml:hubu.ownership"));
        assert!(error.contains("stack.toml:gongbu.ownership"));
        assert!(!error.contains("credentials.toml:files.hubu_auth"));
        assert!(error.contains("providers.toml:mode"));
        assert!(!error.contains("identity"));
        assert!(!profile.join("generated/active-manifest.json").exists());
    }

    #[test]
    fn legacy_identity_is_accepted_but_not_a_completion_decision() {
        let stack: StackSource = toml::from_str(
            "schema_version = 1\n[identity]\naccount_id = \"legacy-account\"\nagent_id = \"legacy-agent\"\n",
        )
        .unwrap();
        let credentials: CredentialsSource = toml::from_str("schema_version = 1\n").unwrap();
        let providers: ProvidersSource =
            toml::from_str("schema_version = 1\nmode = \"disabled\"\n").unwrap();

        assert!(missing_fields(&stack, &credentials, &providers)
            .iter()
            .all(|field| !field.contains("identity")));
    }

    #[test]
    fn managed_topology_derives_private_credentials_while_external_owners_require_refs() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        create_secure_dir(&profile).unwrap();
        let managed: StackSource = toml::from_str(
            r#"schema_version = 1
[hubu]
ownership = "managed"
[gongbu]
ownership = "managed"
"#,
        )
        .unwrap();
        let external: StackSource = toml::from_str(
            r#"schema_version = 1
[hubu]
ownership = "external"
[gongbu]
ownership = "external"
"#,
        )
        .unwrap();
        let credentials: CredentialsSource = toml::from_str("schema_version = 1\n").unwrap();
        let providers: ProvidersSource =
            toml::from_str("schema_version = 1\nmode = \"disabled\"\n").unwrap();

        let managed_missing = missing_fields(&managed, &credentials, &providers);
        assert!(managed_missing
            .iter()
            .all(|field| !field.starts_with("credentials.toml:")));
        let paths = resolve_credential_paths(&profile, &managed, &credentials).unwrap();
        let canonical_profile = fs::canonicalize(&profile).unwrap();
        assert_eq!(
            paths.hubu_auth,
            canonical_profile.join("state/credentials/hubu/auth")
        );
        assert_eq!(
            paths.gongbu_caller,
            canonical_profile.join("state/credentials/gongbu/caller")
        );
        assert!(paths.managed_gongbu_handoff);
        assert!(!paths.hubu_auth.exists());
        assert!(!paths.gongbu_caller.exists());

        let missing_override = profile.join("operator-auth");
        let credentials_with_missing_override: CredentialsSource = toml::from_str(&format!(
            "schema_version = 1\n[files]\nhubu_auth = {}\n",
            quote(missing_override.display().to_string())
        ))
        .unwrap();
        assert!(
            resolve_credential_paths(&profile, &managed, &credentials_with_missing_override)
                .unwrap_err()
                .to_string()
                .contains("credentials.toml:files.hubu_auth")
        );
        assert!(!missing_override.exists());

        let external_missing = missing_fields(&external, &credentials, &providers);
        for field in [
            "credentials.toml:files.hubu_auth",
            "credentials.toml:files.hubu_approval",
            "credentials.toml:files.hubu_reconciliation",
            "credentials.toml:files.gongbu_caller",
        ] {
            assert!(external_missing.contains(&field.to_string()));
        }

        let managed_hubu_external_gongbu: StackSource = toml::from_str(
            r#"schema_version = 1
[hubu]
ownership = "managed"
[gongbu]
ownership = "external"
"#,
        )
        .unwrap();
        let mixed_missing = missing_fields(&managed_hubu_external_gongbu, &credentials, &providers);
        assert_eq!(
            mixed_missing
                .iter()
                .filter(|field| field.starts_with("credentials.toml:"))
                .cloned()
                .collect::<Vec<_>>(),
            vec!["credentials.toml:files.gongbu_caller".to_owned()]
        );

        let external_hubu_managed_gongbu: StackSource = toml::from_str(
            r#"schema_version = 1
[hubu]
ownership = "external"
[gongbu]
ownership = "managed"
"#,
        )
        .unwrap();
        let mixed_missing = missing_fields(&external_hubu_managed_gongbu, &credentials, &providers);
        assert!(mixed_missing.contains(&"credentials.toml:files.hubu_auth".into()));
        assert!(mixed_missing.contains(&"credentials.toml:files.hubu_approval".into()));
        assert!(mixed_missing.contains(&"credentials.toml:files.hubu_reconciliation".into()));
        assert!(!mixed_missing.contains(&"credentials.toml:files.gongbu_caller".into()));
    }

    #[test]
    fn static_principal_gongbu_schemas_have_actionable_upgrade_errors() {
        for version in [1, 2] {
            let provenance = BinaryProvenance {
                component: "gongbu-server".into(),
                path: "/gongbu-server".into(),
                product_version: "0.1.0".into(),
                source_commit: "commit".into(),
                executor_contract: "hubu-executor.v1".into(),
                server_config_schema_version: Some(version),
            };
            let error = negotiate_principal_neutral_gongbu_schema(&provenance)
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!(
                "static-principal server config schema version {version}"
            )));
            assert!(error.contains("upgrade Gongbu"));
            assert!(error.contains("schema version 3 or newer"));
        }
    }

    #[test]
    fn partial_provider_and_credential_blocks_remain_incomplete() {
        let credentials: CredentialsSource = toml::from_str(
            "schema_version = 1\n[opaque.provider]\nservice = \"gongbu.provider\"\n",
        )
        .unwrap();
        let providers: ProvidersSource = toml::from_str(
            "schema_version = 1\nmode = \"live\"\n[[targets]]\nprovider = \"example\"\n",
        )
        .unwrap();
        let stack: StackSource = toml::from_str("schema_version = 1\n").unwrap();
        let missing = missing_fields(&stack, &credentials, &providers);
        assert!(missing.contains(&"credentials.toml:opaque.provider.account".into()));
        assert!(missing.contains(&"providers.toml:targets[0].model".into()));
        assert!(missing.contains(&"providers.toml:targets[0].settings".into()));
    }

    #[test]
    fn managed_endpoint_must_match_the_listen_host() {
        assert!(validate_endpoint_matches_listen(
            "http://127.0.0.1:8787",
            "127.0.0.2:8787".parse().unwrap(),
            "hubu",
        )
        .is_err());
    }

    #[test]
    fn managed_services_reject_shared_sockets_and_state_files() {
        let root = tempdir().unwrap();
        let shared = quote(root.path().join("shared.sqlite3").display().to_string());
        let stack = |gongbu_listen: &str, gongbu_database: &str| {
            toml::from_str::<StackSource>(&format!(
                r#"schema_version = 1
[hubu]
ownership = "managed"
endpoint = "http://127.0.0.1:8787"
listen = "127.0.0.1:8787"
database_path = {shared}
[gongbu]
ownership = "managed"
endpoint = "http://{gongbu_listen}"
listen = "{gongbu_listen}"
database_path = {gongbu_database}
"#
            ))
            .unwrap()
        };

        let distinct = quote(root.path().join("gongbu.sqlite3").display().to_string());
        let shared_socket = stack("127.0.0.1:8787", &distinct);
        assert!(validate_topology(&shared_socket)
            .unwrap_err()
            .to_string()
            .contains("distinct managed sockets"));

        let shared_state = stack("127.0.0.1:8788", &shared);
        assert!(validate_topology(&shared_state)
            .unwrap_err()
            .to_string()
            .contains("must not overlap managed resources"));
    }

    #[test]
    fn managed_temporal_ports_reject_service_and_each_other_collisions() {
        let root = tempdir().unwrap();
        let stack = |rpc_port: u16, ui_port: u16| {
            toml::from_str::<StackSource>(&format!(
                r#"schema_version = 1
[hubu]
ownership = "managed"
endpoint = "http://127.0.0.1:8787"
listen = "127.0.0.1:8787"
database_path = {}
[gongbu]
ownership = "managed"
endpoint = "http://127.0.0.1:8788"
listen = "127.0.0.1:8788"
database_path = {}
artifact_root = {}
[temporal]
mode = "managed_local"
rpc_port = {rpc_port}
ui_port = {ui_port}
"#,
                quote(root.path().join("hubu.sqlite3").display().to_string()),
                quote(root.path().join("gongbu.sqlite3").display().to_string()),
                quote(root.path().join("artifacts").display().to_string())
            ))
            .unwrap()
        };

        assert!(validate_topology(&stack(8787, 8233))
            .unwrap_err()
            .to_string()
            .contains("conflicts with managed socket"));
        assert!(validate_topology(&stack(7233, 7233))
            .unwrap_err()
            .to_string()
            .contains("must be distinct managed ports"));
    }

    #[test]
    fn managed_gongbu_database_must_not_overlap_artifact_root() {
        let root = tempdir().unwrap();
        let shared = quote(root.path().join("gongbu-resource").display().to_string());
        let stack: StackSource = toml::from_str(&format!(
            r#"schema_version = 1
[hubu]
ownership = "external"
endpoint = "http://127.0.0.1:8787"
[gongbu]
ownership = "managed"
endpoint = "http://127.0.0.1:8788"
listen = "127.0.0.1:8788"
database_path = {shared}
artifact_root = {shared}
"#
        ))
        .unwrap();

        let error = validate_topology(&stack).unwrap_err().to_string();
        assert!(error.contains("gongbu.database_path"));
        assert!(error.contains("gongbu.artifact_root"));
    }

    #[test]
    fn managed_service_state_cannot_overlap_derived_credentials() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        create_secure_dir(&profile).unwrap();
        let credentials: CredentialsSource = toml::from_str("schema_version = 1\n").unwrap();
        let stack = |gongbu_log: &Path, artifacts: &Path| {
            toml::from_str::<StackSource>(&format!(
                r#"schema_version = 1
[hubu]
ownership = "managed"
endpoint = "http://127.0.0.1:8787"
listen = "127.0.0.1:8787"
database_path = {}
[gongbu]
ownership = "managed"
endpoint = "http://127.0.0.1:8788"
listen = "127.0.0.1:8788"
database_path = {}
artifact_root = {}
log_file = {}
"#,
                quote(profile.join("state/hubu.sqlite3").display().to_string()),
                quote(profile.join("state/gongbu.sqlite3").display().to_string()),
                quote(artifacts.display().to_string()),
                quote(gongbu_log.display().to_string()),
            ))
            .unwrap()
        };

        let caller = profile.join("state/credentials/gongbu/caller");
        let distinct_artifacts = profile.join("artifacts");
        let stack_with_log_collision = stack(&caller, &distinct_artifacts);
        let paths =
            resolve_credential_paths(&profile, &stack_with_log_collision, &credentials).unwrap();
        assert!(validate_credential_resource_separation(
            &profile,
            &stack_with_log_collision,
            &paths
        )
        .unwrap_err()
        .to_string()
        .contains("gongbu.log_file"));

        let credential_artifacts = profile.join("state/credentials/gongbu");
        let distinct_log = profile.join("logs/gongbu.jsonl");
        let stack_with_artifact_collision = stack(&distinct_log, &credential_artifacts);
        let paths =
            resolve_credential_paths(&profile, &stack_with_artifact_collision, &credentials)
                .unwrap();
        assert!(validate_credential_resource_separation(
            &profile,
            &stack_with_artifact_collision,
            &paths
        )
        .unwrap_err()
        .to_string()
        .contains("gongbu.artifact_root"));
    }

    #[cfg(unix)]
    #[test]
    fn managed_database_hard_links_are_the_same_resource() {
        let root = tempdir().unwrap();
        let hubu_database = root.path().join("hubu.sqlite3");
        let gongbu_database = root.path().join("gongbu.sqlite3");
        fs::write(&hubu_database, b"").unwrap();
        fs::hard_link(&hubu_database, &gongbu_database).unwrap();

        assert!(paths_resolve_to_same_resource(&hubu_database, &gongbu_database).unwrap());
    }

    #[test]
    fn renderer_must_be_the_configured_hubu_binary() {
        let root = tempdir().unwrap();
        let renderer = root.path().join("hubu-running");
        let configured = root.path().join("hubu-configured");
        fs::write(&renderer, b"running").unwrap();
        fs::write(&configured, b"configured").unwrap();

        assert!(validate_renderer_identity(&renderer, &renderer).is_ok());
        assert!(validate_renderer_identity(&renderer, &configured)
            .unwrap_err()
            .to_string()
            .contains("must identify the running hubu executable"));
    }

    #[test]
    fn provider_config_preserves_legacy_server_shape_and_versions_disabled_mode() {
        let live: ProvidersSource = toml::from_str(&format!(
            "schema_version = 1\nmode = \"live\"\nmaximum_spend_minor = 10\nlive_spend_acknowledgement = \"{LIVE_SPEND_ACKNOWLEDGEMENT}\"\n"
        ))
        .unwrap();
        let v1 = render_provider_config(&live, 1, Path::new("/tmp/generation")).unwrap();
        assert!(v1.get("mode").is_none());
        let v2 = render_provider_config(&live, 2, Path::new("/tmp/generation")).unwrap();
        assert_eq!(v2["mode"], "live");

        let disabled: ProvidersSource =
            toml::from_str("schema_version = 1\nmode = \"disabled\"\n").unwrap();
        assert!(render_provider_config(&disabled, 1, Path::new("/tmp/generation")).is_err());
    }

    #[test]
    fn pricing_catalog_render_is_v2_and_preserves_selector_components() {
        let providers: ProvidersSource = toml::from_str(
            r#"schema_version = 1
mode = "live"
catalog_version = "prices-v2"
[[pricing_rules]]
rule_id = "image-2k"
provider = "google"
model = "gemini-image"
currency = "USD"
selector = { image_size = "2k" }
components = [{ unit = "image", rate_numerator_minor = 67, rate_denominator = 10 }]
"#,
        )
        .unwrap();

        let catalog = render_pricing_catalog(&providers).unwrap();
        assert_eq!(catalog["schema_version"], 2);
        assert_eq!(catalog["rules"][0]["selector"]["image_size"], "2k");
        assert_eq!(
            catalog["rules"][0]["components"][0],
            json!({
                "unit": "image",
                "rate_numerator_minor": 67,
                "rate_denominator": 10,
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_backends_do_not_require_unused_local_server_binaries() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        let binaries = root.path().join("bin");
        let credentials = root.path().join("credentials");
        fs::create_dir(&profile).unwrap();
        fs::create_dir(&binaries).unwrap();
        fs::create_dir(&credentials).unwrap();
        for name in ["hubu", "hubu-unified-mcp"] {
            write_fake_binary(&binaries.join(name), false);
        }
        for name in ["auth", "approval", "reconciliation", "gongbu-caller"] {
            fs::write(credentials.join(name), format!("{name}-secret")).unwrap();
        }
        fs::write(
            profile.join("stack.toml"),
            format!(
                r#"schema_version = 1
allow_development_builds = true
[binaries]
hubu = {}
hubu_unified_mcp = {}
[hubu]
ownership = "external"
endpoint = "http://127.0.0.1:42001"
[gongbu]
ownership = "external"
endpoint = "http://127.0.0.1:42002"
"#,
                quote(binaries.join("hubu").display().to_string()),
                quote(binaries.join("hubu-unified-mcp").display().to_string()),
            ),
        )
        .unwrap();
        fs::write(
            profile.join("credentials.toml"),
            format!(
                r#"schema_version = 1
[files]
hubu_auth = {}
hubu_approval = {}
hubu_reconciliation = {}
gongbu_caller = {}
"#,
                quote(credentials.join("auth").display().to_string()),
                quote(credentials.join("approval").display().to_string()),
                quote(credentials.join("reconciliation").display().to_string()),
                quote(credentials.join("gongbu-caller").display().to_string()),
            ),
        )
        .unwrap();
        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n",
        )
        .unwrap();

        let stack: StackSource =
            toml::from_str(&fs::read_to_string(profile.join("stack.toml")).unwrap()).unwrap();
        let credential_source: CredentialsSource =
            toml::from_str(&fs::read_to_string(profile.join("credentials.toml")).unwrap()).unwrap();
        let providers: ProvidersSource =
            toml::from_str(&fs::read_to_string(profile.join("providers.toml")).unwrap()).unwrap();
        assert!(missing_fields(&stack, &credential_source, &providers).is_empty());

        render_profile_with_renderer(&profile, &binaries.join("hubu")).unwrap();
        let manifest: ActiveManifest =
            read_json(&profile.join("generated/active-manifest.json")).unwrap();
        assert_eq!(
            manifest
                .binary_provenance
                .iter()
                .map(|item| item.component.as_str())
                .collect::<Vec<_>>(),
            ["hubu", "hubu-unified-mcp"]
        );
        assert_eq!(
            manifest
                .generated_file_digests
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["client-handoff.json"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn principal_neutral_profile_ignores_registration_state_and_preserves_active_on_failure() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        let binaries = root.path().join("bin");
        let hubu_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gongbu_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let hubu_addr = hubu_listener.local_addr().unwrap();
        let gongbu_addr = gongbu_listener.local_addr().unwrap();
        fs::create_dir(&binaries).unwrap();
        for name in ["hubu", "hubu-server", "hubu-unified-mcp"] {
            write_fake_binary(&binaries.join(name), false);
        }
        write_fake_gongbu_v3_binary(&binaries.join("gongbu-server"), false);
        let credentials = root.path().join("credentials");
        fs::create_dir(&credentials).unwrap();
        for name in ["auth", "approval", "reconciliation", "gongbu-caller"] {
            fs::write(credentials.join(name), format!("{name}-secret")).unwrap();
        }
        fs::create_dir(&profile).unwrap();
        fs::write(
            profile.join("stack.toml"),
            format!(
                r#"schema_version = 1
allow_development_builds = true
[binaries]
hubu = {}
hubu_server = {}
gongbu_server = {}
hubu_unified_mcp = {}
[identity]
account_id = "account-1"
agent_id = "agent-1"
[hubu]
ownership = "managed"
endpoint = "http://{hubu_addr}"
listen = "{hubu_addr}"
database_path = {}
log_file = {}
[gongbu]
ownership = "managed"
endpoint = "http://{gongbu_addr}"
listen = "{gongbu_addr}"
database_path = {}
artifact_root = {}
log_file = {}
[temporal]
mode = "external"
address = "http://127.0.0.1:7233"
namespace = "default"
task_queue = "gongbu-local"
"#,
                quote(binaries.join("hubu").display().to_string()),
                quote(binaries.join("hubu-server").display().to_string()),
                quote(binaries.join("gongbu-server").display().to_string()),
                quote(binaries.join("hubu-unified-mcp").display().to_string()),
                quote(profile.join("state/hubu.sqlite3").display().to_string()),
                quote(profile.join("logs/hubu.jsonl").display().to_string()),
                quote(profile.join("state/gongbu.sqlite3").display().to_string()),
                quote(profile.join("artifacts").display().to_string()),
                quote(profile.join("logs/gongbu.jsonl").display().to_string()),
            ),
        )
        .unwrap();
        fs::write(
            profile.join("credentials.toml"),
            format!(
                r#"schema_version = 1
[files]
hubu_auth = {}
hubu_approval = {}
hubu_reconciliation = {}
gongbu_caller = {}
[opaque.gongbu_hubu]
service = "hubu-test"
account = "gongbu-hubu"
[opaque.gongbu_caller]
service = "hubu-test"
account = "gongbu-caller"
"#,
                quote(credentials.join("auth").display().to_string()),
                quote(credentials.join("approval").display().to_string()),
                quote(credentials.join("reconciliation").display().to_string()),
                quote(credentials.join("gongbu-caller").display().to_string()),
            ),
        )
        .unwrap();
        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n",
        )
        .unwrap();

        assert!(files_needing_input(&profile).is_empty());
        drop(hubu_listener);
        drop(gongbu_listener);
        let initial =
            render_profile_with_renderer_outcome(&profile, &binaries.join("hubu")).unwrap();
        assert!(initial.activated);
        let active_path = profile.join("generated/active-manifest.json");
        let active = fs::read(&active_path).unwrap();
        let initial_manifest: ActiveManifest = read_json(&active_path).unwrap();
        let hubu_config: Value = read_json(
            &profile
                .join("generated")
                .join(&initial_manifest.generation)
                .join("hubu-launch.json"),
        )
        .unwrap();
        assert_eq!(hubu_config["auth_token_file_generated"], false);
        assert_eq!(hubu_config["approval_token_file_generated"], false);
        assert_eq!(hubu_config["reconciliation_token_file_generated"], false);
        let gongbu_config: Value = read_json(
            &profile
                .join("generated")
                .join(&initial_manifest.generation)
                .join("gongbu-server.json"),
        )
        .unwrap();
        assert_eq!(gongbu_config["schema_version"], 3);
        assert!(gongbu_config["hubu"].get("account_id").is_none());
        assert!(gongbu_config["hubu"].get("agent_id").is_none());
        assert!(gongbu_config["authentication"]
            .get("caller_account_id")
            .is_none());
        assert!(profile
            .join("generated")
            .join(&initial_manifest.generation)
            .join("manifest.json")
            .is_file());
        let handoff = codex_handoff(&profile, root.path()).unwrap();
        assert_eq!(handoff.hubu_endpoint, format!("http://{hubu_addr}"));
        fs::create_dir_all(profile.join("state")).unwrap();
        fs::write(
            profile.join("state/hubu.sqlite3"),
            b"simulated post-render agent registration",
        )
        .unwrap();
        render_profile_with_renderer(&profile, &binaries.join("hubu")).unwrap();
        assert_eq!(fs::read(&active_path).unwrap(), active);
        write_fake_binary(&binaries.join("gongbu-server"), false);
        let upgrade = render_profile_with_renderer(&profile, &binaries.join("hubu"))
            .unwrap_err()
            .to_string();
        assert!(upgrade.contains("static-principal server config schema version 2"));
        assert!(upgrade.contains("upgrade Gongbu"));
        assert_eq!(fs::read(&active_path).unwrap(), active);
        write_fake_gongbu_v3_binary(&binaries.join("gongbu-server"), true);
        assert!(render_profile_with_renderer(&profile, &binaries.join("hubu")).is_err());
        write_fake_gongbu_v3_binary(&binaries.join("gongbu-server"), false);

        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n# changed\n",
        )
        .unwrap();
        let staged =
            render_profile_with_renderer_outcome(&profile, &binaries.join("hubu")).unwrap();
        assert!(!staged.activated);
        assert_eq!(staged.changed_source_files, ["providers.toml"]);
        assert!(staged.affected_components.is_empty());
        assert_eq!(fs::read(&active_path).unwrap(), active);
        let staged_again =
            render_profile_with_renderer_outcome(&profile, &binaries.join("hubu")).unwrap();
        assert_eq!(staged_again, staged);
        assert_eq!(fs::read(&active_path).unwrap(), active);
        let staged_dir = profile
            .join("generated/generations")
            .join(&staged.generation_id);
        fs::write(staged_dir.join("manifest.json"), b"{}\n").unwrap();
        let recovered =
            render_profile_with_renderer_outcome(&profile, &binaries.join("hubu")).unwrap();
        assert_eq!(recovered, staged);
        assert_eq!(fs::read(&active_path).unwrap(), active);
        activate_selected_generation_with_renderer(
            &profile,
            &staged.generation_id,
            false,
            &binaries.join("hubu"),
        )
        .unwrap();
        let comment_only_active = fs::read(&active_path).unwrap();
        let comment_manifest: Value = read_json(&active_path).unwrap();
        assert_eq!(comment_manifest["restart_impact"], json!([]));
        let mismatch = activate_selected_generation_with_renderer(
            &profile,
            &initial_manifest.generation_id,
            true,
            &binaries.join("hubu"),
        )
        .unwrap_err()
        .to_string();
        assert!(mismatch.contains("does not match the current operator-owned source files"));
        assert_eq!(fs::read(&active_path).unwrap(), comment_only_active);

        let credentials_path = profile.join("credentials.toml");
        let original_credentials = fs::read_to_string(&credentials_path).unwrap();
        let rotated_auth = credentials.join("auth-rotated");
        fs::write(&rotated_auth, "rotated-secret-canary").unwrap();
        let rotated_credentials = original_credentials.replace(
            &quote(credentials.join("auth").display().to_string()),
            &quote(rotated_auth.display().to_string()),
        );
        fs::write(&credentials_path, &rotated_credentials).unwrap();
        let rotation =
            render_profile_with_renderer_outcome(&profile, &binaries.join("hubu")).unwrap();
        assert!(!rotation.activated);
        assert_eq!(rotation.changed_source_files, ["credentials.toml"]);
        assert_eq!(
            rotation.affected_components,
            ["hubu-server", "hubu-unified-mcp-client-config"]
        );
        assert_eq!(
            fs::read_to_string(&credentials_path).unwrap(),
            rotated_credentials
        );
        assert_eq!(fs::read(&active_path).unwrap(), comment_only_active);
        for entry in fs::read_dir(
            profile
                .join("generated/generations")
                .join(&rotation.generation_id),
        )
        .unwrap()
        {
            let path = entry.unwrap().path();
            if path.is_file() {
                assert!(!String::from_utf8_lossy(&fs::read(path).unwrap())
                    .contains("rotated-secret-canary"));
            }
        }
        let rotation_dir = profile
            .join("generated/generations")
            .join(&rotation.generation_id);
        let handoff_path = rotation_dir.join("client-handoff.json");
        let mut tampered_handoff: Value = read_json(&handoff_path).unwrap();
        tampered_handoff["hubu_endpoint"] = json!("http://127.0.0.1:9999");
        let tampered_bytes = serde_json::to_vec_pretty(&tampered_handoff).unwrap();
        fs::write(&handoff_path, &tampered_bytes).unwrap();
        let manifest_path = rotation_dir.join("manifest.json");
        let mut tampered_manifest: Value = read_json(&manifest_path).unwrap();
        tampered_manifest["generated_file_digests"]["client-handoff.json"] =
            json!(digest(&tampered_bytes));
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&tampered_manifest).unwrap(),
        )
        .unwrap();
        let tamper_error = render_profile_with_renderer_outcome(&profile, &binaries.join("hubu"))
            .unwrap_err()
            .to_string();
        assert!(tamper_error.contains("re-derived from current inputs"));
        fs::remove_dir_all(rotation_dir).unwrap();
        fs::write(&credentials_path, original_credentials).unwrap();

        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n# changed again\n",
        )
        .unwrap();
        write_fake_gongbu_v3_binary(&binaries.join("gongbu-server"), true);
        assert!(render_profile_with_renderer(&profile, &binaries.join("hubu")).is_err());
        assert_eq!(fs::read(&active_path).unwrap(), comment_only_active);

        write_fake_gongbu_v3_binary(&binaries.join("gongbu-server"), false);
        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n# changed\n",
        )
        .unwrap();
        let mut incomplete_manifest: Value = read_json(&active_path).unwrap();
        incomplete_manifest["generated_file_digests"]
            .as_object_mut()
            .unwrap()
            .remove("gongbu-server.json");
        fs::write(
            &active_path,
            serde_json::to_vec_pretty(&incomplete_manifest).unwrap(),
        )
        .unwrap();
        assert!(render_profile_with_renderer(&profile, &binaries.join("hubu")).is_err());
        fs::write(&active_path, &comment_only_active).unwrap();

        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n",
        )
        .unwrap();
        fs::create_dir_all(profile.join("runtime")).unwrap();
        fs::write(profile.join("runtime/launcher-state.json"), b"owned").unwrap();
        assert!(activate_selected_generation_with_renderer(
            &profile,
            &initial_manifest.generation_id,
            true,
            &binaries.join("hubu"),
        )
        .unwrap_err()
        .to_string()
        .contains("must be stopped"));
        fs::remove_file(profile.join("runtime/launcher-state.json")).unwrap();
        activate_selected_generation_with_renderer(
            &profile,
            &initial_manifest.generation_id,
            true,
            &binaries.join("hubu"),
        )
        .unwrap();
        let rolled_back: ActiveManifest = read_json(&active_path).unwrap();
        assert_eq!(rolled_back.generation_id, initial_manifest.generation_id);

        let manifest: ActiveManifest = read_json(&active_path).unwrap();
        let handoff_path = profile
            .join("generated")
            .join(manifest.generation)
            .join("client-handoff.json");
        fs::write(&handoff_path, b"{}\n").unwrap();
        assert!(codex_handoff(&profile, root.path()).is_err());
    }
}
