//! Deterministic orchestration for one durable execution.
//!
//! Temporal owns delivery of `run`; this module makes each externally visible
//! activity replay-safe by consulting the persisted aggregate before acting.
use crate::{
    execution::{
        AttemptResult, CreateReceiptParams, Error as PersistenceError, Execution, ExecutionUpdate,
        Repository,
    },
    provider_contract::{NormalizedUsage, PricingSnapshot},
};
use serde_json::to_value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderArtifact {
    pub media_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ProviderSuccess {
    pub request_id: String,
    pub usage: NormalizedUsage,
    pub artifacts: Vec<ProviderArtifact>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityError {
    Proven(String),
    Ambiguous(String),
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

impl ExecutionWorkflow<'_> {
    pub fn run(&self, execution_id: &str, now: &str) -> Result<Execution, WorkflowError> {
        loop {
            let execution = self.repository.get_execution(execution_id)?;
            if terminal(&execution.status) {
                return Ok(execution);
            }
            match execution.status.as_str() {
                "pending" => {
                    self.transition(&execution, "preflighting", None, now, None, None, None)?;
                }
                "preflighting" => {
                    if let Err(e) = self.preflight(&execution) {
                        self.fail_before_claim(&execution, e, now)?;
                        continue;
                    }
                    if execution.hubu_claim_id.is_some() {
                        self.repository.accept_existing_claim(
                            execution_id,
                            execution.version,
                            now,
                        )?;
                        continue;
                    }
                    match self.hubu.claim(&execution) {
                        Ok(claim) => {
                            self.repository.set_claim(
                                execution_id,
                                execution.version,
                                &claim,
                                now,
                            )?;
                        }
                        Err(ActivityError::Proven(code)) => {
                            self.transition(
                                &execution,
                                "failed",
                                Some(&code),
                                now,
                                None,
                                None,
                                None,
                            )?;
                        }
                        Err(ActivityError::Ambiguous(code)) => {
                            self.transition(
                                &execution,
                                "reconciliation_required",
                                Some(&code),
                                now,
                                None,
                                None,
                                Some("ambiguous"),
                            )?;
                        }
                    }
                }
                "claimed" => match self.hubu.validate_claim(&execution) {
                    Ok(()) => {
                        self.repository.start_provider_attempt(&execution, now)?;
                    }
                    Err(ActivityError::Proven(code)) | Err(ActivityError::Ambiguous(code)) => {
                        self.transition(
                            &execution,
                            "reconciliation_required",
                            Some(&code),
                            now,
                            None,
                            None,
                            Some("ambiguous"),
                        )?;
                    }
                },
                "executing" => {
                    let attempt = self
                        .repository
                        .get_provider_attempt_for_execution(execution_id)?;
                    if attempt.completed_at.is_some() {
                        self.advance_completed_attempt(&execution, &attempt.outcome, now)?;
                        continue;
                    }
                    if attempt.transmission_started_at.is_some() {
                        self.transition(
                            &execution,
                            "reconciliation_required",
                            Some("provider_delivery_interrupted"),
                            now,
                            Some("ambiguous"),
                            None,
                            None,
                        )?;
                        continue;
                    }
                    self.repository
                        .begin_provider_transmission(&attempt.provider_attempt_id, now)?;
                    match self
                        .provider
                        .invoke(&execution, &attempt.provider_attempt_id)
                    {
                        Ok(success) => {
                            let has_provider_artifact = !success.artifacts.is_empty();
                            self.repository.complete_provider_attempt(
                                &attempt.provider_attempt_id,
                                &AttemptResult {
                                    outcome: "succeeded".into(),
                                    completed_at: now.into(),
                                    usage: usage_value(&success.usage),
                                    usage_schema_version: 1,
                                    provider_amount_minor: None,
                                    provider_currency: None,
                                    failure_code: None,
                                    failure_message_redacted: None,
                                    provider_request_id: Some(success.request_id.clone()),
                                },
                            )?;
                            if !has_provider_artifact {
                                self.transition(
                                    &execution,
                                    "reconciliation_required",
                                    Some("provider_returned_no_artifacts"),
                                    now,
                                    Some("succeeded"),
                                    Some("failed"),
                                    None,
                                )?;
                                continue;
                            }
                            match self.artifacts.persist(
                                &execution,
                                &attempt.provider_attempt_id,
                                &success.artifacts,
                            ) {
                                Ok(())
                                    if self.artifacts_match_usage(
                                        &execution,
                                        &attempt.provider_attempt_id,
                                        &success,
                                    )? =>
                                {
                                    self.transition(
                                        &execution,
                                        "settling",
                                        None,
                                        now,
                                        Some("succeeded"),
                                        Some("succeeded"),
                                        None,
                                    )?;
                                }
                                Ok(()) => {
                                    self.transition(
                                        &execution,
                                        "reconciliation_required",
                                        Some("artifact_count_mismatch"),
                                        now,
                                        Some("succeeded"),
                                        Some("failed"),
                                        None,
                                    )?;
                                }
                                Err(ActivityError::Proven(code))
                                | Err(ActivityError::Ambiguous(code)) => {
                                    self.transition(
                                        &execution,
                                        "reconciliation_required",
                                        Some(&code),
                                        now,
                                        Some("succeeded"),
                                        Some("failed"),
                                        None,
                                    )?;
                                }
                            }
                        }
                        Err(ActivityError::Proven(code)) => {
                            self.repository.complete_provider_attempt(
                                &attempt.provider_attempt_id,
                                &attempt_failure("failed", &code, now),
                            )?;
                            self.release_or_reconcile(&execution, &code, now)?;
                        }
                        Err(ActivityError::Ambiguous(code)) => {
                            self.repository.complete_provider_attempt(
                                &attempt.provider_attempt_id,
                                &attempt_failure("ambiguous", &code, now),
                            )?;
                            self.transition(
                                &execution,
                                "reconciliation_required",
                                Some(&code),
                                now,
                                Some("ambiguous"),
                                None,
                                None,
                            )?;
                        }
                    }
                }
                "settling" => {
                    let attempt = self
                        .repository
                        .get_provider_attempt_for_execution(execution_id)?;
                    let snapshot: PricingSnapshot =
                        serde_json::from_value(execution.pricing_snapshot.clone())
                            .map_err(|_| PersistenceError::Invalid("pricing snapshot"))?;
                    let usage: NormalizedUsage = serde_json::from_value(
                        attempt
                            .usage
                            .clone()
                            .ok_or(PersistenceError::Invalid("attempt usage"))?,
                    )
                    .map_err(|_| PersistenceError::Invalid("attempt usage"))?;
                    let amount = match snapshot.settle(&usage, execution.authorized_minor) {
                        Ok(amount) => amount,
                        Err(_) => {
                            self.transition(
                                &execution,
                                "reconciliation_required",
                                Some("invalid_provider_usage"),
                                now,
                                None,
                                None,
                                Some("ambiguous"),
                            )?;
                            continue;
                        }
                    };
                    let receipt = match self.repository.get_receipt_for_execution(execution_id) {
                        Ok(r) => r,
                        Err(PersistenceError::NotFound) => {
                            self.repository.create_receipt(&CreateReceiptParams {
                                receipt_id: format!("receipt-{execution_id}"),
                                execution_id: execution_id.into(),
                                provider_attempt_id: attempt.provider_attempt_id,
                                settlement_minor: amount,
                                currency: snapshot.currency.clone(),
                                pricing_catalog_version: snapshot.catalog_version,
                                created_at: now.into(),
                                settled_at: None,
                                hubu_settlement_id: None,
                            })?
                        }
                        Err(e) => return Err(e.into()),
                    };
                    if receipt.settled_at.is_some() {
                        self.transition(
                            &execution,
                            "succeeded",
                            Some("succeeded"),
                            now,
                            None,
                            None,
                            Some("succeeded"),
                        )?;
                        continue;
                    }
                    if receipt.transmission_started_at.is_some() {
                        self.transition(
                            &execution,
                            "reconciliation_required",
                            Some("settlement_delivery_interrupted"),
                            now,
                            None,
                            None,
                            Some("ambiguous"),
                        )?;
                        continue;
                    }
                    self.repository
                        .begin_settlement_transmission(&receipt.receipt_id, now)?;
                    match self.hubu.settle(&execution, &receipt.receipt_id, amount) {
                        Ok(id) => {
                            self.repository
                                .complete_receipt(&receipt.receipt_id, &id, now)?;
                            self.transition(
                                &execution,
                                "succeeded",
                                Some("succeeded"),
                                now,
                                None,
                                None,
                                Some("succeeded"),
                            )?;
                        }
                        Err(ActivityError::Proven(code)) | Err(ActivityError::Ambiguous(code)) => {
                            self.transition(
                                &execution,
                                "reconciliation_required",
                                Some(&code),
                                now,
                                None,
                                None,
                                Some("ambiguous"),
                            )?;
                        }
                    }
                }
                _ => return Err(PersistenceError::Invalid("workflow status").into()),
            }
        }
    }
    fn preflight(&self, e: &Execution) -> Result<(), ActivityError> {
        self.hubu.preflight(e)?;
        self.provider.preflight(e)?;
        self.artifacts.preflight()
    }
    fn artifacts_match_usage(
        &self,
        execution: &Execution,
        attempt_id: &str,
        success: &ProviderSuccess,
    ) -> Result<bool, WorkflowError> {
        let durable = self.repository.count_artifacts_for_attempt(attempt_id)?;
        let returned = u64::try_from(success.artifacts.len())
            .map_err(|_| PersistenceError::Invalid("provider artifact count"))?;
        if durable == 0 || durable != returned {
            return Ok(false);
        }
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| PersistenceError::Invalid("pricing snapshot"))?;
        if snapshot.unit == crate::provider_contract::PricingUnit::Image {
            if let Some(images) = success
                .usage
                .images
                .and_then(|value| u64::try_from(value).ok())
            {
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
            ActivityError::Proven(c) | ActivityError::Ambiguous(c) => c,
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
            Err(ActivityError::Proven(c)) | Err(ActivityError::Ambiguous(c)) => {
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
        Ok(self.repository.update_execution(
            &e.execution_id,
            e.version,
            &ExecutionUpdate {
                status: status.into(),
                outcome: outcome.map(str::to_owned),
                started_at: Some(now.into()),
                completed_at: terminal(status).then(|| now.into()),
                failure_code: outcome
                    .filter(|_| matches!(status, "failed" | "reconciliation_required"))
                    .map(str::to_owned),
                failure_message_redacted: None,
                provider_outcome: provider.map(str::to_owned),
                artifact_outcome: artifact.map(str::to_owned),
                settlement_outcome: settlement.map(str::to_owned),
            },
            now,
        )?)
    }
}
fn attempt_failure(outcome: &str, code: &str, now: &str) -> AttemptResult {
    AttemptResult {
        outcome: outcome.into(),
        completed_at: now.into(),
        usage: serde_json::json!({}),
        usage_schema_version: 1,
        provider_amount_minor: None,
        provider_currency: None,
        failure_code: Some(code.into()),
        failure_message_redacted: None,
        provider_request_id: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{CreateArtifactParams, CreateExecutionParams, HubuTokenReference};
    use serde_json::json;
    use std::cell::{Cell, RefCell};

    fn execution(repo: &Repository, key: &str) -> Execution {
        execution_with_quantity(repo, key, 1)
    }
    fn execution_with_quantity(repo: &Repository, key: &str, quantity: i64) -> Execution {
        repo.create_execution(&CreateExecutionParams { account_id:"account".into(),operation_key:key.into(),hubu_authorization_id:"auth".into(),hubu_claim_id:None,hubu_token_reference:HubuTokenReference::new("token-ref").unwrap(),authorized_minor:500,authorization_currency:"USD".into(),normalized_input:json!({"prompt":"cat"}),input_hash:"hash".into(),input_schema_version:1,target:"example/image-v1".into(),config_version:"cfg-1".into(),workload_type:"image_generation".into(),provider:"example".into(),adapter:"fixture".into(),model:"image-v1".into(),provider_config_version:"pcv-1".into(),pricing_snapshot:json!({"provider":"example","model":"image-v1","catalog_version":"prices-v1","catalog_digest":format!("sha256:{}","a".repeat(64)),"pricing_rule_id":"image","unit":"image","unit_amount_minor":100,"quantity":quantity,"estimated_amount_minor":100 * quantity,"currency":"USD"}),pricing_schema_version:1,created_at:"2026-08-05T00:00:00Z".into() }).unwrap()
    }
    struct Hubu {
        claims: Cell<u32>,
        settles: Cell<u32>,
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
        fn settle(&self, _: &Execution, _: &str, _: i64) -> Result<String, ActivityError> {
            self.settles.set(self.settles.get() + 1);
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
    }
    impl Default for Provider {
        fn default() -> Self {
            Self {
                calls: Cell::new(0),
                error: RefCell::new(None),
                empty_artifacts: Cell::new(false),
                image_usage: Cell::new(1),
                artifact_count: Cell::new(1),
            }
        }
    }
    impl ProviderActivities for Provider {
        fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
            Ok(())
        }
        fn invoke(&self, _: &Execution, _: &str) -> Result<ProviderSuccess, ActivityError> {
            self.calls.set(self.calls.get() + 1);
            if let Some(e) = self.error.borrow_mut().take() {
                Err(e)
            } else {
                Ok(ProviderSuccess {
                    request_id: "provider-1".into(),
                    usage: NormalizedUsage {
                        images: Some(self.image_usage.get()),
                        ..Default::default()
                    },
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
                        artifact_id: "artifact-1".into(),
                        execution_id: e.execution_id.clone(),
                        provider_attempt_id: Some(a.into()),
                        kind: "image".into(),
                        storage_backend: "local_fs".into(),
                        media_type: "image/png".into(),
                        storage_key: format!("executions/{}/artifact-1.png", e.execution_id),
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
    fn supplied_claim_is_adopted_without_claiming_again() {
        let repo = Repository::in_memory().unwrap();
        let params = CreateExecutionParams {
            account_id: "account".into(),
            operation_key: "supplied-claim".into(),
            hubu_authorization_id: "auth".into(),
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
            pricing_snapshot: json!({"provider":"example","model":"image-v1","catalog_version":"prices-v1","catalog_digest":format!("sha256:{}","a".repeat(64)),"pricing_rule_id":"image","unit":"image","unit_amount_minor":100,"quantity":1,"estimated_amount_minor":100,"currency":"USD"}),
            pricing_schema_version: 1,
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
        assert_eq!(done.artifact_outcome.as_deref(), Some("failed"));
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
