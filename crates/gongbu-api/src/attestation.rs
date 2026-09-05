//! Gongbu-owned, execution-bound redaction attestation.
//!
//! This projector never accepts probe material from a caller. It resolves the
//! exact provider credential already bound to one persisted terminal FLUX
//! execution, scans fixed Gongbu-owned logical/public surfaces internally, and
//! returns only booleans, counters, and hashes of versioned safe projections.

use crate::{
    artifacts::ArtifactService,
    execution::{
        Artifact, Error as PersistenceError, Execution, HubuAuthorizationSnapshot, ProviderAttempt,
        Receipt, Repository,
    },
    provider::{
        contract::{PricingSnapshot, PricingUnit},
        flux2_api,
        registry::ValidatedProviderCatalog,
    },
    provider_targets::TargetKey,
    redaction::RegisteredSecretScanner,
    secrets::SecretProvider,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use thiserror::Error;

pub const CONTRACT: &str = "gongbu.flux-redaction-attestation/v1";
pub const SCHEMA_VERSION: u32 = 1;
const PROVIDER_CONTRACT_ID: &str = "hubu.flux-2-pro.text-to-image/v1";
const PROVIDER_CONFIG_VERSION: &str = "hubu-flux-2-pro-t2i-2026-08-28-v1";
const PRICING_VERSION: &str = "bfl-flux-2-pro-usd-2026-08-28-v1";
const PRICING_RULE_ID: &str = "bfl-flux-2-pro-1k-2026-08-28-v1";
const QUALIFICATION_ACCOUNT_ID: &str = "aga_n063sdm0pepd";
const QUALIFICATION_AGENT_ID: &str = "agt_wk3q33h3j6w8";
const QUALIFICATION_REASON: &str = "HUB-172 guarded FLUX live qualification: one 1k PNG.";
const QUALIFICATION_PROMPT: &str = "A small blue circle centered on a plain white background.";
const SCOPE_PROVIDER_ID: &str = "provider:black-forest-labs:flux";
const SCOPE_EXECUTOR_ID: &str = "executor:gongbu:image";
const SCOPE_CAPABILITY_ID: &str = "capability:image:generate";
const SCOPE_MERCHANT_ID: &str = "merchant:black-forest-labs";
const SECRET_SERVICE: &str = "gongbu.bfl.hubu-hub-172";
const SECRET_ACCOUNT: &str = "pikachu-live-qualification-v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RedactionAttestation {
    pub schema_version: u32,
    pub attestation_contract: String,
    pub allowlist_projection: bool,
    pub terminal_execution: bool,
    pub registered_provider_secret_resolved: bool,
    pub registered_provider_secret_absent_from_scanned_projections: bool,
    pub scan: RedactionScanCounters,
    pub facts: RedactionAttestationFacts,
    pub execution_sha256: String,
    pub artifact_sha256: String,
    pub settlement_sha256: String,
    pub combined_projection_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RedactionScanCounters {
    pub logical_database_record_count: u64,
    pub artifact_metadata_record_count: u64,
    pub public_projection_count: u64,
    pub bytes_scanned: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RedactionAttestationFacts {
    pub authorization_snapshot_count: u64,
    pub claim_reference_count: u64,
    pub provider_attempt_count: u64,
    pub provider_submission_count: u64,
    pub durable_checkpoint_count: u64,
    pub provider_poll_count: u64,
    pub artifact_fetch_count: u64,
    pub artifact_count: u64,
    pub receipt_count: u64,
    pub settlement_delivery_count: u64,
    pub authorized_minor: i64,
    pub authorization_currency: String,
    pub provider_cost_minor: Option<i64>,
    pub provider_cost_currency: Option<String>,
    pub settled_minor: Option<i64>,
    pub settled_currency: Option<String>,
    pub artifact_content_sha256: String,
}

#[derive(Debug, Error)]
pub enum AttestationError {
    #[error("execution is unavailable")]
    NotFound,
    #[error("execution is not ready for attestation")]
    NotReady,
    #[error("execution is not the contract-bound FLUX target")]
    UnsupportedTarget,
    #[error("registered provider credential is unavailable")]
    SecretUnavailable,
    #[error("Gongbu attestation failed")]
    Internal,
}

#[derive(Clone)]
pub struct RedactionAttestor {
    repository: Repository,
    artifacts: ArtifactService,
    providers: ValidatedProviderCatalog,
    secrets: Arc<dyn SecretProvider>,
}

impl RedactionAttestor {
    pub fn new(
        repository: Repository,
        artifacts: ArtifactService,
        providers: ValidatedProviderCatalog,
        secrets: Arc<dyn SecretProvider>,
    ) -> Self {
        Self {
            repository,
            artifacts,
            providers,
            secrets,
        }
    }

    /// Attest one terminal FLUX execution and the exact public projections
    /// already prepared by the authenticated HTTP layer.
    pub fn attest(
        &self,
        execution: &Execution,
        public_projections: &[Value],
    ) -> Result<RedactionAttestation, AttestationError> {
        if execution.provider != flux2_api::PROVIDER_ID
            || execution.adapter != flux2_api::ADAPTER_ID
            || execution.model != flux2_api::MODEL_ID
        {
            return Err(AttestationError::UnsupportedTarget);
        }

        let key = TargetKey::new(
            &execution.workload_type,
            &execution.provider,
            &execution.adapter,
            &execution.model,
        )
        .map_err(|_| AttestationError::UnsupportedTarget)?;
        let (target, _) = self
            .providers
            .resolve_persisted(
                &key,
                &execution.provider_config_version,
                &execution.provider_config_digest,
            )
            .map_err(|_| AttestationError::UnsupportedTarget)?;
        if !exact_provider_contract(&self.providers, execution, target) {
            return Err(AttestationError::UnsupportedTarget);
        }
        if execution.status != "succeeded" {
            return Err(AttestationError::NotReady);
        }
        let authorization = optional(
            self.repository
                .get_hubu_authorization_snapshot(&execution.execution_id),
        )?;
        let provider_attempt_count = self
            .repository
            .count_provider_attempts_for_execution(&execution.execution_id)
            .map_err(|_| AttestationError::Internal)?;
        let attempt = optional(
            self.repository
                .get_provider_attempt_for_execution(&execution.execution_id),
        )?;
        let receipt = optional(
            self.repository
                .get_receipt_for_execution(&execution.execution_id),
        )?;
        let reconciliation = optional(self.repository.get_reconciliation(&execution.execution_id))?;
        let artifacts = self
            .artifacts
            .list_for_account(&execution.execution_id, &execution.account_id)
            .map_err(|_| AttestationError::Internal)?;
        let (Some(authorization), Some(attempt), Some(receipt)) =
            (authorization.as_ref(), attempt.as_ref(), receipt.as_ref())
        else {
            return Err(AttestationError::NotReady);
        };
        if provider_attempt_count != 1
            || reconciliation.is_some()
            || artifacts.len() != 1
            || public_projections.len() != 3
            || !exact_qualification_tuple(execution, authorization, attempt, &artifacts[0], receipt)
        {
            return Err(AttestationError::NotReady);
        }
        let mut retrieved = self
            .artifacts
            .retrieve_for_account(&artifacts[0].artifact_id, &execution.account_id)
            .map_err(|_| AttestationError::Internal)?;
        if retrieved.artifact != artifacts[0] {
            return Err(AttestationError::Internal);
        }

        // Resolve the credential only after the immutable HUB-172 tuple and
        // all exact-success cardinality checks pass. The endpoint therefore
        // cannot compare attacker-selected persistence content with a secret.
        let reference = target
            .secret_reference()
            .map_err(|_| AttestationError::SecretUnavailable)?;
        if reference.service() != SECRET_SERVICE || reference.account() != SECRET_ACCOUNT {
            return Err(AttestationError::UnsupportedTarget);
        }
        let secret = self
            .secrets
            .resolve(&reference)
            .map_err(|_| AttestationError::SecretUnavailable)?;
        let scanner = RegisteredSecretScanner::new(secret.expose());

        let logical_database =
            logical_database_projection(execution, authorization, attempt, receipt);
        let artifact_metadata = artifact_scan_projection(&artifacts);
        let public_projection = Value::Array(public_projections.to_vec());
        let artifact_bytes = SensitiveBytes(std::mem::take(&mut retrieved.bytes));
        let mut scan = Scan::default();
        scan.observe(&scanner, &logical_database)?;
        scan.observe(&scanner, &artifact_metadata)?;
        scan.observe(&scanner, &public_projection)?;
        scan.observe_bytes(&scanner, &artifact_bytes.0)?;
        if scan.registered_secret_present {
            return Err(AttestationError::Internal);
        }

        let execution_projection = safe_execution_projection(execution, true, Some(attempt));
        let artifact_projection = safe_artifact_projection(&artifacts);
        let settlement_projection = safe_settlement_projection(execution, Some(receipt));
        let combined_projection = json!({
            "schema_version": SCHEMA_VERSION,
            "attestation_contract": CONTRACT,
            "execution": execution_projection,
            "artifact": artifact_projection,
            "settlement": settlement_projection,
        });
        let actual_vendor_cost = &receipt.actual_vendor_cost;
        let provider_cost_minor = actual_vendor_cost
            .to_budget_minor_units(&execution.authorization_currency)
            .map_err(|_| AttestationError::Internal)?;
        let artifact_content_sha256 = format!("sha256:{}", retrieved.artifact.sha256);

        Ok(RedactionAttestation {
            schema_version: SCHEMA_VERSION,
            attestation_contract: CONTRACT.into(),
            allowlist_projection: true,
            terminal_execution: true,
            registered_provider_secret_resolved: true,
            registered_provider_secret_absent_from_scanned_projections: true,
            scan: RedactionScanCounters {
                logical_database_record_count: 4,
                artifact_metadata_record_count: 1,
                public_projection_count: 3,
                bytes_scanned: scan.bytes_scanned,
            },
            facts: RedactionAttestationFacts {
                authorization_snapshot_count: 1,
                claim_reference_count: 1,
                provider_attempt_count,
                provider_submission_count: 1,
                durable_checkpoint_count: 1,
                provider_poll_count: attempt.provider_poll_count,
                artifact_fetch_count: attempt.artifact_fetch_count,
                artifact_count: 1,
                receipt_count: 1,
                settlement_delivery_count: 1,
                authorized_minor: execution.authorized_minor,
                authorization_currency: execution.authorization_currency.clone(),
                provider_cost_minor: Some(provider_cost_minor),
                provider_cost_currency: Some(actual_vendor_cost.currency.clone()),
                settled_minor: Some(receipt.settlement_minor),
                settled_currency: Some(receipt.currency.clone()),
                artifact_content_sha256,
            },
            execution_sha256: fingerprint(&execution_projection)?,
            artifact_sha256: fingerprint(&artifact_projection)?,
            settlement_sha256: fingerprint(&settlement_projection)?,
            combined_projection_sha256: fingerprint(&combined_projection)?,
        })
    }
}

fn exact_provider_contract(
    providers: &ValidatedProviderCatalog,
    execution: &Execution,
    target: &crate::provider_targets::ProviderConfigVersion,
) -> bool {
    let contracts = providers.provider_contracts();
    let mut matching = contracts
        .iter()
        .filter(|contract| contract.contract == PROVIDER_CONTRACT_ID);
    let Some(contract) = matching.next() else {
        return false;
    };
    if matching.next().is_some() {
        return false;
    }
    let Ok(pricing): Result<PricingSnapshot, _> =
        serde_json::from_value(execution.pricing_snapshot.clone())
    else {
        return false;
    };
    contract.contract == PROVIDER_CONTRACT_ID
        && contract.pricing_version == PRICING_VERSION
        && contract.target.workload_type == "image_generation"
        && contract.target.provider == flux2_api::PROVIDER_ID
        && contract.target.adapter == flux2_api::ADAPTER_ID
        && contract.target.model == flux2_api::MODEL_ID
        && contract.policies.generation_retries == 0
        && !contract.policies.fallback
        && contract.readiness.configured
        && contract.readiness.credential_reference_present
        && contract.readiness.production_validated
        && !contract.readiness.live_qualified
        && target.provider_config_version == PROVIDER_CONFIG_VERSION
        && target.digest() == execution.provider_config_digest
        && execution.target == "image_generation/flux/flux2_api/flux-2-pro"
        && execution.config_version == PROVIDER_CONFIG_VERSION
        && execution.provider_config_version == PROVIDER_CONFIG_VERSION
        && execution.authorized_minor == 3
        && execution.authorization_currency == "USD"
        && exact_frozen_pricing(&pricing)
}

fn exact_frozen_pricing(pricing: &PricingSnapshot) -> bool {
    pricing.catalog_version == PRICING_VERSION
        && pricing.provider == flux2_api::PROVIDER_ID
        && pricing.model == flux2_api::MODEL_ID
        && pricing.pricing_rule_id == PRICING_RULE_ID
        && pricing.exact_estimate_numerator == "3"
        && pricing.exact_estimate_denominator == "1"
        && pricing.estimated_amount_minor == 3
        && pricing.currency == "USD"
        && pricing
            .selector
            .as_ref()
            .is_some_and(|selector| selector.image_size == "1k")
        && pricing
            .output_dimensions
            .as_ref()
            .is_some_and(|dimensions| dimensions.width == 1024 && dimensions.height == 1024)
        && pricing.components.as_slice()
            == [crate::provider::contract::FrozenPriceComponent {
                unit: PricingUnit::Image,
                rate_numerator_minor: 3,
                rate_denominator: 1,
                quantity: 1,
            }]
}

fn exact_qualification_tuple(
    execution: &Execution,
    authorization: &HubuAuthorizationSnapshot,
    attempt: &ProviderAttempt,
    artifact: &Artifact,
    receipt: &Receipt,
) -> bool {
    let Some(execution_scope) = execution.execution_scope.as_ref() else {
        return false;
    };
    let canonical_input = json!({
        "prompt": QUALIFICATION_PROMPT,
        "image_count": 1,
        "image_size": "1k",
        "options": {"output_format": "png", "width": 1024, "height": 1024},
    });
    execution.account_id == QUALIFICATION_ACCOUNT_ID
        && execution.normalized_input == canonical_input
        && execution.input_schema_version == 1
        && execution.pricing_schema_version == 2
        && execution.operation_key == authorization.operation_key
        && valid_qualification_operation_key(&execution.operation_key)
        && execution.hubu_claim_id.as_deref().is_some_and(nonempty)
        && exact_scope(execution_scope)
        && authorization.account_id == QUALIFICATION_ACCOUNT_ID
        && authorization.agent_id == QUALIFICATION_AGENT_ID
        && authorization.amount_minor == 3
        && authorization.currency == "USD"
        && authorization.execution_scope == *execution_scope
        && exact_scope(&authorization.execution_scope)
        && authorization.lease_profile == "default"
        && authorization.authorization_status == "available"
        && authorization.task_id.is_none()
        && authorization.reason == QUALIFICATION_REASON
        && execution.outcome.as_deref() == Some("succeeded")
        && execution
            .provider_outcome
            .is_some_and(|value| value.as_str() == "succeeded")
        && execution
            .artifact_outcome
            .is_some_and(|value| value.as_str() == "succeeded")
        && execution
            .settlement_outcome
            .is_some_and(|value| value.as_str() == "succeeded")
        && execution.completed_at.is_some()
        && execution.failure_code.is_none()
        && execution.failure_message_redacted.is_none()
        && execution.release_transmission_started_at.is_none()
        && attempt.execution_id == execution.execution_id
        && attempt.provider == flux2_api::PROVIDER_ID
        && attempt.outcome == "succeeded"
        && attempt.transmission_started_at.is_some()
        && attempt.operation_checkpointed_at.is_some()
        && (1..=540).contains(&attempt.provider_poll_count)
        && attempt.artifact_fetch_count == 1
        && attempt.completed_at.is_some()
        && attempt.failure_code.is_none()
        && attempt.failure_message_redacted.is_none()
        && exact_qualification_artifact(
            artifact,
            &execution.execution_id,
            &attempt.provider_attempt_id,
        )
        && receipt.execution_id == execution.execution_id
        && receipt.provider_attempt_id == attempt.provider_attempt_id
        && receipt.settlement_minor == 3
        && receipt.currency == "USD"
        && receipt.pricing_catalog_version == PRICING_VERSION
        && receipt.actual_vendor_cost.to_budget_minor_units("USD").ok() == Some(3)
        && receipt.actual_vendor_cost.currency == "USD"
        && receipt.transmission_started_at.is_some()
        && receipt.settled_at.is_some()
        && receipt.hubu_settlement_id.as_deref().is_some_and(nonempty)
}

fn exact_qualification_artifact(
    artifact: &Artifact,
    execution_id: &str,
    provider_attempt_id: &str,
) -> bool {
    artifact.execution_id == execution_id
        && artifact.provider_attempt_id.as_deref() == Some(provider_attempt_id)
        && artifact.kind == "image"
        && artifact.storage_backend == "local_fs"
        && artifact.media_type == "image/png"
        && (1..=8_388_608).contains(&artifact.size_bytes)
        && artifact.metadata_schema_version == 1
        && artifact.metadata == json!({"height": 1024, "width": 1024})
        && artifact.sha256.len() == 64
        && artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_qualification_operation_key(value: &str) -> bool {
    value.strip_prefix("codex:v1:").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn exact_scope(scope: &crate::execution_scope::ExecutionScope) -> bool {
    scope.schema_version == 1
        && scope.provider.id == SCOPE_PROVIDER_ID
        && scope.executor.id == SCOPE_EXECUTOR_ID
        && scope.capability.id == SCOPE_CAPABILITY_ID
        && scope.billing_merchant.id == SCOPE_MERCHANT_ID
}

fn nonempty(value: &str) -> bool {
    !value.trim().is_empty()
}

fn optional<T>(result: crate::execution::Result<T>) -> Result<Option<T>, AttestationError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(PersistenceError::NotFound) => Ok(None),
        Err(_) => Err(AttestationError::Internal),
    }
}

#[derive(Default)]
struct Scan {
    registered_secret_present: bool,
    bytes_scanned: u64,
}

impl Scan {
    fn observe(
        &mut self,
        scanner: &RegisteredSecretScanner,
        value: &Value,
    ) -> Result<(), AttestationError> {
        let mut bytes = SensitiveBytes(
            serde_json::to_vec(&canonicalize(value)).map_err(|_| AttestationError::Internal)?,
        );
        self.registered_secret_present |= scanner.contains(&bytes.0);
        self.bytes_scanned = self
            .bytes_scanned
            .checked_add(u64::try_from(bytes.0.len()).map_err(|_| AttestationError::Internal)?)
            .ok_or(AttestationError::Internal)?;
        bytes.0.fill(0);
        Ok(())
    }

    fn observe_bytes(
        &mut self,
        scanner: &RegisteredSecretScanner,
        bytes: &[u8],
    ) -> Result<(), AttestationError> {
        self.registered_secret_present |= scanner.contains(bytes);
        self.bytes_scanned = self
            .bytes_scanned
            .checked_add(u64::try_from(bytes.len()).map_err(|_| AttestationError::Internal)?)
            .ok_or(AttestationError::Internal)?;
        Ok(())
    }
}

struct SensitiveBytes(Vec<u8>);

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

fn fingerprint(value: &Value) -> Result<String, AttestationError> {
    let bytes = serde_json::to_vec(&canonicalize(value)).map_err(|_| AttestationError::Internal)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonicalize(value)))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

fn logical_database_projection(
    execution: &Execution,
    authorization: &HubuAuthorizationSnapshot,
    attempt: &ProviderAttempt,
    receipt: &Receipt,
) -> Value {
    json!({
        "execution": {
            "execution_id": execution.execution_id,
            "account_id": execution.account_id,
            "hubu_authorization_id": execution.hubu_authorization_id,
            "hubu_claim_id": execution.hubu_claim_id,
            "hubu_token_reference": execution.hubu_token_reference.as_str(),
            "authorized_minor": execution.authorized_minor,
            "authorization_currency": execution.authorization_currency,
            "input_hash": execution.input_hash,
            "input_schema_version": execution.input_schema_version,
            "target": execution.target,
            "config_version": execution.config_version,
            "workload_type": execution.workload_type,
            "provider": execution.provider,
            "adapter": execution.adapter,
            "model": execution.model,
            "provider_config_version": execution.provider_config_version,
            "provider_config_digest": execution.provider_config_digest,
            "pricing_snapshot": execution.pricing_snapshot,
            "pricing_schema_version": execution.pricing_schema_version,
            "execution_scope": execution.execution_scope,
            "status": execution.status,
            "outcome": execution.outcome,
            "provider_outcome": execution.provider_outcome.map(|value| value.as_str()),
            "artifact_outcome": execution.artifact_outcome.map(|value| value.as_str()),
            "settlement_outcome": execution.settlement_outcome.map(|value| value.as_str()),
            "failure_code": execution.failure_code,
            "failure_message_redacted": execution.failure_message_redacted,
            "created_at": execution.created_at,
            "updated_at": execution.updated_at,
            "started_at": execution.started_at,
            "completed_at": execution.completed_at,
            "release_transmission_started_at": execution.release_transmission_started_at,
            "version": execution.version,
        },
        "authorization": {
            "account_id": authorization.account_id,
            "agent_id": authorization.agent_id,
            "decision_id": authorization.decision_id,
            "spend_auth_token_id": authorization.spend_auth_token_id,
            "amount_minor": authorization.amount_minor,
            "currency": authorization.currency,
            "execution_scope": authorization.execution_scope,
            "lease_profile": authorization.lease_profile,
            "expires_at": authorization.expires_at,
            "authorization_status": authorization.authorization_status,
        },
        "attempt": {
            "provider_attempt_id": attempt.provider_attempt_id,
            "execution_id": attempt.execution_id,
            "provider": attempt.provider,
            "provider_request_id": attempt.provider_request_id,
            "provider_operation_id": attempt.provider_operation_id,
            "provider_polling_host": attempt.provider_polling_host,
            "provider_deadline_unix_ms": attempt.provider_deadline_unix_ms,
            "operation_checkpointed_at": attempt.operation_checkpointed_at,
            "provider_poll_count": attempt.provider_poll_count,
            "artifact_fetch_count": attempt.artifact_fetch_count,
            "outcome": attempt.outcome,
            "usage": attempt.usage,
            "usage_schema_version": attempt.usage_schema_version,
            "actual_vendor_cost": attempt.actual_vendor_cost,
            "failure_code": attempt.failure_code,
            "failure_message_redacted": attempt.failure_message_redacted,
            "started_at": attempt.started_at,
            "transmission_started_at": attempt.transmission_started_at,
            "completed_at": attempt.completed_at,
        },
        "receipt": {
            "receipt_id": receipt.receipt_id,
            "execution_id": receipt.execution_id,
            "provider_attempt_id": receipt.provider_attempt_id,
            "settlement_minor": receipt.settlement_minor,
            "currency": receipt.currency,
            "pricing_catalog_version": receipt.pricing_catalog_version,
            "actual_vendor_cost": receipt.actual_vendor_cost,
            "provider_request_id": receipt.provider_request_id,
            "price_model_snapshot": receipt.price_model_snapshot,
            "created_at": receipt.created_at,
            "transmission_started_at": receipt.transmission_started_at,
            "settled_at": receipt.settled_at,
            "hubu_settlement_id": receipt.hubu_settlement_id,
        },
    })
}

fn artifact_scan_projection(artifacts: &[Artifact]) -> Value {
    Value::Array(
        artifacts
            .iter()
            .map(|artifact| {
                json!({
                    "artifact_id": artifact.artifact_id,
                    "execution_id": artifact.execution_id,
                    "provider_attempt_id": artifact.provider_attempt_id,
                    "kind": artifact.kind,
                    "storage_backend": artifact.storage_backend,
                    "storage_key": artifact.storage_key,
                    "media_type": artifact.media_type,
                    "size_bytes": artifact.size_bytes,
                    "sha256": artifact.sha256,
                    "metadata": artifact.metadata,
                    "metadata_schema_version": artifact.metadata_schema_version,
                    "created_at": artifact.created_at,
                })
            })
            .collect(),
    )
}

fn safe_execution_projection(
    execution: &Execution,
    authorization_present: bool,
    attempt: Option<&ProviderAttempt>,
) -> Value {
    json!({
        "projection_schema_version": 1,
        "authorization_present": authorization_present,
        "claim_present": execution.hubu_claim_id.is_some(),
        "input_hash": execution.input_hash,
        "input_schema_version": execution.input_schema_version,
        "target": execution.target,
        "config_version": execution.config_version,
        "provider_config_version": execution.provider_config_version,
        "pricing_schema_version": execution.pricing_schema_version,
        "authorized_minor": execution.authorized_minor,
        "authorization_currency": execution.authorization_currency,
        "status": execution.status,
        "outcome": execution.outcome,
        "provider_outcome": execution.provider_outcome.map(|value| value.as_str()),
        "artifact_outcome": execution.artifact_outcome.map(|value| value.as_str()),
        "settlement_outcome": execution.settlement_outcome.map(|value| value.as_str()),
        "provider_attempt": attempt.map(|value| json!({
            "outcome": value.outcome,
            "usage": value.usage,
            "usage_schema_version": value.usage_schema_version,
            "actual_vendor_cost": value.actual_vendor_cost,
            "provider_request_id_present": value.provider_request_id.is_some(),
            "provider_operation_id_present": value.provider_operation_id.is_some(),
            "transmission_started": value.transmission_started_at.is_some(),
            "durable_checkpointed": value.operation_checkpointed_at.is_some(),
            "provider_poll_count": value.provider_poll_count,
            "artifact_fetch_count": value.artifact_fetch_count,
            "completed": value.completed_at.is_some(),
        })),
    })
}

fn safe_artifact_projection(artifacts: &[Artifact]) -> Value {
    json!({
        "projection_schema_version": 1,
        "artifact_count": artifacts.len(),
        "artifacts": artifacts.iter().map(|artifact| json!({
            "kind": artifact.kind,
            "media_type": artifact.media_type,
            "size_bytes": artifact.size_bytes,
            "sha256": artifact.sha256,
            "metadata": artifact.metadata,
            "metadata_schema_version": artifact.metadata_schema_version,
        })).collect::<Vec<_>>(),
    })
}

fn safe_settlement_projection(execution: &Execution, receipt: Option<&Receipt>) -> Value {
    json!({
        "projection_schema_version": 1,
        "authorized_minor": execution.authorized_minor,
        "authorization_currency": execution.authorization_currency,
        "receipt_count": usize::from(receipt.is_some()),
        "receipt": receipt.map(|value| json!({
            "settlement_minor": value.settlement_minor,
            "currency": value.currency,
            "pricing_catalog_version": value.pricing_catalog_version,
            "actual_vendor_cost": value.actual_vendor_cost,
            "transmission_started": value.transmission_started_at.is_some(),
            "settled": value.settled_at.is_some(),
            "settlement_id_present": value.hubu_settlement_id.is_some(),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frozen_pricing() -> PricingSnapshot {
        serde_json::from_value(json!({
            "schema_version": 2,
            "provider": "flux",
            "model": "flux-2-pro",
            "catalog_version": PRICING_VERSION,
            "catalog_digest": "sha256:fixture",
            "pricing_rule_id": PRICING_RULE_ID,
            "selector": {"image_size": "1k"},
            "output_dimensions": {"width": 1024, "height": 1024},
            "components": [{
                "unit": "image",
                "rate_numerator_minor": 3,
                "rate_denominator": 1,
                "quantity": 1
            }],
            "exact_estimate_numerator": "3",
            "exact_estimate_denominator": "1",
            "estimated_amount_minor": 3,
            "currency": "USD"
        }))
        .unwrap()
    }

    #[test]
    fn frozen_pricing_binding_rejects_rule_and_rational_component_mutations() {
        let snapshot = frozen_pricing();
        assert!(exact_frozen_pricing(&snapshot));

        let mut mutations = Vec::new();
        let mut changed = snapshot.clone();
        changed.pricing_rule_id = "same-catalog-other-rule".into();
        mutations.push(changed);
        let mut changed = snapshot.clone();
        changed.exact_estimate_numerator = "6".into();
        mutations.push(changed);
        let mut changed = snapshot.clone();
        changed.exact_estimate_denominator = "2".into();
        mutations.push(changed);
        let mut changed = snapshot.clone();
        changed.components[0].rate_numerator_minor = 6;
        mutations.push(changed);
        let mut changed = snapshot.clone();
        changed.components[0].rate_denominator = 2;
        mutations.push(changed);
        let mut changed = snapshot;
        changed.components[0].quantity = 2;
        mutations.push(changed);

        assert!(mutations
            .iter()
            .all(|pricing| !exact_frozen_pricing(pricing)));
    }

    #[test]
    fn qualification_artifact_binding_rejects_dimensions_metadata_and_size_mutations() {
        let artifact = Artifact {
            artifact_id: "artifact".into(),
            execution_id: "execution".into(),
            provider_attempt_id: Some("attempt".into()),
            kind: "image".into(),
            storage_backend: "local_fs".into(),
            storage_key: "executions/execution/artifact.png".into(),
            media_type: "image/png".into(),
            size_bytes: 1024,
            sha256: "a".repeat(64),
            metadata: json!({"height": 1024, "width": 1024}),
            metadata_schema_version: 1,
            created_at: "fixture".into(),
        };
        assert!(exact_qualification_artifact(
            &artifact,
            "execution",
            "attempt"
        ));

        let mut mutations = Vec::new();
        let mut changed = artifact.clone();
        changed.metadata["height"] = json!(1);
        mutations.push(changed);
        let mut changed = artifact.clone();
        changed.metadata["extra"] = json!(true);
        mutations.push(changed);
        let mut changed = artifact.clone();
        changed.metadata_schema_version = 2;
        mutations.push(changed);
        let mut changed = artifact.clone();
        changed.size_bytes = 8_388_609;
        mutations.push(changed);
        let mut changed = artifact;
        changed.media_type = "image/jpeg".into();
        mutations.push(changed);

        assert!(mutations
            .iter()
            .all(|artifact| !exact_qualification_artifact(artifact, "execution", "attempt")));
    }
}
