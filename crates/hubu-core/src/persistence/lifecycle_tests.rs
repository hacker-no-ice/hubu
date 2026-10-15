use super::*;
use crate::policy::{Effect, Evaluation, Policy};
use hubu_common::ids::{AgentAccountId, SpendAuthTokenId, SpendDecisionId};
use std::collections::HashSet;

fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}
fn unknown<T>() -> LifecycleEvidence<T> {
    LifecycleEvidence::Unavailable {
        reason: EvidenceUnavailableReason::NotCaptured,
    }
}
fn money(units: i64) -> LifecycleAmount {
    LifecycleAmount {
        amount: Amount::from_units(units),
        currency: Currency::Usd,
    }
}

/// Reusable scenario fixture: registered ownership and the same immutable
/// request/decision/token/claim relationship used by future capture producers.
struct Fixture {
    owner: UserId,
    agent: AgentId,
    account: AgentAccountId,
    decision: SpendDecisionId,
    claim: SpendExecutorClaimId,
    token: SpendAuthTokenId,
    key: String,
    budget: BudgetId,
    version: BudgetVersionId,
}
impl Fixture {
    fn seed(repo: &mut SqliteGovernanceRepository, effect: Effect) -> Self {
        crate::storage::init_schema(&repo.conn).unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let account = AgentAccountId::new();
        let decision = SpendDecisionId::new();
        let claim = SpendExecutorClaimId::new();
        let token = SpendAuthTokenId::new();
        let now = at("2026-10-10T00:00:00Z");
        let key = "private-operation-key-never-public".to_string();
        repo.conn.execute("INSERT INTO users(id,pub_id,display_name,status,created_at,updated_at) VALUES(?1,?2,'Owner','active',?3,?3)",params![owner.to_string(),format!("usr_{}",owner.public_suffix()),now.to_rfc3339()]).unwrap();
        repo.conn.execute("INSERT INTO agent_identities(id,pub_id,fingerprint,display_name,owner_user_id,agent_type,agent_status,created_at,updated_at) VALUES(?1,?2,?3,'Fixture agent',?4,'autonomous_agent','active',?5,?5)",params![agent.to_string(),format!("agt_{}",agent.public_suffix()),format!("fingerprint:{agent}"),owner.to_string(),now.to_rfc3339()]).unwrap();
        repo.conn.execute("INSERT INTO agent_accounts(id,pub_id,agent_id,owner_user_id,account_status,created_at,updated_at) VALUES(?1,?2,?3,?4,'active',?5,?5)",params![account.to_string(),format!("aga_{}",account.public_suffix()),agent.to_string(),owner.to_string(),now.to_rfc3339()]).unwrap();
        let request = SpendRequest {
            amount: Amount::from_units(5_000),
            currency: Currency::Usd,
            owner_user_id: owner.clone(),
            agent_id: agent.clone(),
            agent_account_id: account.clone(),
            merchant: Some("private merchant prose".into()),
            execution_scope: None,
            category: None,
            task_id: Some("private task".into()),
            reason: "private prompt".into(),
            lease_profile: "default".into(),
        };
        assert_eq!(
            repo.admit_spend_attempt(&owner, &key, &request, "fixture", now)
                .unwrap(),
            SpendAttemptAdmission::Admitted { revision: 1 }
        );
        repo.save_spend_decision(&SpendDecisionRecord {
            id: decision.clone(),
            owner_user_id: owner.clone(),
            operation_key: key.clone(),
            revision: 1,
            actor: owner.to_string(),
            request,
            evaluation: Evaluation {
                policy_id: "authored-fixture".into(),
                policy_version: "v1".into(),
                decision: effect,
                reasons: vec!["private policy prose".into()],
                rule_results: vec![],
            },
            created_at: now,
        })
        .unwrap();
        let (budget, _) = repo
            .create_budget(
                crate::budget::CreateSingleBudgetRequest {
                    agent_id: agent.clone(),
                    amount_limit: Amount::from_units(10_000),
                    currency: Currency::Usd,
                    period: TimePeriod::new(at("2026-10-01T00:00:00Z"), None).unwrap(),
                },
                crate::budget::BudgetVersionProvenance::new("fixture", "test"),
            )
            .unwrap();
        Self {
            owner,
            agent,
            account,
            decision,
            claim,
            token,
            key,
            budget: budget.budget.id,
            version: budget.version.id,
        }
    }
    fn manager(repo: &SqliteGovernanceRepository) -> crate::budget::BudgetManager {
        crate::budget::BudgetManager::from_records(
            repo.load_budgets().unwrap(),
            repo.load_budget_versions().unwrap(),
            repo.load_budget_balances().unwrap(),
            repo.load_budget_holds().unwrap(),
        )
        .unwrap()
    }
    fn authorize(&self, repo: &mut SqliteGovernanceRepository) {
        let now = Utc::now();
        let token = SpendAuthTokenRecord {
            id: self.token.clone(),
            owner_user_id: self.owner.clone(),
            spend_decision_id: self.decision.clone(),
            expires_at: now + chrono::Duration::hours(1),
            claim_ttl_seconds: 3600,
            used_at: None,
            used_by_payment_id: None,
            revoked_at: None,
        };
        let decision = repo
            .load_spend_decisions()
            .unwrap()
            .into_iter()
            .find(|d| d.id == self.decision)
            .unwrap();
        let reservation = Self::manager(repo)
            .reserve_budget_at(
                crate::budget::ReserveBudgetRequest {
                    budget_id: self.budget.clone(),
                    spend_decision_id: self.decision.clone(),
                    amount: decision.request.amount,
                    currency: Currency::Usd,
                    expires_at: token.expires_at,
                },
                now,
            )
            .unwrap();
        repo.save_approved_spend_transition(
            &decision,
            &token,
            &reservation.hold,
            &reservation.balance,
            &["fixture confirmed".into()],
            now,
        )
        .unwrap();
    }
    fn claim(&self, repo: &mut SqliteGovernanceRepository) {
        let now = Utc::now();
        let hold = repo
            .load_budget_holds()
            .unwrap()
            .into_iter()
            .find(|h| h.spend_decision_id == self.decision)
            .unwrap();
        let claimed = Self::manager(repo)
            .claim_budget(
                &hold.id,
                self.claim.clone(),
                now + chrono::Duration::hours(1),
            )
            .unwrap();
        repo.save_executor_claim_with_budget_hold(
            &SpendExecutorClaimRecord {
                id: self.claim.clone(),
                spend_auth_token_id: self.token.clone(),
                owner_user_id: self.owner.clone(),
                agent_id: self.agent.clone(),
                operation_key: self.key.clone(),
                lease_profile: "default".into(),
                status: SpendExecutorClaimStatus::Claimed,
                claimed_at: now,
                expires_at: now + chrono::Duration::hours(1),
                finalized_at: None,
                settlement_id: None,
                provider_reference: None,
                reconciliation_evidence: None,
                reconciled_at: None,
                reconciled_by_user_id: None,
            },
            &claimed.hold,
            &claimed.balance,
        )
        .unwrap();
    }
    fn reconcile(&self, repo: &mut SqliteGovernanceRepository, billed: bool) {
        let now = Utc::now() + chrono::Duration::hours(2);
        if billed {
            repo.reconcile_executor_claim_as_billed_transactionally(&self.claim,&self.owner,"provider-reference","confirmed fixture invoice",PaymentId::new(),SpendExecutorSettlementReceipt {actual_vendor_cost:crate::spend::SpendExecutorVendorCost {amount:3_000,scale:6,currency:Currency::Usd},provider_request_id:"private provider reference".into(),price_model_snapshot:serde_json::json!({"provider":"fixture","model":"pricing-v1","currency":"usd"}),artifact_reference:"private artifact".into()},now).unwrap();
        } else {
            repo.reconcile_executor_claim_as_not_billed_transactionally(
                &self.claim,
                &self.owner,
                "provider-reference",
                "confirmed no bill",
                now,
            )
            .unwrap();
        }
    }
    fn event(&self, source: LifecycleSource, fact: LifecycleFact) -> LifecycleEventInput {
        LifecycleEventInput {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            correlation: LifecycleCorrelation {
                owner_user_id: self.owner.clone(),
                agent_id: self.agent.clone(),
                agent_account_id: self.account.clone(),
                operation_key: self.key.clone(),
                attempt_revision: 1,
                decision_id: Some(self.decision.clone()),
                correction_of_revision: None,
            },
            source,
            occurred_at: at("2026-10-10T00:00:00.123456Z"),
            requested_amount: LifecycleEvidence::Known {
                value: money(5_000),
            },
            execution_scope: unknown(),
            fact,
            explanation: LifecycleExplanation::default(),
        }
    }
    fn policy(&self, decision: InitialPolicyDecision) -> LifecycleEventInput {
        self.event(
            LifecycleSource::PolicyEngine,
            LifecycleFact::InitialPolicy { decision },
        )
    }
}

#[test]
fn fixture_matrix_preserves_every_stage_and_technical_failure_can_retry() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::seed(&mut repo, Effect::NeedsApproval);
    let cases = vec![
        (
            LifecycleSource::PolicyEngine,
            LifecycleFact::InitialPolicy {
                decision: InitialPolicyDecision::NeedsApproval,
            },
            LifecycleCount::PolicyNeedsApproval,
        ),
        (
            LifecycleSource::AutomatedAdmission,
            LifecycleFact::AdmissionFailed {
                observation_id: LifecycleObservationId::new(),
            },
            LifecycleCount::AdmissionFailed,
        ),
        (
            LifecycleSource::HumanApproval,
            LifecycleFact::HumanEscalation {
                approval_request_id: f.decision.clone(),
            },
            LifecycleCount::HumanEscalation,
        ),
        (
            LifecycleSource::HumanApproval,
            LifecycleFact::HumanResolutionConfirmed {
                approval_request_id: f.decision.clone(),
                resolution: HumanResolution::Approved,
                confirmed_by_user_id: f.owner.clone(),
            },
            LifecycleCount::HumanApproved,
        ),
        (
            LifecycleSource::HumanApproval,
            LifecycleFact::AuthorizationGranted {
                authorization_id: f.decision.clone(),
            },
            LifecycleCount::AuthorizationGranted,
        ),
        (
            LifecycleSource::Executor,
            LifecycleFact::ExecutionStarted {
                claim_id: f.claim.clone(),
            },
            LifecycleCount::ExecutionStarted,
        ),
        (
            LifecycleSource::Executor,
            LifecycleFact::ExecutionFailed {
                claim_id: f.claim.clone(),
            },
            LifecycleCount::ExecutionFailed,
        ),
        (
            LifecycleSource::Executor,
            LifecycleFact::ReconciliationRequired {
                claim_id: f.claim.clone(),
            },
            LifecycleCount::ReconciliationRequired,
        ),
        (
            LifecycleSource::Reconciliation,
            LifecycleFact::ReconciliationResolved {
                claim_id: f.claim.clone(),
                outcome: ReconciliationResolution::VendorBilled,
            },
            LifecycleCount::ReconciliationResolved,
        ),
        (
            LifecycleSource::Reconciliation,
            LifecycleFact::ExecutionFinalized {
                claim_id: f.claim.clone(),
                outcome: ExecutionFinalization::Settled,
            },
            LifecycleCount::ExecutionSettled,
        ),
    ];
    for (source, fact, category) in &cases {
        if matches!(fact, LifecycleFact::AuthorizationGranted { .. }) {
            f.authorize(&mut repo);
        }
        if matches!(fact, LifecycleFact::ExecutionStarted { .. }) {
            f.claim(&mut repo);
        }
        if matches!(fact, LifecycleFact::ReconciliationResolved { .. }) {
            f.reconcile(&mut repo, true);
        }
        let input = f.event(*source, fact.clone());
        assert_eq!(input.fact.count_category(), *category);
        let first = repo.append_lifecycle_event(&input).unwrap();
        let replay = repo.append_lifecycle_event(&input).unwrap();
        assert!(!first.idempotent_replay);
        assert!(replay.idempotent_replay);
        assert_eq!(first.record, replay.record);
    }
    let snapshot = repo.lifecycle_snapshot(&f.owner, None).unwrap();
    assert_eq!(snapshot.events.len(), cases.len());
    assert!(snapshot
        .events
        .iter()
        .all(|e| e.event_id.starts_with("lev_") && !e.event_id.contains(&f.key)));
    assert_eq!(
        snapshot.events[0].event.requested_amount,
        LifecycleEvidence::Known {
            value: money(5_000)
        }
    );
    let mut conflicting = f.event(
        LifecycleSource::HumanApproval,
        LifecycleFact::HumanResolutionConfirmed {
            approval_request_id: f.decision.clone(),
            resolution: HumanResolution::Declined,
            confirmed_by_user_id: f.owner.clone(),
        },
    );
    assert!(repo
        .append_lifecycle_event(&conflicting)
        .unwrap_err()
        .to_string()
        .contains("conflicting lifecycle"));
    conflicting = f.event(
        LifecycleSource::Reconciliation,
        LifecycleFact::ExecutionFinalized {
            claim_id: f.claim.clone(),
            outcome: ExecutionFinalization::Released,
        },
    );
    assert!(repo.append_lifecycle_event(&conflicting).is_err());
}

#[test]
fn policy_denial_human_decline_and_safe_preclaim_release_are_distinct() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    for (effect, decision, category) in [
        (
            Effect::Allow,
            InitialPolicyDecision::Allow,
            LifecycleCount::PolicyAllow,
        ),
        (
            Effect::Deny,
            InitialPolicyDecision::Deny,
            LifecycleCount::PolicyDeny,
        ),
    ] {
        let f = Fixture::seed(&mut repo, effect);
        let input = f.policy(decision);
        assert_eq!(input.fact.count_category(), category);
        repo.append_lifecycle_event(&input).unwrap();
        let synthetic = f.event(
            LifecycleSource::AutomatedAdmission,
            LifecycleFact::HumanEscalation {
                approval_request_id: f.decision.clone(),
            },
        );
        assert!(repo.append_lifecycle_event(&synthetic).is_err());
        let wrong_policy = f.event(
            LifecycleSource::HumanApproval,
            LifecycleFact::HumanEscalation {
                approval_request_id: f.decision.clone(),
            },
        );
        assert!(repo.append_lifecycle_event(&wrong_policy).is_err());
        if effect == Effect::Deny {
            let decision = repo
                .load_spend_decisions()
                .unwrap()
                .into_iter()
                .find(|d| d.id == f.decision)
                .unwrap();
            repo.record_spend_attempt_outcome(
                &decision,
                SpendAuthorizationDecision::Denied,
                &[],
                Utc::now(),
            )
            .unwrap();
            let denied = f.event(
                LifecycleSource::AutomatedAdmission,
                LifecycleFact::AutomatedAdmission {
                    result: AutomatedAdmissionOutcome::Denied {
                        reason: AdmissionDenialReason::Policy,
                    },
                },
            );
            assert_eq!(
                denied.fact.count_category(),
                LifecycleCount::AdmissionDenied
            );
            repo.append_lifecycle_event(&denied).unwrap();
        } else {
            let failure = f.event(
                LifecycleSource::AutomatedAdmission,
                LifecycleFact::AdmissionFailed {
                    observation_id: LifecycleObservationId::new(),
                },
            );
            repo.append_lifecycle_event(&failure).unwrap();
            f.authorize(&mut repo);
            let admitted = f.event(
                LifecycleSource::AutomatedAdmission,
                LifecycleFact::AutomatedAdmission {
                    result: AutomatedAdmissionOutcome::Admitted,
                },
            );
            assert_eq!(
                admitted.fact.count_category(),
                LifecycleCount::AdmissionAllowed
            );
            let original_admitted = repo.append_lifecycle_event(&admitted).unwrap();
            // No claim is necessary for a known-safe authorization release.
            repo.void_unclaimed_authorization(&f.owner, &f.account, &f.token, &f.key)
                .unwrap();
            let released = f.event(
                LifecycleSource::Executor,
                LifecycleFact::AuthorizationReleased {
                    authorization_id: f.decision.clone(),
                    reason: AuthorizationReleaseReason::ExecutorRejectedBeforeClaim,
                },
            );
            assert_eq!(
                released.fact.count_category(),
                LifecycleCount::AuthorizationReleased
            );
            repo.append_lifecycle_event(&released).unwrap();
            let replay = repo.append_lifecycle_event(&admitted).unwrap();
            assert!(replay.idempotent_replay);
            assert_eq!(replay.record, original_admitted.record);
        }
    }
    let f = Fixture::seed(&mut repo, Effect::NeedsApproval);
    let declined = f.event(
        LifecycleSource::HumanApproval,
        LifecycleFact::HumanResolutionConfirmed {
            approval_request_id: f.decision.clone(),
            resolution: HumanResolution::Declined,
            confirmed_by_user_id: f.owner.clone(),
        },
    );
    assert_eq!(
        declined.fact.count_category(),
        LifecycleCount::HumanDeclined
    );
    repo.append_lifecycle_event(&declined).unwrap();
}

fn backlog(snapshot: &LifecycleSnapshot) -> usize {
    let mut opened = HashSet::new();
    let mut resolved = HashSet::new();
    for record in &snapshot.events {
        match record.event.fact.backlog_transition() {
            Some(LifecycleBacklogTransition::Open(s)) => {
                opened.insert(s);
            }
            Some(LifecycleBacklogTransition::Resolve(s)) => {
                resolved.insert(s);
            }
            None => {}
        }
    }
    opened.difference(&resolved).count()
}
#[test]
fn watermark_backlog_uses_preperiod_opens_and_late_arrivals_cannot_reopen() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::seed(&mut repo, Effect::NeedsApproval);
    assert!(repo
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .capture_markers
        .is_empty());
    let marker = repo
        .append_lifecycle_capture_marker(
            &f.owner,
            LifecycleSource::HumanApproval,
            LifecycleCaptureState::Enabled,
            at("2026-10-09T00:00:00Z"),
        )
        .unwrap();
    let mut escalation = f.event(
        LifecycleSource::HumanApproval,
        LifecycleFact::HumanEscalation {
            approval_request_id: f.decision.clone(),
        },
    );
    escalation.occurred_at = at("2026-10-09T23:59:59.999999999Z");
    repo.append_lifecycle_event(&escalation).unwrap();
    let frozen = repo.lifecycle_snapshot(&f.owner, None).unwrap();
    assert_eq!(backlog(&frozen), 1);
    assert_eq!(frozen.capture_markers[0], marker);
    assert_eq!(
        frozen
            .events
            .iter()
            .filter(|r| r.event.occurred_at >= at("2026-10-10T00:00:00Z"))
            .count(),
        0
    );
    let resolved = f.event(
        LifecycleSource::HumanApproval,
        LifecycleFact::HumanResolutionConfirmed {
            approval_request_id: f.decision.clone(),
            resolution: HumanResolution::Approved,
            confirmed_by_user_id: f.owner.clone(),
        },
    );
    repo.append_lifecycle_event(&resolved).unwrap();
    assert_eq!(
        backlog(
            &repo
                .lifecycle_snapshot(&f.owner, Some(frozen.watermark))
                .unwrap()
        ),
        1
    );
    assert_eq!(
        backlog(&repo.lifecycle_snapshot(&f.owner, None).unwrap()),
        0
    );
    let required = f.event(
        LifecycleSource::Executor,
        LifecycleFact::ReconciliationRequired {
            claim_id: f.claim.clone(),
        },
    );
    let resolution = f.event(
        LifecycleSource::Reconciliation,
        LifecycleFact::ReconciliationResolved {
            claim_id: f.claim.clone(),
            outcome: ReconciliationResolution::VendorDidNotBill,
        },
    );
    f.authorize(&mut repo);
    f.claim(&mut repo);
    f.reconcile(&mut repo, false);
    repo.append_lifecycle_event(&resolution).unwrap();
    repo.append_lifecycle_event(&required).unwrap();
    assert_eq!(
        backlog(&repo.lifecycle_snapshot(&f.owner, None).unwrap()),
        0
    );
}

#[test]
fn append_enforces_owner_request_source_and_correlation_and_rolls_back_with_parent() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::seed(&mut repo, Effect::Allow);
    let other = Fixture::seed(&mut repo, Effect::Allow);
    let input = f.policy(InitialPolicyDecision::Allow);
    let mut bad = input.clone();
    bad.correlation.owner_user_id = other.owner.clone();
    assert!(repo.append_lifecycle_event(&bad).is_err());
    bad = input.clone();
    bad.correlation.agent_account_id = other.account.clone();
    assert!(repo.append_lifecycle_event(&bad).is_err());
    bad = input.clone();
    bad.correlation.decision_id = Some(other.decision.clone());
    assert!(repo.append_lifecycle_event(&bad).is_err());
    bad = input.clone();
    bad.correlation.operation_key = "orphan-operation".into();
    bad.correlation.decision_id = None;
    assert!(repo.append_lifecycle_event(&bad).is_err());
    bad = input.clone();
    bad.requested_amount = LifecycleEvidence::Known {
        value: money(5_001),
    };
    assert!(repo.append_lifecycle_event(&bad).is_err());
    bad = input.clone();
    bad.schema_version = 99;
    assert!(repo.append_lifecycle_event(&bad).is_err());
    assert!(append_lifecycle_event_in_transaction(&repo.conn, &input).is_err());
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    append_lifecycle_event_in_transaction(&tx, &input).unwrap();
    tx.execute(
        "INSERT INTO app_state(key,value,updated_at) VALUES('parent-write','pending',?1)",
        [Utc::now().to_rfc3339()],
    )
    .unwrap();
    // A caller-owned write fails after evidence append; its rollback removes both.
    assert!(tx
        .execute(
            "INSERT INTO app_state(key,value,updated_at) VALUES('parent-write','conflict',?1)",
            [Utc::now().to_rfc3339()]
        )
        .is_err());
    tx.rollback().unwrap();
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, None).unwrap().watermark,
        0
    );
    let count: i64 = repo
        .conn
        .query_row(
            "SELECT COUNT(*) FROM app_state WHERE key='parent-write'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    repo.append_lifecycle_event(&input).unwrap();
    repo.append_lifecycle_event(&other.policy(InitialPolicyDecision::Allow))
        .unwrap();
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, None)
            .unwrap()
            .events
            .len(),
        1
    );
    assert_eq!(
        repo.lifecycle_snapshot(&other.owner, None)
            .unwrap()
            .events
            .len(),
        1
    );
}

#[test]
fn additive_migration_restart_replay_and_immutable_request_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("governance.sqlite");
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    let f = Fixture::seed(&mut repo, Effect::Allow);
    let before: String = repo
        .conn
        .query_row(
            "SELECT request_json FROM spend_decisions WHERE id=?1",
            [f.decision.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, None).unwrap().watermark,
        0
    );
    let input = f.policy(InitialPolicyDecision::Allow);
    let first = repo.append_lifecycle_event(&input).unwrap();
    assert!(repo
        .conn
        .execute(
            "UPDATE spend_lifecycle_records SET recorded_at='changed'",
            []
        )
        .is_err());
    assert!(repo
        .conn
        .execute("DELETE FROM spend_lifecycle_records", [])
        .is_err());
    drop(repo);
    let mut reopened = SqliteGovernanceRepository::open(&path).unwrap();
    let replay = reopened.append_lifecycle_event(&input).unwrap();
    assert!(replay.idempotent_replay);
    assert_eq!(replay.record, first.record);
    let after: String = reopened
        .conn
        .query_row(
            "SELECT request_json FROM spend_decisions WHERE id=?1",
            [f.decision.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert!(reopened
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .capture_markers
        .is_empty());
    // Simulate a database from before this additive feature without touching old
    // immutable business rows; reopening installs empty evidence, not fake facts.
    reopened.conn.execute_batch("DROP TRIGGER spend_lifecycle_records_no_update; DROP TRIGGER spend_lifecycle_records_no_delete; DROP TABLE spend_lifecycle_records; DROP TRIGGER spend_lifecycle_schema_no_update; DROP TRIGGER spend_lifecycle_schema_no_delete; DROP TABLE spend_lifecycle_schema_versions;").unwrap();
    drop(reopened);
    let migrated = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(
        migrated
            .lifecycle_snapshot(&f.owner, None)
            .unwrap()
            .watermark,
        0
    );
    let retained: String = migrated
        .conn
        .query_row(
            "SELECT request_json FROM spend_decisions WHERE id=?1",
            [f.decision.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, before);
}

#[test]
fn evidence_is_exact_explicit_safe_and_rejects_foreign_provenance() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::seed(&mut repo, Effect::Allow);
    let mut input = f.policy(InitialPolicyDecision::Allow);
    input.explanation.budget = LifecycleEvidence::Known {
        value: LifecycleBudgetEvidence {
            budget_id: format!("bgt_{}", f.budget.public_suffix()),
            budget_version_id: format!("bgv_{}", f.version.public_suffix()),
            requested: money(5_000),
            limit: LifecycleEvidence::Known {
                value: money(10_000),
            },
            available: LifecycleEvidence::Known { value: money(-1) },
            consumed: LifecycleEvidence::Known {
                value: money(10_001),
            },
            frozen: LifecycleEvidence::Known { value: money(0) },
        },
    };
    let policy = Policy {
        id: "authored-fixture".into(),
        version: "v1".into(),
        owner_user_id: f.owner.clone(),
        rules: vec![],
        default_effect: Effect::Allow,
    };
    repo.save_policy_assignment(&f.owner, &PolicyAssignmentScope::UserDefault, &policy)
        .unwrap();
    let resource = repo.load_policies(&f.owner).unwrap().remove(0);
    input.explanation.policy = LifecycleEvidence::Known {
        value: LifecyclePolicyEvidence {
            policy_id: resource.policy_id,
            revision: resource.current_revision,
            payload_hash: resource.current_hash,
            decisive_rule_ids: vec![],
            amount_constraints: vec![],
        },
    };
    let json = serde_json::to_string(&input).unwrap();
    assert!(json.contains("\"units\":\"5000\""));
    assert!(json.contains("\"units\":\"-1\""));
    assert!(!json.contains("private prompt"));
    assert!(!json.contains("private merchant"));
    repo.append_lifecycle_event(&input).unwrap();
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, None).unwrap().events[0].event,
        input
    );
    let roundtrip: LifecycleAmount =
        serde_json::from_str(&serde_json::to_string(&money(i64::MIN)).unwrap()).unwrap();
    assert_eq!(roundtrip, money(i64::MIN));
    assert!(serde_json::from_str::<LifecycleAmount>(
        r#"{"amount":{"units":"5000","scale":2},"currency":"usd"}"#
    )
    .is_err());
    let other = Fixture::seed(&mut repo, Effect::Allow);
    let mut foreign = other.policy(InitialPolicyDecision::Allow);
    foreign.explanation = input.explanation;
    assert!(repo.append_lifecycle_event(&foreign).is_err());
}

#[test]
fn financial_confirmation_and_source_guards_reject_unresolved_or_contradictory_claims() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::seed(&mut repo, Effect::Allow);
    f.authorize(&mut repo);
    f.claim(&mut repo);
    for fact in [
        LifecycleFact::ExecutionFinalized {
            claim_id: f.claim.clone(),
            outcome: ExecutionFinalization::Settled,
        },
        LifecycleFact::ExecutionFinalized {
            claim_id: f.claim.clone(),
            outcome: ExecutionFinalization::Released,
        },
        LifecycleFact::ReconciliationResolved {
            claim_id: f.claim.clone(),
            outcome: ReconciliationResolution::VendorDidNotBill,
        },
    ] {
        let input = f.event(LifecycleSource::Reconciliation, fact);
        assert!(repo.append_lifecycle_event(&input).is_err());
    }
    let preclaim_release = f.event(
        LifecycleSource::Executor,
        LifecycleFact::AuthorizationReleased {
            authorization_id: f.decision.clone(),
            reason: AuthorizationReleaseReason::ExecutorRejectedBeforeClaim,
        },
    );
    assert!(repo.append_lifecycle_event(&preclaim_release).is_err());
    let false_policy_denial = f.event(
        LifecycleSource::AutomatedAdmission,
        LifecycleFact::AutomatedAdmission {
            result: AutomatedAdmissionOutcome::Denied {
                reason: AdmissionDenialReason::Policy,
            },
        },
    );
    assert!(repo.append_lifecycle_event(&false_policy_denial).is_err());
    f.reconcile(&mut repo, false);
    let released = f.event(
        LifecycleSource::Reconciliation,
        LifecycleFact::ExecutionFinalized {
            claim_id: f.claim.clone(),
            outcome: ExecutionFinalization::Released,
        },
    );
    repo.append_lifecycle_event(&released).unwrap();
    let contradicted = f.event(
        LifecycleSource::Reconciliation,
        LifecycleFact::ReconciliationResolved {
            claim_id: f.claim.clone(),
            outcome: ReconciliationResolution::VendorBilled,
        },
    );
    assert!(repo.append_lifecycle_event(&contradicted).is_err());
    let pending = Fixture::seed(&mut repo, Effect::NeedsApproval);
    let false_admitted = pending.event(
        LifecycleSource::AutomatedAdmission,
        LifecycleFact::AutomatedAdmission {
            result: AutomatedAdmissionOutcome::Admitted,
        },
    );
    assert!(repo.append_lifecycle_event(&false_admitted).is_err());
    let unconfirmed = pending.event(
        LifecycleSource::HumanApproval,
        LifecycleFact::HumanResolutionConfirmed {
            approval_request_id: pending.decision.clone(),
            resolution: HumanResolution::Approved,
            confirmed_by_user_id: f.owner.clone(),
        },
    );
    assert!(repo.append_lifecycle_event(&unconfirmed).is_err());
    let denied = Fixture::seed(&mut repo, Effect::Allow);
    let decision = repo
        .load_spend_decisions()
        .unwrap()
        .into_iter()
        .find(|d| d.id == denied.decision)
        .unwrap();
    repo.record_spend_attempt_outcome(
        &decision,
        SpendAuthorizationDecision::Denied,
        &[],
        Utc::now(),
    )
    .unwrap();
    let contradicted = denied.event(
        LifecycleSource::AutomatedAdmission,
        LifecycleFact::AutomatedAdmission {
            result: AutomatedAdmissionOutcome::Admitted,
        },
    );
    assert!(repo.append_lifecycle_event(&contradicted).is_err());
}

#[test]
fn corrected_revisions_preserve_denials_and_conflicting_replay_does_not_mutate_facts() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::seed(&mut repo, Effect::Deny);
    let original = f.policy(InitialPolicyDecision::Deny);
    let first = repo.append_lifecycle_event(&original).unwrap();
    let mut decision = repo.load_spend_decisions().unwrap().remove(0);
    repo.record_spend_attempt_outcome(
        &decision,
        SpendAuthorizationDecision::Denied,
        &[],
        Utc::now(),
    )
    .unwrap();
    let mut corrected = decision.request.clone();
    corrected.amount = Amount::from_units(4_000);
    assert_eq!(
        repo.admit_spend_attempt(
            &f.owner,
            &f.key,
            &corrected,
            "fixture:correction",
            Utc::now()
        )
        .unwrap(),
        SpendAttemptAdmission::Admitted { revision: 2 }
    );
    decision.id = SpendDecisionId::new();
    decision.revision = 2;
    decision.request = corrected;
    decision.evaluation.decision = Effect::Allow;
    repo.save_spend_decision(&decision).unwrap();
    let mut second = f.policy(InitialPolicyDecision::Allow);
    second.correlation.decision_id = Some(decision.id);
    second.correlation.attempt_revision = 2;
    second.correlation.correction_of_revision = Some(1);
    second.requested_amount = LifecycleEvidence::Known {
        value: money(4_000),
    };
    repo.append_lifecycle_event(&second).unwrap();
    assert_eq!(
        repo.append_lifecycle_event(&original).unwrap().record,
        first.record
    );
    let mut changed_time = original.clone();
    changed_time.occurred_at += chrono::Duration::nanoseconds(1);
    let error = repo
        .append_lifecycle_event(&changed_time)
        .unwrap_err()
        .to_string();
    assert!(error.contains("conflicting lifecycle"));
    assert!(!error.contains(&f.key));
    let snapshot = repo.lifecycle_snapshot(&f.owner, None).unwrap();
    assert_eq!(snapshot.events.len(), 2);
    assert_eq!(
        snapshot.events[0].event.fact.count_category(),
        LifecycleCount::PolicyDeny
    );
    assert_eq!(
        snapshot.events[1].event.correlation.correction_of_revision,
        Some(1)
    );
}

#[test]
fn separate_connection_exact_replays_converge_on_one_identity_and_first_recorded_time() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("concurrent.sqlite");
    let mut seed = SqliteGovernanceRepository::open(&path).unwrap();
    let f = Fixture::seed(&mut seed, Effect::Allow);
    let input = f.policy(InitialPolicyDecision::Allow);
    drop(seed);
    let left = SqliteGovernanceRepository::open(&path).unwrap();
    let right = SqliteGovernanceRepository::open(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for mut repo in [left, right] {
        let barrier = barrier.clone();
        let input = input.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            repo.append_lifecycle_event(&input).unwrap()
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results[0].record, results[1].record);
    assert_eq!(results.iter().filter(|r| r.idempotent_replay).count(), 1);
    let reopened = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(
        reopened
            .lifecycle_snapshot(&f.owner, None)
            .unwrap()
            .events
            .len(),
        1
    );
}
