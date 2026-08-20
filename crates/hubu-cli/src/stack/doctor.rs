use super::*;
use reqwest::{blocking::Client, redirect::Policy, StatusCode};
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::time::Instant;
use std::{collections::BTreeSet, net::ToSocketAddrs, time::Duration};

const REPORT_SCHEMA_VERSION: u32 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProfileClassification {
    Invalid,
    Incomplete,
    ReadyToRender,
    ReadyToStart,
    RunningReady,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProviderReadiness {
    Unknown,
    Disabled,
    FixtureOnly,
    LiveReady,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Pass,
    Fail,
    Warning,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckLayer {
    SourceSyntax,
    Completeness,
    Renderability,
    RuntimeReadiness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceProbe {
    NotRunning,
    Failed,
    Ready,
}

#[derive(Clone, Debug, Serialize)]
struct DoctorCheck {
    layer: CheckLayer,
    status: CheckStatus,
    code: &'static str,
    component: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct DoctorReport {
    schema_version: u32,
    pub(super) classification: ProfileClassification,
    pub(super) provider_readiness: ProviderReadiness,
    checks: Vec<DoctorCheck>,
}

pub(super) fn command(mut args: Vec<String>, hubu_home: &Path) -> Result<()> {
    if take_help(&mut args) {
        print_help();
        return Ok(());
    }
    let json_output = take_flag(&mut args, "--json");
    let profile = take_profile(&mut args, hubu_home)?;
    ensure_no_args(args)?;
    let report = inspect_profile(&profile);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&profile, &report);
    }
    Ok(())
}

pub(super) fn inspect_profile(profile: &Path) -> DoctorReport {
    let renderer = env::current_exe().ok();
    inspect_profile_with(profile, opaque_reference_exists, renderer.as_deref())
}

fn inspect_profile_with(
    profile: &Path,
    opaque_probe: fn(&OpaqueReference) -> Option<bool>,
    renderer: Option<&Path>,
) -> DoctorReport {
    let mut report = DoctorReport {
        schema_version: REPORT_SCHEMA_VERSION,
        classification: ProfileClassification::Invalid,
        provider_readiness: ProviderReadiness::Unknown,
        checks: Vec::new(),
    };

    let stack = read_source::<StackSource>(profile, "stack.toml", "stack", &mut report.checks);
    let credentials = read_source::<CredentialsSource>(
        profile,
        "credentials.toml",
        "credentials",
        &mut report.checks,
    );
    let providers =
        read_source::<ProvidersSource>(profile, "providers.toml", "providers", &mut report.checks);
    let (
        Some((stack, stack_bytes)),
        Some((credentials, credential_bytes)),
        Some((providers, provider_bytes)),
    ) = (stack, credentials, providers)
    else {
        return report;
    };
    report.provider_readiness = provider_readiness(&providers);

    let schemas = [
        ("stack", "stack.toml:schema_version", stack.schema_version),
        (
            "credentials",
            "credentials.toml:schema_version",
            credentials.schema_version,
        ),
        (
            "providers",
            "providers.toml:schema_version",
            providers.schema_version,
        ),
    ];
    let mut schema_valid = true;
    for (component, field, version) in schemas {
        if version == SOURCE_SCHEMA_VERSION {
            report.checks.push(check(
                CheckLayer::SourceSyntax,
                CheckStatus::Pass,
                "source_schema_supported",
                component,
                Some(field.to_owned()),
                "source schema is supported",
            ));
        } else {
            schema_valid = false;
            report.checks.push(check(
                CheckLayer::SourceSyntax,
                CheckStatus::Fail,
                "source_schema_unsupported",
                component,
                Some(field.to_owned()),
                "set schema_version to a supported value",
            ));
        }
    }
    if !schema_valid {
        return report;
    }

    let missing = missing_fields(&stack, &credentials, &providers);
    if !missing.is_empty() {
        report.classification = ProfileClassification::Incomplete;
        for field in missing {
            let component = component_for_field(&field);
            report.checks.push(check(
                CheckLayer::Completeness,
                CheckStatus::Fail,
                "required_decision_missing",
                component,
                Some(field),
                "fill this decision using the annotated starter file",
            ));
        }
        return report;
    }
    report.checks.push(check(
        CheckLayer::Completeness,
        CheckStatus::Pass,
        "required_decisions_complete",
        "profile",
        None,
        "all decisions required by the selected topology are present",
    ));

    let mut source_constraints_valid = true;
    if validate_provider_source(&providers).is_err() {
        source_constraints_valid = false;
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "provider_configuration_invalid",
            "providers",
            Some("providers.toml".into()),
            "provider mode, spend gates, and supplied provider fields are contradictory",
        ));
    }
    let fixture_only = providers.targets.iter().any(|target| {
        target
            .adapter
            .as_deref()
            .is_some_and(|adapter| adapter == "fixture")
    });
    if fixture_only {
        source_constraints_valid = false;
        report.provider_readiness = ProviderReadiness::FixtureOnly;
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "fixture_provider_not_production_renderable",
            "providers",
            Some("providers.toml:targets".into()),
            "fixture adapters are suitable only for deterministic fixture execution",
        ));
    }
    if source_constraints_valid && providers.mode == Some(ProviderMode::Live) {
        report.provider_readiness = ProviderReadiness::LiveReady;
    }
    if validate_topology(&stack).is_err() {
        source_constraints_valid = false;
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "topology_invalid",
            "stack",
            Some("stack.toml".into()),
            "topology contains an unsafe, conflicting, or incoherent endpoint, port, or resource",
        ));
    }
    if !source_constraints_valid {
        return report;
    }
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "source_constraints_valid",
        "profile",
        None,
        "topology and provider source constraints are coherent",
    ));

    let Some(binaries) = stack.binaries.as_ref() else {
        return report;
    };
    let mut binary_fields = vec![("hubu", "stack.toml:binaries.hubu", binaries.hubu.as_deref())];
    if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        binary_fields.push((
            "hubu-server",
            "stack.toml:binaries.hubu_server",
            binaries.hubu_server.as_deref(),
        ));
    }
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        binary_fields.push((
            "gongbu-server",
            "stack.toml:binaries.gongbu_server",
            binaries.gongbu_server.as_deref(),
        ));
    }
    binary_fields.push((
        "hubu-unified-mcp",
        "stack.toml:binaries.hubu_unified_mcp",
        binaries.hubu_unified_mcp.as_deref(),
    ));
    let expected_binary_count = binary_fields.len();
    let mut resolved_binaries = BTreeMap::new();
    let mut provenances = Vec::new();
    for (component, field, path) in binary_fields {
        let Some(path) = path.and_then(|path| existing_absolute(path, field).ok()) else {
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "binary_unavailable",
                component,
                Some(field.to_owned()),
                "select an existing safe absolute executable path",
            ));
            continue;
        };
        let Ok(provenance) = binary_provenance(component, &path) else {
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "binary_version_probe_failed",
                component,
                Some(field.to_owned()),
                "the selected binary did not return valid safe version metadata",
            ));
            continue;
        };
        resolved_binaries.insert(component, path);
        provenances.push(provenance);
    }
    if provenances.len() != expected_binary_count {
        return report;
    }
    if validate_release_lineage(&provenances, stack.allow_development_builds).is_err() {
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "binary_lineage_incompatible",
            "binaries",
            Some("stack.toml:binaries".into()),
            "selected binaries do not share one allowed release lineage",
        ));
        return report;
    }
    if renderer
        .and_then(|renderer| {
            validate_renderer_identity(
                renderer,
                resolved_binaries.get("hubu").expect("resolved hubu"),
            )
            .ok()
        })
        .is_none()
    {
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "renderer_identity_mismatch",
            "hubu",
            Some("stack.toml:binaries.hubu".into()),
            "run doctor with the exact Hubu binary selected by this profile",
        ));
        return report;
    }
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "binary_lineage_compatible",
        "binaries",
        None,
        "selected binaries share compatible safe provenance",
    ));

    let Some(files) = credentials.files.as_ref() else {
        return report;
    };
    let credential_fields = [
        (
            "hubu_auth",
            "credentials.toml:files.hubu_auth",
            files.hubu_auth.as_deref(),
        ),
        (
            "hubu_approval",
            "credentials.toml:files.hubu_approval",
            files.hubu_approval.as_deref(),
        ),
        (
            "hubu_reconciliation",
            "credentials.toml:files.hubu_reconciliation",
            files.hubu_reconciliation.as_deref(),
        ),
        (
            "gongbu_caller",
            "credentials.toml:files.gongbu_caller",
            files.gongbu_caller.as_deref(),
        ),
    ];
    let expected_credential_count = credential_fields.len();
    let mut resolved_credentials = BTreeMap::new();
    for (component, field, path) in credential_fields {
        let Some(path) = path.and_then(|path| existing_absolute(path, field).ok()) else {
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "credential_file_unavailable",
                component,
                Some(field.to_owned()),
                "select an existing readable regular file containing only this capability",
            ));
            continue;
        };
        if fs::File::open(&path).is_err() {
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "credential_file_unreadable",
                component,
                Some(field.to_owned()),
                "make the referenced capability file readable by the current user",
            ));
            continue;
        }
        resolved_credentials.insert(component, path);
    }
    if resolved_credentials.len() != expected_credential_count {
        return report;
    }
    let credential_targets = resolved_credentials.values().collect::<Vec<_>>();
    for index in 0..credential_targets.len() {
        for other in &credential_targets[index + 1..] {
            if paths_resolve_to_same_resource(credential_targets[index], other).unwrap_or(true) {
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Fail,
                    "credential_capability_reused",
                    "credentials",
                    Some("credentials.toml:files".into()),
                    "each credential class must reference a distinct file resource",
                ));
                return report;
            }
        }
    }
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "credential_files_available",
        "credentials",
        None,
        "credential files are readable and distinct; values were not displayed",
    ));

    let opaque_keys = required_opaque_keys(&stack, &providers);
    let mut opaque_available = true;
    for key in opaque_keys {
        let Some(reference) = credentials.opaque.get(&key) else {
            continue;
        };
        match opaque_probe(reference) {
            Some(true) => report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Pass,
                "opaque_credential_reference_available",
                "credentials",
                Some(format!("credentials.toml:opaque.{key}")),
                "opaque credential reference resolves without reading its secret value",
            )),
            Some(false) => {
                opaque_available = false;
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Fail,
                    "opaque_credential_reference_unavailable",
                    "credentials",
                    Some(format!("credentials.toml:opaque.{key}")),
                    "the owning service cannot resolve this opaque credential reference",
                ));
            }
            None => {
                opaque_available = false;
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Fail,
                    "opaque_credential_probe_unsupported",
                    "credentials",
                    Some(format!("credentials.toml:opaque.{key}")),
                    "opaque credential preflight is unavailable on this host",
                ));
            }
        }
    }
    report.classification = ProfileClassification::ReadyToRender;
    let source_digests = BTreeMap::from([
        ("credentials.toml".to_owned(), digest(&credential_bytes)),
        ("providers.toml".to_owned(), digest(&provider_bytes)),
        ("stack.toml".to_owned(), digest(&stack_bytes)),
    ]);
    let Some((generation, manifest)) = inspect_active_generation(
        profile,
        &stack,
        &providers,
        &source_digests,
        &provenances,
        &resolved_binaries,
        &mut report.checks,
    ) else {
        return report;
    };

    if !validate_handoff(
        &generation,
        &manifest,
        &stack,
        &resolved_binaries,
        &resolved_credentials,
    ) {
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "client_handoff_invalid",
            "hubu-unified-mcp",
            None,
            "the active client handoff is missing, stale, or incoherent; render again",
        ));
        return report;
    }
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "active_render_valid",
        "generated",
        None,
        "active generated files are current and accepted by service-owned validators",
    ));
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "client_handoff_compatible",
        "hubu-unified-mcp",
        None,
        "unified MCP binary, endpoint, and separate credential references match the active profile",
    ));
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "provider_catalog_contract_valid",
        "providers",
        None,
        if providers.mode == Some(ProviderMode::Disabled) {
            "provider execution is disabled and no catalog or pricing input is active"
        } else {
            "provider targets, frozen pricing, spend gate, and catalog coverage passed production validation"
        },
    ));
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "artifact_contract_valid",
        "gongbu",
        None,
        "artifact destination and safety limits passed the selected Gongbu validator",
    ));

    let client = match Client::builder()
        .timeout(PROBE_TIMEOUT)
        .redirect(Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return report,
    };
    let hubu = stack.hubu.as_ref().expect("complete");
    let gongbu = stack.gongbu.as_ref().expect("complete");
    let hubu_ready = probe_hubu(
        &client,
        hubu.endpoint.as_deref().expect("complete"),
        resolved_credentials.get("hubu_auth").expect("resolved"),
        provenance_for(&provenances, "hubu-server")
            .or_else(|| provenance_for(&provenances, "hubu")),
        hubu.ownership.expect("complete"),
        &mut report.checks,
    );
    let gongbu_ready = probe_gongbu(
        &client,
        gongbu.endpoint.as_deref().expect("complete"),
        resolved_credentials.get("gongbu_caller").expect("resolved"),
        provenance_for(&provenances, "gongbu-server")
            .or_else(|| provenance_for(&provenances, "hubu")),
        gongbu.ownership.expect("complete"),
        &mut report.checks,
    );
    let temporal_ready = probe_temporal(
        &stack,
        gongbu_ready == ServiceProbe::Ready,
        &mut report.checks,
    );

    let required_external_ready = opaque_available
        && service_can_start(hubu.ownership.expect("complete"), hubu_ready)
        && service_can_start(gongbu.ownership.expect("complete"), gongbu_ready)
        && temporal_ready;
    if required_external_ready {
        report.classification = ProfileClassification::ReadyToStart;
    }
    if opaque_available
        && hubu_ready == ServiceProbe::Ready
        && gongbu_ready == ServiceProbe::Ready
        && temporal_ready
    {
        report.classification = ProfileClassification::RunningReady;
    }
    report
}

fn read_source<T: for<'de> Deserialize<'de>>(
    profile: &Path,
    name: &'static str,
    component: &'static str,
    checks: &mut Vec<DoctorCheck>,
) -> Option<(T, Vec<u8>)> {
    let bytes = match fs::read(profile.join(name)) {
        Ok(bytes) => bytes,
        Err(_) => {
            checks.push(check(
                CheckLayer::SourceSyntax,
                CheckStatus::Fail,
                "source_file_unavailable",
                component,
                Some(name.to_owned()),
                "restore or initialize this source file",
            ));
            return None;
        }
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(_) => {
            checks.push(check(
                CheckLayer::SourceSyntax,
                CheckStatus::Fail,
                "source_not_utf8",
                component,
                Some(name.to_owned()),
                "save this source file as UTF-8 text",
            ));
            return None;
        }
    };
    match toml::from_str(text) {
        Ok(value) => {
            checks.push(check(
                CheckLayer::SourceSyntax,
                CheckStatus::Pass,
                "source_syntax_valid",
                component,
                Some(name.to_owned()),
                "source TOML parses with the strict schema",
            ));
            Some((value, bytes))
        }
        Err(error) => {
            let (code, message) = redacted_toml_error(&error);
            checks.push(check(
                CheckLayer::SourceSyntax,
                CheckStatus::Fail,
                code,
                component,
                Some(name.to_owned()),
                message,
            ));
            None
        }
    }
}

fn inspect_active_generation(
    profile: &Path,
    stack: &StackSource,
    providers: &ProvidersSource,
    source_digests: &BTreeMap<String, String>,
    provenances: &[BinaryProvenance],
    binaries: &BTreeMap<&str, PathBuf>,
    checks: &mut Vec<DoctorCheck>,
) -> Option<(PathBuf, ActiveManifest)> {
    let generated = profile.join("generated");
    let manifest: ActiveManifest = match read_json(&generated.join("active-manifest.json")) {
        Ok(manifest) => manifest,
        Err(_) => {
            checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Skipped,
                "active_render_missing",
                "generated",
                None,
                "run stack render after completing the source profile",
            ));
            return None;
        }
    };
    if manifest.source_digests != *source_digests || manifest.binary_provenance != provenances {
        checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Warning,
            "active_render_stale",
            "generated",
            None,
            "source or selected binary provenance changed; render a new generation",
        ));
        return None;
    }
    let generation = match active_generation_path(&generated, &manifest) {
        Ok(path) => path,
        Err(_) => {
            checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "active_manifest_invalid",
                "generated",
                None,
                "active manifest does not identify a safe immutable generation",
            ));
            return None;
        }
    };
    let expected = expected_generated_files(stack, providers);
    if expected.len() != manifest.generated_file_digests.len()
        || expected
            .iter()
            .any(|name| verify_generated_file(&generation, &manifest, name).is_err())
    {
        checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "active_generation_integrity_failed",
            "generated",
            None,
            "active generation is incomplete or has a digest mismatch; render again",
        ));
        return None;
    }
    if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed)
        && validate_with_binary(
            binaries.get("hubu-server").expect("resolved"),
            &generation.join("hubu-launch.json"),
        )
        .is_err()
    {
        checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "hubu_runtime_validation_failed",
            "hubu-server",
            None,
            "the selected Hubu server rejected its active generated configuration",
        ));
        return None;
    }
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed)
        && validate_with_binary(
            binaries.get("gongbu-server").expect("resolved"),
            &generation.join("gongbu-server.json"),
        )
        .is_err()
    {
        checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "gongbu_runtime_validation_failed",
            "gongbu-server",
            None,
            "the selected Gongbu server rejected its active generated configuration",
        ));
        return None;
    }
    Some((generation, manifest))
}

fn validate_handoff(
    generation: &Path,
    manifest: &ActiveManifest,
    stack: &StackSource,
    binaries: &BTreeMap<&str, PathBuf>,
    credentials: &BTreeMap<&str, PathBuf>,
) -> bool {
    if verify_generated_file(generation, manifest, "client-handoff.json").is_err() {
        return false;
    }
    let Ok(handoff) = read_json::<CodexHandoff>(&generation.join("client-handoff.json")) else {
        return false;
    };
    handoff.schema_version == 1
        && Some(&handoff.mcp_server) == binaries.get("hubu-unified-mcp")
        && Some(&handoff.hubu_token_file) == credentials.get("hubu_auth")
        && Some(&handoff.approval_token_file) == credentials.get("hubu_approval")
        && Some(&handoff.reconciliation_token_file) == credentials.get("hubu_reconciliation")
        && Some(&handoff.gongbu_token_file) == credentials.get("gongbu_caller")
        && stack
            .hubu
            .as_ref()
            .and_then(|value| value.endpoint.as_ref())
            == Some(&handoff.hubu_endpoint)
        && stack
            .gongbu
            .as_ref()
            .and_then(|value| value.endpoint.as_ref())
            == Some(&handoff.gongbu_endpoint)
}

fn probe_hubu(
    client: &Client,
    endpoint: &str,
    token_path: &Path,
    expected: Option<&BinaryProvenance>,
    ownership: Ownership,
    checks: &mut Vec<DoctorCheck>,
) -> ServiceProbe {
    let Some((status, health)) = http_get(client, endpoint, "/health", None) else {
        service_not_running("hubu", ownership, checks);
        return ServiceProbe::NotRunning;
    };
    if status != StatusCode::OK || health.get("status").and_then(Value::as_str) != Some("ok") {
        checks.push(runtime_fail(
            "hubu_health_failed",
            "hubu",
            "Hubu responded but did not satisfy its liveness contract",
        ));
        return ServiceProbe::Failed;
    }
    let Some((status, version)) = http_get(client, endpoint, "/version", None) else {
        checks.push(runtime_fail(
            "hubu_version_unavailable",
            "hubu",
            "Hubu safe version metadata is unavailable",
        ));
        return ServiceProbe::Failed;
    };
    if status != StatusCode::OK || !version_matches(&version, expected) {
        checks.push(runtime_fail(
            "hubu_version_incompatible",
            "hubu",
            "running Hubu provenance does not match the selected stack release",
        ));
        return ServiceProbe::Failed;
    }
    let Some(token) = read_secret(token_path) else {
        checks.push(runtime_fail(
            "hubu_auth_credential_empty",
            "hubu",
            "Hubu authentication capability is empty or unreadable",
        ));
        return ServiceProbe::Failed;
    };
    let Some((status, _)) = http_get(client, endpoint, "/agents", Some(&token)) else {
        checks.push(runtime_fail(
            "hubu_protected_check_failed",
            "hubu",
            "protected Hubu access could not be checked",
        ));
        return ServiceProbe::Failed;
    };
    if status != StatusCode::OK {
        checks.push(runtime_fail(
            "hubu_protected_access_denied",
            "hubu",
            "Hubu rejected the configured authentication capability",
        ));
        return ServiceProbe::Failed;
    }
    checks.push(runtime_pass(
        "hubu_running_ready",
        "hubu",
        "Hubu liveness, version, and protected access passed",
    ));
    ServiceProbe::Ready
}

fn probe_gongbu(
    client: &Client,
    endpoint: &str,
    token_path: &Path,
    expected: Option<&BinaryProvenance>,
    ownership: Ownership,
    checks: &mut Vec<DoctorCheck>,
) -> ServiceProbe {
    let Some((status, live)) = http_get(client, endpoint, "/livez", None) else {
        service_not_running("gongbu", ownership, checks);
        return ServiceProbe::NotRunning;
    };
    if status != StatusCode::OK || live.get("status").and_then(Value::as_str) != Some("live") {
        checks.push(runtime_fail(
            "gongbu_liveness_failed",
            "gongbu",
            "Gongbu responded but did not satisfy its liveness contract",
        ));
        return ServiceProbe::Failed;
    }
    let Some((status, readiness)) = http_get(client, endpoint, "/readyz", None) else {
        checks.push(runtime_fail(
            "gongbu_readiness_unavailable",
            "gongbu",
            "Gongbu worker readiness could not be checked",
        ));
        return ServiceProbe::Failed;
    };
    if status != StatusCode::OK || readiness.get("status").and_then(Value::as_str) != Some("ready")
    {
        checks.push(runtime_fail(
            "gongbu_worker_not_ready",
            "gongbu",
            "Gongbu is live but its Temporal worker or dependencies are not ready",
        ));
        return ServiceProbe::Failed;
    }
    let Some((status, version)) = http_get(client, endpoint, "/version", None) else {
        checks.push(runtime_fail(
            "gongbu_version_unavailable",
            "gongbu",
            "Gongbu safe version metadata is unavailable",
        ));
        return ServiceProbe::Failed;
    };
    if status != StatusCode::OK || !version_matches(&version, expected) {
        checks.push(runtime_fail(
            "gongbu_version_incompatible",
            "gongbu",
            "running Gongbu provenance does not match the selected stack release",
        ));
        return ServiceProbe::Failed;
    }
    let Some(token) = read_secret(token_path) else {
        checks.push(runtime_fail(
            "gongbu_auth_credential_empty",
            "gongbu",
            "Gongbu caller capability is empty or unreadable",
        ));
        return ServiceProbe::Failed;
    };
    let Some((status, _)) = http_get(
        client,
        endpoint,
        "/v1/executions/__stack_doctor_read_only_probe__",
        Some(&token),
    ) else {
        checks.push(runtime_fail(
            "gongbu_protected_check_failed",
            "gongbu",
            "protected Gongbu access could not be checked",
        ));
        return ServiceProbe::Failed;
    };
    if status != StatusCode::NOT_FOUND {
        checks.push(runtime_fail(
            "gongbu_protected_access_denied",
            "gongbu",
            "Gongbu rejected the configured caller capability",
        ));
        return ServiceProbe::Failed;
    }
    checks.push(runtime_pass(
        "gongbu_running_ready",
        "gongbu",
        "Gongbu liveness, worker readiness, version, and protected access passed",
    ));
    ServiceProbe::Ready
}

fn probe_temporal(stack: &StackSource, gongbu_ready: bool, checks: &mut Vec<DoctorCheck>) -> bool {
    if stack.gongbu.as_ref().and_then(|value| value.ownership) != Some(Ownership::Managed) {
        checks.push(check(
            CheckLayer::RuntimeReadiness,
            CheckStatus::Skipped,
            "temporal_owned_by_external_gongbu",
            "temporal",
            None,
            "Temporal readiness is owned by the external Gongbu service",
        ));
        return true;
    }
    let temporal = stack.temporal.as_ref().expect("complete");
    let endpoint = match temporal.mode.expect("complete") {
        TemporalMode::External => temporal.address.as_deref().expect("complete").to_owned(),
        TemporalMode::ManagedLocal => {
            format!("http://127.0.0.1:{}", temporal.rpc_port.expect("complete"))
        }
    };
    let reachable = reqwest::Url::parse(&endpoint)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            let port = url.port_or_known_default()?;
            let mut addresses = (host.as_str(), port).to_socket_addrs().ok()?;
            addresses.find_map(|address| {
                std::net::TcpStream::connect_timeout(&address, PROBE_TIMEOUT).ok()
            })
        })
        .is_some();
    if reachable && (temporal.mode == Some(TemporalMode::External) || gongbu_ready) {
        checks.push(runtime_pass(
            "temporal_reachable",
            "temporal",
            "Temporal is reachable and selected worker readiness passed",
        ));
        true
    } else if temporal.mode == Some(TemporalMode::ManagedLocal) && !gongbu_ready {
        checks.push(check(
            CheckLayer::RuntimeReadiness,
            CheckStatus::Skipped,
            "managed_temporal_not_running",
            "temporal",
            None,
            "managed Temporal will be started by Gongbu",
        ));
        true
    } else {
        checks.push(runtime_fail(
            "temporal_unreachable",
            "temporal",
            "required external Temporal is not reachable",
        ));
        false
    }
}

fn http_get(
    client: &Client,
    endpoint: &str,
    path: &str,
    bearer: Option<&str>,
) -> Option<(StatusCode, Value)> {
    let base = reqwest::Url::parse(endpoint).ok()?;
    let url = base.join(path.trim_start_matches('/')).ok()?;
    let mut request = client.get(url);
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request.send().ok()?;
    let status = response.status();
    let body = response.json().unwrap_or(Value::Null);
    Some((status, body))
}

fn version_matches(value: &Value, expected: Option<&BinaryProvenance>) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    value.get("product_version").and_then(Value::as_str) == Some(&expected.product_version)
        && value.get("source_commit").and_then(Value::as_str) == Some(&expected.source_commit)
        && value
            .get("executor_contract")
            .or_else(|| value.get("hubu_executor_contract"))
            .and_then(Value::as_str)
            == Some(&expected.executor_contract)
}

fn provenance_for<'a>(
    provenances: &'a [BinaryProvenance],
    component: &str,
) -> Option<&'a BinaryProvenance> {
    provenances.iter().find(|item| item.component == component)
}

fn provider_readiness(source: &ProvidersSource) -> ProviderReadiness {
    if source.targets.iter().any(|target| {
        target
            .adapter
            .as_deref()
            .is_some_and(|adapter| adapter == "fixture")
    }) {
        ProviderReadiness::FixtureOnly
    } else {
        match source.mode {
            Some(ProviderMode::Disabled) => ProviderReadiness::Disabled,
            Some(ProviderMode::Live) => ProviderReadiness::Unknown,
            None => ProviderReadiness::Unknown,
        }
    }
}

fn required_opaque_keys(stack: &StackSource, providers: &ProvidersSource) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        keys.insert("gongbu_hubu".to_owned());
        keys.insert("gongbu_caller".to_owned());
        if providers.mode == Some(ProviderMode::Live) {
            keys.extend(
                providers
                    .targets
                    .iter()
                    .filter_map(|target| target.credential.clone()),
            );
        }
    }
    keys
}

#[cfg(target_os = "macos")]
fn opaque_reference_exists(reference: &OpaqueReference) -> Option<bool> {
    let service = reference.service.as_deref()?;
    let account = reference.account.as_deref()?;
    let mut command = Command::new("/usr/bin/security");
    command
        .args(["find-generic-password", "-s", service, "-a", account])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command_succeeds_bounded(&mut command)
}

#[cfg(not(target_os = "macos"))]
fn opaque_reference_exists(_reference: &OpaqueReference) -> Option<bool> {
    None
}

#[cfg(target_os = "macos")]
fn command_succeeds_bounded(command: &mut Command) -> Option<bool> {
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(false);
            }
            Err(_) => return None,
        }
    }
}

fn read_secret(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn redacted_toml_error(error: &toml::de::Error) -> (&'static str, String) {
    let message = error.to_string();
    for (needle, code, prefix) in [
        (
            "unknown field `",
            "source_unknown_field",
            "remove or correct unknown field",
        ),
        (
            "duplicate key `",
            "source_duplicate_field",
            "remove duplicate field",
        ),
    ] {
        if let Some(rest) = message.split(needle).nth(1) {
            if let Some(field) = rest.split('`').next() {
                return (code, format!("{prefix} `{field}`"));
            }
        }
    }
    (
        "source_toml_invalid",
        "fix TOML syntax or the invalid field value; secret-like values are intentionally omitted"
            .to_owned(),
    )
}

fn component_for_field(field: &str) -> &'static str {
    if field.starts_with("stack.toml:") {
        "stack"
    } else if field.starts_with("credentials.toml:") {
        "credentials"
    } else {
        "providers"
    }
}

fn service_not_running(
    component: &'static str,
    ownership: Ownership,
    checks: &mut Vec<DoctorCheck>,
) {
    checks.push(check(
        CheckLayer::RuntimeReadiness,
        if ownership == Ownership::Managed {
            CheckStatus::Skipped
        } else {
            CheckStatus::Fail
        },
        if ownership == Ownership::Managed {
            "managed_service_not_running"
        } else {
            "external_service_unreachable"
        },
        component,
        None,
        if ownership == Ownership::Managed {
            "managed service is not running yet"
        } else {
            "required external service is not reachable"
        },
    ));
}

fn service_can_start(ownership: Ownership, probe: ServiceProbe) -> bool {
    match ownership {
        Ownership::Managed => probe != ServiceProbe::Failed,
        Ownership::External => probe == ServiceProbe::Ready,
    }
}

fn runtime_pass(code: &'static str, component: &'static str, message: &str) -> DoctorCheck {
    check(
        CheckLayer::RuntimeReadiness,
        CheckStatus::Pass,
        code,
        component,
        None,
        message,
    )
}

fn runtime_fail(code: &'static str, component: &'static str, message: &str) -> DoctorCheck {
    check(
        CheckLayer::RuntimeReadiness,
        CheckStatus::Fail,
        code,
        component,
        None,
        message,
    )
}

fn check(
    layer: CheckLayer,
    status: CheckStatus,
    code: &'static str,
    component: &'static str,
    field: Option<String>,
    message: impl Into<String>,
) -> DoctorCheck {
    DoctorCheck {
        layer,
        status,
        code,
        component,
        field,
        message: message.into(),
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn print_human(profile: &Path, report: &DoctorReport) {
    println!("profile: {}", profile.display());
    println!("classification: {}", enum_name(report.classification));
    println!(
        "provider readiness: {}",
        provider_name(report.provider_readiness)
    );
    for item in &report.checks {
        let field = item
            .field
            .as_deref()
            .map(|field| format!(" ({field})"))
            .unwrap_or_default();
        println!(
            "[{}] {} / {}{}: {}",
            status_name(item.status),
            item.component,
            item.code,
            field,
            item.message
        );
    }
}

fn enum_name(value: ProfileClassification) -> &'static str {
    match value {
        ProfileClassification::Invalid => "invalid",
        ProfileClassification::Incomplete => "incomplete",
        ProfileClassification::ReadyToRender => "ready_to_render",
        ProfileClassification::ReadyToStart => "ready_to_start",
        ProfileClassification::RunningReady => "running_ready",
    }
}

fn provider_name(value: ProviderReadiness) -> &'static str {
    match value {
        ProviderReadiness::Unknown => "unknown",
        ProviderReadiness::Disabled => "disabled",
        ProviderReadiness::FixtureOnly => "fixture_only",
        ProviderReadiness::LiveReady => "live_ready",
    }
}

fn status_name(value: CheckStatus) -> &'static str {
    match value {
        CheckStatus::Pass => "pass",
        CheckStatus::Fail => "fail",
        CheckStatus::Warning => "warning",
        CheckStatus::Skipped => "skipped",
    }
}

fn print_help() {
    println!(
        "Diagnose a local stack profile without writing files or starting services\n\nUsage:\n  hubu stack doctor [--profile ABSOLUTE_DIR] [--json]\n\nOptions:\n  --json   Print a stable, redacted machine-readable report"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use tempfile::tempdir;

    #[cfg(unix)]
    fn write_fake_binary(path: &Path, reject_validation: bool) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(
            path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo '{{\"product_version\":\"0.1.0\",\"source_commit\":\"unknown\",\"executor_contract\":\"hubu-executor.v1\",\"server_config_schema_version\":2}}'\n  exit 0\nfi\nif [ \"$1\" = \"validate-config\" ]; then\n  exit {}\nfi\nexit 2\n",
                if reject_validation { 9 } else { 0 }
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    fn write_complete_managed_profile(root: &Path) -> (PathBuf, PathBuf) {
        let profile = root.join("profile");
        let binaries = root.join("bin");
        let credential_root = root.join("credentials");
        fs::create_dir(&profile).unwrap();
        fs::create_dir(&binaries).unwrap();
        fs::create_dir(&credential_root).unwrap();
        for name in ["hubu", "hubu-server", "gongbu-server", "hubu-unified-mcp"] {
            write_fake_binary(&binaries.join(name), false);
        }
        let temporal = binaries.join("temporal");
        fs::write(&temporal, b"temporal fixture").unwrap();
        for name in ["auth", "approval", "reconciliation", "gongbu-caller"] {
            fs::write(credential_root.join(name), format!("{name}-secret")).unwrap();
        }
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
endpoint = "http://127.0.0.1:41001"
listen = "127.0.0.1:41001"
database_path = {}
log_file = {}
[gongbu]
ownership = "managed"
endpoint = "http://127.0.0.1:41002"
listen = "127.0.0.1:41002"
database_path = {}
artifact_root = {}
log_file = {}
[temporal]
mode = "managed_local"
binary_path = {}
expected_cli_version = "1.0.0"
data_path = {}
rpc_port = 41003
ui_port = 41004
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
                quote(temporal.display().to_string()),
                quote(profile.join("state/temporal").display().to_string()),
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
                quote(credential_root.join("auth").display().to_string()),
                quote(credential_root.join("approval").display().to_string()),
                quote(credential_root.join("reconciliation").display().to_string()),
                quote(credential_root.join("gongbu-caller").display().to_string()),
            ),
        )
        .unwrap();
        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = \"disabled\"\n",
        )
        .unwrap();
        (profile, binaries.join("hubu"))
    }

    fn opaque_available(_reference: &OpaqueReference) -> Option<bool> {
        Some(true)
    }

    fn opaque_unavailable(_reference: &OpaqueReference) -> Option<bool> {
        Some(false)
    }

    fn spawn_http_server(
        listener: std::net::TcpListener,
        responses: Vec<(&'static str, u16, &'static str)>,
        bearer: Option<&'static str>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            for (expected_path, status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let length = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..length]);
                assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1")));
                if let Some(bearer) = bearer.filter(|_| expected_path.contains("__stack_doctor")) {
                    assert!(request
                        .to_ascii_lowercase()
                        .contains(&format!("authorization: bearer {bearer}")));
                }
                if expected_path == "/agents" {
                    assert!(request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer auth-secret"));
                }
                let reason = if status == 200 { "OK" } else { "Not Found" };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        })
    }

    #[test]
    fn incomplete_profile_reports_stable_fields_without_mutation() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        init(
            vec!["--profile".into(), profile.display().to_string()],
            root.path(),
        )
        .unwrap();
        let before = ["stack.toml", "credentials.toml", "providers.toml"]
            .map(|name| fs::read(profile.join(name)).unwrap());
        let report = inspect_profile(&profile);
        assert_eq!(report.classification, ProfileClassification::Incomplete);
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("stack.toml:hubu.ownership"));
        assert!(json.contains("stack.toml:gongbu.ownership"));
        assert!(json.contains("credentials.toml:files.hubu_auth"));
        assert!(json.contains("providers.toml:mode"));
        let after = ["stack.toml", "credentials.toml", "providers.toml"]
            .map(|name| fs::read(profile.join(name)).unwrap());
        assert_eq!(before, after);
        assert!(!profile.join("generated/active-manifest.json").exists());
    }

    #[test]
    fn parse_errors_are_redacted_and_unknown_fields_are_actionable() {
        let root = tempdir().unwrap();
        let profile = root.path().join("profile");
        fs::create_dir(&profile).unwrap();
        fs::write(
            profile.join("stack.toml"),
            "schema_version = 1\nsecret_canary = \"do-not-print-this\"\n",
        )
        .unwrap();
        fs::write(
            profile.join("credentials.toml"),
            "schema_version = 1\ncredential_canary = \"also-do-not-print\"\n",
        )
        .unwrap();
        fs::write(
            profile.join("providers.toml"),
            "schema_version = 1\nmode = [\"not-valid\"\n",
        )
        .unwrap();
        let json = serde_json::to_string(&inspect_profile(&profile)).unwrap();
        assert!(json.contains("source_unknown_field"));
        assert!(json.contains("secret_canary"));
        assert!(json.contains("credential_canary"));
        assert!(json.contains("source_toml_invalid"));
        assert!(!json.contains("do-not-print-this"));
        assert!(!json.contains("also-do-not-print"));
        assert!(!json.contains(root.path().to_str().unwrap()));
    }

    #[test]
    fn fixture_targets_are_classified_without_claiming_live_readiness() {
        let providers: ProvidersSource = toml::from_str(
            "schema_version = 1\nmode = \"live\"\n[[targets]]\nadapter = \"fixture\"\n",
        )
        .unwrap();
        assert_eq!(
            provider_readiness(&providers),
            ProviderReadiness::FixtureOnly
        );
    }

    #[cfg(unix)]
    #[test]
    fn current_render_is_validated_read_only_and_validator_failures_are_actionable() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        render_profile_with_renderer(&profile, &renderer).unwrap();
        let source_before = ["stack.toml", "credentials.toml", "providers.toml"]
            .map(|name| fs::read(profile.join(name)).unwrap());
        let active_path = profile.join("generated/active-manifest.json");
        let active_before = fs::read(&active_path).unwrap();

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(report.classification, ProfileClassification::ReadyToStart);
        assert!(report
            .checks
            .iter()
            .any(|check| check.code == "active_render_valid"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.code == "client_handoff_compatible"));
        assert_eq!(
            source_before,
            ["stack.toml", "credentials.toml", "providers.toml"]
                .map(|name| fs::read(profile.join(name)).unwrap())
        );
        assert_eq!(active_before, fs::read(&active_path).unwrap());

        let missing_opaque = inspect_profile_with(&profile, opaque_unavailable, Some(&renderer));
        assert_eq!(
            missing_opaque.classification,
            ProfileClassification::ReadyToRender
        );
        assert!(missing_opaque
            .checks
            .iter()
            .any(|check| check.code == "opaque_credential_reference_unavailable"));
        assert!(missing_opaque
            .checks
            .iter()
            .any(|check| check.code == "active_render_valid"));

        let manifest: ActiveManifest = read_json(&active_path).unwrap();
        let generation = active_generation_path(&profile.join("generated"), &manifest).unwrap();
        let handoff_path = generation.join("client-handoff.json");
        let handoff_before = fs::read(&handoff_path).unwrap();
        fs::write(&handoff_path, b"{}\n").unwrap();
        let corrupted = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(
            corrupted.classification,
            ProfileClassification::ReadyToRender
        );
        assert!(corrupted
            .checks
            .iter()
            .any(|check| check.code == "active_generation_integrity_failed"));
        fs::write(&handoff_path, handoff_before).unwrap();

        write_fake_binary(&root.path().join("bin/hubu-server"), true);
        let rejected = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(
            rejected.classification,
            ProfileClassification::ReadyToRender
        );
        assert!(rejected
            .checks
            .iter()
            .any(|check| check.code == "hubu_runtime_validation_failed"));
        assert_eq!(active_before, fs::read(&active_path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn protected_runtime_probes_produce_running_ready() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        let hubu_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gongbu_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let temporal_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ui_guard = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ports = [
            hubu_listener.local_addr().unwrap().port(),
            gongbu_listener.local_addr().unwrap().port(),
            temporal_listener.local_addr().unwrap().port(),
            ui_guard.local_addr().unwrap().port(),
        ];
        let stack_path = profile.join("stack.toml");
        let mut stack = fs::read_to_string(&stack_path).unwrap();
        for (old, new) in [41001_u16, 41002, 41003, 41004].into_iter().zip(ports) {
            stack = stack.replace(&old.to_string(), &new.to_string());
        }
        fs::write(&stack_path, stack).unwrap();
        render_profile_with_renderer(&profile, &renderer).unwrap();

        let version = r#"{"product_version":"0.1.0","source_commit":"unknown","executor_contract":"hubu-executor.v1"}"#;
        let hubu_server = spawn_http_server(
            hubu_listener,
            vec![
                ("/health", 200, r#"{"status":"ok"}"#),
                ("/version", 200, version),
                ("/agents", 200, "[]"),
            ],
            None,
        );
        let gongbu_server = spawn_http_server(
            gongbu_listener,
            vec![
                ("/livez", 200, r#"{"status":"live"}"#),
                ("/readyz", 200, r#"{"status":"ready"}"#),
                ("/version", 200, version),
                (
                    "/v1/executions/__stack_doctor_read_only_probe__",
                    404,
                    r#"{"error":{"code":"not_found"}}"#,
                ),
            ],
            Some("gongbu-caller-secret"),
        );

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(report.classification, ProfileClassification::RunningReady);
        assert!(report
            .checks
            .iter()
            .any(|check| check.code == "hubu_running_ready"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.code == "gongbu_running_ready"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.code == "temporal_reachable"));
        hubu_server.join().unwrap();
        gongbu_server.join().unwrap();
    }
}
