//! Deterministic orchestration for one durable execution.
//!
//! Temporal owns delivery of `run`; this module makes each externally visible
//! activity replay-safe by consulting the persisted aggregate before acting.
use crate::{
    execution::{
        AttemptResult, CreateReceiptParams, Error as PersistenceError, Execution, ExecutionUpdate,
        LifecycleOutcome, Repository, StagedProviderArtifact,
    },
    provider_contract::{
        ActualVendorCost, AsyncProviderOperation, ContractError, NormalizedUsage, PricingSnapshot,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::to_value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderArtifact {
    pub media_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ProviderSuccess {
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
    pub usage: NormalizedUsage,
    /// Exact provider-reported billing evidence. The frozen catalog is used
    /// only when the provider does not report a charge.
    pub actual_vendor_cost: Option<ActualVendorCost>,
    pub artifacts: Vec<ProviderArtifact>,
}
#[derive(Clone, Debug)]
pub enum ProviderSubmission {
    Complete(ProviderSuccess),
    Pending(AsyncProviderOperation),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityError {
    Proven(String),
    ProvenWithEvidence {
        code: String,
        request_id: Option<String>,
        operation_id: Option<String>,
    },
    Ambiguous(String),
    AmbiguousWithEvidence {
        code: String,
        request_id: Option<String>,
        operation_id: Option<String>,
    },
}

pub trait HubuActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError>;
    fn claim(&self, execution: &Execution) -> Result<String, ActivityError>;
    /// Confirm the persisted claim is still active immediately before paid work.
    fn validate_claim(&self, execution: &Execution) -> Result<(), ActivityError>;
    fn settle(
        &self,
        execution: &Execution,
        receipt_id: &str,
        amount_minor: i64,
    ) -> Result<String, ActivityError>;
    fn release(&self, execution: &Execution) -> Result<(), ActivityError>;
}
pub trait ProviderActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError>;
    fn invoke(
        &self,
        execution: &Execution,
        attempt_id: &str,
    ) -> Result<ProviderSuccess, ActivityError>;
    fn submit(
        &self,
        execution: &Execution,
        attempt_id: &str,
    ) -> Result<ProviderSubmission, ActivityError> {
        self.invoke(execution, attempt_id)
            .map(ProviderSubmission::Complete)
    }
    fn poll_existing(
        &self,
        _execution: &Execution,
        _attempt_id: &str,
        operation: &AsyncProviderOperation,
    ) -> Result<ProviderSuccess, ActivityError> {
        Err(ActivityError::AmbiguousWithEvidence {
            code: "provider_resume_unsupported".into(),
            request_id: operation.provider_request_id.clone(),
            operation_id: Some(operation.provider_operation_id.clone()),
        })
    }
}
pub trait ArtifactActivities {
    fn preflight(&self) -> Result<(), ActivityError>;
    fn persist(
        &self,
        execution: &Execution,
        attempt_id: &str,
        artifacts: &[ProviderArtifact],
    ) -> Result<(), ActivityError>;
}

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}

pub struct ExecutionWorkflow<'a> {
    pub repository: &'a Repository,
    pub hubu: &'a dyn HubuActivities,
    pub provider: &'a dyn ProviderActivities,
    pub artifacts: &'a dyn ArtifactActivities,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationAction {
    Reinspect,
    Settle,
    Release,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct OperatorReconciliationRequest {
    pub action_id: String,
    pub action: ReconciliationAction,
    pub evidence: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExecutionPhaseResult {
    pub status: String,
    pub failure_code: Option<String>,
}

impl From<Execution> for ExecutionPhaseResult {
    fn from(execution: Execution) -> Self {
        Self {
            status: execution.status,
            failure_code: execution.failure_code,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPhaseOutcome {
    PollExisting,
    PersistArtifacts,
    ReleaseAuthorization,
    Complete(ExecutionPhaseResult),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderCheckpointBoundary {
    BeforePersist,
    AfterPersist,
}

impl ExecutionWorkflow<'_> {
    pub fn run(&self, execution_id: &str, now: &str) -> Result<Execution, WorkflowError> {
        self.run_with_clock(execution_id, &|| now.to_owned())
    }

    /// Run using a fresh durable timestamp for each phase and both provider
    /// transmission boundaries. Temporal's granular runner uses the same clock
    /// path for its provider activity.
    pub(crate) fn run_with_clock(
        &self,
        execution_id: &str,
        clock: &dyn Fn() -> String,
    ) -> Result<Execution, WorkflowError> {
        loop {
            let execution = self.repository.get_execution(execution_id)?;
            let now = clock();
            if execution.status == "reconciliation_required"
                && matches!(
                    self.repository.get_reconciliation(execution_id),
                    Err(PersistenceError::NotFound)
                )
            {
                self.repository.record_reconciliation(
                    &execution,
                    "reconciliation_replay",
                    execution.failure_code.as_deref(),
                    &now,
                )?;
            }
            if terminal(&execution.status) {
                return Ok(execution);
            }
            match execution.status.as_str() {
                "pending" => {
                    self.preflight_phase(execution_id, &now)?;
                }
                "preflighting" => {
                    self.claim_phase(execution_id, &now)?;
                }
                "claimed" => {
                    self.validate_claim_phase(execution_id, &now)?;
                }
                "executing" => {
                    let mut outcome =
                        self.provider_submit_phase_with_clock(execution_id, &now, clock)?;
                    if outcome == ProviderPhaseOutcome::PollExisting {
                        outcome =
                            self.provider_poll_phase_with_clock(execution_id, &clock(), clock)?;
                    }
                    match outcome {
                        ProviderPhaseOutcome::PersistArtifacts => {
                            self.artifact_phase(execution_id, &clock())?;
                        }
                        ProviderPhaseOutcome::ReleaseAuthorization => {
                            self.release_phase(execution_id, &clock())?;
                        }
                        ProviderPhaseOutcome::PollExisting | ProviderPhaseOutcome::Complete(_) => {}
                    }
                }
                "settling" => {
                    self.settlement_phase(execution_id, &now)?;
                }
                _ => return Err(PersistenceError::Invalid("workflow status").into()),
            }
        }
    }
    pub fn preflight_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<Execution, WorkflowError> {
        let mut execution = self.repository.get_execution(execution_id)?;
        if execution.status == "pending" {
            execution = self.transition(&execution, "preflighting", None, now, None, None, None)?;
        }
        if execution.status == "preflighting" {
            if let Err(error) = self.preflight(&execution) {
                self.fail_before_claim(&execution, error, now)?;
            }
        }
        self.repository
            .get_execution(execution_id)
            .map_err(Into::into)
    }

    pub fn claim_phase(&self, execution_id: &str, now: &str) -> Result<Execution, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "preflighting" {
            return Ok(execution);
        }
        if execution.hubu_claim_id.is_some() {
            return self
                .repository
                .accept_existing_claim(execution_id, execution.version, now)
                .map_err(Into::into);
        }
        match self.hubu.claim(&execution) {
            Ok(claim) => self
                .repository
                .set_claim(execution_id, execution.version, &claim, now)
                .map_err(Into::into),
            Err(ActivityError::Proven(code))
            | Err(ActivityError::ProvenWithEvidence { code, .. }) => {
                self.transition(&execution, "failed", Some(&code), now, None, None, None)
            }
            Err(ActivityError::Ambiguous(code))
            | Err(ActivityError::AmbiguousWithEvidence { code, .. }) => self.transition(
                &execution,
                "reconciliation_required",
                Some(&code),
                now,
                None,
                None,
                Some("ambiguous"),
            ),
        }
    }

    pub fn validate_claim_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<Execution, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "claimed" {
            return Ok(execution);
        }
        match self.hubu.validate_claim(&execution) {
            Ok(()) => {
                self.repository.start_provider_attempt(&execution, now)?;
                self.repository
                    .get_execution(execution_id)
                    .map_err(Into::into)
            }
            Err(ActivityError::Proven(code))
            | Err(ActivityError::ProvenWithEvidence { code, .. })
            | Err(ActivityError::Ambiguous(code))
            | Err(ActivityError::AmbiguousWithEvidence { code, .. }) => self.transition(
                &execution,
                "reconciliation_required",
                Some(&code),
                now,
                None,
                None,
                Some("ambiguous"),
            ),
        }
    }

    pub fn provider_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        self.provider_phase_with_clock(execution_id, now, &|| now.to_owned())
    }

    /// Legacy single-activity wrapper retained for Temporal histories created
    /// before HUB-170. New workflows call the submit and poll phases as
    /// separate activities.
    pub(crate) fn provider_phase_with_clock(
        &self,
        execution_id: &str,
        phase_at: &str,
        clock: &dyn Fn() -> String,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        let outcome = self.provider_submit_phase_with_clock(execution_id, phase_at, clock)?;
        if outcome == ProviderPhaseOutcome::PollExisting {
            self.provider_poll_phase_with_clock(execution_id, &clock(), clock)
        } else {
            Ok(outcome)
        }
    }

    pub fn provider_submit_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        self.provider_submit_phase_with_clock(execution_id, now, &|| now.to_owned())
    }

    pub(crate) fn provider_submit_phase_with_clock(
        &self,
        execution_id: &str,
        phase_at: &str,
        clock: &dyn Fn() -> String,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        self.provider_submit_phase_with_checkpoint_hook(execution_id, phase_at, clock, &|_| {})
    }

    fn provider_submit_phase_with_checkpoint_hook(
        &self,
        execution_id: &str,
        phase_at: &str,
        clock: &dyn Fn() -> String,
        checkpoint_hook: &dyn Fn(ProviderCheckpointBoundary),
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "executing" {
            return Ok(ProviderPhaseOutcome::Complete(execution.into()));
        }
        let attempt = self
            .repository
            .get_provider_attempt_for_execution(execution_id)?;
        if let Some(outcome) = self.completed_provider_phase(&execution, &attempt, phase_at)? {
            return Ok(outcome);
        }
        match self.repository.provider_operation(&attempt) {
            Ok(Some(_)) => return Ok(ProviderPhaseOutcome::PollExisting),
            Err(_) => {
                let held = self.transition(
                    &execution,
                    "reconciliation_required",
                    Some("provider_operation_checkpoint_invalid"),
                    phase_at,
                    Some("ambiguous"),
                    None,
                    None,
                )?;
                return Ok(ProviderPhaseOutcome::Complete(held.into()));
            }
            Ok(None) => {}
        }
        if attempt.transmission_started_at.is_some() {
            let held = self.transition(
                &execution,
                "reconciliation_required",
                Some("provider_submission_interrupted"),
                phase_at,
                Some("ambiguous"),
                None,
                None,
            )?;
            return Ok(ProviderPhaseOutcome::Complete(held.into()));
        }
        let transmission_started_at = clock();
        self.repository
            .begin_provider_transmission(&attempt.provider_attempt_id, &transmission_started_at)?;
        let provider_result = self
            .provider
            .submit(&execution, &attempt.provider_attempt_id);
        if let Ok(ProviderSubmission::Pending(operation)) = &provider_result {
            checkpoint_hook(ProviderCheckpointBoundary::BeforePersist);
            let checkpointed_at = clock();
            match self.repository.record_provider_operation(
                &attempt.provider_attempt_id,
                operation,
                &checkpointed_at,
            ) {
                Ok(_) => {
                    checkpoint_hook(ProviderCheckpointBoundary::AfterPersist);
                    return Ok(ProviderPhaseOutcome::PollExisting);
                }
                Err(_) => {
                    let persisted = self
                        .repository
                        .get_provider_attempt(&attempt.provider_attempt_id)
                        .ok()
                        .and_then(|attempt| self.repository.provider_operation(&attempt).ok())
                        .flatten();
                    if persisted.as_ref() == Some(operation) {
                        checkpoint_hook(ProviderCheckpointBoundary::AfterPersist);
                        return Ok(ProviderPhaseOutcome::PollExisting);
                    }
                    let held = self.transition(
                        &execution,
                        "reconciliation_required",
                        Some("provider_operation_persistence_failed"),
                        &checkpointed_at,
                        Some("ambiguous"),
                        None,
                        None,
                    )?;
                    return Ok(ProviderPhaseOutcome::Complete(held.into()));
                }
            }
        }
        let completed_at = clock();
        match provider_result {
            Ok(ProviderSubmission::Complete(success)) => self.finish_provider_result(
                &execution,
                &attempt.provider_attempt_id,
                Ok(success),
                &completed_at,
            ),
            Ok(ProviderSubmission::Pending(_)) => {
                unreachable!("pending provider submission returns after checkpoint")
            }
            Err(error) => self.finish_provider_result(
                &execution,
                &attempt.provider_attempt_id,
                Err(error),
                &completed_at,
            ),
        }
    }

    pub fn provider_poll_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        self.provider_poll_phase_with_clock(execution_id, now, &|| now.to_owned())
    }

    pub(crate) fn provider_poll_phase_with_clock(
        &self,
        execution_id: &str,
        phase_at: &str,
        clock: &dyn Fn() -> String,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "executing" {
            return Ok(ProviderPhaseOutcome::Complete(execution.into()));
        }
        let attempt = self
            .repository
            .get_provider_attempt_for_execution(execution_id)?;
        if let Some(outcome) = self.completed_provider_phase(&execution, &attempt, phase_at)? {
            return Ok(outcome);
        }
        let operation = match self.repository.provider_operation(&attempt) {
            Ok(Some(operation)) => operation,
            Ok(None) | Err(_) => {
                let held = self.transition(
                    &execution,
                    "reconciliation_required",
                    Some("provider_operation_checkpoint_invalid"),
                    phase_at,
                    Some("ambiguous"),
                    None,
                    None,
                )?;
                return Ok(ProviderPhaseOutcome::Complete(held.into()));
            }
        };
        let provider_result =
            self.provider
                .poll_existing(&execution, &attempt.provider_attempt_id, &operation);
        let provider_result = bind_poll_result_to_checkpoint(provider_result, &operation);
        self.finish_provider_result(
            &execution,
            &attempt.provider_attempt_id,
            provider_result,
            &clock(),
        )
    }

    fn completed_provider_phase(
        &self,
        execution: &Execution,
        attempt: &crate::execution::ProviderAttempt,
        phase_at: &str,
    ) -> Result<Option<ProviderPhaseOutcome>, WorkflowError> {
        if attempt.completed_at.is_none() {
            return Ok(None);
        }
        let outcome = match attempt.outcome.as_str() {
            "succeeded"
                if !self
                    .repository
                    .get_staged_provider_artifacts(&attempt.provider_attempt_id)?
                    .is_empty() =>
            {
                ProviderPhaseOutcome::PersistArtifacts
            }
            "failed" => ProviderPhaseOutcome::ReleaseAuthorization,
            outcome => {
                self.advance_completed_attempt(execution, outcome, phase_at)?;
                ProviderPhaseOutcome::Complete(
                    self.repository
                        .get_execution(&execution.execution_id)?
                        .into(),
                )
            }
        };
        Ok(Some(outcome))
    }

    fn finish_provider_result(
        &self,
        execution: &Execution,
        attempt_id: &str,
        provider_result: Result<ProviderSuccess, ActivityError>,
        completed_at: &str,
    ) -> Result<ProviderPhaseOutcome, WorkflowError> {
        match provider_result {
            Ok(success) if success.artifacts.is_empty() => {
                self.repository.complete_provider_attempt(
                    attempt_id,
                    &successful_attempt(&success, completed_at),
                )?;
                let held = self.transition(
                    execution,
                    "reconciliation_required",
                    Some("provider_returned_no_artifacts"),
                    completed_at,
                    Some("succeeded"),
                    Some("failed"),
                    None,
                )?;
                Ok(ProviderPhaseOutcome::Complete(held.into()))
            }
            Ok(success) => {
                let staged = success
                    .artifacts
                    .iter()
                    .map(|artifact| StagedProviderArtifact {
                        media_type: artifact.media_type.clone(),
                        bytes: artifact.bytes.clone(),
                    })
                    .collect::<Vec<_>>();
                self.repository.complete_provider_attempt_with_artifacts(
                    attempt_id,
                    &successful_attempt(&success, completed_at),
                    &staged,
                )?;
                Ok(ProviderPhaseOutcome::PersistArtifacts)
            }
            Err(ActivityError::Proven(code)) => {
                self.repository.complete_provider_attempt(
                    attempt_id,
                    &attempt_failure("failed", &code, completed_at),
                )?;
                Ok(ProviderPhaseOutcome::ReleaseAuthorization)
            }
            Err(ActivityError::ProvenWithEvidence {
                code,
                request_id,
                operation_id,
            }) => {
                let mut failure = attempt_failure("failed", &code, completed_at);
                failure.provider_request_id = request_id;
                failure.provider_operation_id = operation_id;
                self.repository
                    .complete_provider_attempt(attempt_id, &failure)?;
                Ok(ProviderPhaseOutcome::ReleaseAuthorization)
            }
            Err(ActivityError::Ambiguous(code)) => {
                self.repository.complete_provider_attempt(
                    attempt_id,
                    &attempt_failure("ambiguous", &code, completed_at),
                )?;
                let held = self.transition(
                    execution,
                    "reconciliation_required",
                    Some(&code),
                    completed_at,
                    Some("ambiguous"),
                    None,
                    None,
                )?;
                Ok(ProviderPhaseOutcome::Complete(held.into()))
            }
            Err(ActivityError::AmbiguousWithEvidence {
                code,
                request_id,
                operation_id,
            }) => {
                let mut failure = attempt_failure("ambiguous", &code, completed_at);
                failure.provider_request_id = request_id;
                failure.provider_operation_id = operation_id;
                self.repository
                    .complete_provider_attempt(attempt_id, &failure)?;
                if code == "polling_origin_rejected" {
                    let attempt = self.repository.get_provider_attempt(attempt_id)?;
                    if let (Some(context), Some(operation_id)) = (
                        attempt.provider_recovery_context,
                        attempt.provider_operation_id.as_deref(),
                    ) {
                        crate::lifecycle::log_polling_origin_rejection(
                            &attempt.provider,
                            &execution.execution_id,
                            attempt_id,
                            attempt.provider_request_id.as_deref(),
                            operation_id,
                            &context.policy_version,
                            &context.url_fingerprint,
                        );
                    }
                }
                let held = self.transition(
                    execution,
                    "reconciliation_required",
                    Some(&code),
                    completed_at,
                    Some("ambiguous"),
                    None,
                    None,
                )?;
                Ok(ProviderPhaseOutcome::Complete(held.into()))
            }
        }
    }

    pub fn artifact_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<Execution, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "executing" {
            return Ok(execution);
        }
        let attempt = self
            .repository
            .get_provider_attempt_for_execution(execution_id)?;
        if attempt.completed_at.is_none() || attempt.outcome != "succeeded" {
            return Ok(execution);
        }
        let staged = self
            .repository
            .get_staged_provider_artifacts(&attempt.provider_attempt_id)?;
        if staged.is_empty() {
            self.advance_completed_attempt(&execution, &attempt.outcome, now)?;
            return self
                .repository
                .get_execution(execution_id)
                .map_err(Into::into);
        }
        let durable_count = self
            .repository
            .count_artifacts_for_attempt(&attempt.provider_attempt_id)?;
        if durable_count == 0 {
            let artifacts = staged
                .iter()
                .map(|artifact| ProviderArtifact {
                    media_type: artifact.media_type.clone(),
                    bytes: artifact.bytes.clone(),
                })
                .collect::<Vec<_>>();
            if let Err(error) =
                self.artifacts
                    .persist(&execution, &attempt.provider_attempt_id, &artifacts)
            {
                let code = activity_error_code(error);
                return self.transition(
                    &execution,
                    "reconciliation_required",
                    Some(&code),
                    now,
                    Some("succeeded"),
                    Some("failed"),
                    None,
                );
            }
        }
        let durable_count = self
            .repository
            .count_artifacts_for_attempt(&attempt.provider_attempt_id)?;
        let usage: NormalizedUsage = serde_json::from_value(
            attempt
                .usage
                .ok_or(PersistenceError::Invalid("attempt usage"))?,
        )
        .map_err(|_| PersistenceError::Invalid("attempt usage"))?;
        if !self.artifacts_match_usage_count(&execution, durable_count, staged.len(), &usage)? {
            return self.transition(
                &execution,
                "reconciliation_required",
                Some("artifact_count_mismatch"),
                now,
                Some("succeeded"),
                Some("failed"),
                None,
            );
        }
        self.repository
            .complete_artifact_persistence(&execution, &attempt.provider_attempt_id, now)
            .map_err(Into::into)
    }

    pub fn release_phase(&self, execution_id: &str, now: &str) -> Result<Execution, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "executing" {
            return Ok(execution);
        }
        let attempt = self
            .repository
            .get_provider_attempt_for_execution(execution_id)?;
        if attempt.completed_at.is_some() && attempt.outcome == "failed" {
            self.release_or_reconcile(
                &execution,
                attempt.failure_code.as_deref().unwrap_or("provider_failed"),
                now,
            )?;
        }
        self.repository
            .get_execution(execution_id)
            .map_err(Into::into)
    }

    pub fn settlement_phase(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<Execution, WorkflowError> {
        let execution = self.repository.get_execution(execution_id)?;
        if execution.status != "settling" {
            return Ok(execution);
        }
        let attempt = self
            .repository
            .get_provider_attempt_for_execution(execution_id)?;
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| PersistenceError::Invalid("pricing snapshot"))?;
        let usage: NormalizedUsage = serde_json::from_value(
            attempt
                .usage
                .clone()
                .ok_or(PersistenceError::Invalid("attempt usage"))?,
        )
        .map_err(|_| PersistenceError::Invalid("attempt usage"))?;
        let settlement = match snapshot.settle_precise(
            &usage,
            attempt.actual_vendor_cost.as_ref(),
            execution.authorized_minor,
        ) {
            Ok(settlement) => settlement,
            Err(error) => {
                let failure_code = if error == ContractError::SettlementOverage {
                    "settlement_over_authorization"
                } else {
                    "invalid_provider_usage"
                };
                return self.transition(
                    &execution,
                    "reconciliation_required",
                    Some(failure_code),
                    now,
                    None,
                    None,
                    Some("ambiguous"),
                );
            }
        };
        let amount = settlement.budget_amount_minor;
        let receipt = match self.repository.get_receipt_for_execution(execution_id) {
            Ok(receipt) => receipt,
            Err(PersistenceError::NotFound) => {
                self.repository.create_receipt(&CreateReceiptParams {
                    receipt_id: format!("receipt-{execution_id}"),
                    execution_id: execution_id.into(),
                    provider_attempt_id: attempt.provider_attempt_id,
                    settlement_minor: amount,
                    currency: snapshot.currency.clone(),
                    pricing_catalog_version: snapshot.catalog_version,
                    actual_vendor_cost: settlement.actual_vendor_cost,
                    created_at: now.into(),
                    settled_at: None,
                    hubu_settlement_id: None,
                })?
            }
            Err(error) => return Err(error.into()),
        };
        if receipt.settled_at.is_some() {
            return self.transition(
                &execution,
                "succeeded",
                Some("succeeded"),
                now,
                None,
                None,
                Some("succeeded"),
            );
        }
        if receipt.transmission_started_at.is_some() {
            return self.transition(
                &execution,
                "reconciliation_required",
                Some("settlement_delivery_interrupted"),
                now,
                None,
                None,
                Some("ambiguous"),
            );
        }
        self.repository
            .begin_settlement_transmission(&receipt.receipt_id, now)?;
        match self.hubu.settle(&execution, &receipt.receipt_id, amount) {
            Ok(settlement_id) => {
                self.repository
                    .complete_receipt(&receipt.receipt_id, &settlement_id, now)?;
                self.transition(
                    &execution,
                    "succeeded",
                    Some("succeeded"),
                    now,
                    None,
                    None,
                    Some("succeeded"),
                )
            }
            Err(error) => {
                let code = activity_error_code(error);
                self.transition(
                    &execution,
                    "reconciliation_required",
                    Some(&code),
                    now,
                    None,
                    None,
                    Some("ambiguous"),
                )
            }
        }
    }

    pub fn recover(
        &self,
        execution_id: &str,
        now: &str,
        operator: Option<&OperatorReconciliationRequest>,
    ) -> Result<Execution, WorkflowError> {
        let mut execution = self.repository.get_execution(execution_id)?;
        // Reopening a rejected polling origin and the subsequent GET/artifact
        // work share one Temporal activity. If that activity is retried after
        // the durable reopen transaction, resume from the checkpointed
        // operation or downstream durable phase; never re-enter submission.
        if execution.status == "executing"
            && execution.provider_outcome == Some(LifecycleOutcome::Ambiguous)
        {
            let attempt = self
                .repository
                .get_provider_attempt_for_execution(execution_id)?;
            if attempt.operation_checkpointed_at.is_some()
                && attempt
                    .provider_recovery_context
                    .as_ref()
                    .is_some_and(|context| {
                        context.validation_reason.as_deref() == Some("host_not_allowlisted")
                    })
            {
                return self.resume_reopened_provider_poll(execution_id, now);
            }
        }
        if execution.status == "settling"
            && execution.provider_outcome == Some(LifecycleOutcome::Succeeded)
        {
            return self.settlement_phase(execution_id, now);
        }
        if execution.status != "reconciliation_required" {
            return Ok(execution);
        }
        if let Some(request) = operator {
            if request.action_id.trim().is_empty() || !request.evidence.is_object() {
                return Ok(execution);
            }
            if !self.repository.record_operator_action(
                execution_id,
                &request.action_id,
                match request.action {
                    ReconciliationAction::Reinspect => "reinspect",
                    ReconciliationAction::Settle => "settle",
                    ReconciliationAction::Release => "release",
                },
                &request.evidence,
                now,
            )? {
                return Ok(execution);
            }
        }
        let attempt = match self
            .repository
            .get_provider_attempt_for_execution(execution_id)
        {
            Ok(value) => Some(value),
            Err(PersistenceError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };
        let receipt = match self.repository.get_receipt_for_execution(execution_id) {
            Ok(value) => Some(value),
            Err(PersistenceError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };
        let requested = operator.map(|r| &r.action);

        // Ambiguous claim delivery is safely replayable because the immutable
        // account/operation/authorization identity is preserved. A recovered
        // claim is persisted before any release is attempted.
        if attempt.is_none() && execution.hubu_claim_id.is_none() {
            match self.hubu.claim(&execution) {
                Ok(claim_id) => {
                    execution = self.repository.set_reconciliation_claim(
                        execution_id,
                        execution.version,
                        &claim_id,
                        now,
                    )?;
                    self.repository.record_reconciliation(
                        &execution,
                        "claim_recovered",
                        None,
                        now,
                    )?;
                }
                Err(_) => return Ok(execution),
            }
        }

        // Reinspect is the only action that may resume provider traffic. It
        // reopens the same completed ambiguous attempt atomically, then enters
        // the poll-existing path. The generation POST is unreachable here.
        if matches!(requested, Some(ReconciliationAction::Reinspect)) {
            if let Some(attempt) = attempt.as_ref().filter(|attempt| {
                attempt.outcome == "ambiguous"
                    && attempt
                        .provider_recovery_context
                        .as_ref()
                        .is_some_and(|context| {
                            context.validation_reason.as_deref() == Some("host_not_allowlisted")
                        })
            }) {
                if self
                    .repository
                    .begin_provider_reconciliation_poll(
                        execution_id,
                        &attempt.provider_attempt_id,
                        execution.version,
                        now,
                    )
                    .is_ok()
                {
                    return self.resume_reopened_provider_poll(execution_id, now);
                }
            }
        }

        // A durable succeeded attempt, durable artifacts, frozen usage/cost, and stable
        // receipt prove that replaying the same Hubu settlement cannot create provider work.
        if !matches!(requested, Some(ReconciliationAction::Release)) {
            if let (Some(attempt), Some(receipt)) = (&attempt, &receipt) {
                if attempt.outcome == "succeeded"
                    && attempt.usage.is_some()
                    && self
                        .repository
                        .count_artifacts_for_attempt(&attempt.provider_attempt_id)?
                        > 0
                {
                    match self.hubu.settle(
                        &execution,
                        &receipt.receipt_id,
                        receipt.settlement_minor,
                    ) {
                        Ok(settlement_id) => {
                            if receipt.settled_at.is_none() {
                                self.repository.complete_receipt(
                                    &receipt.receipt_id,
                                    &settlement_id,
                                    now,
                                )?;
                            }
                            return self.transition(
                                &execution,
                                "succeeded",
                                Some("succeeded"),
                                now,
                                Some("succeeded"),
                                Some("succeeded"),
                                Some("succeeded"),
                            );
                        }
                        Err(_) => return Ok(execution),
                    }
                }
            }
        }

        // Release is proven safe only before provider transmission, or after a
        // completed proven provider failure. Ambiguous/transmitted work is held.
        let safe_release = match &attempt {
            None => execution.hubu_claim_id.is_some(),
            Some(a) => {
                a.transmission_started_at.is_none()
                    || (a.outcome == "failed" && a.completed_at.is_some())
            }
        };
        if safe_release
            && !matches!(requested, Some(ReconciliationAction::Settle))
            && self.hubu.release(&execution).is_ok()
        {
            return self.transition(
                &execution,
                "released",
                Some("recovered_release"),
                now,
                attempt.as_ref().map(|_| "failed"),
                None,
                Some("released"),
            );
        }
        Ok(execution)
    }

    fn resume_reopened_provider_poll(
        &self,
        execution_id: &str,
        now: &str,
    ) -> Result<Execution, WorkflowError> {
        let outcome = self.provider_poll_phase(execution_id, now)?;
        match outcome {
            ProviderPhaseOutcome::PersistArtifacts => {
                let persisted = self.artifact_phase(execution_id, now)?;
                if persisted.status == "settling" {
                    self.settlement_phase(execution_id, now)
                } else {
                    Ok(persisted)
                }
            }
            ProviderPhaseOutcome::ReleaseAuthorization => self.release_phase(execution_id, now),
            ProviderPhaseOutcome::Complete(_) | ProviderPhaseOutcome::PollExisting => self
                .repository
                .get_execution(execution_id)
                .map_err(Into::into),
        }
    }
    fn preflight(&self, e: &Execution) -> Result<(), ActivityError> {
        self.hubu.preflight(e)?;
        self.provider.preflight(e)?;
        self.artifacts.preflight()
    }
    fn artifacts_match_usage_count(
        &self,
        execution: &Execution,
        durable: u64,
        returned: usize,
        usage: &NormalizedUsage,
    ) -> Result<bool, WorkflowError> {
        let returned = u64::try_from(returned)
            .map_err(|_| PersistenceError::Invalid("provider artifact count"))?;
        if durable == 0 || durable != returned {
            return Ok(false);
        }
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| PersistenceError::Invalid("pricing snapshot"))?;
        if snapshot.has_unit(crate::provider_contract::PricingUnit::Image) {
            if let Some(images) = usage.images.and_then(|value| u64::try_from(value).ok()) {
                return Ok(durable == images);
            }
        }
        Ok(true)
    }
    fn fail_before_claim(
        &self,
        e: &Execution,
        error: ActivityError,
        now: &str,
    ) -> Result<(), WorkflowError> {
        let code = match error {
            ActivityError::Proven(c)
            | ActivityError::ProvenWithEvidence { code: c, .. }
            | ActivityError::Ambiguous(c) => c,
            ActivityError::AmbiguousWithEvidence { code, .. } => code,
        };
        self.transition(e, "failed", Some(&code), now, None, None, None)
            .map(|_| ())
    }
    fn release_or_reconcile(
        &self,
        e: &Execution,
        code: &str,
        now: &str,
    ) -> Result<(), WorkflowError> {
        if e.release_transmission_started_at.is_some() {
            self.transition(
                e,
                "reconciliation_required",
                Some("release_delivery_interrupted"),
                now,
                Some("failed"),
                None,
                Some("ambiguous"),
            )?;
            return Ok(());
        }
        let marked = self
            .repository
            .begin_release_transmission(&e.execution_id, e.version, now)?;
        match self.hubu.release(&marked) {
            Ok(()) => {
                self.transition(
                    &marked,
                    "released",
                    Some(code),
                    now,
                    Some("failed"),
                    None,
                    Some("released"),
                )?;
            }
            Err(ActivityError::Proven(c))
            | Err(ActivityError::ProvenWithEvidence { code: c, .. })
            | Err(ActivityError::Ambiguous(c)) => {
                self.transition(
                    &marked,
                    "reconciliation_required",
                    Some(&c),
                    now,
                    Some("failed"),
                    None,
                    Some("ambiguous"),
                )?;
            }
            Err(ActivityError::AmbiguousWithEvidence { code, .. }) => {
                self.transition(
                    &marked,
                    "reconciliation_required",
                    Some(&code),
                    now,
                    Some("failed"),
                    None,
                    Some("ambiguous"),
                )?;
            }
        }
        Ok(())
    }
    fn advance_completed_attempt(
        &self,
        e: &Execution,
        outcome: &str,
        now: &str,
    ) -> Result<(), WorkflowError> {
        match outcome {
            "succeeded" => {
                self.transition(
                    e,
                    "reconciliation_required",
                    Some("artifact_delivery_interrupted"),
                    now,
                    Some("succeeded"),
                    Some("ambiguous"),
                    None,
                )?;
            }
            "failed" => self.release_or_reconcile(e, "provider_failed", now)?,
            _ => {
                self.transition(
                    e,
                    "reconciliation_required",
                    Some("provider_ambiguous"),
                    now,
                    Some("ambiguous"),
                    None,
                    None,
                )?;
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // The three independent durable outcomes are intentionally explicit at every transition.
    fn transition(
        &self,
        e: &Execution,
        status: &str,
        outcome: Option<&str>,
        now: &str,
        provider: Option<&str>,
        artifact: Option<&str>,
        settlement: Option<&str>,
    ) -> Result<Execution, WorkflowError> {
        let updated = self.repository.update_execution(
            &e.execution_id,
            e.version,
            &ExecutionUpdate {
                status: status.into(),
                outcome: outcome.map(str::to_owned),
                started_at: Some(now.into()),
                completed_at: final_terminal(status).then(|| now.into()),
                failure_code: outcome
                    .filter(|_| matches!(status, "failed" | "reconciliation_required"))
                    .map(str::to_owned),
                failure_message_redacted: None,
                provider_outcome: provider.map(typed_outcome),
                artifact_outcome: artifact.map(typed_outcome),
                settlement_outcome: settlement.map(typed_outcome),
            },
            now,
        )?;
        if status == "reconciliation_required" {
            self.repository
                .record_reconciliation(&updated, e.status.as_str(), outcome, now)?;
        }
        if status == "failed" || (status == "released" && provider == Some("failed")) {
            crate::lifecycle::log(crate::lifecycle::LifecycleReason::ExecutionFailure);
        }
        Ok(updated)
    }
}

fn bind_poll_result_to_checkpoint(
    result: Result<ProviderSuccess, ActivityError>,
    operation: &AsyncProviderOperation,
) -> Result<ProviderSuccess, ActivityError> {
    let request_id = operation.provider_request_id.clone();
    let operation_id = Some(operation.provider_operation_id.clone());
    match result {
        Ok(mut success) => {
            let candidate = AsyncProviderOperation {
                provider_request_id: success.request_id.clone(),
                provider_operation_id: success.operation_id.clone().unwrap_or_default(),
                polling_host: operation.polling_host.clone(),
                polling_recovery: operation.polling_recovery.clone(),
                deadline_unix_ms: operation.deadline_unix_ms,
            };
            let request_matches = operation.provider_request_id.is_none()
                || success.request_id == operation.provider_request_id;
            if candidate.validate().is_err()
                || candidate.provider_operation_id != operation.provider_operation_id
                || !request_matches
            {
                return Err(ActivityError::AmbiguousWithEvidence {
                    code: "provider_operation_identity_mismatch".into(),
                    request_id,
                    operation_id,
                });
            }
            success.operation_id = Some(operation.provider_operation_id.clone());
            if operation.provider_request_id.is_some() {
                success.request_id = operation.provider_request_id.clone();
            }
            Ok(success)
        }
        Err(ActivityError::Proven(code) | ActivityError::ProvenWithEvidence { code, .. }) => {
            Err(ActivityError::ProvenWithEvidence {
                code,
                request_id,
                operation_id,
            })
        }
        Err(ActivityError::Ambiguous(code) | ActivityError::AmbiguousWithEvidence { code, .. }) => {
            Err(ActivityError::AmbiguousWithEvidence {
                code,
                request_id,
                operation_id,
            })
        }
    }
}

fn typed_outcome(value: &str) -> crate::execution::LifecycleOutcome {
    crate::execution::LifecycleOutcome::parse(value)
        .expect("workflow only emits defined lifecycle outcomes")
}
fn attempt_failure(outcome: &str, code: &str, now: &str) -> AttemptResult {
    AttemptResult {
        outcome: outcome.into(),
        completed_at: now.into(),
        usage: serde_json::json!({}),
        usage_schema_version: 1,
        actual_vendor_cost: None,
        failure_code: Some(code.into()),
        failure_message_redacted: None,
        provider_request_id: None,
        provider_operation_id: None,
    }
}
fn successful_attempt(success: &ProviderSuccess, now: &str) -> AttemptResult {
    AttemptResult {
        outcome: "succeeded".into(),
        completed_at: now.into(),
        usage: usage_value(&success.usage),
        usage_schema_version: 1,
        actual_vendor_cost: success.actual_vendor_cost.clone(),
        failure_code: None,
        failure_message_redacted: None,
        provider_request_id: success.request_id.clone(),
        provider_operation_id: success.operation_id.clone(),
    }
}
fn activity_error_code(error: ActivityError) -> String {
    match error {
        ActivityError::Proven(code)
        | ActivityError::ProvenWithEvidence { code, .. }
        | ActivityError::Ambiguous(code)
        | ActivityError::AmbiguousWithEvidence { code, .. } => code,
    }
}
fn usage_value(usage: &NormalizedUsage) -> serde_json::Value {
    let mut value = to_value(usage).expect("usage serializes");
    if let Some(map) = value.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    value
}
fn terminal(s: &str) -> bool {
    matches!(
        s,
        "succeeded" | "released" | "failed" | "reconciliation_required"
    )
}
fn final_terminal(s: &str) -> bool {
    matches!(s, "succeeded" | "released" | "failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        execution::{CreateArtifactParams, CreateExecutionParams, HubuTokenReference},
        redaction::Redactor,
    };
    use serde_json::json;
    use std::cell::{Cell, RefCell};

    fn execution(repo: &Repository, key: &str) -> Execution {
        execution_with_quantity(repo, key, 1)
    }

    fn bfl_cost(raw_credits: &str) -> ActualVendorCost {
        crate::provider::flux2_api::bfl_credit_cost_to_usd(raw_credits).unwrap()
    }

    fn execution_with_quantity(repo: &Repository, key: &str, quantity: i64) -> Execution {
        repo.create_execution(&CreateExecutionParams { account_id:"account".into(),operation_key:key.into(),hubu_authorization_id:"token-ref".into(),hubu_claim_id:None,hubu_token_reference:HubuTokenReference::new("token-ref").unwrap(),authorized_minor:500,authorization_currency:"USD".into(),normalized_input:json!({"prompt":"cat"}),input_hash:"hash".into(),input_schema_version:1,target:"example/image-v1".into(),config_version:"cfg-1".into(),workload_type:"image_generation".into(),provider:"example".into(),adapter:"fixture".into(),model:"image-v1".into(),provider_config_version:"pcv-1".into(),provider_config_digest:format!("sha256:{}","a".repeat(64)),pricing_snapshot:json!({"schema_version":2,"provider":"example","model":"image-v1","catalog_version":"prices-v2","catalog_digest":format!("sha256:{}","a".repeat(64)),"pricing_rule_id":"image","components":[{"unit":"image","rate_numerator_minor":100,"rate_denominator":1,"quantity":quantity}],"exact_estimate_numerator":(100 * quantity).to_string(),"exact_estimate_denominator":"1","estimated_amount_minor":100 * quantity,"currency":"USD"}),pricing_schema_version:2,execution_scope:None,created_at:"2026-08-05T00:00:00Z".into() }).unwrap()
    }
    struct Hubu {
        claims: Cell<u32>,
        settles: Cell<u32>,
        settle_amounts: RefCell<Vec<i64>>,
        releases: Cell<u32>,
        panic_on_settle: Cell<bool>,
        panic_on_release: Cell<bool>,
        claim_error: RefCell<Option<ActivityError>>,
        claim_validation_error: RefCell<Option<ActivityError>>,
        settle_error: RefCell<Option<ActivityError>>,
    }
    impl Default for Hubu {
        fn default() -> Self {
            Self {
                claims: Cell::new(0),
                settles: Cell::new(0),
                settle_amounts: RefCell::new(Vec::new()),
                releases: Cell::new(0),
                panic_on_settle: Cell::new(false),
                panic_on_release: Cell::new(false),
                claim_error: RefCell::new(None),
                claim_validation_error: RefCell::new(None),
                settle_error: RefCell::new(None),
            }
        }
    }
    impl HubuActivities for Hubu {
        fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
            Ok(())
        }
        fn claim(&self, _: &Execution) -> Result<String, ActivityError> {
            self.claims.set(self.claims.get() + 1);
            if let Some(e) = self.claim_error.borrow_mut().take() {
                Err(e)
            } else {
                Ok("claim-1".into())
            }
        }
        fn validate_claim(&self, _: &Execution) -> Result<(), ActivityError> {
            if let Some(error) = self.claim_validation_error.borrow_mut().take() {
                Err(error)
            } else {
                Ok(())
            }
        }
        fn settle(&self, _: &Execution, _: &str, amount: i64) -> Result<String, ActivityError> {
            self.settles.set(self.settles.get() + 1);
            self.settle_amounts.borrow_mut().push(amount);
            assert!(
                !self.panic_on_settle.replace(false),
                "simulated worker loss"
            );
            if let Some(e) = self.settle_error.borrow_mut().take() {
                Err(e)
            } else {
                Ok("settlement-1".into())
            }
        }
        fn release(&self, _: &Execution) -> Result<(), ActivityError> {
            self.releases.set(self.releases.get() + 1);
            assert!(
                !self.panic_on_release.replace(false),
                "simulated worker loss"
            );
            Ok(())
        }
    }
    struct Provider {
        calls: Cell<u32>,
        error: RefCell<Option<ActivityError>>,
        empty_artifacts: Cell<bool>,
        image_usage: Cell<i64>,
        artifact_count: Cell<usize>,
        actual_vendor_cost: RefCell<ActualVendorCost>,
    }
    impl Default for Provider {
        fn default() -> Self {
            Self {
                calls: Cell::new(0),
                error: RefCell::new(None),
                empty_artifacts: Cell::new(false),
                image_usage: Cell::new(1),
                artifact_count: Cell::new(1),
                actual_vendor_cost: RefCell::new(ActualVendorCost::new(100, 2, "USD").unwrap()),
            }
        }
    }
    impl ProviderActivities for Provider {
        fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
            Ok(())
        }
        fn invoke(
            &self,
            _: &Execution,
            attempt_id: &str,
        ) -> Result<ProviderSuccess, ActivityError> {
            self.calls.set(self.calls.get() + 1);
            if let Some(e) = self.error.borrow_mut().take() {
                Err(e)
            } else {
                Ok(ProviderSuccess {
                    request_id: Some(format!("provider-{attempt_id}")),
                    operation_id: None,
                    usage: NormalizedUsage {
                        images: Some(self.image_usage.get()),
                        ..Default::default()
                    },
                    actual_vendor_cost: Some(self.actual_vendor_cost.borrow().clone()),
                    artifacts: if self.empty_artifacts.get() {
                        vec![]
                    } else {
                        (0..self.artifact_count.get())
                            .map(|_| ProviderArtifact {
                                media_type: "image/png".into(),
                                bytes: vec![1],
                            })
                            .collect()
                    },
                })
            }
        }
    }
    struct AsyncProvider {
        submits: Cell<u32>,
        polls: Cell<u32>,
        submit_error: RefCell<Option<ActivityError>>,
        poll_error: RefCell<Option<ActivityError>>,
        operation: AsyncProviderOperation,
        last_polled_operation: RefCell<Option<AsyncProviderOperation>>,
    }
    impl Default for AsyncProvider {
        fn default() -> Self {
            Self {
                submits: Cell::new(0),
                polls: Cell::new(0),
                submit_error: RefCell::new(None),
                poll_error: RefCell::new(None),
                operation: AsyncProviderOperation {
                    provider_request_id: Some("request-170".into()),
                    provider_operation_id: "operation-170".into(),
                    polling_host: "api.bfl.ai".into(),
                    polling_recovery: None,
                    deadline_unix_ms: 1_799_999_999_000,
                },
                last_polled_operation: RefCell::new(None),
            }
        }
    }
    impl ProviderActivities for AsyncProvider {
        fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
            Ok(())
        }

        fn invoke(&self, _: &Execution, _: &str) -> Result<ProviderSuccess, ActivityError> {
            unreachable!("the asynchronous test provider uses submit and poll_existing")
        }

        fn submit(&self, _: &Execution, _: &str) -> Result<ProviderSubmission, ActivityError> {
            self.submits.set(self.submits.get() + 1);
            if let Some(error) = self.submit_error.borrow_mut().take() {
                return Err(error);
            }
            Ok(ProviderSubmission::Pending(self.operation.clone()))
        }

        fn poll_existing(
            &self,
            _: &Execution,
            _: &str,
            operation: &AsyncProviderOperation,
        ) -> Result<ProviderSuccess, ActivityError> {
            self.polls.set(self.polls.get() + 1);
            self.last_polled_operation.replace(Some(operation.clone()));
            if let Some(error) = self.poll_error.borrow_mut().take() {
                return Err(error);
            }
            Ok(ProviderSuccess {
                request_id: operation.provider_request_id.clone(),
                operation_id: Some(operation.provider_operation_id.clone()),
                usage: NormalizedUsage {
                    images: Some(1),
                    ..Default::default()
                },
                actual_vendor_cost: Some(ActualVendorCost::new(100, 2, "USD").unwrap()),
                artifacts: vec![ProviderArtifact {
                    media_type: "image/png".into(),
                    bytes: vec![1],
                }],
            })
        }
    }
    struct Artifacts<'a> {
        repo: &'a Repository,
        calls: Cell<u32>,
    }
    impl ArtifactActivities for Artifacts<'_> {
        fn preflight(&self) -> Result<(), ActivityError> {
            Ok(())
        }
        fn persist(
            &self,
            e: &Execution,
            a: &str,
            _: &[ProviderArtifact],
        ) -> Result<(), ActivityError> {
            self.calls.set(self.calls.get() + 1);
            if self
                .repo
                .count_artifacts_for_execution(&e.execution_id)
                .unwrap()
                == 0
            {
                self.repo
                    .create_artifact(&CreateArtifactParams {
                        artifact_id: format!("artifact-{a}"),
                        execution_id: e.execution_id.clone(),
                        provider_attempt_id: Some(a.into()),
                        kind: "image".into(),
                        storage_backend: "local_fs".into(),
                        media_type: "image/png".into(),
                        storage_key: format!("executions/{}/artifact-{a}.png", e.execution_id),
                        size_bytes: 1,
                        sha256: "a".repeat(64),
                        metadata: json!({}),
                        metadata_schema_version: 1,
                        created_at: "2026-08-05T00:00:01Z".into(),
                    })
                    .unwrap();
            }
            Ok(())
        }
    }
    #[test]
    fn happy_path_is_durable_and_duplicate_delivery_is_a_noop() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "happy");
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        let done = w.run(&e.execution_id, "2026-08-05T00:00:01Z").unwrap();
        assert_eq!(done.status, "succeeded");
        assert_eq!(
            repo.count_artifacts_for_execution(&e.execution_id).unwrap(),
            1
        );
        assert!(repo
            .get_receipt_for_execution(&e.execution_id)
            .unwrap()
            .settled_at
            .is_some());
        let attempt = repo
            .get_provider_attempt_for_execution(&e.execution_id)
            .unwrap();
        assert_eq!(
            attempt.actual_vendor_cost,
            Some(ActualVendorCost::new(100, 2, "USD").unwrap())
        );
        assert_eq!(
            repo.get_receipt_for_execution(&e.execution_id)
                .unwrap()
                .settlement_minor,
            100
        );
        w.run(&e.execution_id, "later").unwrap();
        assert_eq!(
            (
                h.claims.get(),
                p.calls.get(),
                a.calls.get(),
                h.settles.get()
            ),
            (1, 1, 1, 1)
        );
    }

    #[test]
    fn bfl_credit_cost_settles_once_and_overage_replays_in_reconciliation() {
        let repo = Repository::in_memory().unwrap();
        let hubu = Hubu::default();
        let provider = Provider::default();

        provider.actual_vendor_cost.replace(bfl_cost("1.0001"));
        let normal = execution(&repo, "bfl-credit-normal");
        let normal_artifacts = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let normal_workflow = ExecutionWorkflow {
            repository: &repo,
            hubu: &hubu,
            provider: &provider,
            artifacts: &normal_artifacts,
        };
        assert_eq!(
            normal_workflow
                .run(&normal.execution_id, "normal")
                .unwrap()
                .status,
            "succeeded"
        );
        let receipt = repo
            .get_receipt_for_execution(&normal.execution_id)
            .unwrap();
        assert_eq!(receipt.actual_vendor_cost, bfl_cost("1.0001"));
        assert_eq!(receipt.settlement_minor, 2);
        normal_workflow
            .run(&normal.execution_id, "normal-replay")
            .unwrap();
        assert_eq!(hubu.settle_amounts.borrow().as_slice(), &[2]);
        assert_eq!(provider.calls.get(), 1);

        provider.actual_vendor_cost.replace(bfl_cost("100.0001"));
        let overage = execution(&repo, "bfl-credit-overage");
        let overage_artifacts = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let overage_workflow = ExecutionWorkflow {
            repository: &repo,
            hubu: &hubu,
            provider: &provider,
            artifacts: &overage_artifacts,
        };
        assert_eq!(
            overage_workflow
                .run(&overage.execution_id, "overage")
                .unwrap()
                .status,
            "reconciliation_required"
        );
        assert_eq!(
            repo.get_provider_attempt_for_execution(&overage.execution_id)
                .unwrap()
                .actual_vendor_cost,
            Some(bfl_cost("100.0001"))
        );
        assert_eq!(
            repo.get_reconciliation(&overage.execution_id)
                .unwrap()
                .evidence["actual_vendor_cost"],
            json!({"amount":1000001,"scale":6,"currency":"USD"})
        );
        overage_workflow
            .run(&overage.execution_id, "overage-replay")
            .unwrap();
        assert_eq!(hubu.settle_amounts.borrow().as_slice(), &[2]);
        assert_eq!(provider.calls.get(), 2);
    }

    #[test]
    fn repeated_hubu_rejections_do_not_poison_subsequent_execution() {
        let repo = Repository::in_memory().unwrap();
        let hubu = Hubu::default();
        let provider = Provider::default();
        let artifacts = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &repo,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };

        let rejection_codes = [
            "hubu_request_rejected",
            "hubu_scope_mismatch",
            "hubu_authorization_expired",
            "hubu_provider_rejected",
            "hubu_other_proven_failure",
        ];
        for index in 0..64 {
            let execution = execution(&repo, &format!("rejected-{index}"));
            let code = rejection_codes[index % rejection_codes.len()];
            hubu.claim_error
                .replace(Some(ActivityError::Proven(code.into())));
            let terminal = workflow
                .run(&execution.execution_id, "2026-08-05T00:00:01Z")
                .unwrap();
            assert_eq!(terminal.status, "failed");
            assert_eq!(terminal.failure_code.as_deref(), Some(code));
        }

        let valid = execution(&repo, "valid-after-rejections");
        let completed = workflow
            .run(&valid.execution_id, "2026-08-05T00:00:02Z")
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        assert_eq!(hubu.claims.get(), 65);
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(artifacts.calls.get(), 1);
        assert_eq!(hubu.settles.get(), 1);
        assert_eq!(hubu.releases.get(), 0);
        assert!(repo.list_nonterminal_execution_ids().unwrap().is_empty());
    }

    #[test]
    fn granular_phase_redelivery_never_duplicates_side_effects() {
        let repo = Repository::in_memory().unwrap();
        let execution = execution(&repo, "granular-redelivery");
        let hubu = Hubu::default();
        let provider = Provider::default();
        let artifacts = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &repo,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };

        assert_eq!(
            workflow
                .preflight_phase(&execution.execution_id, "preflight")
                .unwrap()
                .status,
            "preflighting"
        );
        workflow
            .preflight_phase(&execution.execution_id, "preflight-redelivery")
            .unwrap();
        assert_eq!(
            workflow
                .claim_phase(&execution.execution_id, "claim")
                .unwrap()
                .status,
            "claimed"
        );
        workflow
            .claim_phase(&execution.execution_id, "claim-response-lost")
            .unwrap();
        assert_eq!(hubu.claims.get(), 1);

        workflow
            .validate_claim_phase(&execution.execution_id, "validate")
            .unwrap();
        workflow
            .validate_claim_phase(&execution.execution_id, "validate-redelivery")
            .unwrap();
        assert_eq!(
            workflow
                .provider_phase(&execution.execution_id, "provider")
                .unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        assert_eq!(
            workflow
                .provider_phase(&execution.execution_id, "provider-response-lost")
                .unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        assert_eq!(provider.calls.get(), 1);

        assert_eq!(
            workflow
                .artifact_phase(&execution.execution_id, "artifacts")
                .unwrap()
                .status,
            "settling"
        );
        workflow
            .artifact_phase(&execution.execution_id, "artifacts-response-lost")
            .unwrap();
        assert_eq!(artifacts.calls.get(), 1);
        assert!(repo
            .get_staged_provider_artifacts(
                &repo
                    .get_provider_attempt_for_execution(&execution.execution_id)
                    .unwrap()
                    .provider_attempt_id
            )
            .unwrap()
            .is_empty());

        assert_eq!(
            workflow
                .settlement_phase(&execution.execution_id, "settle")
                .unwrap()
                .status,
            "succeeded"
        );
        workflow
            .settlement_phase(&execution.execution_id, "settle-response-lost")
            .unwrap();
        assert_eq!(hubu.settles.get(), 1);
    }

    #[test]
    fn provider_phase_records_distinct_transmission_boundaries_without_replay() {
        let repo = Repository::in_memory().unwrap();
        let execution = execution(&repo, "provider-timing");
        let hubu = Hubu::default();
        let provider = Provider::default();
        let artifacts = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &repo,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };
        workflow
            .preflight_phase(&execution.execution_id, "2026-08-05T00:00:00Z")
            .unwrap();
        workflow
            .claim_phase(&execution.execution_id, "2026-08-05T00:00:00.500Z")
            .unwrap();
        workflow
            .validate_claim_phase(&execution.execution_id, "2026-08-05T00:00:01Z")
            .unwrap();

        let clock_index = Cell::new(0);
        let provider_clock = || {
            let timestamps = ["2026-08-05T00:00:02Z", "2026-08-05T00:00:05.500Z"];
            let index = clock_index.get();
            clock_index.set(index + 1);
            timestamps[index].to_owned()
        };
        assert_eq!(
            workflow
                .provider_phase_with_clock(
                    &execution.execution_id,
                    "2026-08-05T00:00:01Z",
                    &provider_clock,
                )
                .unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        let attempt = repo
            .get_provider_attempt_for_execution(&execution.execution_id)
            .unwrap();
        assert_eq!(
            attempt.transmission_started_at.as_deref(),
            Some("2026-08-05T00:00:02Z")
        );
        assert_eq!(
            attempt.completed_at.as_deref(),
            Some("2026-08-05T00:00:05.500Z")
        );
        assert_eq!(provider.calls.get(), 1);

        // A redelivered activity observes the durable completion and neither
        // consults the provider clock nor invokes the provider again.
        assert_eq!(
            workflow
                .provider_phase_with_clock(
                    &execution.execution_id,
                    "2026-08-05T00:00:06Z",
                    &|| panic!("completed provider activity must not sample transmission time"),
                )
                .unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        assert_eq!(provider.calls.get(), 1);
    }

    #[test]
    fn granular_release_redelivery_never_releases_twice() {
        let repo = Repository::in_memory().unwrap();
        let execution = execution(&repo, "granular-release-redelivery");
        let hubu = Hubu::default();
        let provider = Provider::default();
        provider
            .error
            .replace(Some(ActivityError::Proven("provider_rejected".into())));
        let artifacts = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &repo,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };

        workflow
            .preflight_phase(&execution.execution_id, "preflight")
            .unwrap();
        workflow
            .claim_phase(&execution.execution_id, "claim")
            .unwrap();
        workflow
            .validate_claim_phase(&execution.execution_id, "validate")
            .unwrap();
        assert_eq!(
            workflow
                .provider_phase(&execution.execution_id, "provider")
                .unwrap(),
            ProviderPhaseOutcome::ReleaseAuthorization
        );
        assert_eq!(
            workflow
                .release_phase(&execution.execution_id, "release")
                .unwrap()
                .status,
            "released"
        );
        workflow
            .release_phase(&execution.execution_id, "release-response-lost")
            .unwrap();
        assert_eq!(hubu.releases.get(), 1);
    }

    #[test]
    fn granular_provider_result_survives_worker_and_repository_restart() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("execution.sqlite3");
        let hubu = Hubu::default();
        let provider = Provider::default();
        provider.actual_vendor_cost.replace(bfl_cost("1.0001"));
        let execution_id;
        {
            let repository = Repository::open(&database, Redactor::default()).unwrap();
            let created = execution(&repository, "granular-worker-restart");
            execution_id = created.execution_id;
            let artifacts = Artifacts {
                repo: &repository,
                calls: Cell::new(0),
            };
            let workflow = ExecutionWorkflow {
                repository: &repository,
                hubu: &hubu,
                provider: &provider,
                artifacts: &artifacts,
            };
            workflow
                .preflight_phase(&execution_id, "preflight")
                .unwrap();
            workflow.claim_phase(&execution_id, "claim").unwrap();
            workflow
                .validate_claim_phase(&execution_id, "validate")
                .unwrap();
            assert_eq!(
                workflow.provider_phase(&execution_id, "provider").unwrap(),
                ProviderPhaseOutcome::PersistArtifacts
            );
        }

        let repository = Repository::open(&database, Redactor::default()).unwrap();
        assert_eq!(
            repository
                .get_provider_attempt_for_execution(&execution_id)
                .unwrap()
                .actual_vendor_cost,
            Some(bfl_cost("1.0001"))
        );
        let artifacts = Artifacts {
            repo: &repository,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &repository,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };
        assert_eq!(
            workflow
                .provider_phase(&execution_id, "provider-redelivery")
                .unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(
            workflow
                .artifact_phase(&execution_id, "artifacts-after-restart")
                .unwrap()
                .status,
            "settling"
        );
        assert_eq!(artifacts.calls.get(), 1);
        assert_eq!(
            workflow
                .settlement_phase(&execution_id, "settle-after-restart")
                .unwrap()
                .status,
            "succeeded"
        );
        assert_eq!(hubu.settles.get(), 1);
        let receipt = repository.get_receipt_for_execution(&execution_id).unwrap();
        assert_eq!(receipt.actual_vendor_cost, bfl_cost("1.0001"));
        assert_eq!(receipt.settlement_minor, 2);
        workflow.run(&execution_id, "terminal-replay").unwrap();
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(hubu.settle_amounts.borrow().as_slice(), &[2]);
    }
    #[test]
    fn supplied_claim_is_adopted_without_claiming_again() {
        let repo = Repository::in_memory().unwrap();
        let params = CreateExecutionParams {
            account_id: "account".into(),
            operation_key: "supplied-claim".into(),
            hubu_authorization_id: "token-ref".into(),
            hubu_claim_id: Some("existing-claim".into()),
            hubu_token_reference: HubuTokenReference::new("token-ref").unwrap(),
            authorized_minor: 500,
            authorization_currency: "USD".into(),
            normalized_input: json!({"prompt":"cat"}),
            input_hash: "hash".into(),
            input_schema_version: 1,
            target: "example/image-v1".into(),
            config_version: "cfg-1".into(),
            workload_type: "image_generation".into(),
            provider: "example".into(),
            adapter: "fixture".into(),
            model: "image-v1".into(),
            provider_config_version: "pcv-1".into(),
            provider_config_digest: format!("sha256:{}", "a".repeat(64)),
            pricing_snapshot: json!({"schema_version":2,"provider":"example","model":"image-v1","catalog_version":"prices-v2","catalog_digest":format!("sha256:{}","a".repeat(64)),"pricing_rule_id":"image","components":[{"unit":"image","rate_numerator_minor":100,"rate_denominator":1,"quantity":1}],"exact_estimate_numerator":"100","exact_estimate_denominator":"1","estimated_amount_minor":100,"currency":"USD"}),
            pricing_schema_version: 2,
            execution_scope: None,
            created_at: "now".into(),
        };
        let e = repo.create_execution(&params).unwrap();
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert_eq!(w.run(&e.execution_id, "later").unwrap().status, "succeeded");
        assert_eq!(h.claims.get(), 0);
    }

    #[test]
    fn invalid_provider_usage_reconciles_without_settlement_retry() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "invalid-provider-usage");
        let h = Hubu::default();
        let p = Provider::default();
        p.image_usage.set(-1);
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };

        let done = w.run(&e.execution_id, "now").unwrap();
        assert_eq!(done.status, "reconciliation_required");
        assert_eq!(done.failure_code.as_deref(), Some("invalid_provider_usage"));
        assert_eq!(h.settles.get(), 0);
        assert!(matches!(
            repo.get_receipt_for_execution(&e.execution_id),
            Err(PersistenceError::NotFound)
        ));

        w.run(&e.execution_id, "later").unwrap();
        assert_eq!(p.calls.get(), 1);
        assert_eq!(h.settles.get(), 0);
    }

    #[test]
    fn invalid_or_expired_claim_reconciles_before_provider_attempt() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "expired-claim");
        let h = Hubu::default();
        h.claim_validation_error
            .replace(Some(ActivityError::Proven("claim_expired".into())));
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };

        let done = w.run(&e.execution_id, "now").unwrap();
        assert_eq!(done.status, "reconciliation_required");
        assert_eq!(done.failure_code.as_deref(), Some("claim_expired"));
        assert_eq!(p.calls.get(), 0);
        assert!(matches!(
            repo.get_provider_attempt_for_execution(&e.execution_id),
            Err(PersistenceError::NotFound)
        ));
    }

    #[test]
    fn partial_artifact_publication_reconciles_without_settlement() {
        let repo = Repository::in_memory().unwrap();
        let e = execution_with_quantity(&repo, "partial-artifacts", 2);
        let h = Hubu::default();
        let p = Provider::default();
        p.image_usage.set(2);
        p.artifact_count.set(2);
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };

        let done = w.run(&e.execution_id, "now").unwrap();
        assert_eq!(done.status, "reconciliation_required");
        assert_eq!(
            done.failure_code.as_deref(),
            Some("artifact_count_mismatch")
        );
        assert_eq!(
            repo.count_artifacts_for_attempt(
                &repo
                    .get_provider_attempt_for_execution(&e.execution_id)
                    .unwrap()
                    .provider_attempt_id
            )
            .unwrap(),
            1
        );
        assert_eq!(h.settles.get(), 0);
    }

    #[test]
    fn interrupted_settlement_reconciles_without_second_settle() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "settlement-interrupted");
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        h.panic_on_settle.set(true);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            w.run(&e.execution_id, "now")
        }))
        .is_err());
        assert!(repo
            .get_receipt_for_execution(&e.execution_id)
            .unwrap()
            .transmission_started_at
            .is_some());
        assert_eq!(
            w.run(&e.execution_id, "later").unwrap().status,
            "reconciliation_required"
        );
        assert_eq!(h.settles.get(), 1);
    }

    #[test]
    fn bfl_credit_cost_reconciled_completion_reuses_same_receipt_amount() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "recover-settle");
        let h = Hubu::default();
        let p = Provider::default();
        p.actual_vendor_cost.replace(bfl_cost("1.0001"));
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        h.panic_on_settle.set(true);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            w.run(&e.execution_id, "now")
        }));
        assert_eq!(
            w.run(&e.execution_id, "restart").unwrap().status,
            "reconciliation_required"
        );
        let interrupted_receipt = repo.get_receipt_for_execution(&e.execution_id).unwrap();
        let receipt_id = interrupted_receipt.receipt_id.clone();
        assert_eq!(interrupted_receipt.actual_vendor_cost, bfl_cost("1.0001"));
        assert_eq!(interrupted_receipt.settlement_minor, 2);
        let done = w.recover(&e.execution_id, "timer", None).unwrap();
        assert_eq!(done.status, "succeeded");
        assert_eq!(h.settles.get(), 2);
        assert_eq!(
            repo.get_receipt_for_execution(&e.execution_id)
                .unwrap()
                .receipt_id,
            receipt_id
        );
        assert_eq!(p.calls.get(), 1);
        assert_eq!(h.settle_amounts.borrow().as_slice(), &[2, 2]);
    }

    #[test]
    fn transmitted_ambiguity_remains_signalable_without_provider_or_release() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "hold-ambiguous");
        let h = Hubu::default();
        let p = Provider::default();
        p.error.replace(Some(ActivityError::AmbiguousWithEvidence {
            code: "timeout".into(),
            request_id: Some("req-1".into()),
            operation_id: Some("op-1".into()),
        }));
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert_eq!(
            w.run(&e.execution_id, "now").unwrap().status,
            "reconciliation_required"
        );
        for n in 0..3 {
            assert_eq!(
                w.recover(&e.execution_id, &format!("timer-{n}"), None)
                    .unwrap()
                    .status,
                "reconciliation_required"
            );
        }
        let request = OperatorReconciliationRequest {
            action_id: "action-1".into(),
            action: ReconciliationAction::Release,
            evidence: json!({"provider_outcome":"unknown"}),
        };
        w.recover(&e.execution_id, "operator", Some(&request))
            .unwrap();
        let second = OperatorReconciliationRequest {
            action_id: "action-2".into(),
            action: ReconciliationAction::Reinspect,
            evidence: json!({"provider_outcome":"unknown"}),
        };
        w.recover(&e.execution_id, "operator-second", Some(&second))
            .unwrap();
        w.recover(&e.execution_id, "operator-duplicate", Some(&request))
            .unwrap();
        assert_eq!(p.calls.get(), 1);
        assert_eq!(h.releases.get(), 0);
        assert_eq!(
            repo.get_reconciliation(&e.execution_id)
                .unwrap()
                .last_operator_action_id
                .as_deref(),
            Some("action-2")
        );
        let evidence = repo.get_reconciliation(&e.execution_id).unwrap().evidence;
        assert_eq!(evidence["provider_request_id"], "req-1");
        assert_eq!(evidence["provider_operation_id"], "op-1");
        assert!(evidence["pricing_snapshot"].is_object());
        assert!(evidence["authorization"].is_object());
    }

    #[test]
    fn pre_transmission_crash_releases_without_provider_work() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "before-send");
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        let pre = repo
            .update_execution(
                &e.execution_id,
                e.version,
                &ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: Some("now".into()),
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "now",
            )
            .unwrap();
        let claimed = repo
            .set_claim(&e.execution_id, pre.version, "claim-1", "now")
            .unwrap();
        repo.start_provider_attempt(&claimed, "now").unwrap();
        let executing = repo.get_execution(&e.execution_id).unwrap();
        let held = repo
            .update_execution(
                &e.execution_id,
                executing.version,
                &ExecutionUpdate {
                    status: "reconciliation_required".into(),
                    outcome: Some("worker_lost_before_send".into()),
                    started_at: None,
                    completed_at: None,
                    failure_code: Some("worker_lost_before_send".into()),
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "later",
            )
            .unwrap();
        repo.record_reconciliation(&held, "executing", Some("worker_lost_before_send"), "later")
            .unwrap();
        assert_eq!(
            w.recover(&e.execution_id, "timer", None).unwrap().status,
            "released"
        );
        assert_eq!(p.calls.get(), 0);
        assert_eq!(h.releases.get(), 1);
    }

    #[test]
    fn replay_repairs_missing_reconciliation_evidence() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "repair-evidence");
        let pre = repo
            .update_execution(
                &e.execution_id,
                e.version,
                &ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: Some("now".into()),
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "now",
            )
            .unwrap();
        repo.update_execution(
            &e.execution_id,
            pre.version,
            &ExecutionUpdate {
                status: "reconciliation_required".into(),
                outcome: Some("ambiguous_claim".into()),
                started_at: None,
                completed_at: None,
                failure_code: Some("ambiguous_claim".into()),
                failure_message_redacted: None,
                provider_outcome: None,
                artifact_outcome: None,
                settlement_outcome: None,
            },
            "crash",
        )
        .unwrap();
        assert!(matches!(
            repo.get_reconciliation(&e.execution_id),
            Err(PersistenceError::NotFound)
        ));
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        }
        .run(&e.execution_id, "replay")
        .unwrap();
        assert_eq!(
            repo.get_reconciliation(&e.execution_id)
                .unwrap()
                .last_confirmed_step,
            "reconciliation_replay"
        );
    }

    #[test]
    fn received_but_unfinished_operator_action_is_retried() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "retry-operator");
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        h.claim_validation_error
            .replace(Some(ActivityError::Proven("claim_expired".into())));
        assert_eq!(
            w.run(&e.execution_id, "now").unwrap().status,
            "reconciliation_required"
        );
        let request = OperatorReconciliationRequest {
            action_id: "received-before-crash".into(),
            action: ReconciliationAction::Release,
            evidence: json!({"claim":"proven"}),
        };
        repo.record_operator_action(
            &e.execution_id,
            &request.action_id,
            "release",
            &request.evidence,
            "received",
        )
        .unwrap();
        assert_eq!(
            w.recover(&e.execution_id, "redelivery", Some(&request))
                .unwrap()
                .status,
            "released"
        );
        assert_eq!(p.calls.get(), 0);
        assert_eq!(h.releases.get(), 1);
    }

    #[test]
    fn conflicting_operator_action_identity_does_not_block_later_signals() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "operator-conflict");
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        h.claim_validation_error
            .replace(Some(ActivityError::Proven("claim_expired".into())));
        assert_eq!(
            w.run(&e.execution_id, "now").unwrap().status,
            "reconciliation_required"
        );
        repo.record_operator_action(
            &e.execution_id,
            "reused",
            "release",
            &json!({"proof":"original"}),
            "received",
        )
        .unwrap();
        let conflict = OperatorReconciliationRequest {
            action_id: "reused".into(),
            action: ReconciliationAction::Settle,
            evidence: json!({"proof":"different"}),
        };
        assert_eq!(
            w.recover(&e.execution_id, "conflict", Some(&conflict))
                .unwrap()
                .status,
            "reconciliation_required"
        );
        let valid = OperatorReconciliationRequest {
            action_id: "fresh".into(),
            action: ReconciliationAction::Release,
            evidence: json!({"proof":"original"}),
        };
        assert_eq!(
            w.recover(&e.execution_id, "next-signal", Some(&valid))
                .unwrap()
                .status,
            "released"
        );
        assert_eq!(p.calls.get(), 0);
        assert_eq!(h.releases.get(), 1);
    }
    #[test]
    fn empty_provider_artifacts_reconcile_without_settlement() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "empty-artifacts");
        let h = Hubu::default();
        let p = Provider::default();
        p.empty_artifacts.set(true);
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        let done = w.run(&e.execution_id, "now").unwrap();
        assert_eq!(done.status, "reconciliation_required");
        assert_eq!(
            done.artifact_outcome,
            Some(crate::execution::LifecycleOutcome::Failed)
        );
        assert_eq!(h.settles.get(), 0);
    }

    #[test]
    fn interrupted_release_reconciles_without_second_release() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "release-interrupted");
        let h = Hubu::default();
        h.panic_on_release.set(true);
        let p = Provider::default();
        p.error
            .replace(Some(ActivityError::Proven("declined".into())));
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || w.run(&e.execution_id, "now")
        ))
        .is_err());
        assert!(repo
            .get_execution(&e.execution_id)
            .unwrap()
            .release_transmission_started_at
            .is_some());
        assert_eq!(
            w.run(&e.execution_id, "later").unwrap().status,
            "reconciliation_required"
        );
        assert_eq!(h.releases.get(), 1);
    }
    #[test]
    fn ambiguous_provider_stops_without_retry_or_release() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "ambiguous");
        let h = Hubu::default();
        let p = Provider::default();
        p.error
            .replace(Some(ActivityError::Ambiguous("timeout".into())));
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert_eq!(
            w.run(&e.execution_id, "now").unwrap().status,
            "reconciliation_required"
        );
        w.run(&e.execution_id, "later").unwrap();
        assert_eq!(p.calls.get(), 1);
        assert_eq!(h.releases.get(), 0);
    }
    #[test]
    fn ambiguous_provider_evidence_is_persisted_for_reconciliation() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "ambiguous-evidence");
        let h = Hubu::default();
        let p = Provider::default();
        p.error.replace(Some(ActivityError::AmbiguousWithEvidence {
            code: "artifact_policy_failure".into(),
            request_id: Some("request-123".into()),
            operation_id: Some("operation-456".into()),
        }));
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert_eq!(
            w.run(&e.execution_id, "now").unwrap().status,
            "reconciliation_required"
        );
        let attempt = repo
            .get_provider_attempt_for_execution(&e.execution_id)
            .unwrap();
        assert_eq!(attempt.provider_request_id.as_deref(), Some("request-123"));
        assert_eq!(
            attempt.provider_operation_id.as_deref(),
            Some("operation-456")
        );
        assert_eq!(attempt.outcome, "ambiguous");
        assert_eq!(p.calls.get(), 1);
        assert_eq!(h.releases.get(), 0);
    }

    #[test]
    fn proven_provider_evidence_is_persisted_before_release() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "proven-evidence");
        let h = Hubu::default();
        let p = Provider::default();
        p.error.replace(Some(ActivityError::ProvenWithEvidence {
            code: "provider_rejected".into(),
            request_id: Some("request-rejected".into()),
            operation_id: Some("operation-rejected".into()),
        }));
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert_eq!(w.run(&e.execution_id, "now").unwrap().status, "released");
        let attempt = repo
            .get_provider_attempt_for_execution(&e.execution_id)
            .unwrap();
        assert_eq!(attempt.outcome, "failed");
        assert_eq!(
            attempt.provider_request_id.as_deref(),
            Some("request-rejected")
        );
        assert_eq!(
            attempt.provider_operation_id.as_deref(),
            Some("operation-rejected")
        );
        assert_eq!(h.releases.get(), 1);
    }
    #[test]
    fn interrupted_transmission_reconciles_without_second_invoke_or_attempt() {
        let repo = Repository::in_memory().unwrap();
        let pending = execution(&repo, "interrupted");
        let preflight = repo
            .update_execution(
                &pending.execution_id,
                pending.version,
                &ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: Some("now".into()),
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "now",
            )
            .unwrap();
        let claimed = repo
            .set_claim(&preflight.execution_id, preflight.version, "claim-1", "now")
            .unwrap();
        let attempt = repo.start_provider_attempt(&claimed, "now").unwrap();
        assert!(matches!(
            repo.start_provider_attempt(&claimed, "now"),
            Err(PersistenceError::Stale)
        ));
        repo.begin_provider_transmission(&attempt.provider_attempt_id, "now")
            .unwrap();
        let h = Hubu::default();
        let p = Provider::default();
        let a = Artifacts {
            repo: &repo,
            calls: Cell::new(0),
        };
        let w = ExecutionWorkflow {
            repository: &repo,
            hubu: &h,
            provider: &p,
            artifacts: &a,
        };
        assert_eq!(
            w.run(&pending.execution_id, "later").unwrap().status,
            "reconciliation_required"
        );
        assert_eq!(p.calls.get(), 0);
        assert_eq!(
            repo.get_provider_attempt_for_execution(&pending.execution_id)
                .unwrap()
                .provider_attempt_id,
            attempt.provider_attempt_id
        );
    }

    #[test]
    fn async_worker_loss_before_checkpoint_reconciles_without_resubmit_or_release() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("before-checkpoint.sqlite3");
        let hubu = Hubu::default();
        let provider = AsyncProvider::default();

        let (execution_id, attempt_id) = {
            let repository = Repository::open(&path, Redactor::default()).unwrap();
            let execution = execution(&repository, "async-before-checkpoint");
            let artifacts = Artifacts {
                repo: &repository,
                calls: Cell::new(0),
            };
            let workflow = ExecutionWorkflow {
                repository: &repository,
                hubu: &hubu,
                provider: &provider,
                artifacts: &artifacts,
            };
            workflow
                .preflight_phase(&execution.execution_id, "preflight")
                .unwrap();
            workflow
                .claim_phase(&execution.execution_id, "claim")
                .unwrap();
            workflow
                .validate_claim_phase(&execution.execution_id, "validate")
                .unwrap();
            let attempt = repository
                .get_provider_attempt_for_execution(&execution.execution_id)
                .unwrap();

            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                workflow.provider_submit_phase_with_checkpoint_hook(
                    &execution.execution_id,
                    "submit",
                    &|| "2026-08-28T18:00:00Z".into(),
                    &|boundary| {
                        assert_eq!(boundary, ProviderCheckpointBoundary::BeforePersist);
                        panic!("simulated worker loss before operation checkpoint")
                    },
                )
            }))
            .is_err());

            let interrupted = repository
                .get_provider_attempt(&attempt.provider_attempt_id)
                .unwrap();
            assert!(interrupted.transmission_started_at.is_some());
            assert_eq!(repository.provider_operation(&interrupted).unwrap(), None);
            assert_eq!(provider.submits.get(), 1);
            assert_eq!(provider.polls.get(), 0);
            assert_eq!(hubu.releases.get(), 0);
            assert_eq!(hubu.settles.get(), 0);
            (execution.execution_id.clone(), attempt.provider_attempt_id)
        };

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let artifacts = Artifacts {
            repo: &restarted,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &restarted,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };
        let outcome = workflow
            .provider_submit_phase(&execution_id, "restart")
            .unwrap();
        assert!(matches!(
            outcome,
            ProviderPhaseOutcome::Complete(ExecutionPhaseResult {
                ref status,
                ..
            }) if status == "reconciliation_required"
        ));
        assert_eq!(
            restarted
                .get_execution(&execution_id)
                .unwrap()
                .failure_code
                .as_deref(),
            Some("provider_submission_interrupted")
        );
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 0);
        assert_eq!(hubu.releases.get(), 0);
        assert_eq!(hubu.settles.get(), 0);
        assert_eq!(
            restarted
                .get_provider_attempt_for_execution(&execution_id)
                .unwrap()
                .provider_attempt_id,
            attempt_id
        );
        let attempts = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM provider_attempts WHERE execution_id=?1",
                [&execution_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(attempts, 1);

        let evidence = restarted
            .get_reconciliation(&execution_id)
            .unwrap()
            .evidence;
        assert_eq!(evidence["provider_attempt_id"], attempt_id);
        assert!(evidence["provider_request_id"].is_null());
        assert!(evidence["provider_operation_id"].is_null());
        assert!(evidence["timestamps"]["transmission_started_at"].is_string());
        assert_eq!(
            evidence["recovery_guidance"]["provider_outcome_ambiguous"],
            true
        );
        assert_eq!(evidence["recovery_guidance"]["do_not_resubmit"], true);
        let encoded = serde_json::to_string(&evidence).unwrap();
        assert!(!encoded.contains("signed_url"));
        assert!(!encoded.contains("storage_key"));
        assert!(!encoded.contains("raw_body"));
    }

    #[test]
    fn resumed_poll_results_cannot_replace_or_bypass_checkpoint_identity() {
        let operation = AsyncProviderOperation {
            provider_request_id: Some("request-170".into()),
            provider_operation_id: "operation-170".into(),
            polling_host: "api.bfl.ai".into(),
            polling_recovery: None,
            deadline_unix_ms: 1_799_999_999_000,
        };
        let mismatched = ProviderSuccess {
            request_id: Some("https://storage.invalid/raw?signature=secret".into()),
            operation_id: Some("different-operation".into()),
            usage: NormalizedUsage {
                images: Some(1),
                ..Default::default()
            },
            actual_vendor_cost: None,
            artifacts: vec![ProviderArtifact {
                media_type: "image/png".into(),
                bytes: vec![1],
            }],
        };
        assert!(matches!(
            bind_poll_result_to_checkpoint(Ok(mismatched), &operation),
            Err(ActivityError::AmbiguousWithEvidence {
                code,
                request_id: Some(request_id),
                operation_id: Some(operation_id),
            }) if code == "provider_operation_identity_mismatch"
                && request_id == "request-170"
                && operation_id == "operation-170"
        ));

        let provider_rejection = ActivityError::ProvenWithEvidence {
            code: "provider_rejected".into(),
            request_id: Some("https://unsafe.invalid/request".into()),
            operation_id: Some("https://unsafe.invalid/operation".into()),
        };
        assert!(matches!(
            bind_poll_result_to_checkpoint(Err(provider_rejection), &operation),
            Err(ActivityError::ProvenWithEvidence {
                code,
                request_id: Some(request_id),
                operation_id: Some(operation_id),
            }) if code == "provider_rejected"
                && request_id == "request-170"
                && operation_id == "operation-170"
        ));
    }

    #[test]
    fn async_worker_loss_after_checkpoint_resumes_same_operation_and_settles_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("after-checkpoint.sqlite3");
        let hubu = Hubu::default();
        let provider = AsyncProvider::default();

        let (execution_id, attempt_id) = {
            let repository = Repository::open(&path, Redactor::default()).unwrap();
            let execution = execution(&repository, "async-after-checkpoint");
            let artifacts = Artifacts {
                repo: &repository,
                calls: Cell::new(0),
            };
            let workflow = ExecutionWorkflow {
                repository: &repository,
                hubu: &hubu,
                provider: &provider,
                artifacts: &artifacts,
            };
            workflow
                .preflight_phase(&execution.execution_id, "preflight")
                .unwrap();
            workflow
                .claim_phase(&execution.execution_id, "claim")
                .unwrap();
            workflow
                .validate_claim_phase(&execution.execution_id, "validate")
                .unwrap();
            let attempt = repository
                .get_provider_attempt_for_execution(&execution.execution_id)
                .unwrap();

            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                workflow.provider_submit_phase_with_checkpoint_hook(
                    &execution.execution_id,
                    "submit",
                    &|| "2026-08-28T18:00:01Z".into(),
                    &|boundary| {
                        if boundary == ProviderCheckpointBoundary::AfterPersist {
                            panic!("simulated worker loss after operation checkpoint")
                        }
                    },
                )
            }))
            .is_err());

            let checkpointed = repository
                .get_provider_attempt(&attempt.provider_attempt_id)
                .unwrap();
            assert_eq!(
                repository.provider_operation(&checkpointed).unwrap(),
                Some(provider.operation.clone())
            );
            assert_eq!(
                checkpointed.operation_checkpointed_at.as_deref(),
                Some("2026-08-28T18:00:01Z")
            );
            assert_eq!(
                checkpointed.provider_deadline_unix_ms,
                Some(provider.operation.deadline_unix_ms)
            );
            assert_eq!(provider.submits.get(), 1);
            assert_eq!(provider.polls.get(), 0);
            (execution.execution_id.clone(), attempt.provider_attempt_id)
        };

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let artifacts = Artifacts {
            repo: &restarted,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &restarted,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };
        assert_eq!(
            workflow
                .provider_submit_phase(&execution_id, "submit-redelivery")
                .unwrap(),
            ProviderPhaseOutcome::PollExisting
        );
        assert_eq!(
            workflow
                .provider_submit_phase(&execution_id, "submit-redelivery-again")
                .unwrap(),
            ProviderPhaseOutcome::PollExisting
        );
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 0);

        assert_eq!(
            workflow.provider_poll_phase(&execution_id, "poll").unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        assert_eq!(
            workflow
                .provider_poll_phase(&execution_id, "poll-response-lost")
                .unwrap(),
            ProviderPhaseOutcome::PersistArtifacts
        );
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 1);
        assert_eq!(
            provider.last_polled_operation.borrow().as_ref(),
            Some(&provider.operation)
        );
        assert_eq!(
            provider
                .last_polled_operation
                .borrow()
                .as_ref()
                .unwrap()
                .deadline_unix_ms,
            1_799_999_999_000
        );

        assert_eq!(
            workflow
                .artifact_phase(&execution_id, "artifacts")
                .unwrap()
                .status,
            "settling"
        );
        assert_eq!(
            workflow
                .settlement_phase(&execution_id, "settlement")
                .unwrap()
                .status,
            "succeeded"
        );
        assert_eq!(artifacts.calls.get(), 1);
        assert_eq!(hubu.settles.get(), 1);
        assert_eq!(hubu.releases.get(), 0);
        assert_eq!(
            restarted
                .get_provider_attempt_for_execution(&execution_id)
                .unwrap()
                .provider_attempt_id,
            attempt_id
        );
        assert!(restarted
            .get_receipt_for_execution(&execution_id)
            .unwrap()
            .settled_at
            .is_some());
        let attempts = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM provider_attempts WHERE execution_id=?1",
                [&execution_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(attempts, 1);

        assert_eq!(
            workflow
                .run(&execution_id, "terminal-replay")
                .unwrap()
                .status,
            "succeeded"
        );
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 1);
        assert_eq!(hubu.settles.get(), 1);
        assert_eq!(hubu.releases.get(), 0);
        assert_eq!(artifacts.calls.get(), 1);
    }

    #[test]
    fn explicit_reinspect_recovers_origin_rejection_after_restart_without_resubmission() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("origin-rejection-recovery.sqlite3");
        let hubu = Hubu::default();
        let provider = AsyncProvider {
            operation: AsyncProviderOperation {
                provider_request_id: Some("request-200".into()),
                provider_operation_id: "operation-200".into(),
                polling_host: "api.us7.bfl.ai".into(),
                polling_recovery: Some(crate::provider_contract::PollingRecoveryContext {
                    schema_version: 1,
                    policy_version: "bfl-polling-origin-v2".into(),
                    scheme: Some("https".into()),
                    normalized_host: Some("api.us7.bfl.ai".into()),
                    explicit_port: None,
                    endpoint_shape: "v1/get_result".into(),
                    query_keys: vec!["id".into()],
                    url_fingerprint: format!("sha256:{}", "b".repeat(64)),
                    validation_reason: Some("host_not_allowlisted".into()),
                }),
                deadline_unix_ms: 1_799_999_999_000,
            },
            ..Default::default()
        };
        provider
            .poll_error
            .replace(Some(ActivityError::AmbiguousWithEvidence {
                code: "polling_origin_rejected".into(),
                request_id: Some("request-200".into()),
                operation_id: Some("operation-200".into()),
            }));

        let execution_id = {
            let repository = Repository::open(&path, Redactor::default()).unwrap();
            let execution = execution(&repository, "origin-rejection-recovery");
            let artifacts = Artifacts {
                repo: &repository,
                calls: Cell::new(0),
            };
            let workflow = ExecutionWorkflow {
                repository: &repository,
                hubu: &hubu,
                provider: &provider,
                artifacts: &artifacts,
            };
            let held = workflow
                .run(&execution.execution_id, "2026-09-03T21:00:00Z")
                .unwrap();
            assert_eq!(held.status, "reconciliation_required");
            assert_eq!(provider.submits.get(), 1);
            assert_eq!(provider.polls.get(), 1);
            let attempt = repository
                .get_provider_attempt_for_execution(&execution.execution_id)
                .unwrap();
            assert_eq!(
                attempt.provider_recovery_context,
                provider.operation.polling_recovery
            );
            let evidence = repository
                .get_reconciliation(&execution.execution_id)
                .unwrap()
                .evidence;
            assert_eq!(
                evidence["last_confirmed_step"],
                "provider_operation_checkpointed"
            );
            assert_eq!(
                evidence["polling_recovery"]["validation_reason"],
                "host_not_allowlisted"
            );
            assert_eq!(evidence["recovery_guidance"]["do_not_resubmit"], true);
            assert_eq!(
                evidence["recovery_guidance"]["action"],
                "update_policy_then_reinspect"
            );
            assert_eq!(evidence["provider_operation_id"], "operation-200");
            let encoded = evidence.to_string();
            for forbidden in ["https://", "signed_url", "storage_path", "raw_body"] {
                assert!(!encoded.contains(forbidden));
            }
            execution.execution_id
        };

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let artifacts = Artifacts {
            repo: &restarted,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &restarted,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };
        provider
            .poll_error
            .replace(Some(ActivityError::AmbiguousWithEvidence {
                code: "timeout_unknown_outcome".into(),
                request_id: Some("request-200".into()),
                operation_id: Some("operation-200".into()),
            }));
        let held = restarted.get_execution(&execution_id).unwrap();
        let attempt = restarted
            .get_provider_attempt_for_execution(&execution_id)
            .unwrap();
        let first_reinspect = OperatorReconciliationRequest {
            action_id: "recover-operation-200".into(),
            action: ReconciliationAction::Reinspect,
            evidence: json!({"reason":"polling_policy_updated"}),
        };
        assert!(restarted
            .record_operator_action(
                &execution_id,
                &first_reinspect.action_id,
                "reinspect",
                &first_reinspect.evidence,
                "2026-09-03T21:00:01Z",
            )
            .unwrap());
        restarted
            .begin_provider_reconciliation_poll(
                &execution_id,
                &attempt.provider_attempt_id,
                held.version,
                "2026-09-03T21:00:01Z",
            )
            .unwrap();
        // Simulate activity loss immediately after the durable reopen. The
        // redelivery must resume the checkpointed GET path, not finish the
        // Temporal workflow in an orphaned `executing` state.
        let still_ambiguous = workflow
            .recover(
                &execution_id,
                "2026-09-03T21:00:01Z",
                Some(&first_reinspect),
            )
            .unwrap();
        assert_eq!(still_ambiguous.status, "reconciliation_required");
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 2);
        assert_eq!(
            restarted
                .get_provider_attempt_for_execution(&execution_id)
                .unwrap()
                .failure_code
                .as_deref(),
            Some("timeout_unknown_outcome")
        );

        let recovered = workflow
            .recover(
                &execution_id,
                "2026-09-03T21:00:02Z",
                Some(&OperatorReconciliationRequest {
                    action_id: "recover-operation-200-again".into(),
                    action: ReconciliationAction::Reinspect,
                    evidence: json!({"reason":"retry_same_operation_after_transient_poll"}),
                }),
            )
            .unwrap();
        assert_eq!(recovered.status, "succeeded");
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 3);
        let attempt_count = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM provider_attempts", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(attempt_count, 1);
        assert_eq!(
            restarted
                .count_artifacts_for_execution(&execution_id)
                .unwrap(),
            1
        );
        assert_eq!(hubu.settles.get(), 1);
    }

    #[test]
    fn async_proven_pre_send_failure_releases_without_poll_or_settlement() {
        let repository = Repository::in_memory().unwrap();
        let execution = execution(&repository, "async-proven-before-send");
        let hubu = Hubu::default();
        let provider = AsyncProvider::default();
        provider.submit_error.replace(Some(ActivityError::Proven(
            "provider_not_transmitted".into(),
        )));
        let artifacts = Artifacts {
            repo: &repository,
            calls: Cell::new(0),
        };
        let workflow = ExecutionWorkflow {
            repository: &repository,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };

        assert_eq!(
            workflow.run(&execution.execution_id, "now").unwrap().status,
            "released"
        );
        assert_eq!(provider.submits.get(), 1);
        assert_eq!(provider.polls.get(), 0);
        assert_eq!(hubu.releases.get(), 1);
        assert_eq!(hubu.settles.get(), 0);
        assert_eq!(artifacts.calls.get(), 0);
        let attempt = repository
            .get_provider_attempt_for_execution(&execution.execution_id)
            .unwrap();
        assert_eq!(attempt.outcome, "failed");
        assert_eq!(
            attempt.failure_code.as_deref(),
            Some("provider_not_transmitted")
        );
        assert!(attempt.completed_at.is_some());
    }
    #[test]
    fn terminal_states_are_immutable_and_skips_are_forbidden() {
        let repo = Repository::in_memory().unwrap();
        let e = execution(&repo, "states");
        let bad = ExecutionUpdate {
            status: "executing".into(),
            outcome: None,
            started_at: None,
            completed_at: None,
            failure_code: None,
            failure_message_redacted: None,
            provider_outcome: None,
            artifact_outcome: None,
            settlement_outcome: None,
        };
        assert!(matches!(
            repo.update_execution(&e.execution_id, e.version, &bad, "now"),
            Err(PersistenceError::ForbiddenTransition { .. })
        ));
        let terminal_update = ExecutionUpdate {
            status: "failed".into(),
            ..bad
        };
        let done = repo
            .update_execution(&e.execution_id, e.version, &terminal_update, "now")
            .unwrap();
        assert!(matches!(
            repo.update_execution(&e.execution_id, done.version, &terminal_update, "later"),
            Err(PersistenceError::ForbiddenTransition { .. })
        ));
    }
}
