use super::*;
use crate::app::{
    AuthorizeSpendRequest, HumanApprovalDecision, SpendApprovalService, SpendAuthorizationOutcome,
};
use crate::budget::{BudgetManager, CreateSingleBudgetRequest};
use crate::spend::{LifecycleCount, SpendManager};
use hubu_common::{ids::AgentAccountId, models::UserContext};

struct Harness {
    repo: SqliteGovernanceRepository,
    spend: SpendManager,
    budgets: BudgetManager,
    request: AuthorizeSpendRequest,
    selected: automatic_lifecycle::SelectedSpendPolicy,
}
impl Harness {
    fn new(limit: Option<&str>) -> Self {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let account = AgentAccountId::new();
        seed_lifecycle_test_identity(&mut repo, &owner, &agent, &account);
        repo.save_policy_assignment(
            &owner,
            &PolicyAssignmentScope::UserDefault,
            &Policy {
                id: "original-authored-policy".into(),
                version: "same-label".into(),
                owner_user_id: owner.clone(),
                default_effect: Effect::NeedsApproval,
                rules: vec![],
            },
        )
        .unwrap();
        let selected = repo.load_policy_assignments().unwrap().remove(0).into();
        let mut budgets = BudgetManager::new();
        if let Some(limit) = limit {
            let b = budgets
                .create_single_budget(CreateSingleBudgetRequest {
                    agent_id: agent.clone(),
                    amount_limit: limit.parse().unwrap(),
                    currency: Currency::Usd,
                    period: TimePeriod::new(Utc::now() - chrono::Duration::days(1), None).unwrap(),
                })
                .unwrap();
            repo.save_budget_with_balance(&b.budget, &b.version, &b.balance)
                .unwrap();
        }
        Self {
            repo,
            spend: SpendManager::new(),
            budgets,
            selected,
            request: AuthorizeSpendRequest {
                operation_key: "private-human-operation".into(),
                user: UserContext::new(owner),
                agent_id: agent,
                agent_account_id: account,
                amount: "0.005".parse().unwrap(),
                currency: Currency::Usd,
                merchant: None,
                execution_scope: None,
                task_id: None,
                reason: "private prompt".into(),
                lease_profile: "default".into(),
            },
        }
    }
    fn pending(&mut self) -> SpendDecisionRecord {
        assert!(matches!(
            SpendApprovalService
                .authorize_selected_at(
                    self.request.clone(),
                    &self.selected,
                    &mut self.spend,
                    &mut self.budgets,
                    &mut self.repo,
                    Utc::now(),
                )
                .unwrap(),
            SpendAuthorizationOutcome::Rejected(_)
        ));
        self.repo.load_spend_decisions().unwrap().remove(0)
    }
    fn resolve(
        &mut self,
        id: &hubu_common::ids::SpendDecisionId,
        decision: HumanApprovalDecision,
    ) -> Result<SpendAuthorizationOutcome, crate::app::SpendApprovalError> {
        SpendApprovalService.resolve_human_approval_at(
            self.request.user.clone(),
            id,
            decision,
            &mut self.spend,
            &mut self.budgets,
            &mut self.repo,
            Utc::now(),
        )
    }
    fn snapshot(&self) -> LifecycleSnapshot {
        self.repo
            .lifecycle_snapshot(&self.request.user.user_id, None)
            .unwrap()
    }
    fn hydrate(&mut self) {
        self.spend = SpendManager::from_records(
            self.repo.load_spend_decisions().unwrap(),
            self.repo.load_spend_auth_tokens().unwrap(),
        );
        self.budgets = BudgetManager::from_records(
            self.repo.load_budgets().unwrap(),
            self.repo.load_budget_versions().unwrap(),
            self.repo.load_budget_balances().unwrap(),
            self.repo.load_budget_holds().unwrap(),
        )
        .unwrap();
    }
}
fn balance_state(balance: &BudgetBalance) -> (BudgetId, Amount, Amount, Amount) {
    (
        balance.budget_id.clone(),
        balance.consumed_amount,
        balance.frozen_amount,
        balance.remaining_amount,
    )
}
fn value<T>(e: &LifecycleEvidence<T>) -> &T {
    match e {
        LifecycleEvidence::Known { value } => value,
        _ => panic!("known evidence expected"),
    }
}

#[test]
fn genuine_pending_and_both_confirmed_branches_preserve_original_policy() {
    for action in [HumanApprovalDecision::Approve, HumanApprovalDecision::Deny] {
        let mut h = Harness::new(Some("0.010"));
        let decision = h.pending();
        let pending = h.snapshot();
        assert_eq!(
            pending
                .events
                .iter()
                .map(|e| e.event.fact.count_category())
                .collect::<Vec<_>>(),
            vec![
                LifecycleCount::PolicyNeedsApproval,
                LifecycleCount::HumanEscalation
            ]
        );
        assert_eq!(pending.events[0].event.occurred_at, decision.created_at);
        assert_eq!(pending.events[1].event.occurred_at, decision.created_at);
        assert_eq!(pending.capture_markers.len(), 3);
        // A later policy assignment does not replace the evaluated original.
        h.repo
            .save_policy_assignment(
                &h.request.user.user_id,
                &PolicyAssignmentScope::UserDefault,
                &Policy {
                    id: "different-current-policy".into(),
                    version: "later".into(),
                    owner_user_id: h.request.user.user_id.clone(),
                    default_effect: Effect::Deny,
                    rules: vec![],
                },
            )
            .unwrap();
        h.resolve(&decision.id, action).unwrap();
        let confirmed = h.snapshot();
        assert_eq!(&confirmed.events[..2], pending.events.as_slice());
        assert_eq!(
            confirmed.events.len(),
            if action == HumanApprovalDecision::Approve {
                4
            } else {
                3
            }
        );
        for event in &confirmed.events[2..] {
            assert_eq!(event.event.source, LifecycleSource::HumanApproval);
            assert_eq!(
                event.event.explanation.policy,
                pending.events[0].event.explanation.policy
            );
            assert!(event.event.occurred_at >= decision.created_at);
        }
        assert_eq!(
            h.repo.load_spend_decisions().unwrap()[0]
                .evaluation
                .decision,
            Effect::NeedsApproval
        );
        h.hydrate();
        h.resolve(&decision.id, action).unwrap();
        assert_eq!(h.snapshot().events, confirmed.events);
        assert!(h
            .resolve(
                &decision.id,
                if action == HumanApprovalDecision::Approve {
                    HumanApprovalDecision::Deny
                } else {
                    HumanApprovalDecision::Approve
                }
            )
            .is_err());
        assert_eq!(h.snapshot().events, confirmed.events);
    }
}

#[test]
fn approval_records_actual_reservation_version_and_preimage() {
    let mut h = Harness::new(Some("0.010"));
    let decision = h.pending();
    let b = h
        .budgets
        .get_budgets_by_agent_id(&h.request.agent_id)
        .remove(0);
    h.repo
        .append_budget_version(&AppendBudgetVersionRequest {
            budget_id: b.budget.id,
            amount_limit: "0.006".parse().unwrap(),
            expected_revision: 1,
            actor: "test".into(),
            source: "test".into(),
            reason: None,
            effective_at: Utc::now(),
        })
        .unwrap();
    h.hydrate();
    h.resolve(&decision.id, HumanApprovalDecision::Approve)
        .unwrap();
    let snapshot = h.snapshot();
    let budget = value(&snapshot.events[2].event.explanation.budget);
    assert_eq!(value(&budget.limit).amount, "0.006".parse().unwrap());
    assert_eq!(value(&budget.available).amount, "0.006".parse().unwrap());
    assert_eq!(value(&budget.frozen).amount, Amount::ZERO);
    let hold = h.repo.load_budget_holds().unwrap().remove(0);
    assert_eq!(
        budget.budget_version_id,
        format!("bgv_{}", hold.budget_version_id.public_suffix())
    );
}

#[test]
fn missing_budget_and_stale_revocation_leave_pending_without_provisional_state() {
    for stale in [false, true] {
        let mut h = Harness::new(if stale { Some("0.010") } else { None });
        let decision = h.pending();
        let pending = h.snapshot();
        if stale {
            h.repo
                .conn
                .execute("UPDATE budgets SET administrative_state='revoked'", [])
                .unwrap();
        }
        assert!(h
            .resolve(&decision.id, HumanApprovalDecision::Approve)
            .is_err());
        assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
        assert!(h.repo.load_budget_holds().unwrap().is_empty());
        assert!(h
            .spend
            .evaluate_spend(
                &h.request.user,
                &decision.operation_key,
                decision.request.clone(),
                h.selected.policy()
            )
            .unwrap()
            .auth_token
            .is_none());
        assert!(h
            .budgets
            .get_budget_hold_by_spend_decision(&decision.id)
            .is_none());
        assert_eq!(h.snapshot().events, pending.events);
        assert_eq!(
            h.repo
                .load_spend_attempt_history(&h.request.agent_id, &h.request.operation_key)
                .unwrap()[0]
                .final_decision,
            SpendAuthorizationDecision::PendingApproval
        );
    }
}

#[test]
fn legacy_confirmed_resolution_does_not_backfill_missing_escalation() {
    let mut h = Harness::new(Some("0.010"));
    let request = SpendRequest {
        amount: h.request.amount,
        currency: h.request.currency,
        owner_user_id: h.request.user.user_id.clone(),
        agent_id: h.request.agent_id.clone(),
        agent_account_id: h.request.agent_account_id.clone(),
        merchant: None,
        execution_scope: None,
        category: None,
        task_id: None,
        reason: h.request.reason.clone(),
        lease_profile: "default".into(),
    };
    h.repo
        .admit_spend_attempt(
            &request.owner_user_id,
            &h.request.operation_key,
            &request,
            "test",
            Utc::now() - chrono::Duration::hours(2),
        )
        .unwrap();
    let record = SpendDecisionRecord {
        id: hubu_common::ids::SpendDecisionId::new(),
        owner_user_id: request.owner_user_id.clone(),
        operation_key: h.request.operation_key.clone(),
        revision: 1,
        actor: "test".into(),
        evaluation: crate::policy::evaluate_policy(&request, h.selected.policy()).unwrap(),
        request,
        created_at: Utc::now() - chrono::Duration::hours(2),
    };
    h.repo.save_spend_decision(&record).unwrap();
    h.hydrate();
    h.resolve(&record.id, HumanApprovalDecision::Approve)
        .unwrap();
    let s = h.snapshot();
    assert_eq!(
        s.events
            .iter()
            .map(|e| e.event.fact.count_category())
            .collect::<Vec<_>>(),
        vec![
            LifecycleCount::HumanApproved,
            LifecycleCount::AuthorizationGranted
        ]
    );
    assert!(s.events.iter().all(|e| matches!(
        e.event.explanation.policy,
        LifecycleEvidence::Unavailable {
            reason: EvidenceUnavailableReason::NotCaptured
        }
    )));
    assert_eq!(s.capture_markers.len(), 1);
}

#[test]
fn evaluation_placeholder_and_unconfirmed_prompt_have_no_human_facts() {
    let mut h = Harness::new(Some("0.010"));
    let request = SpendRequest {
        amount: h.request.amount,
        currency: h.request.currency,
        owner_user_id: h.request.user.user_id.clone(),
        agent_id: h.request.agent_id.clone(),
        agent_account_id: h.request.agent_account_id.clone(),
        merchant: None,
        execution_scope: None,
        category: None,
        task_id: None,
        reason: h.request.reason.clone(),
        lease_profile: "default".into(),
    };
    h.repo
        .admit_spend_attempt(
            &request.owner_user_id,
            &h.request.operation_key,
            &request,
            "test",
            Utc::now(),
        )
        .unwrap();
    assert!(h.snapshot().events.is_empty());
    assert!(h.snapshot().capture_markers.is_empty());
    // An actual pending request with no accepted human action has only its open.
    let mut h = Harness::new(Some("0.010"));
    h.pending();
    let s = h.snapshot();
    assert_eq!(s.events.len(), 2);
    assert!(!s
        .events
        .iter()
        .any(|e| matches!(e.event.fact, LifecycleFact::HumanResolutionConfirmed { .. })));
}

#[test]
fn grant_fact_failure_rolls_back_confirmation_accounting_and_memory_then_retries() {
    let mut h = Harness::new(Some("0.010"));
    let decision = h.pending();
    let pending = h.snapshot();
    let before = h
        .budgets
        .get_budgets_by_agent_id(&h.request.agent_id)
        .remove(0);
    h.repo.conn.execute_batch("CREATE TRIGGER fail_human_grant BEFORE INSERT ON spend_lifecycle_records WHEN json_extract(NEW.payload_json,'$.input.source')='human_approval' AND json_extract(NEW.payload_json,'$.input.fact.kind')='authorization_granted' BEGIN SELECT RAISE(ABORT,'test grant failure'); END;").unwrap();
    assert!(h
        .resolve(&decision.id, HumanApprovalDecision::Approve)
        .is_err());
    assert_eq!(h.snapshot().events, pending.events);
    assert!(h.repo.load_budget_holds().unwrap().is_empty());
    assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
    assert_eq!(
        balance_state(
            &h.budgets
                .get_budget_by_id(&before.budget.id)
                .unwrap()
                .balance
        ),
        balance_state(&before.balance)
    );
    assert!(h
        .budgets
        .get_budget_hold_by_spend_decision(&decision.id)
        .is_none());
    assert!(h
        .spend
        .evaluate_spend(
            &h.request.user,
            &decision.operation_key,
            decision.request.clone(),
            h.selected.policy()
        )
        .unwrap()
        .auth_token
        .is_none());
    assert_eq!(
        h.repo
            .load_spend_attempt_history(&h.request.agent_id, &h.request.operation_key)
            .unwrap()[0]
            .final_decision,
        SpendAuthorizationDecision::PendingApproval
    );
    h.repo
        .conn
        .execute_batch("DROP TRIGGER fail_human_grant;")
        .unwrap();
    h.resolve(&decision.id, HumanApprovalDecision::Approve)
        .unwrap();
    assert_eq!(h.snapshot().events.len(), 4);
    assert_eq!(h.repo.load_budget_holds().unwrap().len(), 1);
}

#[test]
fn escalation_fact_failure_rolls_back_real_decision_and_capture_markers() {
    let mut h = Harness::new(Some("0.010"));
    h.repo.conn.execute_batch("CREATE TRIGGER fail_escalation BEFORE INSERT ON spend_lifecycle_records WHEN json_extract(NEW.payload_json,'$.input.fact.kind')='human_escalation' BEGIN SELECT RAISE(ABORT,'test escalation failure'); END;").unwrap();
    assert!(SpendApprovalService
        .authorize_selected_at(
            h.request.clone(),
            &h.selected,
            &mut h.spend,
            &mut h.budgets,
            &mut h.repo,
            Utc::now()
        )
        .is_err());
    assert!(h.repo.load_spend_decisions().unwrap().is_empty());
    assert!(h.repo.load_budget_holds().unwrap().is_empty());
    assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
    assert!(h.snapshot().events.is_empty());
    assert!(h.snapshot().capture_markers.is_empty());
    // The admitted evaluation placeholder remains pending, never an escalation.
    assert_eq!(
        h.repo
            .load_spend_attempt_history(&h.request.agent_id, &h.request.operation_key)
            .unwrap()[0]
            .final_decision,
        SpendAuthorizationDecision::PendingApproval
    );
}

#[test]
fn stale_budget_version_and_balance_cannot_publish_a_human_grant() {
    for change_version in [false, true] {
        let mut h = Harness::new(Some("0.010"));
        let decision = h.pending();
        let pending = h.snapshot();
        let b = h
            .budgets
            .get_budgets_by_agent_id(&h.request.agent_id)
            .remove(0);
        if change_version {
            h.repo
                .append_budget_version(&AppendBudgetVersionRequest {
                    budget_id: b.budget.id.clone(),
                    amount_limit: "0.011".parse().unwrap(),
                    expected_revision: 1,
                    actor: "test".into(),
                    source: "test".into(),
                    reason: None,
                    effective_at: Utc::now(),
                })
                .unwrap();
        } else {
            h.repo.conn.execute("UPDATE budget_balances SET consumed_amount=1, remaining_amount=9999 WHERE budget_id=?1",[b.budget.id.to_string()]).unwrap();
        }
        assert!(h
            .resolve(&decision.id, HumanApprovalDecision::Approve)
            .is_err());
        assert_eq!(h.snapshot().events, pending.events);
        assert!(h.repo.load_budget_holds().unwrap().is_empty());
        assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
        assert!(h
            .budgets
            .get_budget_hold_by_spend_decision(&decision.id)
            .is_none());
        assert_eq!(
            balance_state(&h.budgets.get_budget_by_id(&b.budget.id).unwrap().balance),
            balance_state(&b.balance)
        );
    }
}

#[test]
fn stale_pending_writer_cannot_overwrite_a_confirmed_human_resolution() {
    let mut h = Harness::new(Some("0.010"));
    let decision = h.pending();
    h.resolve(&decision.id, HumanApprovalDecision::Deny)
        .unwrap();
    let confirmed = h.snapshot();
    let before = h
        .budgets
        .get_budgets_by_agent_id(&h.request.agent_id)
        .remove(0);
    // Simulate a second writer that read pending before the first writer's
    // commit and now tries to persist its provisional approval.
    let token = h
        .spend
        .issue_auth_token_for_approved_decision(&decision.id)
        .unwrap();
    let token_record = h.spend.auth_token_record(&token.id).unwrap();
    let reservation = h
        .budgets
        .reserve_budget_at(
            crate::budget::ReserveBudgetRequest {
                budget_id: before.budget.id.clone(),
                spend_decision_id: decision.id.clone(),
                amount: decision.request.amount,
                currency: decision.request.currency,
                expires_at: token.expires_at,
            },
            Utc::now(),
        )
        .unwrap();
    assert!(save_approved(
        &mut h.repo,
        &decision,
        &h.request.user.user_id,
        &token_record,
        &reservation.hold,
        &reservation.balance,
        &before,
        &["stale approval".into()],
        Utc::now(),
        Utc::now()
    )
    .is_err());
    assert_eq!(h.snapshot().events, confirmed.events);
    assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
    assert!(h.repo.load_budget_holds().unwrap().is_empty());
    assert!(save_declined(
        &mut h.repo,
        &decision,
        &h.request.user.user_id,
        &["duplicate stale decline".into()],
        Utc::now()
    )
    .is_err());
    assert_eq!(h.snapshot().events, confirmed.events);
    assert_eq!(
        h.repo
            .load_spend_attempt_history(&h.request.agent_id, &h.request.operation_key)
            .unwrap()[0]
            .final_decision,
        SpendAuthorizationDecision::Denied
    );
}
