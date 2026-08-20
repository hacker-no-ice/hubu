use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const RUNTIME_STATE_SCHEMA_VERSION: u32 = 1;
const STATUS_SCHEMA_VERSION: u32 = 1;
const STATE_FILE: &str = "launcher-state.json";
const DEFAULT_LOG_LINES: usize = 200;
const MAX_LOG_LINES: usize = 10_000;
const MAX_LOG_READ_BYTES: u64 = 4 * 1024 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STOP_GRACE_FLOOR: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeState {
    schema_version: u32,
    profile: PathBuf,
    generation_id: String,
    launch_id: String,
    processes: BTreeMap<String, OwnedProcess>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedProcess {
    component: String,
    pid: u32,
    process_identity: String,
    binary: PathBuf,
    config: PathBuf,
    config_digest: String,
    generation_id: String,
    log_file: PathBuf,
    started_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordedProcessState {
    Running,
    Exited,
    IdentityMismatch,
}

#[derive(Clone, Debug, Serialize)]
struct StackStatusReport {
    schema_version: u32,
    profile: PathBuf,
    classification: String,
    generation_id: Option<String>,
    source_or_render_drift: bool,
    restart_impact: Vec<String>,
    components: Vec<ComponentStatus>,
    temporal: TemporalStatus,
    unified_mcp: UnifiedMcpStatus,
    commands: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
struct ComponentStatus {
    component: &'static str,
    ownership: &'static str,
    lifecycle: &'static str,
    ready: bool,
    pid: Option<u32>,
    log_file: Option<PathBuf>,
    guidance: String,
}

#[derive(Clone, Debug, Serialize)]
struct TemporalStatus {
    ownership: &'static str,
    ui_url: Option<String>,
    namespace: Option<String>,
    task_queue: Option<String>,
    worker_ready: bool,
}

#[derive(Clone, Debug, Serialize)]
struct UnifiedMcpStatus {
    lifecycle: &'static str,
    compatible: bool,
    guidance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComponentSelection {
    Hubu,
    Gongbu,
    All,
}

pub(super) fn start(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_start_help();
        return Ok(());
    }
    let confirm_restart = take_flag(&mut args, "--confirm-restart");
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let _lock = acquire_lifecycle_lock(&profile)?;
    start_profile(&profile, confirm_restart)
}

pub(super) fn status(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_status_help();
        return Ok(());
    }
    let json_output = take_flag(&mut args, "--json");
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let report = inspect_status(&profile)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_status(&report);
    }
    Ok(())
}

pub(super) fn logs(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_logs_help();
        return Ok(());
    }
    let component = take_value(&mut args, "--component")?
        .as_deref()
        .map(parse_component)
        .transpose()?
        .unwrap_or(ComponentSelection::All);
    let execution_id = take_value(&mut args, "--execution-id")?;
    let lines = take_value(&mut args, "--lines")?
        .map(|value| value.parse::<usize>().context("--lines must be an integer"))
        .transpose()?
        .unwrap_or(DEFAULT_LOG_LINES);
    if lines == 0 || lines > MAX_LOG_LINES {
        bail!("--lines must be between 1 and {MAX_LOG_LINES}");
    }
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    print_logs(&profile, component, execution_id.as_deref(), lines)
}

pub(super) fn restart(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_restart_help();
        return Ok(());
    }
    let component = take_value(&mut args, "--component")?
        .as_deref()
        .map(parse_component)
        .transpose()?
        .unwrap_or(ComponentSelection::All);
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let _lock = acquire_lifecycle_lock(&profile)?;
    restart_profile(&profile, component)
}

pub(super) fn stop(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_stop_help();
        return Ok(());
    }
    let forget_stale = take_flag(&mut args, "--forget-stale");
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let _lock = acquire_lifecycle_lock(&profile)?;
    stop_profile(&profile, ComponentSelection::All, forget_stale)
}

fn inspect_status(profile: &Path) -> Result<StackStatusReport> {
    let stack = read_toml::<StackSource>(&profile.join("stack.toml")).ok();
    let manifest = read_active_manifest(profile).ok();
    let state = read_runtime_state(profile)?;
    let doctor = doctor::inspect_profile(profile);
    let components = vec![
        component_status(
            "hubu-server",
            "hubu",
            stack
                .as_ref()
                .and_then(|value| value.hubu.as_ref())
                .and_then(|value| value.ownership),
            doctor.component_ready("hubu"),
            state.as_ref(),
        ),
        component_status(
            "gongbu-server",
            "gongbu",
            stack
                .as_ref()
                .and_then(|value| value.gongbu.as_ref())
                .and_then(|value| value.ownership),
            doctor.component_ready("gongbu"),
            state.as_ref(),
        ),
    ];
    let temporal = stack.as_ref().and_then(|value| value.temporal.as_ref());
    let gongbu_ownership = stack
        .as_ref()
        .and_then(|value| value.gongbu.as_ref())
        .and_then(|value| value.ownership);
    let profile_arg = shell_display(profile);
    let commands = BTreeMap::from([
        (
            "doctor".into(),
            format!("hubu stack doctor --profile {profile_arg}"),
        ),
        (
            "start".into(),
            format!("hubu stack start --profile {profile_arg}"),
        ),
        (
            "logs".into(),
            format!("hubu stack logs --profile {profile_arg}"),
        ),
        (
            "codex_init".into(),
            format!("hubu init codex --stack-profile {profile_arg}"),
        ),
        (
            "temporal_workflows".into(),
            temporal_workflow_command(temporal, gongbu_ownership),
        ),
        (
            "artifact_retrieval".into(),
            "use the authenticated Gongbu artifact endpoint for the execution artifact id".into(),
        ),
    ]);
    let generation_id = manifest.as_ref().map(|value| value.generation_id.clone());
    let source_or_render_drift = manifest.as_ref().is_none_or(|value| {
        source_digests(profile)
            .map(|digests| digests != value.source_digests)
            .unwrap_or(true)
    });
    let restart_impact = match (stack.as_ref(), manifest.as_ref(), state.as_ref()) {
        (Some(stack), Some(manifest), Some(state)) => {
            required_restart_plan(profile, stack, manifest, state)
                .unwrap_or_else(|_| manifest.restart_impact.clone())
        }
        _ => Vec::new(),
    };
    Ok(StackStatusReport {
        schema_version: STATUS_SCHEMA_VERSION,
        profile: profile.to_path_buf(),
        classification: classification_name(doctor.classification).into(),
        generation_id,
        source_or_render_drift,
        restart_impact,
        components,
        temporal: TemporalStatus {
            ownership: match (gongbu_ownership, temporal.and_then(|value| value.mode)) {
                (Some(Ownership::External), _) => "external_gongbu",
                (_, Some(TemporalMode::ManagedLocal)) => "gongbu_managed_local",
                (_, Some(TemporalMode::External)) => "external_temporal",
                _ => "unconfigured",
            },
            ui_url: temporal.and_then(|value| {
                value.ui_url.clone().or_else(|| {
                    (value.mode == Some(TemporalMode::ManagedLocal))
                        .then(|| format!("http://127.0.0.1:{}", value.ui_port.unwrap_or(8233)))
                })
            }),
            namespace: temporal.and_then(|value| value.namespace.clone()),
            task_queue: temporal.and_then(|value| value.task_queue.clone()),
            worker_ready: doctor.component_ready("gongbu"),
        },
        unified_mcp: UnifiedMcpStatus {
            lifecycle: "client_owned",
            compatible: doctor.check_passed("client_handoff_compatible"),
            guidance: format!("hubu init codex --stack-profile {profile_arg}"),
        },
        commands,
    })
}

fn component_status(
    state_key: &str,
    component: &'static str,
    ownership: Option<Ownership>,
    ready: bool,
    runtime: Option<&RuntimeState>,
) -> ComponentStatus {
    if ownership.is_none() {
        return ComponentStatus {
            component,
            ownership: "unconfigured",
            lifecycle: "unconfigured",
            ready,
            pid: None,
            log_file: None,
            guidance: "complete and validate stack.toml before lifecycle management".into(),
        };
    }
    if ownership == Some(Ownership::External) {
        return ComponentStatus {
            component,
            ownership: "external",
            lifecycle: if ready {
                "external_ready"
            } else {
                "external_unavailable"
            },
            ready,
            pid: None,
            log_file: None,
            guidance: "lifecycle and logs remain the external operator's responsibility".into(),
        };
    }
    let process = runtime.and_then(|value| value.processes.get(state_key));
    let (lifecycle, pid, log_file, guidance) = match process {
        Some(process) => match recorded_process_state(process) {
            RecordedProcessState::Running => (
                "owned_running",
                Some(process.pid),
                Some(process.log_file.clone()),
                "this profile may restart or stop the recorded process".into(),
            ),
            RecordedProcessState::Exited => (
                "owned_exited",
                Some(process.pid),
                Some(process.log_file.clone()),
                "run stack start to recover the missing managed component".into(),
            ),
            RecordedProcessState::IdentityMismatch => (
                "stale_identity",
                Some(process.pid),
                Some(process.log_file.clone()),
                "run stack stop --forget-stale after confirming the recorded process is no longer owned".into(),
            ),
        },
        None if ready => (
            "compatible_unowned",
            None,
            None,
            "a compatible pre-existing process is running and will not be signalled".into(),
        ),
        None => (
            "stopped",
            None,
            None,
            "run stack start to launch this managed component".into(),
        ),
    };
    ComponentStatus {
        component,
        ownership: "managed",
        lifecycle,
        ready,
        pid,
        log_file,
        guidance,
    }
}

fn print_status(report: &StackStatusReport) {
    println!("profile: {}", report.profile.display());
    println!("classification: {}", report.classification);
    println!(
        "generation: {}",
        report.generation_id.as_deref().unwrap_or("not rendered")
    );
    if report.source_or_render_drift {
        println!("source/render drift: yes");
    }
    if !report.restart_impact.is_empty() {
        println!("restart impact: {}", report.restart_impact.join(", "));
    }
    for component in &report.components {
        println!(
            "{}: ownership={}, lifecycle={}, ready={}{}",
            component.component,
            component.ownership,
            component.lifecycle,
            component.ready,
            component
                .pid
                .map(|pid| format!(", pid={pid}"))
                .unwrap_or_default()
        );
        println!("  {}", component.guidance);
    }
    println!(
        "Temporal: ownership={}, worker_ready={}, namespace={}, task_queue={}, ui={}",
        report.temporal.ownership,
        report.temporal.worker_ready,
        report.temporal.namespace.as_deref().unwrap_or("unknown"),
        report.temporal.task_queue.as_deref().unwrap_or("unknown"),
        report
            .temporal
            .ui_url
            .as_deref()
            .unwrap_or("not configured")
    );
    println!(
        "unified MCP: lifecycle={}, compatible={}",
        report.unified_mcp.lifecycle, report.unified_mcp.compatible
    );
    println!("  {}", report.unified_mcp.guidance);
    println!("commands:");
    for (name, command) in &report.commands {
        println!("  {name}: {command}");
    }
}

fn print_logs(
    profile: &Path,
    selection: ComponentSelection,
    execution_id: Option<&str>,
    lines: usize,
) -> Result<()> {
    let stack = read_toml::<StackSource>(&profile.join("stack.toml")).ok();
    let state = read_runtime_state(profile)?;
    let manifest = read_active_manifest(profile).ok();
    let selected = selected_components(selection);
    let mut printed = false;
    for (state_key, label, ownership) in [
        (
            "hubu-server",
            "hubu",
            stack
                .as_ref()
                .and_then(|value| value.hubu.as_ref())
                .and_then(|value| value.ownership),
        ),
        (
            "gongbu-server",
            "gongbu",
            stack
                .as_ref()
                .and_then(|value| value.gongbu.as_ref())
                .and_then(|value| value.ownership),
        ),
    ] {
        if !selected.contains(state_key) {
            continue;
        }
        if ownership == Some(Ownership::External) {
            println!("{label}: external logs remain the external operator's responsibility");
            continue;
        }
        let Some(path) = state
            .as_ref()
            .and_then(|value| value.processes.get(state_key))
            .map(|value| value.log_file.as_path())
        else {
            println!("{label}: no launcher-owned log has been recorded");
            continue;
        };
        let expected_log = manifest
            .as_ref()
            .and_then(|manifest| current_component_input(profile, manifest, state_key).ok())
            .map(|input| input.log_file);
        if expected_log.as_deref() != Some(path) {
            bail!(
                "refusing to read unauthenticated launcher log path for {label}; render or repair ownership metadata"
            );
        }
        println!("== {label} ({}) ==", path.display());
        for line in tail_lines(path, execution_id, lines)? {
            println!("{line}");
        }
        printed = true;
    }
    if !printed && execution_id.is_some() {
        println!("no launcher-owned log lines matched the execution id");
    }
    Ok(())
}

fn start_profile(profile: &Path, confirm_restart: bool) -> Result<()> {
    prepare_startable_profile(profile)?;
    let stack = read_toml::<StackSource>(&profile.join("stack.toml"))?;
    let manifest = read_active_manifest(profile)?;
    let mut state = read_runtime_state(profile)?.unwrap_or_else(|| RuntimeState {
        schema_version: RUNTIME_STATE_SCHEMA_VERSION,
        profile: profile.to_path_buf(),
        generation_id: manifest.generation_id.clone(),
        launch_id: Uuid::new_v4().to_string(),
        processes: BTreeMap::new(),
    });
    reconcile_exited_metadata(profile, &mut state)?;
    refuse_identity_mismatches(&state)?;

    let restart_plan = required_restart_plan(profile, &stack, &manifest, &state)?;
    if !restart_plan.is_empty() {
        println!("managed restart required: {}", restart_plan.join(", "));
        if !confirm_restart {
            bail!("rendered inputs changed; review the plan, then rerun with --confirm-restart");
        }
        stop_components(profile, selection_for_keys(&restart_plan), false)?;
        state = read_runtime_state(profile)?.unwrap_or_else(|| RuntimeState {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            profile: profile.to_path_buf(),
            generation_id: manifest.generation_id.clone(),
            launch_id: Uuid::new_v4().to_string(),
            processes: BTreeMap::new(),
        });
    }
    state.generation_id = manifest.generation_id.clone();
    write_or_remove_runtime_state(profile, &state)?;

    start_missing_components(profile, &stack, &manifest, state)
}

fn prepare_startable_profile(profile: &Path) -> Result<()> {
    let renderer = env::current_exe().context("locate the running hubu executable")?;
    let initial = doctor::inspect_profile(profile);
    if !initial.is_source_complete() {
        doctor::print_human(profile, &initial);
        bail!("stack source is incomplete or invalid; no process was started");
    }
    if !initial.is_startable() {
        render_profile_with_renderer(profile, &renderer)?;
    }
    let preflight = doctor::inspect_profile(profile);
    if !preflight.is_startable() {
        doctor::print_human(profile, &preflight);
        bail!("stack dependencies or credential references are not ready; no process was started");
    }
    Ok(())
}

fn start_missing_components(
    profile: &Path,
    stack: &StackSource,
    manifest: &ActiveManifest,
    mut state: RuntimeState,
) -> Result<()> {
    let mut started = Vec::<StartedProcess>::new();
    let result = (|| {
        if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed)
            && !doctor::inspect_profile(profile).component_ready("hubu")
        {
            refuse_duplicate_owned_spawn(&state, "hubu-server")?;
            let process = spawn_component(profile, manifest, "hubu-server")?;
            state
                .processes
                .insert("hubu-server".into(), process.record.clone());
            started.push(process);
            write_runtime_state(profile, &state)?;
            let timeout = Duration::from_millis(stack.runtime.hubu_startup_timeout_ms);
            wait_for_component(
                profile,
                "hubu",
                started.last_mut().expect("started"),
                timeout,
            )?;
            refresh_started_identity(&mut state, started.last_mut().expect("started"), profile)?;
        }

        if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed)
            && !doctor::inspect_profile(profile).component_ready("gongbu")
        {
            refuse_duplicate_owned_spawn(&state, "gongbu-server")?;
            if !doctor::inspect_profile(profile).component_ready("hubu") {
                bail!("Hubu did not pass its dependency gate before Gongbu startup");
            }
            let process = spawn_component(profile, manifest, "gongbu-server")?;
            state
                .processes
                .insert("gongbu-server".into(), process.record.clone());
            started.push(process);
            write_runtime_state(profile, &state)?;
            let timeout = Duration::from_millis(
                stack.runtime.temporal_startup_timeout_ms + stack.runtime.hubu_startup_timeout_ms,
            );
            wait_for_component(
                profile,
                "gongbu",
                started.last_mut().expect("started"),
                timeout,
            )?;
            refresh_started_identity(&mut state, started.last_mut().expect("started"), profile)?;
        }

        let final_report = doctor::inspect_profile(profile);
        if !final_report.is_running_ready() {
            doctor::print_human(profile, &final_report);
            bail!("stack did not reach running_ready");
        }
        Ok(())
    })();

    if let Err(error) = result {
        rollback_started_processes(profile, &mut state, &mut started);
        return Err(error);
    }
    drop(started);
    println!("stack running_ready: {}", profile.display());
    println!(
        "next: hubu init codex --stack-profile {}",
        profile.display()
    );
    Ok(())
}

fn restart_profile(profile: &Path, selection: ComponentSelection) -> Result<()> {
    prepare_rendered_profile(profile)?;
    let stack = read_toml::<StackSource>(&profile.join("stack.toml"))?;
    let expanded = if selection == ComponentSelection::Hubu {
        ComponentSelection::All
    } else {
        selection
    };
    validate_restart_dependencies(profile, &stack, expanded)?;
    if let Some(state) = read_runtime_state(profile)? {
        refuse_identity_mismatches(&state)?;
        let manifest = read_active_manifest(profile)?;
        let drift = required_restart_plan(profile, &stack, &manifest, &state)?;
        let allowed = selected_components(expanded);
        let outside_selection = drift
            .iter()
            .filter(|component| !allowed.contains(component.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !outside_selection.is_empty() {
            bail!(
                "restart scope would leave changed managed components running: {}; use stack start --confirm-restart or restart --component all",
                outside_selection.join(", ")
            );
        }
    }
    stop_components(profile, expanded, false)?;
    start_profile(profile, true)
}

fn prepare_rendered_profile(profile: &Path) -> Result<()> {
    let renderer = env::current_exe().context("locate the running hubu executable")?;
    let initial = doctor::inspect_profile(profile);
    if !initial.is_source_complete() {
        doctor::print_human(profile, &initial);
        bail!("stack source is incomplete or invalid; no process was stopped");
    }
    if !initial.check_passed("active_render_valid") {
        render_profile_with_renderer(profile, &renderer)?;
    }
    let rendered = doctor::inspect_profile(profile);
    if !rendered.is_renderable() || !rendered.check_passed("active_render_valid") {
        doctor::print_human(profile, &rendered);
        bail!("stack inputs are not renderable; no process was stopped");
    }
    Ok(())
}

fn validate_restart_dependencies(
    profile: &Path,
    stack: &StackSource,
    selection: ComponentSelection,
) -> Result<()> {
    let selected = selected_components(selection);
    let gongbu_managed =
        stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed);
    if !gongbu_managed || !selected.contains("gongbu-server") {
        return Ok(());
    }
    let report = doctor::inspect_profile(profile);
    let hubu_selected = stack.hubu.as_ref().and_then(|value| value.ownership)
        == Some(Ownership::Managed)
        && selected.contains("hubu-server");
    if !hubu_selected && !report.component_ready("hubu") {
        bail!("Hubu dependency is not ready; no managed process was stopped");
    }
    if stack.temporal.as_ref().and_then(|value| value.mode) == Some(TemporalMode::External)
        && !report.check_passed("temporal_reachable")
    {
        bail!("external Temporal dependency is not reachable; no managed process was stopped");
    }
    Ok(())
}

fn refuse_duplicate_owned_spawn(state: &RuntimeState, component: &str) -> Result<()> {
    if let Some(process) = state.processes.get(component) {
        bail!(
            "refusing to start a duplicate {component}: launcher-owned PID {} is alive but not ready; use stack restart after inspecting status and logs",
            process.pid
        );
    }
    Ok(())
}

fn stop_profile(profile: &Path, selection: ComponentSelection, forget_stale: bool) -> Result<()> {
    stop_components(profile, selection, forget_stale)
}

fn stop_components(
    profile: &Path,
    selection: ComponentSelection,
    forget_stale: bool,
) -> Result<()> {
    let Some(mut state) = read_runtime_state(profile)? else {
        println!("stack stop unchanged: no launcher-owned processes");
        return Ok(());
    };
    let worker_drain_timeout_ms = read_toml::<StackSource>(&profile.join("stack.toml"))
        .map(|stack| stack.runtime.worker_drain_timeout_ms)
        .unwrap_or_else(|_| RuntimePolicy::default().worker_drain_timeout_ms);
    let selected = selected_components(selection);
    let order = ["gongbu-server", "hubu-server"];
    let mut stale = Vec::new();
    for key in order {
        if !selected.contains(key) {
            continue;
        }
        let Some(process) = state.processes.get(key).cloned() else {
            continue;
        };
        match recorded_process_state(&process) {
            RecordedProcessState::Exited => {
                state.processes.remove(key);
            }
            RecordedProcessState::IdentityMismatch if forget_stale => {
                println!(
                    "forgot stale {key} ownership metadata without signalling PID {}",
                    process.pid
                );
                state.processes.remove(key);
            }
            RecordedProcessState::IdentityMismatch => {
                stale.push(format!("{key} PID {}", process.pid))
            }
            RecordedProcessState::Running => {
                let grace = if key == "gongbu-server" {
                    Duration::from_millis(worker_drain_timeout_ms).saturating_add(STOP_GRACE_FLOOR)
                } else {
                    STOP_GRACE_FLOOR
                };
                stop_owned_process(&process, grace)?;
                state.processes.remove(key);
                println!("stopped {key} PID {}", process.pid);
            }
        }
        write_or_remove_runtime_state(profile, &state)?;
    }
    if !stale.is_empty() {
        bail!(
            "stale ownership metadata was not signalled: {}; inspect stack status, then use --forget-stale only after confirming ownership is gone",
            stale.join(", ")
        );
    }
    write_or_remove_runtime_state(profile, &state)?;
    Ok(())
}

struct StartedProcess {
    record: OwnedProcess,
    child: Child,
}

fn spawn_component(
    profile: &Path,
    manifest: &ActiveManifest,
    component: &str,
) -> Result<StartedProcess> {
    let generation = active_generation_path(&profile.join("generated"), manifest)?;
    let (config_name, default_log_name) = match component {
        "hubu-server" => ("hubu-launch.json", "hubu-server.log"),
        "gongbu-server" => ("gongbu-server.json", "gongbu-server.log"),
        _ => bail!("unsupported managed component `{component}`"),
    };
    let binary = manifest
        .binary_provenance
        .iter()
        .find(|value| value.component == component)
        .map(|value| value.path.clone())
        .ok_or_else(|| anyhow!("active manifest has no `{component}` binary provenance"))?;
    let config = generation.join(config_name);
    verify_generated_file(&generation, manifest, config_name)?;
    let config_digest = manifest
        .generated_file_digests
        .get(config_name)
        .cloned()
        .ok_or_else(|| anyhow!("active manifest has no `{config_name}` digest"))?;
    let log_file = manifest
        .process_log_files
        .get(component)
        .and_then(Clone::clone)
        .unwrap_or_else(|| runtime_dir(profile).join("logs").join(default_log_name));
    if let Some(parent) = log_file.parent() {
        create_secure_dir(parent)?;
    }
    let log = open_append_secure(&log_file)?;
    let stderr = log.try_clone()?;
    let mut command = Command::new(&binary);
    command
        .args(["serve", "--config"])
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    for name in [
        "HUBU_AUTH_TOKEN",
        "HUBU_APPROVAL_TOKEN",
        "HUBU_RECONCILIATION_TOKEN",
        "HUBU_DB_PATH",
        "HUBU_LOG_FILE",
        "HUBU_LOG_STDERR",
    ] {
        command.env_remove(name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("start `{component}` from `{}`", binary.display()))?;
    let identity_deadline = Instant::now() + Duration::from_secs(2);
    let identity = loop {
        if let Some(identity) = process_identity(child.id()) {
            break identity;
        }
        if let Some(status) = child.try_wait()? {
            bail!("{component} exited before ownership identity was recorded: {status}");
        }
        if Instant::now() >= identity_deadline {
            let _ = terminate_child_group(&mut child);
            bail!("could not record a stable process identity for {component}");
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    };
    Ok(StartedProcess {
        record: OwnedProcess {
            component: component.into(),
            pid: child.id(),
            process_identity: identity,
            binary,
            config,
            config_digest,
            generation_id: manifest.generation_id.clone(),
            log_file,
            started_at: chrono::Utc::now().to_rfc3339(),
        },
        child,
    })
}

fn wait_for_component(
    profile: &Path,
    component: &str,
    process: &mut StartedProcess,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if doctor::inspect_profile(profile).component_ready(component) {
            return Ok(());
        }
        if let Some(status) = process.child.try_wait()? {
            bail!("{component} exited before readiness with {status}");
        }
        if Instant::now() >= deadline {
            bail!(
                "{component} readiness timed out after {} ms",
                timeout.as_millis()
            );
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

fn refresh_started_identity(
    state: &mut RuntimeState,
    process: &mut StartedProcess,
    profile: &Path,
) -> Result<()> {
    let identity = process_identity(process.record.pid)
        .ok_or_else(|| anyhow!("managed process exited while finalizing ownership metadata"))?;
    process.record.process_identity = identity;
    state
        .processes
        .insert(process.record.component.clone(), process.record.clone());
    write_runtime_state(profile, state)
}

fn rollback_started_processes(
    profile: &Path,
    state: &mut RuntimeState,
    started: &mut [StartedProcess],
) {
    for process in started.iter_mut().rev() {
        let _ = terminate_child_group(&mut process.child);
        state.processes.remove(&process.record.component);
    }
    let _ = write_or_remove_runtime_state(profile, state);
}

fn terminate_child_group(child: &mut Child) -> Result<()> {
    signal_process_group(child.id(), "TERM")?;
    let deadline = Instant::now() + STOP_GRACE_FLOOR;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    signal_process_group(child.id(), "KILL")?;
    let _ = child.wait();
    Ok(())
}

fn stop_owned_process(process: &OwnedProcess, grace: Duration) -> Result<()> {
    if recorded_process_state(process) != RecordedProcessState::Running {
        bail!(
            "refusing to signal {} because its process identity changed",
            process.component
        );
    }
    signal_process(process.pid, "TERM")?;
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if recorded_process_state(process) != RecordedProcessState::Running {
            return Ok(());
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    if recorded_process_state(process) == RecordedProcessState::Running {
        signal_process_group(process.pid, "KILL")?;
    }
    let hard_deadline = Instant::now() + STOP_GRACE_FLOOR;
    while Instant::now() < hard_deadline {
        if recorded_process_state(process) != RecordedProcessState::Running {
            return Ok(());
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    bail!(
        "{} did not exit after graceful and hard termination",
        process.component
    )
}

fn signal_process(pid: u32, signal: &str) -> Result<()> {
    let status = Command::new("/bin/kill")
        .args([format!("-{signal}"), pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        bail!("failed to send SIG{signal} to PID {pid}");
    }
    Ok(())
}

fn signal_process_group(pid: u32, signal: &str) -> Result<()> {
    let status = Command::new("/bin/kill")
        .args([format!("-{signal}"), "--".into(), format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        bail!("failed to send SIG{signal} to process group {pid}");
    }
    Ok(())
}

fn open_append_secure(path: &Path) -> Result<File> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!(
            "refusing to use symlinked launcher log `{}`",
            path.display()
        );
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open launcher log `{}`", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn reconcile_exited_metadata(profile: &Path, state: &mut RuntimeState) -> Result<()> {
    state
        .processes
        .retain(|_, process| recorded_process_state(process) != RecordedProcessState::Exited);
    write_or_remove_runtime_state(profile, state)
}

fn refuse_identity_mismatches(state: &RuntimeState) -> Result<()> {
    let stale = state
        .processes
        .values()
        .filter(|process| recorded_process_state(process) == RecordedProcessState::IdentityMismatch)
        .map(|process| format!("{} PID {}", process.component, process.pid))
        .collect::<Vec<_>>();
    if stale.is_empty() {
        Ok(())
    } else {
        bail!(
            "stale launcher ownership metadata detected for {}; no PID was signalled; inspect status and use stack stop --forget-stale after confirming ownership",
            stale.join(", ")
        )
    }
}

fn required_restart_plan(
    profile: &Path,
    stack: &StackSource,
    manifest: &ActiveManifest,
    state: &RuntimeState,
) -> Result<Vec<String>> {
    let mut keys = BTreeSet::new();
    for key in ["hubu-server", "gongbu-server"] {
        let Some(process) = state.processes.get(key) else {
            continue;
        };
        let ownership = if key == "hubu-server" {
            stack.hubu.as_ref().and_then(|value| value.ownership)
        } else {
            stack.gongbu.as_ref().and_then(|value| value.ownership)
        };
        if ownership != Some(Ownership::Managed) {
            keys.insert(key.to_owned());
            continue;
        }
        let current = current_component_input(profile, manifest, key)?;
        if process.binary != current.binary
            || process.config_digest != current.config_digest
            || process.log_file != current.log_file
        {
            keys.insert(key.to_owned());
        }
    }
    if keys.contains("hubu-server") && state.processes.contains_key("gongbu-server") {
        keys.insert("gongbu-server".into());
    }
    let order = ["gongbu-server", "hubu-server"];
    Ok(order
        .into_iter()
        .filter(|key| keys.contains(*key))
        .map(str::to_owned)
        .collect())
}

struct ComponentInput {
    binary: PathBuf,
    config_digest: String,
    log_file: PathBuf,
}

fn current_component_input(
    profile: &Path,
    manifest: &ActiveManifest,
    component: &str,
) -> Result<ComponentInput> {
    let (config_name, default_log_name) = match component {
        "hubu-server" => ("hubu-launch.json", "hubu-server.log"),
        "gongbu-server" => ("gongbu-server.json", "gongbu-server.log"),
        _ => bail!("unknown managed component `{component}`"),
    };
    let binary = manifest
        .binary_provenance
        .iter()
        .find(|value| value.component == component)
        .map(|value| value.path.clone())
        .ok_or_else(|| anyhow!("active manifest has no `{component}` binary"))?;
    let config_digest = manifest
        .generated_file_digests
        .get(config_name)
        .cloned()
        .ok_or_else(|| anyhow!("active manifest has no `{config_name}` digest"))?;
    let log_file = manifest
        .process_log_files
        .get(component)
        .and_then(Clone::clone)
        .unwrap_or_else(|| runtime_dir(profile).join("logs").join(default_log_name));
    Ok(ComponentInput {
        binary,
        config_digest,
        log_file,
    })
}

fn source_digests(profile: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for name in ["credentials.toml", "providers.toml", "stack.toml"] {
        values.insert(name.to_owned(), digest(&fs::read(profile.join(name))?));
    }
    Ok(values)
}

fn selection_for_keys(keys: &[String]) -> ComponentSelection {
    let hubu = keys.iter().any(|key| key == "hubu-server");
    let gongbu = keys.iter().any(|key| key == "gongbu-server");
    match (hubu, gongbu) {
        (true, _) => ComponentSelection::All,
        (false, true) => ComponentSelection::Gongbu,
        (false, false) => ComponentSelection::All,
    }
}

fn write_or_remove_runtime_state(profile: &Path, state: &RuntimeState) -> Result<()> {
    if state.processes.is_empty() {
        remove_runtime_state(profile)
    } else {
        write_runtime_state(profile, state)
    }
}

fn tail_lines(path: &Path, filter: Option<&str>, limit: usize) -> Result<Vec<String>> {
    let mut file = File::open(path).with_context(|| format!("open log `{}`", path.display()))?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_LOG_READ_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_READ_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut selected = text
        .lines()
        .filter(|line| filter.is_none_or(|needle| line.contains(needle)))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if selected.len() > limit {
        selected.drain(..selected.len() - limit);
    }
    Ok(selected)
}

fn runtime_dir(profile: &Path) -> PathBuf {
    profile.join("runtime")
}

#[derive(Debug)]
struct LifecycleLock {
    _file: File,
}

fn acquire_lifecycle_lock(profile: &Path) -> Result<LifecycleLock> {
    if !profile.is_dir() {
        bail!(
            "stack profile `{}` does not exist; run stack init first",
            profile.display()
        );
    }
    create_secure_dir(&runtime_dir(profile))?;
    let path = runtime_dir(profile).join("lifecycle.lock");
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(&path)
            .with_context(|| format!("open lifecycle lock `{}`", path.display()))?;
        // SAFETY: flock only observes this live file descriptor; LifecycleLock owns the
        // File for the full mutation and Drop closes it, releasing the kernel lock.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                bail!(
                    "another lifecycle command is already operating on profile `{}`",
                    profile.display()
                );
            }
            return Err(error).with_context(|| format!("lock `{}`", path.display()));
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(LifecycleLock { _file: file })
    }
    #[cfg(not(unix))]
    {
        let _ = options;
        bail!("stack lifecycle locking is not supported on this platform")
    }
}

fn runtime_state_path(profile: &Path) -> PathBuf {
    runtime_dir(profile).join(STATE_FILE)
}

fn read_runtime_state(profile: &Path) -> Result<Option<RuntimeState>> {
    let path = runtime_state_path(profile);
    if !path.exists() {
        return Ok(None);
    }
    let state: RuntimeState = read_json(&path)
        .with_context(|| format!("read launcher ownership metadata `{}`", path.display()))?;
    if state.schema_version != RUNTIME_STATE_SCHEMA_VERSION || state.profile != profile {
        bail!("launcher ownership metadata is incompatible with this profile");
    }
    let mut pids = BTreeSet::new();
    for (key, process) in &state.processes {
        if !matches!(key.as_str(), "hubu-server" | "gongbu-server")
            || process.component != *key
            || process.pid == 0
            || !pids.insert(process.pid)
            || !process.binary.is_absolute()
            || !process.config.is_absolute()
            || !process.log_file.is_absolute()
        {
            bail!("launcher ownership metadata contains an invalid process record");
        }
    }
    Ok(Some(state))
}

fn write_runtime_state(profile: &Path, state: &RuntimeState) -> Result<()> {
    create_secure_dir(&runtime_dir(profile))?;
    let path = runtime_state_path(profile);
    let temp = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    write_json_secure(&temp, &serde_json::to_value(state)?)?;
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("activate `{}`", path.display()));
    }
    Ok(())
}

fn remove_runtime_state(profile: &Path) -> Result<()> {
    let path = runtime_state_path(profile);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove `{}`", path.display()))?;
    }
    Ok(())
}

fn recorded_process_state(process: &OwnedProcess) -> RecordedProcessState {
    match process_identity(process.pid) {
        None => RecordedProcessState::Exited,
        Some(identity) if identity == process.process_identity => RecordedProcessState::Running,
        Some(_) => RecordedProcessState::IdentityMismatch,
    }
}

fn process_identity(pid: u32) -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "lstart=", "-o", "command="])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 32 * 1024 {
        return None;
    }
    let identity = String::from_utf8(output.stdout).ok()?;
    let normalized = identity.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

fn read_active_manifest(profile: &Path) -> Result<ActiveManifest> {
    let manifest: ActiveManifest = read_json(&profile.join("generated/active-manifest.json"))?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        bail!("active manifest has an unsupported schema version");
    }
    active_generation_path(&profile.join("generated"), &manifest)?;
    Ok(manifest)
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("read `{}`", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse `{}`", path.display()))
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_value(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    args.remove(index);
    if index >= args.len() || args[index].starts_with('-') {
        bail!("missing value for {name}");
    }
    Ok(Some(args.remove(index)))
}

fn parse_component(value: &str) -> Result<ComponentSelection> {
    match value {
        "hubu" | "hubu-server" => Ok(ComponentSelection::Hubu),
        "gongbu" | "gongbu-server" => Ok(ComponentSelection::Gongbu),
        "all" => Ok(ComponentSelection::All),
        _ => bail!("--component must be hubu, gongbu, or all"),
    }
}

fn selected_components(selection: ComponentSelection) -> BTreeSet<&'static str> {
    match selection {
        ComponentSelection::Hubu => BTreeSet::from(["hubu-server"]),
        ComponentSelection::Gongbu => BTreeSet::from(["gongbu-server"]),
        ComponentSelection::All => BTreeSet::from(["hubu-server", "gongbu-server"]),
    }
}

fn classification_name(value: doctor::ProfileClassification) -> &'static str {
    match value {
        doctor::ProfileClassification::Invalid => "invalid",
        doctor::ProfileClassification::Incomplete => "incomplete",
        doctor::ProfileClassification::ReadyToRender => "ready_to_render",
        doctor::ProfileClassification::ReadyToStart => "ready_to_start",
        doctor::ProfileClassification::RunningReady => "running_ready",
    }
}

fn temporal_workflow_command(
    temporal: Option<&TemporalSource>,
    gongbu_ownership: Option<Ownership>,
) -> String {
    if gongbu_ownership == Some(Ownership::External) {
        return "consult the external Gongbu operator for its Temporal workflow endpoint".into();
    }
    let namespace = temporal
        .and_then(|value| value.namespace.as_deref())
        .unwrap_or("default");
    match temporal.and_then(|value| value.mode) {
        Some(TemporalMode::ManagedLocal) => format!(
            "temporal workflow list --address 127.0.0.1:{} --namespace {}",
            temporal.and_then(|value| value.rpc_port).unwrap_or(7233),
            shell_word(namespace)
        ),
        Some(TemporalMode::External) => format!(
            "temporal workflow list --address <configured-external-address> --namespace {}",
            shell_word(namespace)
        ),
        None => "complete stack.toml:temporal before listing workflows".into(),
    }
}

fn shell_display(path: &Path) -> String {
    shell_word(&path.display().to_string())
}

fn shell_word(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn print_start_help() {
    println!(
        "Start or reconcile managed local stack components\n\nUsage:\n  hubu stack start [--profile ABSOLUTE_DIR] [--confirm-restart]\n\n--confirm-restart permits the displayed affected-component restart plan after rendered configuration changes"
    );
}

fn print_status_help() {
    println!(
        "Show component-aware local stack state without changing it\n\nUsage:\n  hubu stack status [--profile ABSOLUTE_DIR] [--json]"
    );
}

fn print_logs_help() {
    println!(
        "Read launcher-owned managed component logs\n\nUsage:\n  hubu stack logs [--profile ABSOLUTE_DIR] [--component hubu|gongbu|all] [--execution-id ID] [--lines N]"
    );
}

fn print_restart_help() {
    println!(
        "Explicitly restart launcher-owned components in dependency order\n\nUsage:\n  hubu stack restart [--profile ABSOLUTE_DIR] [--component hubu|gongbu|all]"
    );
}

fn print_stop_help() {
    println!(
        "Stop launcher-owned components in reverse dependency order\n\nUsage:\n  hubu stack stop [--profile ABSOLUTE_DIR] [--forget-stale]\n\n--forget-stale removes stale ownership metadata only after refusing to signal the mismatched PID"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn process(component: &str, pid: u32, identity: String) -> OwnedProcess {
        OwnedProcess {
            component: component.into(),
            pid,
            process_identity: identity,
            binary: PathBuf::from(format!("/tmp/{component}")),
            config: PathBuf::from(format!("/tmp/{component}.json")),
            config_digest: "sha256:old".into(),
            generation_id: "generation-old".into(),
            log_file: PathBuf::from(format!("/tmp/{component}.log")),
            started_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn manifest(root: &Path) -> ActiveManifest {
        ActiveManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            generation_id: "a".repeat(64),
            generation: format!("generations/{}", "a".repeat(64)),
            source_digests: BTreeMap::new(),
            generated_file_digests: BTreeMap::from([
                ("hubu-launch.json".into(), "sha256:new-hubu".into()),
                ("gongbu-server.json".into(), "sha256:old".into()),
            ]),
            binary_provenance: vec![
                BinaryProvenance {
                    component: "hubu-server".into(),
                    path: PathBuf::from("/tmp/hubu-server"),
                    product_version: "1".into(),
                    source_commit: "commit".into(),
                    executor_contract: "contract".into(),
                    server_config_schema_version: None,
                },
                BinaryProvenance {
                    component: "gongbu-server".into(),
                    path: PathBuf::from("/tmp/gongbu-server"),
                    product_version: "1".into(),
                    source_commit: "commit".into(),
                    executor_contract: "contract".into(),
                    server_config_schema_version: Some(2),
                },
            ],
            process_log_files: BTreeMap::from([
                (
                    "hubu-server".into(),
                    Some(root.join("runtime/logs/hubu-server.log")),
                ),
                (
                    "gongbu-server".into(),
                    Some(root.join("runtime/logs/gongbu-server.log")),
                ),
            ]),
            restart_impact: Vec::new(),
        }
    }

    #[test]
    fn stale_identity_is_never_signalled_and_may_be_explicitly_forgotten() {
        let temp = tempdir().unwrap();
        let profile = temp.path();
        fs::write(
            profile.join("stack.toml"),
            "schema_version = 1\n[runtime]\n",
        )
        .unwrap();
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let Some(actual_identity) = process_identity(child.id()) else {
            child.kill().unwrap();
            child.wait().unwrap();
            return;
        };
        let state = RuntimeState {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            profile: profile.to_path_buf(),
            generation_id: "generation".into(),
            launch_id: "launch".into(),
            processes: BTreeMap::from([(
                "hubu-server".into(),
                process(
                    "hubu-server",
                    child.id(),
                    format!("{actual_identity} deliberately-wrong"),
                ),
            )]),
        };
        write_runtime_state(profile, &state).unwrap();

        let error = stop_components(profile, ComponentSelection::All, false).unwrap_err();
        assert!(error.to_string().contains("stale ownership metadata"));
        assert!(child.try_wait().unwrap().is_none());

        stop_components(profile, ComponentSelection::All, true).unwrap();
        assert!(!runtime_state_path(profile).exists());
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn hubu_input_change_expands_restart_to_owned_gongbu_dependent() {
        let temp = tempdir().unwrap();
        let profile = temp.path();
        let stack: StackSource = toml::from_str(
            r#"schema_version = 1
[hubu]
ownership = "managed"
[gongbu]
ownership = "managed"
"#,
        )
        .unwrap();
        let manifest = manifest(profile);
        let mut hubu = process("hubu-server", 1, "identity".into());
        hubu.log_file = profile.join("runtime/logs/hubu-server.log");
        let mut gongbu = process("gongbu-server", 2, "identity".into());
        gongbu.log_file = profile.join("runtime/logs/gongbu-server.log");
        let state = RuntimeState {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            profile: profile.to_path_buf(),
            generation_id: "old".into(),
            launch_id: "launch".into(),
            processes: BTreeMap::from([
                ("hubu-server".into(), hubu),
                ("gongbu-server".into(), gongbu),
            ]),
        };

        assert_eq!(
            required_restart_plan(profile, &stack, &manifest, &state).unwrap(),
            vec!["gongbu-server", "hubu-server"]
        );
    }

    #[test]
    fn log_tail_is_bounded_and_filters_execution_correlation() {
        let temp = tempdir().unwrap();
        let log = temp.path().join("hubu.log");
        fs::write(
            &log,
            "old execution-a\nkeep execution-b\nnew execution-a\nlast execution-a\n",
        )
        .unwrap();
        assert_eq!(
            tail_lines(&log, Some("execution-a"), 2).unwrap(),
            vec!["new execution-a", "last execution-a"]
        );
    }

    #[test]
    fn lifecycle_value_options_reject_a_missing_value() {
        for name in ["--component", "--execution-id", "--lines"] {
            let mut args = vec![name.to_owned(), "--profile".to_owned()];
            assert!(take_value(&mut args, name)
                .unwrap_err()
                .to_string()
                .contains("missing value"));
        }
    }

    #[test]
    fn lifecycle_lock_rejects_a_concurrent_mutation_and_releases_on_drop() {
        let temp = tempdir().unwrap();
        let first = acquire_lifecycle_lock(temp.path()).unwrap();
        assert!(acquire_lifecycle_lock(temp.path())
            .unwrap_err()
            .to_string()
            .contains("already operating"));
        drop(first);
        acquire_lifecycle_lock(temp.path()).unwrap();
    }

    #[test]
    fn live_owned_record_blocks_duplicate_spawn() {
        let state = RuntimeState {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            profile: PathBuf::from("/tmp/profile"),
            generation_id: "generation".into(),
            launch_id: "launch".into(),
            processes: BTreeMap::from([(
                "hubu-server".into(),
                process("hubu-server", 42, "identity".into()),
            )]),
        };
        assert!(refuse_duplicate_owned_spawn(&state, "hubu-server")
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        assert!(refuse_duplicate_owned_spawn(&state, "gongbu-server").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn launcher_log_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let target = temp.path().join("target.log");
        let link = temp.path().join("launcher.log");
        fs::write(&target, "do-not-append").unwrap();
        symlink(&target, &link).unwrap();
        assert!(open_append_secure(&link)
            .unwrap_err()
            .to_string()
            .contains("symlinked launcher log"));
        assert_eq!(fs::read_to_string(target).unwrap(), "do-not-append");
    }

    #[test]
    fn failed_start_rollback_terminates_only_recorded_children_and_clears_state() {
        #[cfg(unix)]
        use std::os::unix::process::CommandExt;

        let temp = tempdir().unwrap();
        let profile = temp.path();
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().unwrap();
        let record = process("hubu-server", child.id(), "rollback-owned-child".into());
        let mut state = RuntimeState {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            profile: profile.to_path_buf(),
            generation_id: "generation".into(),
            launch_id: "launch".into(),
            processes: BTreeMap::from([("hubu-server".into(), record.clone())]),
        };
        write_runtime_state(profile, &state).unwrap();
        let mut started = vec![StartedProcess { record, child }];

        rollback_started_processes(profile, &mut state, &mut started);

        assert!(state.processes.is_empty());
        assert!(!runtime_state_path(profile).exists());
        assert!(started[0].child.try_wait().unwrap().is_some());
    }
}
