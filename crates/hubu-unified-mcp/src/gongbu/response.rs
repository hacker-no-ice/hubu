use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

use super::AdmissionDiagnostic;

const MCP_SCHEMA_VERSION: u32 = 2;
pub(super) const EXECUTION_V1_SCHEMA_VERSION: u32 = 1;
pub(super) const EXECUTION_V2_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApiErrorContext {
    General,
    CreateExecutionV2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolErrorClass {
    Permanent,
    Transient,
    InvalidSuccessfulResponse,
    IdentityConflict,
}

#[derive(Debug)]
pub(super) struct ToolError {
    code: &'static str,
    message: &'static str,
    diagnostic: Option<AdmissionDiagnostic>,
    class: ToolErrorClass,
}

impl ToolError {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            diagnostic: None,
            class: ToolErrorClass::Permanent,
        }
    }

    fn with_diagnostic(mut self, diagnostic: Option<AdmissionDiagnostic>) -> Self {
        self.diagnostic = diagnostic;
        self
    }

    pub(super) fn invalid() -> Self {
        Self::new("invalid_request", "tool arguments failed validation")
    }

    pub(super) fn transport() -> Self {
        Self::transient("gongbu_unavailable", "Gongbu could not be reached")
    }

    pub(super) fn upstream(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message)
    }

    pub(super) fn transient(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            diagnostic: None,
            class: ToolErrorClass::Transient,
        }
    }

    pub(super) fn invalid_response() -> Self {
        Self {
            code: "invalid_response",
            message: "Gongbu returned an invalid response",
            diagnostic: None,
            class: ToolErrorClass::InvalidSuccessfulResponse,
        }
    }

    fn identity_conflict(message: &'static str) -> Self {
        Self {
            code: "identity_conflict",
            message,
            diagnostic: None,
            class: ToolErrorClass::IdentityConflict,
        }
    }

    pub(super) fn into_result(self) -> ToolResult {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(diagnostic) = self.diagnostic {
            error["reason_code"] = json!(diagnostic.reason_code());
            error["fields"] = json!(diagnostic.fields());
        }
        ToolResult {
            content: vec![Content::Text {
                text: json!({
                    "schema_version": MCP_SCHEMA_VERSION,
                    "error": error
                })
                .to_string(),
            }],
            is_error: true,
            structured_content: None,
        }
    }

    pub(super) fn code(&self) -> &'static str {
        self.code
    }

    pub(super) fn class(&self) -> ToolErrorClass {
        self.class
    }

    pub(super) fn admission_diagnostic(&self) -> Option<AdmissionDiagnostic> {
        self.diagnostic
    }
}

pub(super) fn api_error(
    status: StatusCode,
    body: Option<&[u8]>,
    context: ApiErrorContext,
) -> ToolError {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        schema_version: Option<u32>,
        error: ErrorCode,
    }
    #[derive(Deserialize)]
    struct ErrorCode {
        code: String,
        #[serde(default)]
        reason_code: Option<Value>,
        #[serde(default)]
        fields: Option<Value>,
    }

    let reported = body.and_then(|bytes| serde_json::from_slice::<Envelope>(bytes).ok());
    match (
        status,
        reported.as_ref().map(|value| value.error.code.as_str()),
    ) {
        (StatusCode::BAD_REQUEST, Some("invalid_request")) => {
            let diagnostic = reported
                .as_ref()
                .filter(|reported| {
                    context == ApiErrorContext::CreateExecutionV2
                        && reported.schema_version == Some(2)
                })
                .and_then(|reported| {
                    allowlisted_validation_diagnostic(
                        reported.error.reason_code.as_ref(),
                        reported.error.fields.as_ref(),
                    )
                });
            ToolError::new("invalid_request", "request validation failed")
                .with_diagnostic(diagnostic)
        }
        (StatusCode::UNAUTHORIZED, _) => {
            ToolError::new("unauthorized", "Gongbu rejected operator authentication")
        }
        (StatusCode::FORBIDDEN, _) => ToolError::new("forbidden", "resource access is forbidden"),
        (StatusCode::NOT_FOUND, _) => ToolError::new("not_found", "resource was not found"),
        (StatusCode::CONFLICT, Some("immutable_scope_conflict")) => ToolError::new(
            "immutable_scope_conflict",
            "authorization continuation was already used with different immutable input",
        ),
        (StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY, _) => ToolError::transient(
            "gongbu_unavailable",
            "Gongbu could not complete the request",
        ),
        (StatusCode::TOO_MANY_REQUESTS, _) => {
            ToolError::transient("rate_limited", "Gongbu rate limit exceeded")
        }
        _ if status.is_server_error() => ToolError::transient(
            "gongbu_internal_error",
            "Gongbu could not complete the request",
        ),
        _ => ToolError::new("gongbu_api_error", "Gongbu rejected the request"),
    }
}

fn allowlisted_validation_diagnostic(
    reason_code: Option<&Value>,
    fields: Option<&Value>,
) -> Option<AdmissionDiagnostic> {
    let reason_code = reason_code?.as_str()?;
    let fields = fields?.as_array()?;
    let exactly_matches = |expected: &[&str]| {
        fields.len() == expected.len()
            && fields
                .iter()
                .zip(expected)
                .all(|(reported, expected)| reported.as_str() == Some(*expected))
    };
    match reason_code {
        "target_not_selectable"
            if exactly_matches(AdmissionDiagnostic::TargetIdNotSelectable.fields()) =>
        {
            Some(AdmissionDiagnostic::TargetIdNotSelectable)
        }
        "pricing_selector_not_matched"
            if exactly_matches(AdmissionDiagnostic::PricingSelectorNotMatched.fields()) =>
        {
            Some(AdmissionDiagnostic::PricingSelectorNotMatched)
        }
        _ => None,
    }
}

pub(super) fn text_result(value: &impl Serialize) -> ToolResult {
    ToolResult {
        content: vec![Content::Text {
            text: serde_json::to_string(value).expect("response serializes"),
        }],
        is_error: false,
        structured_content: None,
    }
}

pub(super) fn execution_target_catalog_result(
    response: ExecutionTargetCatalogResponse,
) -> Result<ToolResult, ToolError> {
    let mut ids = BTreeSet::new();
    let valid = response.schema_version == EXECUTION_V2_SCHEMA_VERSION
        && response.targets.iter().all(|target| {
            valid_target_id(&target.target_id)
                && ids.insert(target.target_id.clone())
                && !target.workload_type.is_empty()
                && !target.provider.is_empty()
                && !target.model.is_empty()
                && target.execution_scope.validate()
                && target
                    .image_sizes
                    .iter()
                    .all(|size| matches!(size.as_str(), "1k" | "2k" | "4k"))
                && target.image_sizes.windows(2).all(|pair| pair[0] < pair[1])
                && !target.pricing.is_empty()
                && target.pricing.iter().all(|price| {
                    !price.rule_id.is_empty()
                        && price.currency.len() == 3
                        && price.currency.bytes().all(|byte| byte.is_ascii_uppercase())
                        && price.selector.as_ref().is_none_or(|selector| {
                            target.image_sizes.contains(&selector.image_size)
                        })
                        && !price.components.is_empty()
                        && price.components.iter().all(|component| {
                            matches!(
                                component.unit.as_str(),
                                "image" | "input_token" | "output_token"
                            ) && component.rate_numerator_minor >= 0
                                && component.rate_denominator > 0
                        })
                })
        });
    if !valid {
        return Err(ToolError::invalid_response());
    }
    let structured = serde_json::to_value(&response).map_err(|_| ToolError::invalid_response())?;
    Ok(ToolResult {
        content: vec![Content::Text {
            text: serde_json::to_string(&response).map_err(|_| ToolError::invalid_response())?,
        }],
        is_error: false,
        structured_content: Some(structured),
    })
}

fn valid_target_id(value: &str) -> bool {
    value
        .strip_prefix("gongbu:target:v1:")
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

pub(super) fn execution_result(
    response: ExecutionResponse,
    expected: Option<&crate::operation_registry::GongbuContinuation>,
    requested_execution_id: Option<&str>,
    expected_schema_version: u32,
) -> Result<(ToolResult, crate::operation_registry::GongbuLifecycle), ToolError> {
    if expected.is_some_and(|expected| expected.operation_key != response.operation_key) {
        return Err(ToolError::identity_conflict(
            "Gongbu returned an execution for a different normalized operation",
        ));
    }
    if requested_execution_id.is_some_and(|execution_id| execution_id != response.execution_id)
        || expected
            .and_then(|expected| expected.execution_id.as_deref())
            .is_some_and(|execution_id| execution_id != response.execution_id)
    {
        return Err(ToolError::identity_conflict(
            "Gongbu returned a conflicting execution identity",
        ));
    }
    if response.schema_version != expected_schema_version
        || !valid_execution_id(&response.execution_id)
        || !valid_execution_status(&response.status)
    {
        return Err(ToolError::invalid_response());
    }
    let lifecycle = crate::operation_registry::GongbuLifecycle {
        execution_id: response.execution_id.clone(),
        operation_key: response.operation_key.clone(),
        status: response.status.clone(),
        outcome: response.outcome.clone(),
    };
    let timing = response.timing();
    let provider_transport = response.provider_transport();
    let private_operation_key = response.operation_key.clone();
    let public = PublicExecutionResponse {
        schema_version: response.schema_version,
        execution_id: response.execution_id,
        operation_handle: expected.map(|expected| expected.operation_handle.clone()),
        status: response.status,
        outcome: response.outcome,
        failure: response.failure,
        authorization: response.authorization,
        created_at: response.created_at,
        updated_at: response.updated_at,
        started_at: response.started_at,
        completed_at: response.completed_at,
        timing,
        provider_transport,
    };
    let mut public = serde_json::to_value(public).expect("public execution response serializes");
    scrub_private_projection(&mut public, &private_operation_key);
    Ok((text_result(&public), lifecycle))
}

fn valid_execution_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_execution_status(status: &str) -> bool {
    matches!(
        status,
        "pending"
            | "preflighting"
            | "claimed"
            | "executing"
            | "settling"
            | "succeeded"
            | "released"
            | "failed"
            | "reconciliation_required"
    )
}

pub(super) fn artifact_result(
    artifact_id: String,
    media_type: String,
    bytes: Vec<u8>,
) -> ToolResult {
    let metadata = json!({
        "schema_version": 1,
        "artifact_id": artifact_id,
        "media_type": media_type,
        "size_bytes": bytes.len(),
        "sha256": format!("sha256:{:x}", Sha256::digest(&bytes)),
        "encoding": "base64"
    });
    ToolResult {
        content: vec![
            Content::Text {
                text: serde_json::to_string(&metadata).expect("metadata serializes"),
            },
            Content::Image {
                data: BASE64.encode(bytes),
                mime_type: media_type,
            },
        ],
        is_error: false,
        structured_content: None,
    }
}

pub(super) fn scrub_artifact_metadata(response: &mut ArtifactListResponse) {
    for artifact in &mut response.artifacts {
        scrub_metadata(&mut artifact.metadata);
    }
}

fn scrub_metadata(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| {
                let key = key.to_ascii_lowercase();
                !key.contains("storage_key")
                    && !key.contains("storage_path")
                    && key != "operation_key"
                    && key != "path"
                    && !key.ends_with("_path")
            });
            for (key, value) in object {
                let key = key.to_ascii_lowercase();
                if ["token", "secret", "credential", "authorization", "header"]
                    .iter()
                    .any(|needle| key.contains(needle))
                {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    scrub_metadata(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(scrub_metadata),
        Value::String(text) => scrub_private_text(text, None),
        _ => {}
    }
}

fn scrub_private_text(text: &mut String, private_operation_key: Option<&str>) {
    if let Some(operation_key) = private_operation_key {
        *text = text.replace(operation_key, "<private operation redacted>");
    }
    if text.contains("hubu:operation:v1:") {
        *text = "<private operation redacted>".into();
    }
}

fn scrub_private_projection(value: &mut Value, private_operation_key: &str) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| key != "operation_key");
            object
                .values_mut()
                .for_each(|value| scrub_private_projection(value, private_operation_key));
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| scrub_private_projection(value, private_operation_key)),
        Value::String(text) => scrub_private_text(text, Some(private_operation_key)),
        _ => {}
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderCatalogResponse {
    schema_version: u32,
    contracts: Vec<ProviderCatalogContract>,
}

impl ProviderCatalogResponse {
    pub(super) fn validate(&self) -> Result<(), ToolError> {
        let mut ids = std::collections::BTreeSet::new();
        if self.schema_version != 1
            || self.contracts.len() > 2
            || self
                .contracts
                .iter()
                .any(|contract| !ids.insert(contract.contract.as_str()))
        {
            return Err(ToolError::invalid_response());
        }
        self.contracts
            .iter()
            .try_for_each(validate_provider_contract)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RedactionAttestationResponse {
    schema_version: u32,
    attestation_contract: String,
    allowlist_projection: bool,
    terminal_execution: bool,
    registered_provider_secret_resolved: bool,
    registered_provider_secret_absent_from_scanned_projections: bool,
    scan: RedactionScanCounters,
    facts: RedactionAttestationFacts,
    execution_sha256: String,
    artifact_sha256: String,
    settlement_sha256: String,
    combined_projection_sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RedactionScanCounters {
    logical_database_record_count: u64,
    artifact_metadata_record_count: u64,
    public_projection_count: u64,
    bytes_scanned: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RedactionAttestationFacts {
    authorization_snapshot_count: u64,
    claim_reference_count: u64,
    provider_attempt_count: u64,
    provider_submission_count: u64,
    durable_checkpoint_count: u64,
    provider_poll_count: u64,
    artifact_fetch_count: u64,
    artifact_count: u64,
    receipt_count: u64,
    settlement_delivery_count: u64,
    authorized_minor: i64,
    authorization_currency: String,
    provider_cost_minor: Option<i64>,
    provider_cost_currency: Option<String>,
    settled_minor: Option<i64>,
    settled_currency: Option<String>,
    artifact_content_sha256: String,
}

impl RedactionAttestationResponse {
    pub(super) fn validate(&self) -> Result<(), ToolError> {
        let bounded_counts = self.scan.logical_database_record_count == 4
            && self.scan.artifact_metadata_record_count == 1
            && self.scan.public_projection_count == 3
            && (1..=16 * 1024 * 1024).contains(&self.scan.bytes_scanned);
        let facts = &self.facts;
        let bounded_facts = facts.authorization_snapshot_count == 1
            && facts.claim_reference_count == 1
            && facts.provider_attempt_count == 1
            && facts.provider_submission_count == 1
            && facts.durable_checkpoint_count == 1
            && (1..=540).contains(&facts.provider_poll_count)
            && facts.artifact_fetch_count == 1
            && facts.artifact_count == 1
            && self.scan.artifact_metadata_record_count == 1
            && facts.receipt_count == 1
            && facts.settlement_delivery_count == 1
            && facts.authorized_minor == 3
            && facts.authorization_currency == "USD"
            && facts.provider_cost_minor.is_some() == facts.provider_cost_currency.is_some()
            && facts.settled_minor.is_some() == facts.settled_currency.is_some()
            && facts.provider_cost_minor == Some(3)
            && facts.provider_cost_currency.as_deref() == Some("USD")
            && facts.settled_minor == Some(3)
            && facts.settled_currency.as_deref() == Some("USD");
        let valid_digests = [
            &self.execution_sha256,
            &self.artifact_sha256,
            &self.settlement_sha256,
            &self.combined_projection_sha256,
            &facts.artifact_content_sha256,
        ]
        .into_iter()
        .all(|digest| {
            digest.strip_prefix("sha256:").is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            })
        });
        if self.schema_version != 1
            || self.attestation_contract != "gongbu.flux-redaction-attestation/v1"
            || !self.allowlist_projection
            || !self.terminal_execution
            || !self.registered_provider_secret_resolved
            || !self.registered_provider_secret_absent_from_scanned_projections
            || !bounded_counts
            || !bounded_facts
            || !valid_digests
        {
            return Err(ToolError::invalid_response());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderCatalogContract {
    contract: String,
    pricing_version: String,
    pricing_reviewed_on: String,
    target: ProviderCatalogTarget,
    capability: ProviderCatalogCapability,
    policies: ProviderCatalogPolicies,
    readiness: ProviderCatalogReadiness,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderCatalogTarget {
    workload_type: String,
    provider: String,
    adapter: String,
    model: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderCatalogCapability {
    image_count: u32,
    output_formats: Vec<String>,
    presets: Vec<ProviderCatalogPreset>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderCatalogPreset {
    name: String,
    width: u32,
    height: u32,
    currency: String,
    rate_numerator_minor: i64,
    rate_denominator: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderCatalogPolicies {
    generation_retries: u32,
    fallback: bool,
    poll: String,
    artifact_delivery: String,
    recovery: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderCatalogReadiness {
    configured: bool,
    credential_reference_present: bool,
    production_validated: bool,
    live_qualified: bool,
    live_qualification: String,
}

fn validate_provider_contract(contract: &ProviderCatalogContract) -> Result<(), ToolError> {
    match contract.contract.as_str() {
        "hubu.gemini-3.1-flash-lite-image.text-to-image/v1" => {
            validate_gemini_provider_contract(contract)
        }
        "hubu.flux-2-pro.text-to-image/v1" => validate_flux_provider_contract(contract),
        _ => Err(ToolError::invalid_response()),
    }
}

fn validate_gemini_provider_contract(contract: &ProviderCatalogContract) -> Result<(), ToolError> {
    let exact_target = contract.target.workload_type == "image_generation"
        && contract.target.provider == "google"
        && contract.target.adapter == "gemini_developer_image"
        && contract.target.model == "gemini-3.1-flash-lite-image";
    let exact_capability = contract.capability.image_count == 1
        && contract.capability.output_formats == ["png", "jpeg"]
        && contract.capability.presets.len() == 1
        && exact_provider_preset(&contract.capability.presets[0], "1k", 1024, 1024, 336, 100);
    let exact_policies = contract.policies.generation_retries == 0
        && !contract.policies.fallback
        && contract.policies.poll == "synchronous-response-v1"
        && contract.policies.artifact_delivery == "google-inline-image-v1"
        && contract.policies.recovery == "hubu-durable-synchronous-replay-v1";
    if contract.pricing_version != "google-gemini-3.1-flash-lite-image-usd-2026-09-01-v1"
        || contract.pricing_reviewed_on != "2026-09-01"
        || !exact_target
        || !exact_capability
        || !exact_policies
        || !exact_provider_readiness(contract)
    {
        return Err(ToolError::invalid_response());
    }
    Ok(())
}

fn validate_flux_provider_contract(contract: &ProviderCatalogContract) -> Result<(), ToolError> {
    let exact_target = contract.target.workload_type == "image_generation"
        && contract.target.provider == "flux"
        && contract.target.adapter == "flux2_api"
        && contract.target.model == "flux-2-pro";
    let exact_capability = contract.capability.image_count == 1
        && contract.capability.output_formats == ["png", "jpeg"]
        && exact_provider_presets(&contract.capability.presets);
    let exact_policies = contract.policies.generation_retries == 0
        && !contract.policies.fallback
        && contract.policies.poll == "bfl-async-status-poll-500ms-v1"
        && contract.policies.artifact_delivery == "bfl-delivery-single-region-label-v1"
        && contract.policies.recovery == "hubu-durable-async-resume-v1";
    if contract.pricing_version != "bfl-flux-2-pro-usd-2026-08-28-v1"
        || contract.pricing_reviewed_on != "2026-08-28"
        || !exact_target
        || !exact_capability
        || !exact_policies
        || !exact_provider_readiness(contract)
    {
        return Err(ToolError::invalid_response());
    }
    Ok(())
}

fn exact_provider_readiness(contract: &ProviderCatalogContract) -> bool {
    contract.readiness.configured
        && contract.readiness.credential_reference_present
        && contract.readiness.production_validated
        && !contract.readiness.live_qualified
        && contract.readiness.live_qualification == "not_performed"
}

fn exact_provider_preset(
    preset: &ProviderCatalogPreset,
    name: &str,
    width: u32,
    height: u32,
    numerator: i64,
    denominator: i64,
) -> bool {
    preset.name == name
        && preset.width == width
        && preset.height == height
        && preset.currency == "USD"
        && preset.rate_numerator_minor == numerator
        && preset.rate_denominator == denominator
}

fn exact_provider_presets(presets: &[ProviderCatalogPreset]) -> bool {
    let expected = [
        ("1k", 1024, 1024, 3, 1),
        ("2k", 1920, 1088, 45, 10),
        ("4k", 2048, 2048, 75, 10),
    ];
    presets.len() == expected.len()
        && presets.iter().zip(expected).all(
            |(preset, (name, width, height, numerator, denominator))| {
                exact_provider_preset(preset, name, width, height, numerator, denominator)
            },
        )
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Money {
    amount_minor: i64,
    currency: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ExecutionResponse {
    schema_version: u32,
    execution_id: String,
    operation_key: String,
    status: String,
    outcome: Option<String>,
    failure: Option<FailureResponse>,
    authorization: Money,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    #[serde(default)]
    timing: ExecutionTiming,
    #[serde(default)]
    provider_transport: Option<ProviderTransport>,
}

impl ExecutionResponse {
    pub(super) fn timing(&self) -> ExecutionTiming {
        let timing = &self.timing;
        let arithmetic_is_valid = match (
            timing.execution_total_ms,
            timing.provider_interaction_ms,
            timing.non_provider_ms,
        ) {
            (Some(total), Some(provider), non_provider) => total
                .checked_sub(provider)
                .is_some_and(|expected| non_provider == Some(expected)),
            (_, _, None) => true,
            (_, _, Some(_)) => false,
        };
        if timing.schema_version == 1 && timing.scope == "gongbu_execution" && arithmetic_is_valid {
            timing.clone()
        } else {
            ExecutionTiming::default()
        }
    }

    pub(super) fn provider_transport(&self) -> Option<ProviderTransport> {
        self.provider_transport
            .as_ref()
            .filter(|transport| {
                transport.schema_version == 1
                    && transport.poll_count <= i64::MAX.unsigned_abs()
                    && transport.artifact_fetch_count <= i64::MAX.unsigned_abs()
            })
            .cloned()
    }
}

#[derive(Debug, Serialize)]
struct PublicExecutionResponse {
    schema_version: u32,
    execution_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_handle: Option<String>,
    status: String,
    outcome: Option<String>,
    failure: Option<FailureResponse>,
    authorization: Money,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    timing: ExecutionTiming,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_transport: Option<ProviderTransport>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderTransport {
    pub(crate) schema_version: u32,
    pub(crate) poll_count: u64,
    pub(crate) artifact_fetch_count: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ExecutionTiming {
    #[serde(default)]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) scope: String,
    #[serde(default)]
    pub(crate) execution_total_ms: Option<u64>,
    #[serde(default)]
    pub(crate) provider_interaction_ms: Option<u64>,
    #[serde(default)]
    pub(crate) non_provider_ms: Option<u64>,
}

impl Default for ExecutionTiming {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scope: "gongbu_execution".into(),
            execution_total_ms: None,
            provider_interaction_ms: None,
            non_provider_ms: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct FailureResponse {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ArtifactListResponse {
    schema_version: u32,
    execution_id: String,
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutionTargetCatalogResponse {
    schema_version: u32,
    targets: Vec<ExecutionTargetResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutionTargetResponse {
    target_id: String,
    workload_type: String,
    provider: String,
    model: String,
    execution_scope: ExecutionTargetScope,
    image_sizes: Vec<String>,
    pricing: Vec<ExecutionTargetPricing>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutionTargetScope {
    schema_version: u32,
    provider: String,
    executor: String,
    capability: String,
    billing_merchant: String,
}

impl ExecutionTargetScope {
    fn validate(&self) -> bool {
        self.schema_version == 1
            && [
                &self.provider,
                &self.executor,
                &self.capability,
                &self.billing_merchant,
            ]
            .iter()
            .all(|identity| !identity.is_empty())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutionTargetPricing {
    rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selector: Option<ExecutionTargetPricingSelector>,
    currency: String,
    components: Vec<ExecutionTargetPriceComponent>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutionTargetPricingSelector {
    image_size: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutionTargetPriceComponent {
    unit: String,
    rate_numerator_minor: i64,
    rate_denominator: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArtifactResponse {
    artifact_id: String,
    execution_id: String,
    kind: String,
    media_type: String,
    size_bytes: i64,
    sha256: String,
    metadata: Value,
    metadata_schema_version: i64,
    created_at: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ToolResult {
    content: Vec<Content>,
    #[serde(rename = "isError")]
    is_error: bool,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    structured_content: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Content {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_registry::GongbuContinuation;

    fn execution() -> ExecutionResponse {
        ExecutionResponse {
            schema_version: EXECUTION_V2_SCHEMA_VERSION,
            execution_id: "exec-1".into(),
            operation_key: "operation-1".into(),
            status: "executing".into(),
            outcome: None,
            failure: None,
            authorization: Money {
                amount_minor: 25,
                currency: "USD".into(),
            },
            created_at: "now".into(),
            updated_at: "now".into(),
            started_at: None,
            completed_at: None,
            timing: ExecutionTiming::default(),
            provider_transport: Some(ProviderTransport {
                schema_version: 1,
                poll_count: 2,
                artifact_fetch_count: 1,
            }),
        }
    }

    fn continuation(execution_id: Option<&str>) -> GongbuContinuation {
        GongbuContinuation {
            operation_key: "operation-1".into(),
            operation_handle: "hubu:public-operation:v1:test".into(),
            execution_id: execution_id.map(str::to_owned),
        }
    }

    fn result_error(
        result: Result<(ToolResult, crate::operation_registry::GongbuLifecycle), ToolError>,
    ) -> ToolError {
        match result {
            Ok(_) => panic!("execution response unexpectedly passed validation"),
            Err(error) => error,
        }
    }

    fn result_json(result: ToolResult) -> Value {
        let Content::Text { text } = &result.content[0] else {
            panic!("execution result must begin with JSON text")
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn semantic_execution_response_failures_are_classified_before_persistence() {
        let mut wrong_schema = execution();
        wrong_schema.schema_version = 99;
        let mut invalid_id = execution();
        invalid_id.execution_id = "bad/execution".into();
        let mut unknown_status = execution();
        unknown_status.status = "future_status".into();

        for response in [wrong_schema, invalid_id, unknown_status] {
            let error = result_error(execution_result(
                response,
                Some(&continuation(None)),
                None,
                EXECUTION_V2_SCHEMA_VERSION,
            ));
            assert_eq!(error.code(), "invalid_response");
            assert_eq!(error.class(), ToolErrorClass::InvalidSuccessfulResponse);
        }
    }

    #[test]
    fn execution_identity_conflicts_remain_permanent_and_fail_closed() {
        let mut wrong_operation = execution();
        wrong_operation.operation_key = "another-operation".into();
        let error = result_error(execution_result(
            wrong_operation,
            Some(&continuation(None)),
            None,
            EXECUTION_V2_SCHEMA_VERSION,
        ));
        assert_eq!(error.code(), "identity_conflict");
        assert_eq!(error.class(), ToolErrorClass::IdentityConflict);

        let mut wrong_execution = execution();
        wrong_execution.schema_version = EXECUTION_V1_SCHEMA_VERSION;
        wrong_execution.execution_id = "another-execution".into();
        let error = result_error(execution_result(
            wrong_execution,
            Some(&continuation(Some("exec-1"))),
            Some("exec-1"),
            EXECUTION_V1_SCHEMA_VERSION,
        ));
        assert_eq!(error.code(), "identity_conflict");
        assert_eq!(error.class(), ToolErrorClass::IdentityConflict);
    }

    #[test]
    fn additive_execution_response_fields_remain_forward_compatible() {
        let mut value = serde_json::to_value(execution()).unwrap();
        value["future_additive_field"] = json!({"safe":true});
        let response: ExecutionResponse = serde_json::from_value(value).unwrap();
        let (_, lifecycle) = execution_result(
            response,
            Some(&continuation(None)),
            None,
            EXECUTION_V2_SCHEMA_VERSION,
        )
        .unwrap();
        assert_eq!(lifecycle.execution_id, "exec-1");
        assert_eq!(lifecycle.status, "executing");
    }

    #[test]
    fn validated_gongbu_timing_is_projected_without_private_boundaries() {
        let mut response = execution();
        response.timing = ExecutionTiming {
            schema_version: 1,
            scope: "gongbu_execution".into(),
            execution_total_ms: Some(4_000),
            provider_interaction_ms: Some(3_500),
            non_provider_ms: Some(500),
        };
        let (result, _) = execution_result(
            response,
            Some(&continuation(None)),
            None,
            EXECUTION_V2_SCHEMA_VERSION,
        )
        .unwrap();
        let public = result_json(result);
        assert_eq!(public["timing"]["schema_version"], 1);
        assert_eq!(public["timing"]["scope"], "gongbu_execution");
        assert_eq!(public["timing"]["execution_total_ms"], 4_000);
        assert_eq!(public["timing"]["provider_interaction_ms"], 3_500);
        assert_eq!(public["timing"]["non_provider_ms"], 500);
        assert_eq!(public["provider_transport"]["schema_version"], 1);
        assert_eq!(public["provider_transport"]["poll_count"], 2);
        assert_eq!(public["provider_transport"]["artifact_fetch_count"], 1);
        assert!(public["timing"].get("transmission_started_at").is_none());
        assert!(public.get("operation_key").is_none());
    }

    #[test]
    fn absent_or_inconsistent_timing_is_safely_unavailable() {
        let mut legacy = serde_json::to_value(execution()).unwrap();
        legacy.as_object_mut().unwrap().remove("timing");
        legacy.as_object_mut().unwrap().remove("provider_transport");
        let legacy: ExecutionResponse = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.timing(), ExecutionTiming::default());
        assert_eq!(legacy.provider_transport(), None);

        let mut inconsistent = execution();
        inconsistent.timing = ExecutionTiming {
            schema_version: 1,
            scope: "gongbu_execution".into(),
            execution_total_ms: Some(3_000),
            provider_interaction_ms: Some(3_500),
            non_provider_ms: None,
        };
        assert_eq!(inconsistent.timing(), ExecutionTiming::default());

        inconsistent.timing.scope = "router_polling".into();
        assert_eq!(inconsistent.timing(), ExecutionTiming::default());

        inconsistent
            .provider_transport
            .as_mut()
            .unwrap()
            .schema_version = 99;
        assert_eq!(inconsistent.provider_transport(), None);

        let mut out_of_range = execution();
        out_of_range.provider_transport.as_mut().unwrap().poll_count = u64::MAX;
        assert_eq!(out_of_range.provider_transport(), None);
    }
}
