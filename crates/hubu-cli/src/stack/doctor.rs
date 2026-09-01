use super::*;
use reqwest::{blocking::Client, redirect::Policy, StatusCode};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    io::Read,
    net::ToSocketAddrs,
    process::{ExitStatus, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const REPORT_SCHEMA_VERSION: u32 = 2;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
const OPERATION_REGISTRY_APPLICATION_ID: i64 = 0x4855_424f;
const OPERATION_REGISTRY_SCHEMA_VERSION: i64 = 6;

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
    Configured,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubprocessProbeError {
    Failed,
    TimedOut,
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
    pub(super) provider_profiles: Vec<ProviderProfileCatalogEntry>,
    checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub(super) fn is_source_complete(&self) -> bool {
        !matches!(
            self.classification,
            ProfileClassification::Invalid | ProfileClassification::Incomplete
        )
    }

    pub(super) fn is_startable(&self) -> bool {
        matches!(
            self.classification,
            ProfileClassification::ReadyToStart | ProfileClassification::RunningReady
        )
    }

    pub(super) fn is_running_ready(&self) -> bool {
        self.classification == ProfileClassification::RunningReady
    }

    pub(super) fn is_renderable(&self) -> bool {
        !self.checks.iter().any(|check| {
            check.status == CheckStatus::Fail
                && matches!(
                    check.layer,
                    CheckLayer::SourceSyntax | CheckLayer::Completeness | CheckLayer::Renderability
                )
        })
    }

    pub(super) fn component_ready(&self, component: &str) -> bool {
        self.checks.iter().any(|check| {
            check.status == CheckStatus::Pass
                && check.component == component
                && matches!(check.code, "hubu_running_ready" | "gongbu_running_ready")
        })
    }

    pub(super) fn check_passed(&self, code: &str) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == CheckStatus::Pass && check.code == code)
    }
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
        provider_profiles: Vec::new(),
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
    report.provider_profiles = provider_profile_catalog_entries(&providers, false);
    for profile in &mut report.provider_profiles {
        if !credentials.opaque.contains_key(&profile.credential_alias) {
            profile.readiness.credential_reference_present = Some(false);
        }
    }

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
    if validate_provider_source(&providers).is_err()
        || validate_provider_credential_isolation(&providers, &credentials).is_err()
        || validate_stack_mode(&stack, &providers).is_err()
    {
        source_constraints_valid = false;
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Fail,
            "provider_configuration_invalid",
            "providers",
            Some("providers.toml".into()),
            "provider mode, spend gates, and supplied provider fields are contradictory",
        ));
    } else {
        for profile in &mut report.provider_profiles {
            profile.readiness.configured = true;
        }
    }
    let fixture_only = providers.targets.iter().any(|target| {
        target
            .adapter
            .as_deref()
            .is_some_and(|adapter| adapter == "fixture")
    });
    let local_fixture_canary = cfg!(feature = "local-fixture-canary")
        && std::env::var("HUBU_LOCAL_FIXTURE_CANARY").as_deref() == Ok("1");
    if fixture_only && stack.mode != StackMode::Sandbox && !local_fixture_canary {
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
    } else if fixture_only {
        report.provider_readiness = ProviderReadiness::FixtureOnly;
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Pass,
            "local_fixture_canary_explicit",
            "providers",
            Some("providers.toml:targets".into()),
            if stack.mode == StackMode::Sandbox {
                "sandbox mode uses the deterministic fixture only at the external provider edge"
            } else {
                "the feature-gated local acceptance canary explicitly selected a fixture adapter"
            },
        ));
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
        let provenance = match binary_provenance_bounded(component, &path) {
            Ok(provenance) => provenance,
            Err(error) => {
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Fail,
                    if error == SubprocessProbeError::TimedOut {
                        "binary_version_probe_timed_out"
                    } else {
                        "binary_version_probe_failed"
                    },
                    component,
                    Some(field.to_owned()),
                    if error == SubprocessProbeError::TimedOut {
                        "the selected binary did not return safe version metadata before the diagnostic deadline"
                    } else {
                        "the selected binary did not return valid safe version metadata"
                    },
                ));
                continue;
            }
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
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        let gongbu = provenances
            .iter()
            .find(|provenance| provenance.component == "gongbu-server")
            .expect("managed Gongbu provenance was resolved");
        if let Err(error) = negotiate_principal_neutral_gongbu_schema(gongbu) {
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "gongbu_principal_neutral_schema_unsupported",
                "gongbu-server",
                Some("stack.toml:binaries.gongbu_server".into()),
                error.to_string(),
            ));
            return report;
        }
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Pass,
            "gongbu_principal_neutral_schema_supported",
            "gongbu-server",
            Some("stack.toml:binaries.gongbu_server".into()),
            "selected Gongbu binary supports principal-neutral server config rendering",
        ));
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

    let credential_paths = match resolve_credential_paths(profile, &stack, &credentials) {
        Ok(paths) => paths,
        Err(_) => {
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "credential_reference_invalid",
                "credentials",
                Some("credentials.toml".into()),
                "credential references are unsafe, conflicting, or unavailable to their selected owner",
            ));
            return report;
        }
    };
    let configured_files = credentials.files.as_ref();
    let hubu_managed =
        stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed);
    let mut credential_fields = vec![
        (
            "hubu_auth",
            "hubu",
            "credentials.toml:files.hubu_auth",
            credential_paths.hubu_auth.clone(),
            hubu_managed
                && configured_files
                    .and_then(|files| files.hubu_auth.as_ref())
                    .is_none(),
            configured_files
                .and_then(|files| files.hubu_auth.as_ref())
                .is_some(),
        ),
        (
            "hubu_approval",
            "hubu",
            "credentials.toml:files.hubu_approval",
            credential_paths.hubu_approval.clone(),
            hubu_managed
                && configured_files
                    .and_then(|files| files.hubu_approval.as_ref())
                    .is_none(),
            configured_files
                .and_then(|files| files.hubu_approval.as_ref())
                .is_some(),
        ),
        (
            "hubu_reconciliation",
            "hubu",
            "credentials.toml:files.hubu_reconciliation",
            credential_paths.hubu_reconciliation.clone(),
            hubu_managed
                && configured_files
                    .and_then(|files| files.hubu_reconciliation.as_ref())
                    .is_none(),
            configured_files
                .and_then(|files| files.hubu_reconciliation.as_ref())
                .is_some(),
        ),
    ];
    if let Some(gongbu_caller) = credential_paths.gongbu_caller.clone() {
        credential_fields.push((
            "gongbu_caller",
            "gongbu",
            "credentials.toml:files.gongbu_caller",
            gongbu_caller,
            credential_paths.managed_gongbu_handoff,
            configured_files
                .and_then(|files| files.gongbu_caller.as_ref())
                .is_some(),
        ));
    }
    let mut resolved_credentials = BTreeMap::new();
    let mut pending_managed_owners = BTreeSet::new();
    for (component, owner, field, path, provisioned_by_managed_start, explicitly_configured) in
        credential_fields
    {
        let source_field = explicitly_configured.then(|| field.to_owned());
        match inspect_credential_file(&path) {
            CredentialFileState::Ready => {
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Pass,
                    "credential_file_available",
                    component,
                    source_field,
                    "credential capability is private and available; its value was not displayed",
                ));
            }
            CredentialFileState::Unsafe => {
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Fail,
                    "credential_file_unsafe",
                    component,
                    source_field,
                    "make the capability a non-empty readable regular file that excludes group and other access",
                ));
                return report;
            }
            CredentialFileState::Missing if provisioned_by_managed_start => {
                pending_managed_owners.insert(owner);
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Pass,
                    "managed_credential_pending",
                    component,
                    source_field,
                    "managed startup will provision this private capability before its consumer starts",
                ));
            }
            CredentialFileState::Missing => {
                report.checks.push(check(
                    CheckLayer::Renderability,
                    CheckStatus::Fail,
                    "credential_file_unavailable",
                    component,
                    source_field,
                    "select an existing readable regular file containing only this externally owned capability",
                ));
                return report;
            }
        }
        resolved_credentials.insert(component, path);
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
        "credential_references_ready",
        "credentials",
        None,
        "credential references are distinct and either available or owned by managed startup",
    ));

    let opaque_keys = required_opaque_keys(&stack, &credentials, &providers);
    let mut opaque_available = true;
    for key in opaque_keys {
        let Some(reference) = credentials.opaque.get(&key) else {
            opaque_available = false;
            report.checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                "opaque_credential_reference_missing",
                "credentials",
                Some(format!("credentials.toml:opaque.{key}")),
                "add the opaque credential reference selected by the live provider target",
            ));
            continue;
        };
        let reference_present = opaque_probe(reference);
        for profile in &mut report.provider_profiles {
            if profile.credential_alias == key {
                profile.readiness.credential_reference_present = reference_present;
            }
        }
        match reference_present {
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
            "the active client handoff is missing, stale, incoherent, or older than schema v2; run `hubu stack render`, activate the new generation, and reinitialize the client",
        ));
        return report;
    }
    let handoff: CodexHandoff = read_json(&generation.join("client-handoff.json"))
        .expect("validated client handoff remains readable");
    report_operation_registry_path(&handoff.operation_state_path, &mut report.checks);
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "active_render_valid",
        "generated",
        None,
        "active generated files are current and managed service files are accepted by their owning validators",
    ));
    report.checks.push(check(
        CheckLayer::Renderability,
        CheckStatus::Pass,
        "client_handoff_compatible",
        "hubu-unified-mcp",
        None,
        "unified MCP binary, endpoint, and separate credential references match the active profile",
    ));
    let gongbu_managed =
        stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed);
    if gongbu_managed {
        if providers.mode == Some(ProviderMode::Live) {
            report.provider_readiness = ProviderReadiness::Configured;
            for profile in &mut report.provider_profiles {
                profile.readiness.production_validated = true;
            }
        }
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
    } else if stack.mode == StackMode::HubuOnly {
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Skipped,
            "gongbu_intentionally_absent",
            "gongbu",
            None,
            "Hubu-only mode intentionally omits Gongbu, Temporal, provider catalogs, and artifacts",
        ));
    } else {
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Skipped,
            "provider_catalog_owned_by_external_gongbu",
            "providers",
            None,
            "external Gongbu owns provider catalog, pricing, and spend-gate validation; this profile is not locally certified",
        ));
        report.checks.push(check(
            CheckLayer::Renderability,
            CheckStatus::Skipped,
            "artifact_contract_owned_by_external_gongbu",
            "gongbu",
            None,
            "external Gongbu owns artifact destination and safety-limit validation",
        ));
    }

    let client = match Client::builder()
        .timeout(PROBE_TIMEOUT)
        .redirect(Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return report,
    };
    let hubu = stack.hubu.as_ref().expect("complete");
    let hubu_ready = probe_hubu(
        &client,
        hubu.endpoint.as_deref().expect("complete"),
        resolved_credentials.get("hubu_auth").expect("resolved"),
        provenance_for(&provenances, "hubu-server")
            .or_else(|| provenance_for(&provenances, "hubu")),
        hubu.ownership.expect("complete"),
        &mut report.checks,
    );
    let gongbu_ready = stack.gongbu.as_ref().map(|gongbu| {
        probe_gongbu(
            &client,
            gongbu.endpoint.as_deref().expect("complete"),
            resolved_credentials.get("gongbu_caller").expect("resolved"),
            provenance_for(&provenances, "gongbu-server")
                .or_else(|| provenance_for(&provenances, "hubu")),
            gongbu.ownership.expect("complete"),
            &mut report.checks,
        )
    });
    let temporal_ready = gongbu_ready.is_none_or(|ready| {
        probe_temporal(&stack, ready == ServiceProbe::Ready, &mut report.checks)
    });

    let pending_while_running = (pending_managed_owners.contains("hubu")
        && hubu_ready != ServiceProbe::NotRunning)
        || (pending_managed_owners.contains("gongbu")
            && gongbu_ready != Some(ServiceProbe::NotRunning));
    if pending_while_running {
        report.checks.push(runtime_fail(
            "managed_credential_missing_while_running",
            "credentials",
            "a running managed component has lost a required private capability; stop the whole stack before recovery",
        ));
    }

    let required_external_ready = opaque_available
        && !pending_while_running
        && service_can_start(hubu.ownership.expect("complete"), hubu_ready)
        && stack
            .gongbu
            .as_ref()
            .zip(gongbu_ready)
            .is_none_or(|(gongbu, ready)| {
                service_can_start(gongbu.ownership.expect("complete"), ready)
            })
        && temporal_ready;
    if required_external_ready {
        report.classification = ProfileClassification::ReadyToStart;
    }
    if opaque_available
        && !pending_while_running
        && hubu_ready == ServiceProbe::Ready
        && gongbu_ready.is_none_or(|ready| ready == ServiceProbe::Ready)
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
    if stack.hubu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        if let Err(error) = validate_with_binary_bounded(
            binaries.get("hubu-server").expect("resolved"),
            &generation.join("hubu-launch.json"),
        ) {
            checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                if error == SubprocessProbeError::TimedOut {
                    "hubu_runtime_validation_timed_out"
                } else {
                    "hubu_runtime_validation_failed"
                },
                "hubu-server",
                None,
                if error == SubprocessProbeError::TimedOut {
                    "the selected Hubu server validator exceeded the diagnostic deadline"
                } else {
                    "the selected Hubu server rejected its active generated configuration"
                },
            ));
            return None;
        }
    }
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        if let Err(error) = validate_with_binary_bounded(
            binaries.get("gongbu-server").expect("resolved"),
            &generation.join("gongbu-server.json"),
        ) {
            checks.push(check(
                CheckLayer::Renderability,
                CheckStatus::Fail,
                if error == SubprocessProbeError::TimedOut {
                    "gongbu_runtime_validation_timed_out"
                } else {
                    "gongbu_runtime_validation_failed"
                },
                "gongbu-server",
                None,
                if error == SubprocessProbeError::TimedOut {
                    "the selected Gongbu server validator exceeded the diagnostic deadline"
                } else {
                    "the selected Gongbu server rejected its active generated configuration"
                },
            ));
            return None;
        }
    }
    Some((generation, manifest))
}

fn binary_provenance_bounded(
    component: &str,
    path: &Path,
) -> std::result::Result<BinaryProvenance, SubprocessProbeError> {
    let mut command = Command::new(path);
    command.arg("--version");
    let (status, stdout) = command_stdout_bounded(&mut command)?;
    if !status.success() {
        return Err(SubprocessProbeError::Failed);
    }
    binary_provenance_from_output(component, path, &stdout)
        .map_err(|_| SubprocessProbeError::Failed)
}

fn validate_with_binary_bounded(
    binary: &Path,
    config: &Path,
) -> std::result::Result<(), SubprocessProbeError> {
    let mut command = Command::new(binary);
    command.args(["validate-config", "--config"]).arg(config);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    prepare_bounded_command(&mut command);
    let mut child = command.spawn().map_err(|_| SubprocessProbeError::Failed)?;
    let status = wait_for_bounded_child(&mut child, Instant::now() + PROBE_TIMEOUT)?;
    if status.success() {
        Ok(())
    } else {
        Err(SubprocessProbeError::Failed)
    }
}

fn command_stdout_bounded(
    command: &mut Command,
) -> std::result::Result<(ExitStatus, Vec<u8>), SubprocessProbeError> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    prepare_bounded_command(command);
    let mut child = command.spawn().map_err(|_| SubprocessProbeError::Failed)?;
    let mut stdout = child.stdout.take().ok_or(SubprocessProbeError::Failed)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = wait_for_bounded_child(&mut child, deadline)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    match receiver.recv_timeout(remaining) {
        Ok(Ok(stdout)) => Ok((status, stdout)),
        Ok(Err(_)) => Err(SubprocessProbeError::Failed),
        Err(_) => {
            terminate_bounded_child(&mut child);
            Err(SubprocessProbeError::TimedOut)
        }
    }
}

fn prepare_bounded_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

fn wait_for_bounded_child(
    child: &mut std::process::Child,
    deadline: Instant,
) -> std::result::Result<ExitStatus, SubprocessProbeError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                terminate_bounded_child(child);
                return Err(SubprocessProbeError::TimedOut);
            }
            Err(_) => {
                terminate_bounded_child(child);
                return Err(SubprocessProbeError::Failed);
            }
        }
    }
}

fn terminate_bounded_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &process_group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
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
    handoff.schema_version == 2
        && Some(&handoff.mcp_server) == binaries.get("hubu-unified-mcp")
        && Some(&handoff.hubu_token_file) == credentials.get("hubu_auth")
        && Some(&handoff.approval_token_file) == credentials.get("hubu_approval")
        && Some(&handoff.reconciliation_token_file) == credentials.get("hubu_reconciliation")
        && handoff.gongbu_token_file.as_ref() == credentials.get("gongbu_caller")
        && handoff.operation_state_path.is_absolute()
        && generation.ancestors().nth(3).is_some_and(|profile| {
            handoff.operation_state_path == profile.join("state/hubu-unified-operations.sqlite3")
        })
        && stack
            .hubu
            .as_ref()
            .and_then(|value| value.endpoint.as_ref())
            == Some(&handoff.hubu_endpoint)
        && stack
            .gongbu
            .as_ref()
            .and_then(|value| value.endpoint.as_ref())
            == handoff.gongbu_endpoint.as_ref()
}

fn report_operation_registry_path(path: &Path, checks: &mut Vec<DoctorCheck>) {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o222 == 0 {
                    checks.push(check(
                        CheckLayer::RuntimeReadiness,
                        CheckStatus::Warning,
                        "operation_registry_not_writable",
                        "hubu-unified-mcp",
                        Some("client-handoff.json:operation_state_path".into()),
                        "the stack remains available, but new billable Hubu operations are disabled until the unified MCP registry is writable",
                    ));
                    return;
                }
            }
            if validate_operation_registry(path).is_err() {
                checks.push(check(
                    CheckLayer::RuntimeReadiness,
                    CheckStatus::Warning,
                    "operation_registry_invalid",
                    "hubu-unified-mcp",
                    Some("client-handoff.json:operation_state_path".into()),
                    "the stack and unrelated tools remain available, but billable Hubu tools are disabled because the configured registry is not a valid unified MCP operation database",
                ));
                return;
            }
            checks.push(check(
                CheckLayer::RuntimeReadiness,
                CheckStatus::Pass,
                "operation_registry_path_ready",
                "hubu-unified-mcp",
                Some("client-handoff.json:operation_state_path".into()),
                "the separate unified MCP operation registry path is initialized for billable operations",
            ));
        }
        Ok(_) => checks.push(check(
            CheckLayer::RuntimeReadiness,
            CheckStatus::Warning,
            "operation_registry_path_invalid",
            "hubu-unified-mcp",
            Some("client-handoff.json:operation_state_path".into()),
            "the stack and unrelated tools remain available, but billable Hubu tools are disabled because the registry path is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => checks.push(check(
            CheckLayer::RuntimeReadiness,
            CheckStatus::Warning,
            "operation_registry_uninitialized",
            "hubu-unified-mcp",
            Some("client-handoff.json:operation_state_path".into()),
            "the stack remains available; the client-owned unified MCP process will initialize this registry before billable Hubu tools become available",
        )),
        Err(_) => checks.push(check(
            CheckLayer::RuntimeReadiness,
            CheckStatus::Warning,
            "operation_registry_path_unreadable",
            "hubu-unified-mcp",
            Some("client-handoff.json:operation_state_path".into()),
            "the stack and unrelated tools remain available, but billable Hubu tools are disabled until the registry path is accessible",
        )),
    }
}

fn validate_operation_registry(path: &Path) -> rusqlite::Result<()> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let application_id =
        connection.query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))?;
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    let quick_check =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))?;
    if application_id != OPERATION_REGISTRY_APPLICATION_ID
        || version != OPERATION_REGISTRY_SCHEMA_VERSION
        || quick_check != "ok"
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection.prepare(
        "SELECT singleton, installation_id, created_at FROM installation_identity LIMIT 0",
    )?;
    connection.prepare(
        "SELECT platform, installation_id, harness_call_id, request_hash,
                normalized_request_json, tool_name, operation_key, operation_key_record_id,
                governed_result_json, operation_handle,
                codex_call_id, claude_tool_use_id, hubu_invocation_id,
                controlled_installation_id, task_id, decision, decision_id,
                auth_token_id, approval_request_id, approval_status, approval_synced_at,
                authorization_expires_at,
                result_json, dispatch_started_at, result_recorded_at,
                gongbu_request_hash, gongbu_request_json, gongbu_execution_id, gongbu_status,
                gongbu_outcome, gongbu_create_started_at,
                gongbu_result_recorded_at, operation_state, operation_result_code,
                dispatch_attempts, observation_failures, reconciliation_attempts,
                operation_deadline_at, next_operation_attempt_at,
                worker_lease_id, worker_lease_expires_at,
                operation_updated_at, created_at
         FROM harness_operations LIMIT 0",
    )?;
    let installation_count = connection.query_row(
        "SELECT COUNT(*) FROM installation_identity WHERE singleton = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if installation_count != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
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
            Some(ProviderMode::Sandbox) => ProviderReadiness::FixtureOnly,
            Some(ProviderMode::Live) => ProviderReadiness::Unknown,
            None => ProviderReadiness::Unknown,
        }
    }
}

fn required_opaque_keys(
    stack: &StackSource,
    credentials: &CredentialsSource,
    providers: &ProvidersSource,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if stack.gongbu.as_ref().and_then(|value| value.ownership) == Some(Ownership::Managed) {
        if !uses_managed_gongbu_handoff(stack, credentials) {
            keys.insert("gongbu_hubu".to_owned());
            keys.insert("gongbu_caller".to_owned());
        }
        if providers.mode == Some(ProviderMode::Live) {
            keys.extend(
                providers
                    .targets
                    .iter()
                    .filter_map(|target| target.credential.clone()),
            );
            keys.extend(
                providers
                    .supported_profiles
                    .iter()
                    .filter_map(|profile| profile.credential.clone()),
            );
        }
    }
    keys
}

#[cfg(target_os = "macos")]
fn opaque_reference_exists(reference: &OpaqueReference) -> Option<bool> {
    #[cfg(feature = "local-fixture-canary")]
    if let Some(available) = local_fixture_reference_exists(reference) {
        return Some(available);
    }
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
fn opaque_reference_exists(reference: &OpaqueReference) -> Option<bool> {
    #[cfg(feature = "local-fixture-canary")]
    if let Some(available) = local_fixture_reference_exists(reference) {
        return Some(available);
    }
    let _ = reference;
    None
}

#[cfg(feature = "local-fixture-canary")]
fn local_fixture_reference_exists(reference: &OpaqueReference) -> Option<bool> {
    if std::env::var("HUBU_LOCAL_FIXTURE_CANARY").as_deref() != Ok("1") {
        return None;
    }
    let name = match (reference.service.as_deref()?, reference.account.as_deref()?) {
        ("hubu.local-fixture", "caller") => "gongbu-caller",
        ("hubu.local-fixture", "executor") => "hubu-auth",
        ("hubu.local-fixture", "provider") => "provider",
        _ => return Some(false),
    };
    Some(
        std::env::var_os("GONGBU_LOCAL_FIXTURE_SECRET_DIR")
            .map(PathBuf::from)
            .filter(|root| root.is_absolute())
            .is_some_and(|root| root.join(name).is_file()),
    )
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialFileState {
    Ready,
    Missing,
    Unsafe,
}

fn inspect_credential_file(path: &Path) -> CredentialFileState {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CredentialFileState::Missing;
        }
        Err(_) => return CredentialFileState::Unsafe,
    };
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return CredentialFileState::Unsafe;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 {
            return CredentialFileState::Unsafe;
        }
        let mut options = OpenOptions::new();
        options.read(true).custom_flags(libc::O_NOFOLLOW);
        if options.open(path).is_err() {
            return CredentialFileState::Unsafe;
        }
    }
    #[cfg(not(unix))]
    if OpenOptions::new().read(true).open(path).is_err() {
        return CredentialFileState::Unsafe;
    }
    CredentialFileState::Ready
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

pub(super) fn print_human(profile: &Path, report: &DoctorReport) {
    print!(
        "{}",
        render_human(crate::terminal::stdout(), profile, report)
    );
}

fn render_human(
    style: crate::terminal::TerminalStyle,
    profile: &Path,
    report: &DoctorReport,
) -> String {
    let mut output = String::new();
    let counts = |status| {
        report
            .checks
            .iter()
            .filter(|check| check.status == status)
            .count()
    };

    writeln!(output, "{}", style.title("Hubu stack doctor")).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", style.heading("Summary")).unwrap();
    write_summary_field(
        &mut output,
        style,
        "Profile",
        style.accent(profile.display()),
    );
    write_summary_field(
        &mut output,
        style,
        "Classification",
        classification_display(style, report.classification),
    );
    write_summary_field(
        &mut output,
        style,
        "Provider readiness",
        provider_display(style, report.provider_readiness),
    );
    write_summary_field(
        &mut output,
        style,
        "Checks",
        format!(
            "{}  {}  {}  {}",
            style.success(format!("{} pass", counts(CheckStatus::Pass))),
            style.warning(format!("{} warning", counts(CheckStatus::Warning))),
            style.error(format!("{} fail", counts(CheckStatus::Fail))),
            style.muted(format!("{} skipped", counts(CheckStatus::Skipped))),
        ),
    );

    for layer in [
        CheckLayer::SourceSyntax,
        CheckLayer::Completeness,
        CheckLayer::Renderability,
        CheckLayer::RuntimeReadiness,
    ] {
        let checks = report
            .checks
            .iter()
            .filter(|check| check.layer == layer)
            .collect::<Vec<_>>();
        if checks.is_empty() {
            continue;
        }
        writeln!(output).unwrap();
        writeln!(
            output,
            "{}",
            style.heading(format!(
                "{} ({} {})",
                layer_name(layer),
                checks.len(),
                if checks.len() == 1 { "check" } else { "checks" }
            ))
        )
        .unwrap();
        for check in checks {
            writeln!(
                output,
                "  {} {}",
                status_badge(style, check.status),
                style.accent(format!("{} / {}", check.component, check.code))
            )
            .unwrap();
            if let Some(field) = &check.field {
                writeln!(output, "            {} {}", style.label("Field:"), field).unwrap();
            }
            writeln!(output, "            {}", check.message).unwrap();
        }
    }
    output
}

fn write_summary_field(
    output: &mut String,
    style: crate::terminal::TerminalStyle,
    label: &str,
    value: impl std::fmt::Display,
) {
    let label = format!("{label:<20}");
    writeln!(output, "  {}  {value}", style.label(label)).unwrap();
}

fn classification_display(
    style: crate::terminal::TerminalStyle,
    value: ProfileClassification,
) -> String {
    let name = enum_name(value);
    match value {
        ProfileClassification::Invalid => style.error(name),
        ProfileClassification::Incomplete
        | ProfileClassification::ReadyToRender
        | ProfileClassification::ReadyToStart => style.warning(name),
        ProfileClassification::RunningReady => style.success(name),
    }
}

fn provider_display(style: crate::terminal::TerminalStyle, value: ProviderReadiness) -> String {
    let name = provider_name(value);
    match value {
        ProviderReadiness::Unknown | ProviderReadiness::Disabled => style.muted(name),
        ProviderReadiness::FixtureOnly => style.warning(name),
        ProviderReadiness::Configured => style.success(name),
    }
}

fn status_badge(style: crate::terminal::TerminalStyle, value: CheckStatus) -> String {
    match value {
        CheckStatus::Pass => style.success(format!("{:<9}", "[PASS]")),
        CheckStatus::Fail => style.error(format!("{:<9}", "[FAIL]")),
        CheckStatus::Warning => style.warning(format!("{:<9}", "[WARN]")),
        CheckStatus::Skipped => style.muted(format!("{:<9}", "[SKIP]")),
    }
}

fn layer_name(value: CheckLayer) -> &'static str {
    match value {
        CheckLayer::SourceSyntax => "Source syntax",
        CheckLayer::Completeness => "Completeness",
        CheckLayer::Renderability => "Renderability",
        CheckLayer::RuntimeReadiness => "Runtime readiness",
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
        ProviderReadiness::Configured => "configured",
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

    fn presentation_report() -> DoctorReport {
        DoctorReport {
            schema_version: REPORT_SCHEMA_VERSION,
            classification: ProfileClassification::Incomplete,
            provider_readiness: ProviderReadiness::FixtureOnly,
            provider_profiles: Vec::new(),
            checks: vec![
                check(
                    CheckLayer::SourceSyntax,
                    CheckStatus::Pass,
                    "stack_source_valid",
                    "stack",
                    None,
                    "stack.toml parsed successfully",
                ),
                check(
                    CheckLayer::Completeness,
                    CheckStatus::Fail,
                    "required_field_missing",
                    "stack",
                    Some("stack.toml:hubu.ownership".into()),
                    "choose managed or external ownership",
                ),
                check(
                    CheckLayer::Renderability,
                    CheckStatus::Warning,
                    "development_binary",
                    "hubu",
                    None,
                    "binary has no release stamp",
                ),
                check(
                    CheckLayer::RuntimeReadiness,
                    CheckStatus::Skipped,
                    "runtime_probe_skipped",
                    "gongbu",
                    None,
                    "complete the profile before probing runtime readiness",
                ),
            ],
        }
    }

    #[test]
    fn report_schema_versions_the_supported_profile_readiness_contract() {
        let value = serde_json::to_value(presentation_report()).unwrap();
        assert_eq!(value["schema_version"], REPORT_SCHEMA_VERSION);
        assert_eq!(value["schema_version"], 2);
        assert!(value.get("provider_profiles").is_some());
    }

    #[test]
    fn human_report_plain_output_groups_checks_by_layer() {
        let output = render_human(
            crate::terminal::TerminalStyle::plain(),
            Path::new("/profiles/demo"),
            &presentation_report(),
        );

        assert!(!output.contains('\u{1b}'));
        assert!(output.starts_with("Hubu stack doctor\n\nSummary\n"));
        assert!(output.contains("Classification        incomplete"));
        assert!(output.contains("1 pass  1 warning  1 fail  1 skipped"));
        for heading in [
            "Source syntax (1 check)",
            "Completeness (1 check)",
            "Renderability (1 check)",
            "Runtime readiness (1 check)",
        ] {
            assert!(output.contains(heading));
        }
        assert!(output.contains("[PASS]    stack / stack_source_valid"));
        assert!(output.contains("Field: stack.toml:hubu.ownership"));
        assert!(output.contains("[FAIL]    stack / required_field_missing"));
        assert!(output.contains("[WARN]    hubu / development_binary"));
        assert!(output.contains("[SKIP]    gongbu / runtime_probe_skipped"));
    }

    #[test]
    fn human_report_ansi_output_preserves_plain_text() {
        let report = presentation_report();
        let plain = render_human(
            crate::terminal::TerminalStyle::plain(),
            Path::new("/profiles/demo"),
            &report,
        );
        let colored = render_human(
            crate::terminal::TerminalStyle::colored(),
            Path::new("/profiles/demo"),
            &report,
        );

        assert!(colored.contains("\u{1b}["));
        assert!(colored.matches("\u{1b}[").count() >= 12);
        assert_eq!(strip_ansi(&colored), plain);
    }

    fn write_operation_registry_fixture(
        path: &Path,
        schema_version: i64,
        include_governed_result: bool,
    ) {
        let governed_result_column = if include_governed_result {
            "governed_result_json TEXT,"
        } else {
            ""
        };
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE installation_identity (
                     singleton INTEGER PRIMARY KEY,
                     installation_id TEXT NOT NULL,
                     created_at TEXT NOT NULL
                 );
                 CREATE TABLE harness_operations (
                     platform TEXT NOT NULL,
                     installation_id TEXT NOT NULL,
                     harness_call_id TEXT NOT NULL,
                     request_hash TEXT NOT NULL,
                     normalized_request_json TEXT,
                     tool_name TEXT NOT NULL,
                     operation_key TEXT,
                     operation_key_record_id TEXT,
                     {governed_result_column}
                     operation_handle TEXT NOT NULL,
                     codex_call_id TEXT,
                     claude_tool_use_id TEXT,
                     hubu_invocation_id TEXT,
                     controlled_installation_id TEXT,
                     task_id TEXT,
                     decision TEXT,
                     decision_id TEXT,
                     auth_token_id TEXT,
                     approval_request_id TEXT,
                     approval_status TEXT,
                     approval_synced_at TEXT,
                     authorization_expires_at TEXT,
                     result_json TEXT,
                     dispatch_started_at TEXT,
                     result_recorded_at TEXT,
                     gongbu_request_hash TEXT,
                     gongbu_request_json TEXT,
                     gongbu_execution_id TEXT,
                     gongbu_status TEXT,
                     gongbu_outcome TEXT,
                     gongbu_create_started_at TEXT,
                     gongbu_result_recorded_at TEXT,
                     operation_state TEXT,
                     operation_result_code TEXT,
                     dispatch_attempts INTEGER NOT NULL DEFAULT 0,
                     observation_failures INTEGER NOT NULL DEFAULT 0,
                     reconciliation_attempts INTEGER NOT NULL DEFAULT 0,
                     operation_deadline_at TEXT,
                     next_operation_attempt_at TEXT,
                     worker_lease_id TEXT,
                     worker_lease_expires_at TEXT,
                     operation_updated_at TEXT,
                     created_at TEXT NOT NULL
                 );
                 INSERT INTO installation_identity VALUES (
                     1, 'hubu-installation:v1:test', CURRENT_TIMESTAMP
                 );
                 PRAGMA application_id = {OPERATION_REGISTRY_APPLICATION_ID};
                 PRAGMA user_version = {schema_version};"
            ))
            .unwrap();
    }

    #[test]
    fn operation_registry_path_reports_degraded_billable_capability_without_failing_stack() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let mut checks = Vec::new();
        report_operation_registry_path(&path, &mut checks);
        assert_eq!(checks[0].status, CheckStatus::Warning);
        assert_eq!(checks[0].code, "operation_registry_uninitialized");
        assert!(checks[0].message.contains("stack remains available"));

        fs::create_dir(&path).unwrap();
        checks.clear();
        report_operation_registry_path(&path, &mut checks);
        assert_eq!(checks[0].status, CheckStatus::Warning);
        assert_eq!(checks[0].code, "operation_registry_path_invalid");
        assert!(checks[0]
            .message
            .contains("billable Hubu tools are disabled"));

        let corrupt_path = root.path().join("corrupt.sqlite3");
        fs::write(&corrupt_path, b"not sqlite").unwrap();
        checks.clear();
        report_operation_registry_path(&corrupt_path, &mut checks);
        assert_eq!(checks[0].status, CheckStatus::Warning);
        assert_eq!(checks[0].code, "operation_registry_invalid");

        let valid_path = root.path().join("valid.sqlite3");
        write_operation_registry_fixture(&valid_path, OPERATION_REGISTRY_SCHEMA_VERSION, true);
        checks.clear();
        report_operation_registry_path(&valid_path, &mut checks);
        assert_eq!(checks[0].status, CheckStatus::Pass);
        assert_eq!(checks[0].code, "operation_registry_path_ready");
    }

    #[test]
    fn operation_registry_validation_is_read_only_and_schema_strict() {
        let root = tempdir().unwrap();
        let valid_path = root.path().join("valid-v6.sqlite3");
        write_operation_registry_fixture(&valid_path, OPERATION_REGISTRY_SCHEMA_VERSION, true);
        let bytes_before = fs::read(&valid_path).unwrap();

        validate_operation_registry(&valid_path).unwrap();

        assert_eq!(fs::read(&valid_path).unwrap(), bytes_before);

        for version in [
            OPERATION_REGISTRY_SCHEMA_VERSION - 1,
            OPERATION_REGISTRY_SCHEMA_VERSION + 1,
        ] {
            let path = root.path().join(format!("unsupported-v{version}.sqlite3"));
            write_operation_registry_fixture(&path, version, true);
            assert!(validate_operation_registry(&path).is_err());
        }
    }

    #[test]
    fn operation_registry_validation_requires_current_v6_columns() {
        let root = tempdir().unwrap();
        let path = root.path().join("missing-governed-result.sqlite3");
        write_operation_registry_fixture(&path, OPERATION_REGISTRY_SCHEMA_VERSION, false);

        assert!(validate_operation_registry(&path).is_err());
    }

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
    fn write_fake_gongbu_v3_binary(path: &Path, reject_validation: bool) {
        use std::os::unix::fs::PermissionsExt;

        let validation = if reject_validation {
            "exit 9"
        } else {
            "exit 0"
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

    #[cfg(unix)]
    fn write_hanging_binary(path: &Path, hang_on: &str) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(
            path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"{hang_on}\" ]; then\n  while :; do :; done\nfi\nif [ \"$1\" = \"--version\" ]; then\n  echo '{{\"product_version\":\"0.1.0\",\"source_commit\":\"unknown\",\"executor_contract\":\"hubu-executor.v1\",\"server_config_schema_version\":2}}'\n  exit 0\nfi\nif [ \"$1\" = \"validate-config\" ]; then\n  exit 0\nfi\nexit 2\n"
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    fn write_complete_managed_profile(root: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let profile = root.join("profile");
        let binaries = root.join("bin");
        let credential_root = root.join("credentials");
        fs::create_dir(&profile).unwrap();
        fs::create_dir(&binaries).unwrap();
        fs::create_dir(&credential_root).unwrap();
        for name in ["hubu", "hubu-server", "hubu-unified-mcp"] {
            write_fake_binary(&binaries.join(name), false);
        }
        write_fake_gongbu_v3_binary(&binaries.join("gongbu-server"), false);
        let temporal = binaries.join("temporal");
        fs::write(&temporal, b"temporal fixture").unwrap();
        for name in ["auth", "approval", "reconciliation", "gongbu-caller"] {
            fs::write(credential_root.join(name), format!("{name}-secret")).unwrap();
            fs::set_permissions(
                credential_root.join(name),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
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
            vec![
                "--mode".into(),
                "local-stack".into(),
                "--profile".into(),
                profile.display().to_string(),
            ],
            root.path(),
        )
        .unwrap();
        let before = ["stack.toml", "credentials.toml", "providers.toml"]
            .map(|name| fs::read(profile.join(name)).unwrap());
        let report = inspect_profile(&profile);
        assert_eq!(report.classification, ProfileClassification::Incomplete);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("stack.toml:hubu.ownership"));
        assert!(!json.contains("stack.toml:gongbu.ownership"));
        assert!(!json.contains("credentials.toml:files.hubu_auth"));
        assert!(json.contains("providers.toml:targets"));
        assert!(!json.contains("stack.toml:identity"));
        let after = ["stack.toml", "credentials.toml", "providers.toml"]
            .map(|name| fs::read(profile.join(name)).unwrap());
        assert_eq!(before, after);
        assert!(!profile.join("generated/active-manifest.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn static_principal_gongbu_binary_reports_upgrade_diagnostic() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        write_fake_binary(&root.path().join("bin/gongbu-server"), false);

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(report.classification, ProfileClassification::Invalid);
        let diagnostic = report
            .checks
            .iter()
            .find(|check| check.code == "gongbu_principal_neutral_schema_unsupported")
            .expect("schema incompatibility diagnostic");
        assert_eq!(
            diagnostic.field.as_deref(),
            Some("stack.toml:binaries.gongbu_server")
        );
        assert!(diagnostic
            .message
            .contains("static-principal server config schema version 2"));
        assert!(diagnostic.message.contains("upgrade Gongbu"));
        assert!(diagnostic.message.contains("schema version 3 or newer"));
    }

    #[cfg(unix)]
    #[test]
    fn managed_credentials_are_pending_before_first_start_without_doctor_mutation() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        fs::write(profile.join("credentials.toml"), "schema_version = 1\n").unwrap();
        let managed_root = profile.join("state/credentials");

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));

        assert_eq!(report.classification, ProfileClassification::ReadyToRender);
        assert_eq!(
            report
                .checks
                .iter()
                .filter(|check| check.code == "managed_credential_pending")
                .count(),
            4
        );
        assert!(!managed_root.exists());
        assert!(report.checks.iter().all(|check| {
            check.code != "credential_file_unavailable"
                && check.code != "opaque_credential_reference_missing"
        }));
    }

    #[cfg(unix)]
    #[test]
    fn missing_explicit_managed_hubu_override_is_not_pending_managed_work() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        let explicit_auth = root.path().join("credentials/auth");
        fs::remove_file(&explicit_auth).unwrap();

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));

        assert_eq!(report.classification, ProfileClassification::Invalid);
        assert!(report
            .checks
            .iter()
            .any(|check| check.code == "credential_reference_invalid"));
        assert!(report
            .checks
            .iter()
            .all(|check| check.code != "managed_credential_pending"));
        assert!(!explicit_auth.exists());
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
    fn supported_profile_readiness_keeps_configuration_reference_and_live_qualification_separate() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        let mut credentials = fs::read_to_string(profile.join("credentials.toml")).unwrap();
        credentials.push_str(
            "\n[opaque.bfl_flux2_pro]\nservice = \"operator.bfl\"\naccount = \"flux\"\n\n[opaque.gemini]\nservice = \"operator.google\"\naccount = \"gemini\"\n",
        );
        fs::write(profile.join("credentials.toml"), credentials).unwrap();
        fs::write(
            profile.join("providers.toml"),
            format!(
                r#"schema_version = 1
mode = "live"
catalog_version = "operator-mixed-2026-08-28-v1"
maximum_spend_minor = 25
live_spend_acknowledgement = "{LIVE_SPEND_ACKNOWLEDGEMENT}"
[[supported_profiles]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux2_pro"

[[targets]]
provider_config_version = "gemini-v1"
workload_type = "image_generation"
provider = "google"
adapter = "gemini_developer_image"
model = "gemini-image-v1"
credential = "gemini"
active = true
execution_enabled = true
[targets.settings]
type = "gemini_developer_image"
[targets.settings.config]
endpoint = "https://generativelanguage.googleapis.com"
api_version = "v1beta"
timeout_ms = 30000
max_retries = 0
headers = {{}}

[[pricing_rules]]
rule_id = "gemini-v1"
provider = "google"
model = "gemini-image-v1"
currency = "USD"
components = [{{ unit = "image", rate_numerator_minor = 4, rate_denominator = 1 }}]
"#
            ),
        )
        .unwrap();

        let present = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(present.classification, ProfileClassification::ReadyToRender);
        assert_eq!(present.provider_readiness, ProviderReadiness::Unknown);
        assert_eq!(present.provider_profiles.len(), 1);
        let readiness = &present.provider_profiles[0].readiness;
        assert!(readiness.configured);
        assert_eq!(readiness.credential_reference_present, Some(true));
        assert!(!readiness.production_validated);
        assert!(!readiness.live_qualified);
        assert_eq!(readiness.live_qualification, "not_performed");

        render_profile_with_renderer(&profile, &renderer).unwrap();
        let validated = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(validated.provider_readiness, ProviderReadiness::Configured);
        assert!(validated.provider_profiles[0].readiness.configured);
        assert!(
            validated.provider_profiles[0]
                .readiness
                .production_validated
        );
        assert_eq!(
            validated.provider_profiles[0]
                .readiness
                .credential_reference_present,
            Some(true)
        );
        assert!(!validated.provider_profiles[0].readiness.live_qualified);

        let absent = inspect_profile_with(&profile, opaque_unavailable, Some(&renderer));
        assert_eq!(
            absent.provider_profiles[0]
                .readiness
                .credential_reference_present,
            Some(false)
        );
        assert!(!absent.provider_profiles[0].readiness.live_qualified);
        assert!(absent.checks.iter().all(|check| {
            !check.message.contains("api.bfl.ai") && !check.message.contains("provider call")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn missing_supported_profile_reference_is_incomplete_before_render() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        fs::write(
            profile.join("providers.toml"),
            format!(
                r#"schema_version = 1
mode = "live"
maximum_spend_minor = 25
live_spend_acknowledgement = "{LIVE_SPEND_ACKNOWLEDGEMENT}"
[[supported_profiles]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux2_pro"
"#
            ),
        )
        .unwrap();

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(report.classification, ProfileClassification::Incomplete);
        assert_eq!(report.provider_profiles.len(), 1);
        assert_eq!(
            report.provider_profiles[0]
                .readiness
                .credential_reference_present,
            Some(false)
        );
        assert!(!report.provider_profiles[0].readiness.live_qualified);
        assert!(report.checks.iter().any(|check| {
            check.code == "required_decision_missing"
                && check.field.as_deref() == Some("credentials.toml:opaque.bfl_flux2_pro")
        }));
        let error = render_profile_with_renderer(&profile, &renderer)
            .unwrap_err()
            .to_string();
        assert!(error.contains("credentials.toml:opaque.bfl_flux2_pro"));
        assert!(!profile.join("generated/active-manifest.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn missing_live_target_credential_is_incomplete_before_render() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        fs::write(
            profile.join("providers.toml"),
            format!(
                r#"schema_version = 1
mode = "live"
catalog_version = "catalog-v2"
maximum_spend_minor = 10
live_spend_acknowledgement = "{LIVE_SPEND_ACKNOWLEDGEMENT}"
[[targets]]
provider_config_version = "provider.v1"
workload_type = "image.generate"
provider = "example"
adapter = "http_json"
model = "model"
credential = "missing_provider"
active = true
execution_enabled = true
settings = {{ base_url = "https://example.invalid" }}
[[pricing_rules]]
schema_version = 1
"#
            ),
        )
        .unwrap();

        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(report.classification, ProfileClassification::Incomplete);
        assert_eq!(report.provider_readiness, ProviderReadiness::Unknown);
        assert!(report.checks.iter().any(|check| {
            check.code == "required_decision_missing"
                && check.field.as_deref() == Some("credentials.toml:opaque.missing_provider")
        }));
        let error = render_profile_with_renderer(&profile, &renderer)
            .unwrap_err()
            .to_string();
        assert!(error.contains("credentials.toml:opaque.missing_provider"));
        assert!(!profile.join("generated/active-manifest.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn external_gongbu_does_not_certify_local_provider_or_artifact_contracts() {
        use std::os::unix::fs::PermissionsExt;

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
            fs::set_permissions(credentials.join(name), fs::Permissions::from_mode(0o600)).unwrap();
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
[opaque.provider]
service = "provider-test"
account = "provider-account"
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
            format!(
                r#"schema_version = 1
mode = "live"
catalog_version = "external-v1"
maximum_spend_minor = 10
live_spend_acknowledgement = "{LIVE_SPEND_ACKNOWLEDGEMENT}"
[[targets]]
provider_config_version = "provider.v1"
workload_type = "image.generate"
provider = "external"
adapter = "http_json"
model = "model"
credential = "provider"
active = true
execution_enabled = true
settings = {{ base_url = "https://example.invalid" }}
[[pricing_rules]]
deliberately_unvalidated_external_shape = true
"#
            ),
        )
        .unwrap();

        let renderer = binaries.join("hubu");
        render_profile_with_renderer(&profile, &renderer).unwrap();
        let report = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_eq!(report.provider_readiness, ProviderReadiness::Unknown);
        assert!(report.checks.iter().any(|check| {
            check.code == "provider_catalog_owned_by_external_gongbu"
                && check.status == CheckStatus::Skipped
        }));
        assert!(report.checks.iter().any(|check| {
            check.code == "artifact_contract_owned_by_external_gongbu"
                && check.status == CheckStatus::Skipped
        }));
        assert!(!report
            .checks
            .iter()
            .any(|check| check.code == "provider_catalog_contract_valid"));
    }

    #[cfg(unix)]
    #[test]
    fn selected_binary_and_validator_probes_are_bounded() {
        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        let unified = root.path().join("bin/hubu-unified-mcp");
        write_hanging_binary(&unified, "--version");
        let started = Instant::now();
        let version_timeout = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert!(started.elapsed() < PROBE_TIMEOUT * 3);
        assert!(version_timeout
            .checks
            .iter()
            .any(|check| check.code == "binary_version_probe_timed_out"));

        write_fake_binary(&unified, false);
        render_profile_with_renderer(&profile, &renderer).unwrap();
        write_hanging_binary(&root.path().join("bin/hubu-server"), "validate-config");
        let started = Instant::now();
        let validator_timeout = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert!(started.elapsed() < PROBE_TIMEOUT * 3);
        assert!(validator_timeout
            .checks
            .iter()
            .any(|check| check.code == "hubu_runtime_validation_timed_out"));
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
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let (profile, renderer) = write_complete_managed_profile(root.path());
        let managed_hubu = profile.join("state/credentials/hubu");
        fs::create_dir_all(&managed_hubu).unwrap();
        fs::set_permissions(&managed_hubu, fs::Permissions::from_mode(0o700)).unwrap();
        for (name, value) in [
            ("auth", "auth-secret"),
            ("approval", "approval-secret"),
            ("reconciliation", "reconciliation-secret"),
        ] {
            let path = managed_hubu.join(name);
            fs::write(&path, value).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(
            profile.join("credentials.toml"),
            format!(
                r#"schema_version = 1
[files]
gongbu_caller = {}
[opaque.gongbu_hubu]
service = "hubu-test"
account = "gongbu-hubu"
[opaque.gongbu_caller]
service = "hubu-test"
account = "gongbu-caller"
"#,
                quote(
                    root.path()
                        .join("credentials/gongbu-caller")
                        .display()
                        .to_string()
                )
            ),
        )
        .unwrap();
        let hubu_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gongbu_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let temporal_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ui_guard = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addresses = [
            hubu_listener.local_addr().unwrap(),
            gongbu_listener.local_addr().unwrap(),
            temporal_listener.local_addr().unwrap(),
            ui_guard.local_addr().unwrap(),
        ];
        let ports = addresses.map(|address| address.port());
        let stack_path = profile.join("stack.toml");
        let mut stack = fs::read_to_string(&stack_path).unwrap();
        for (old, new) in [41001_u16, 41002, 41003, 41004].into_iter().zip(ports) {
            stack = stack.replace(&old.to_string(), &new.to_string());
        }
        fs::write(&stack_path, stack).unwrap();
        drop(hubu_listener);
        drop(gongbu_listener);
        drop(temporal_listener);
        drop(ui_guard);
        render_profile_with_renderer(&profile, &renderer).unwrap();

        let hubu_listener = std::net::TcpListener::bind(addresses[0]).unwrap();
        let gongbu_listener = std::net::TcpListener::bind(addresses[1]).unwrap();
        let _temporal_listener = std::net::TcpListener::bind(addresses[2]).unwrap();
        let _ui_guard = std::net::TcpListener::bind(addresses[3]).unwrap();

        let version = r#"{"product_version":"0.1.0","source_commit":"unknown","executor_contract":"hubu-executor.v1"}"#;
        let hubu_server = spawn_http_server(
            hubu_listener,
            vec![
                ("/health", 200, r#"{"status":"ok"}"#),
                ("/version", 200, version),
                ("/agents", 200, "[]"),
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

        fs::remove_file(managed_hubu.join("approval")).unwrap();
        let degraded = inspect_profile_with(&profile, opaque_available, Some(&renderer));
        assert_ne!(degraded.classification, ProfileClassification::RunningReady);
        assert!(degraded
            .checks
            .iter()
            .any(|check| check.code == "managed_credential_missing_while_running"));
        hubu_server.join().unwrap();
        gongbu_server.join().unwrap();
    }
}
