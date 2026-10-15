use super::*;
use crate::app::{
    AuthorizeSpendRequest, SpendApprovalError, SpendApprovalService, SpendAuthorizationOutcome,
};
use crate::budget::{BudgetManager, CreateSingleBudgetRequest};
use crate::spend::{LifecycleCount, SpendManager};
use hubu_common::{ids::AgentAccountId, models::UserContext, time::TimePeriod};

struct Harness {
    repo: SqliteGovernanceRepository,
    spend: SpendManager,
    budgets: BudgetManager,
    request: AuthorizeSpendRequest,
    selected: SelectedSpendPolicy,
}
impl Harness {
    fn new(limit: Option<&str>, effect: Effect) -> Self {
        Self::with_repo(
            SqliteGovernanceRepository::in_memory().unwrap(),
            limit,
            effect,
        )
    }
    fn with_repo(repo: SqliteGovernanceRepository, limit: Option<&str>, effect: Effect) -> Self {
        Self::with_repo_currency(repo, limit, effect, Currency::Usd)
    }
    fn new_currency(limit: Option<&str>, effect: Effect, currency: Currency) -> Self {
        Self::with_repo_currency(
            SqliteGovernanceRepository::in_memory().unwrap(),
            limit,
            effect,
            currency,
        )
    }
    fn with_repo_currency(
        mut repo: SqliteGovernanceRepository,
        limit: Option<&str>,
        effect: Effect,
        currency: Currency,
    ) -> Self {
        let owner = UserId::new();
        let agent = AgentId::new();
        let account = AgentAccountId::new();
        seed_lifecycle_test_identity(&mut repo, &owner, &agent, &account);
        let policy = Policy {
            id: "reused-authored-label".into(),
            version: "same-version".into(),
            owner_user_id: owner.clone(),
            default_effect: effect,
            rules: vec![],
        };
        repo.save_policy_assignment(&owner, &PolicyAssignmentScope::UserDefault, &policy)
            .unwrap();
        let selected = repo.load_policy_assignments().unwrap().remove(0).into();
        let mut budgets = BudgetManager::new();
        if let Some(limit) = limit {
            let b = budgets
                .create_single_budget(CreateSingleBudgetRequest {
                    agent_id: agent.clone(),
                    amount_limit: limit.parse().unwrap(),
                    currency,
                    period: TimePeriod::new(
                        Utc::now() - chrono::Duration::minutes(1),
                        Some(Utc::now() + chrono::Duration::hours(1)),
                    )
                    .unwrap(),
                })
                .unwrap();
            repo.save_budget_with_balance(&b.budget, &b.version, &b.balance)
                .unwrap();
        }
        let request = AuthorizeSpendRequest {
            operation_key: "private-operation".into(),
            user: UserContext::new(owner),
            agent_id: agent,
            agent_account_id: account,
            amount: "0.005".parse().unwrap(),
            currency,
            merchant: Some("private merchant".into()),
            execution_scope: None,
            task_id: Some("private task".into()),
            reason: "private prompt".into(),
            lease_profile: "default".into(),
        };
        Self {
            repo,
            spend: SpendManager::new(),
            budgets,
            request,
            selected,
        }
    }
    fn add_budget(&mut self, currency: Currency, limit: &str) -> BudgetWithBalance {
        let b = self
            .budgets
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: self.request.agent_id.clone(),
                amount_limit: limit.parse().unwrap(),
                currency,
                period: TimePeriod::new(
                    Utc::now() - chrono::Duration::minutes(1),
                    Some(Utc::now() + chrono::Duration::hours(1)),
                )
                .unwrap(),
            })
            .unwrap();
        self.repo
            .save_budget_with_balance(&b.budget, &b.version, &b.balance)
            .unwrap();
        self.budgets.get_budget_by_id(&b.budget.id).unwrap()
    }
    fn authorize(&mut self) -> Result<SpendAuthorizationOutcome, SpendApprovalError> {
        SpendApprovalService.authorize_selected_at(
            self.request.clone(),
            &self.selected,
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
    fn categories(&self) -> Vec<LifecycleCount> {
        self.snapshot()
            .events
            .iter()
            .map(|e| e.event.fact.count_category())
            .collect()
    }
    fn set_policy(&mut self, policy: Policy, revision: u64) {
        self.repo
            .apply_policy(
                &self.request.user.user_id,
                &policy.id,
                &policy.id,
                &PolicyAssignmentScope::UserDefault,
                &policy,
                Some(revision),
                None,
                "test",
                "test",
            )
            .unwrap();
        self.selected = self
            .repo
            .load_policy_assignments()
            .unwrap()
            .remove(0)
            .into();
    }
}
fn value<T>(e: &LifecycleEvidence<T>) -> &T {
    match e {
        LifecycleEvidence::Known { value } => value,
        _ => panic!("expected known evidence"),
    }
}

#[test]
fn automatic_allow_captures_pre_reservation_exact_values_and_replay_is_inert() {
    let mut h = Harness::new(Some("0.010"), Effect::Allow);
    let first = h.authorize().unwrap();
    assert!(matches!(first, SpendAuthorizationOutcome::Approved(_)));
    assert_eq!(
        h.categories(),
        vec![
            LifecycleCount::PolicyAllow,
            LifecycleCount::AdmissionAllowed,
            LifecycleCount::AuthorizationGranted
        ]
    );
    let first = h.snapshot();
    assert_eq!(first.capture_markers.len(), 2);
    assert!(first
        .events
        .windows(2)
        .all(|pair| pair[0].event.occurred_at <= pair[1].event.occurred_at));
    assert!(first.capture_markers.iter().all(|m| matches!(
        m.source,
        LifecycleSource::PolicyEngine | LifecycleSource::AutomatedAdmission
    )));
    let budget = value(&first.events[1].event.explanation.budget);
    assert_eq!(value(&budget.available).amount, "0.010".parse().unwrap());
    assert_eq!(value(&budget.frozen).amount, Amount::ZERO);
    assert_eq!(budget.requested.amount, "0.005".parse().unwrap());
    let hold = h.repo.load_budget_holds().unwrap().remove(0);
    assert_eq!(
        budget.budget_version_id,
        format!("bgv_{}", hold.budget_version_id.public_suffix())
    );
    h.authorize().unwrap();
    let replay = h.snapshot();
    assert_eq!(replay.events, first.events);
    assert_eq!(replay.capture_markers, first.capture_markers);
    // Later mutable configuration cannot rewrite captured pre-images.
    let policy = Policy {
        default_effect: Effect::Deny,
        ..h.selected.policy.clone()
    };
    h.set_policy(policy, 1);
    h.repo
        .append_budget_version(&AppendBudgetVersionRequest {
            budget_id: hold.budget_id.clone(),
            amount_limit: "0.011".parse().unwrap(),
            expected_revision: 1,
            actor: "test".into(),
            source: "test".into(),
            reason: None,
            effective_at: Utc::now(),
        })
        .unwrap();
    h.budgets = BudgetManager::from_records(
        h.repo.load_budgets().unwrap(),
        h.repo.load_budget_versions().unwrap(),
        h.repo.load_budget_balances().unwrap(),
        h.repo.load_budget_holds().unwrap(),
    )
    .unwrap();
    let decision = h.repo.load_spend_decisions().unwrap().remove(0);
    assert_eq!(decision.evaluation.decision, Effect::Allow);
    h.authorize().unwrap();
    assert_eq!(h.snapshot().events, first.events);
}

#[test]
fn automatic_denial_causes_and_genuine_initial_needs_approval_are_distinct() {
    for (limit, effect, cause) in [
        (
            Some("0.010"),
            Effect::Deny,
            Some(AdmissionDenialReason::Policy),
        ),
        (
            None,
            Effect::Allow,
            Some(AdmissionDenialReason::NoAvailableBudget),
        ),
        (
            Some("0.004"),
            Effect::Allow,
            Some(AdmissionDenialReason::InsufficientBudget),
        ),
        (Some("0.010"), Effect::NeedsApproval, None),
    ] {
        let mut h = Harness::new(limit, effect);
        h.authorize().unwrap();
        let s = h.snapshot();
        assert_eq!(s.events.len(), 2);
        assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
        assert!(h.repo.load_budget_holds().unwrap().is_empty());
        if let Some(cause) = cause {
            assert_eq!(
                s.events[1].event.fact,
                LifecycleFact::AutomatedAdmission {
                    result: AutomatedAdmissionOutcome::Denied { reason: cause }
                }
            );
        }
        if cause == Some(AdmissionDenialReason::InsufficientBudget) {
            assert_eq!(
                value(&value(&s.events[1].event.explanation.budget).available).amount,
                "0.004".parse().unwrap()
            );
        }
        h.authorize().unwrap();
        assert_eq!(h.snapshot().events, s.events);
    }
}

#[test]
fn actual_selected_policy_revision_survives_reused_labels_and_later_changes() {
    let mut h = Harness::new(Some("0.010"), Effect::Allow);
    // The selected cache snapshot is intentionally old while DB now points to a
    // new payload with the same human-authored id/version.
    let old = h.selected.clone();
    let revised = Policy {
        default_effect: Effect::Deny,
        ..old.policy.clone()
    };
    h.set_policy(revised, 1);
    h.selected = old.clone();
    h.authorize().unwrap();
    let evidence = value(&h.snapshot().events[0].event.explanation.policy).clone();
    assert_eq!(evidence.policy_id, old.policy_id);
    assert_eq!(evidence.revision, 1);
    assert_eq!(evidence.payload_hash, old.payload_hash);
    assert_eq!(h.categories()[0], LifecycleCount::PolicyAllow);
}

#[test]
fn winning_rule_amount_evidence_uses_safe_ast_and_exact_subcent_values() {
    let mut h = Harness::new(Some("0.010"), Effect::Allow);
    let policy = Policy {
        rules: vec![crate::policy::Rule {
            id: "deny-over-limit".into(),
            effect: Effect::Deny,
            when: Condition::All {
                conditions: vec![
                    Condition::Gt {
                        field: Field::Amount,
                        value: PolicyValue::Money("0.004".parse().unwrap()),
                    },
                    Condition::Exists {
                        field: Field::Merchant,
                    },
                ],
            },
            reason: "private rule prose".into(),
        }],
        ..h.selected.policy.clone()
    };
    h.set_policy(policy, 1);
    h.authorize().unwrap();
    let s = h.snapshot();
    let p = value(&s.events[0].event.explanation.policy);
    assert_eq!(p.decisive_rule_ids, vec!["deny-over-limit"]);
    assert_eq!(p.amount_constraints.len(), 1);
    assert_eq!(
        p.amount_constraints[0].configured.amount,
        "0.004".parse().unwrap()
    );
    assert_eq!(
        value(&p.amount_constraints[0].evaluated).amount,
        "0.005".parse().unwrap()
    );
    let json = serde_json::to_string(&s.events[0].event).unwrap();
    assert!(!json.contains("private rule prose"));
    assert!(!json.contains("private prompt"));
}

#[test]
fn event_failure_rolls_back_initial_decision_and_never_leaves_usable_token() {
    let mut h = Harness::new(Some("0.010"), Effect::Allow);
    h.repo.conn.execute_batch("CREATE TRIGGER fail_evidence BEFORE INSERT ON spend_lifecycle_records BEGIN SELECT RAISE(ABORT,'fail evidence'); END;").unwrap();
    assert!(h.authorize().is_err());
    assert!(h.repo.load_spend_decisions().unwrap().is_empty());
    assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
    assert!(h.repo.load_budget_holds().unwrap().is_empty());
    assert!(h.snapshot().events.is_empty());
    assert!(h.snapshot().capture_markers.is_empty());
    assert!(!h.spend.has_decision_for_scope(
        &h.request.agent_id,
        &h.request.operation_key,
        &SpendRequest {
            amount: h.request.amount,
            currency: h.request.currency,
            owner_user_id: h.request.user.user_id.clone(),
            agent_id: h.request.agent_id.clone(),
            agent_account_id: h.request.agent_account_id.clone(),
            merchant: h.request.merchant.clone(),
            execution_scope: None,
            category: None,
            task_id: h.request.task_id.clone(),
            reason: h.request.reason.clone(),
            lease_profile: h.request.lease_profile.clone()
        }
    ));
}

#[test]
fn terminal_fact_failure_rolls_back_grant_or_denial_and_replay_stays_pending() {
    for (effect, failed_kind) in [
        (Effect::Allow, "automated_admission"),
        (Effect::Allow, "authorization_granted"),
        (Effect::Deny, "automated_admission"),
    ] {
        let mut h = Harness::new(Some("0.010"), effect);
        h.repo.conn.execute_batch(&format!("CREATE TRIGGER fail_admission BEFORE INSERT ON spend_lifecycle_records WHEN json_extract(NEW.payload_json,'$.input.fact.kind')='{failed_kind}' BEGIN SELECT RAISE(ABORT,'fail admission'); END;")).unwrap();
        assert!(h.authorize().is_err());
        assert_eq!(h.repo.load_spend_decisions().unwrap().len(), 1);
        assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
        assert!(h.repo.load_budget_holds().unwrap().is_empty());
        let history = h
            .repo
            .load_spend_attempt_history(&h.request.agent_id, &h.request.operation_key)
            .unwrap();
        assert_eq!(
            history[0].final_decision,
            SpendAuthorizationDecision::PendingApproval
        );
        let b = h
            .budgets
            .get_budgets_by_agent_id(&h.request.agent_id)
            .remove(0);
        assert_eq!(b.balance.remaining_amount, "0.010".parse().unwrap());
        assert_eq!(b.balance.frozen_amount, Amount::ZERO);
        assert!(matches!(
            h.authorize(),
            Err(SpendApprovalError::PendingExactReplay { .. })
        ));
        assert!(!h.categories().contains(&LifecycleCount::AdmissionDenied));
        assert!(!h
            .categories()
            .contains(&LifecycleCount::AuthorizationGranted));
    }
}

#[test]
fn concurrent_version_revocation_and_same_version_balance_changes_reject_stale_capture() {
    for mutation in [0, 1, 2] {
        let mut h = Harness::new(Some("0.010"), Effect::Allow);
        let b = h
            .budgets
            .get_budgets_by_agent_id(&h.request.agent_id)
            .remove(0);
        match mutation {
            0 => {
                h.repo.revoke_budget(&b.budget.id, Utc::now()).unwrap();
            }
            1 => {
                h.repo.conn.execute("UPDATE budget_balances SET consumed_amount=1000,remaining_amount=9000 WHERE budget_id=?1",[b.budget.id.to_string()]).unwrap();
            }
            _ => {
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
            }
        }
        assert!(h.authorize().is_err());
        assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
        assert!(h.repo.load_budget_holds().unwrap().is_empty());
        assert!(!h
            .categories()
            .contains(&LifecycleCount::AuthorizationGranted));
    }
}

#[test]
fn evaluation_error_is_one_technical_observation_and_never_a_policy_decision() {
    let mut h = Harness::new(Some("0.010"), Effect::Allow);
    let mut invalid = h.selected.policy.clone();
    invalid.rules.push(crate::policy::Rule {
        id: "bad".into(),
        effect: Effect::NeedsApproval,
        when: Condition::Gt {
            field: Field::Amount,
            value: PolicyValue::String("bad type".into()),
        },
        reason: "invalid".into(),
    });
    assert!(SpendApprovalService
        .authorize_at(
            h.request.clone(),
            &invalid,
            &mut h.spend,
            &mut h.budgets,
            &mut h.repo,
            Utc::now()
        )
        .is_err());
    assert_eq!(h.categories(), vec![LifecycleCount::AdmissionFailed]);
    let first = h.snapshot();
    assert!(h.repo.load_spend_decisions().unwrap().is_empty());
    assert!(h.repo.load_spend_auth_tokens().unwrap().is_empty());
    assert!(matches!(
        h.authorize(),
        Err(SpendApprovalError::PendingExactReplay { .. })
    ));
    assert_eq!(h.snapshot().events, first.events);
}

#[test]
fn exhausted_current_allocation_preserves_zero_and_negative_available_without_reclassifying() {
    for remaining in [0, -1000] {
        let mut h = Harness::new(Some("0.010"), Effect::Allow);
        let b = h
            .budgets
            .get_budgets_by_agent_id(&h.request.agent_id)
            .remove(0);
        h.repo.conn.execute("UPDATE budget_balances SET consumed_amount=?1,remaining_amount=?2 WHERE budget_id=?3",params![10000-remaining,remaining,b.budget.id.to_string()]).unwrap();
        h.budgets = BudgetManager::from_records(
            h.repo.load_budgets().unwrap(),
            h.repo.load_budget_versions().unwrap(),
            h.repo.load_budget_balances().unwrap(),
            h.repo.load_budget_holds().unwrap(),
        )
        .unwrap();
        h.authorize().unwrap();
        let s = h.snapshot();
        assert_eq!(
            s.events[1].event.fact,
            LifecycleFact::AutomatedAdmission {
                result: AutomatedAdmissionOutcome::Denied {
                    reason: AdmissionDenialReason::NoAvailableBudget
                }
            }
        );
        assert_eq!(
            value(&value(&s.events[1].event.explanation.budget).available).amount,
            Amount::from_units(remaining)
        );
    }
}

#[test]
fn correction_revisions_are_distinct_and_old_exact_scope_replays_original_evidence() {
    let mut h = Harness::new(Some("0.010"), Effect::Allow);
    let policy = Policy {
        rules: vec![crate::policy::Rule {
            id: "deny-over".into(),
            effect: Effect::Deny,
            when: Condition::Gt {
                field: Field::Amount,
                value: PolicyValue::Money("0.004".parse().unwrap()),
            },
            reason: "deny".into(),
        }],
        ..h.selected.policy.clone()
    };
    h.set_policy(policy, 1);
    h.authorize().unwrap();
    let old = h.snapshot();
    h.request.amount = "0.003".parse().unwrap();
    h.authorize().unwrap();
    let current = h.snapshot();
    assert_eq!(current.events.len(), 5);
    assert_eq!(&current.events[..2], old.events.as_slice());
    assert!(current.events[2..]
        .iter()
        .all(|e| e.event.correlation.attempt_revision == 2
            && e.event.correlation.correction_of_revision == Some(1)));
    h.request.amount = "0.005".parse().unwrap();
    h.authorize().unwrap();
    assert_eq!(h.snapshot().events, current.events);
}

#[test]
fn restart_replay_preserves_fact_ids_times_and_owner_activation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.sqlite");
    let mut h = Harness::with_repo(
        SqliteGovernanceRepository::open(&path).unwrap(),
        Some("0.010"),
        Effect::Allow,
    );
    h.authorize().unwrap();
    let original = h.snapshot();
    let request = h.request.clone();
    drop(h);
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    let selected = repo.load_policy_assignments().unwrap().remove(0).into();
    let mut spend = SpendManager::from_records(
        repo.load_spend_decisions().unwrap(),
        repo.load_spend_auth_tokens().unwrap(),
    );
    let mut budgets = BudgetManager::from_records(
        repo.load_budgets().unwrap(),
        repo.load_budget_versions().unwrap(),
        repo.load_budget_balances().unwrap(),
        repo.load_budget_holds().unwrap(),
    )
    .unwrap();
    SpendApprovalService
        .authorize_selected_at(
            request.clone(),
            &selected,
            &mut spend,
            &mut budgets,
            &mut repo,
            Utc::now(),
        )
        .unwrap();
    let replay = repo
        .lifecycle_snapshot(&request.user.user_id, None)
        .unwrap();
    assert_eq!(replay.events, original.events);
    assert_eq!(replay.capture_markers, original.capture_markers);
}

#[test]
fn enabling_automatic_capture_never_invents_legacy_history() {
    let mut h = Harness::new(Some("0.010"), Effect::Deny);
    let request = SpendRequest {
        amount: h.request.amount,
        currency: h.request.currency,
        owner_user_id: h.request.user.user_id.clone(),
        agent_id: h.request.agent_id.clone(),
        agent_account_id: h.request.agent_account_id.clone(),
        merchant: h.request.merchant.clone(),
        execution_scope: None,
        category: None,
        task_id: h.request.task_id.clone(),
        reason: h.request.reason.clone(),
        lease_profile: h.request.lease_profile.clone(),
    };
    h.repo
        .admit_spend_attempt(
            &h.request.user.user_id,
            "legacy-operation",
            &request,
            "legacy",
            Utc::now(),
        )
        .unwrap();
    let evaluation = h
        .spend
        .evaluate_spend(
            &h.request.user,
            "legacy-operation",
            request,
            h.selected.policy(),
        )
        .unwrap();
    let decision = h.spend.decision_record(&evaluation.decision_id).unwrap();
    h.repo.save_spend_decision(&decision).unwrap();
    h.repo
        .record_spend_attempt_outcome(
            &decision,
            SpendAuthorizationDecision::Denied,
            &decision.evaluation.reasons,
            Utc::now(),
        )
        .unwrap();
    assert!(h.snapshot().events.is_empty());
    assert!(h.snapshot().capture_markers.is_empty());
    h.authorize().unwrap();
    let captured = h.snapshot();
    assert_eq!(captured.events.len(), 2);
    assert!(captured
        .events
        .iter()
        .all(|e| e.event.correlation.operation_key != "legacy-operation"));
    h.request.operation_key = "legacy-operation".into();
    h.authorize().unwrap();
    assert_eq!(h.snapshot().events, captured.events);
    assert_eq!(h.snapshot().capture_markers, captured.capture_markers);
}

#[test]
fn each_currency_selects_its_own_budget_and_missing_or_insufficient_denials_are_exact() {
    for currency in [Currency::Usd, Currency::Usdc] {
        let other = if currency == Currency::Usd {
            Currency::Usdc
        } else {
            Currency::Usd
        };
        for (limit, cause) in [
            (Some("0.010"), None),
            (
                Some("0.004"),
                Some(AdmissionDenialReason::InsufficientBudget),
            ),
            (None, Some(AdmissionDenialReason::NoAvailableBudget)),
        ] {
            let mut h = Harness::new_currency(limit, Effect::Allow, currency);
            let other_budget = h.add_budget(other, "100");
            if cause.is_none() {
                h.request.amount = "0.005001".parse().unwrap();
            }
            let result = h.authorize().unwrap();
            assert_eq!(
                matches!(result, SpendAuthorizationOutcome::Approved(_)),
                cause.is_none()
            );
            let captured = h.snapshot();
            let admission = &captured.events[1].event;
            assert_eq!(value(&admission.requested_amount).currency, currency);
            assert_eq!(value(&admission.requested_amount).amount, h.request.amount);
            match cause {
                None => {
                    let hold = h.repo.load_budget_holds().unwrap().remove(0);
                    assert_eq!(hold.currency, currency);
                    assert_eq!(hold.amount.units(), 5001);
                    assert_ne!(hold.budget_id, other_budget.budget.id);
                }
                Some(reason) => {
                    assert_eq!(
                        admission.fact,
                        LifecycleFact::AutomatedAdmission {
                            result: AutomatedAdmissionOutcome::Denied { reason }
                        }
                    );
                    assert!(h.repo.load_budget_holds().unwrap().is_empty());
                    if reason == AdmissionDenialReason::NoAvailableBudget {
                        assert!(matches!(
                            admission.explanation.budget,
                            LifecycleEvidence::Unavailable {
                                reason: EvidenceUnavailableReason::NotCaptured
                            }
                        ));
                        let SpendAuthorizationOutcome::Rejected(rejected) = result else {
                            unreachable!()
                        };
                        assert!(rejected.reasons.contains(
                            &SpendApprovalError::MissingActiveBudget { currency }.to_string()
                        ));
                    }
                }
            }
            if let Some(limit) = limit {
                let evidence = value(&admission.explanation.budget);
                assert_eq!(value(&evidence.available).currency, currency);
                assert_eq!(value(&evidence.available).amount, limit.parse().unwrap());
                assert_ne!(
                    evidence.budget_id,
                    format!("bgt_{}", other_budget.budget.id.public_suffix())
                );
            }
            let unchanged = h.budgets.get_budget_by_id(&other_budget.budget.id).unwrap();
            assert_eq!(unchanged.balance.remaining_amount, "100".parse().unwrap());
            assert_eq!(unchanged.balance.frozen_amount, Amount::ZERO);
            h.authorize().unwrap();
            assert_eq!(h.snapshot().events, captured.events);
        }
    }
}

#[test]
fn exhausted_evidence_stays_in_request_currency_even_with_another_currency_available() {
    for currency in [Currency::Usd, Currency::Usdc] {
        let other = if currency == Currency::Usd {
            Currency::Usdc
        } else {
            Currency::Usd
        };
        for remaining in [0, -1] {
            let mut h = Harness::new_currency(Some("0.010"), Effect::Allow, currency);
            let own = h
                .budgets
                .get_budgets_by_agent_id(&h.request.agent_id)
                .remove(0);
            h.repo.conn.execute("UPDATE budget_balances SET consumed_amount=?1,remaining_amount=?2 WHERE budget_id=?3", params![10000-remaining,remaining,own.budget.id.to_string()]).unwrap();
            h.budgets = BudgetManager::from_records(
                h.repo.load_budgets().unwrap(),
                h.repo.load_budget_versions().unwrap(),
                h.repo.load_budget_balances().unwrap(),
                h.repo.load_budget_holds().unwrap(),
            )
            .unwrap();
            h.add_budget(other, "100");
            h.authorize().unwrap();
            let captured = h.snapshot();
            let event = &captured.events[1].event;
            assert_eq!(
                event.fact,
                LifecycleFact::AutomatedAdmission {
                    result: AutomatedAdmissionOutcome::Denied {
                        reason: AdmissionDenialReason::NoAvailableBudget
                    }
                }
            );
            let evidence = value(&event.explanation.budget);
            assert_eq!(
                evidence.budget_id,
                format!("bgt_{}", own.budget.id.public_suffix())
            );
            assert_eq!(value(&evidence.available).amount.units(), remaining);
            assert_eq!(value(&evidence.available).currency, currency);
            assert_eq!(value(&evidence.limit).currency, currency);
            assert!(h.repo.load_budget_holds().unwrap().is_empty());
            h.authorize().unwrap();
            assert_eq!(h.snapshot().events, captured.events);
        }
    }
}

#[test]
fn no_available_budget_rechecks_only_matching_currency_under_the_write_lock() {
    for currency in [Currency::Usd, Currency::Usdc] {
        let other = if currency == Currency::Usd {
            Currency::Usdc
        } else {
            Currency::Usd
        };
        let mut h = Harness::new_currency(None, Effect::Allow, currency);
        h.add_budget(other, "100");
        h.authorize().unwrap();
        let decision = h.repo.load_spend_decisions().unwrap().remove(0);
        let checked_at = Utc::now();
        let tx = h
            .repo
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        validate_no_available_budget(&tx, &decision, checked_at).unwrap();
        tx.commit().unwrap();
        h.add_budget(currency, "100");
        let tx = h
            .repo
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let error = validate_no_available_budget(&tx, &decision, checked_at).unwrap_err();
        assert!(error.to_string().contains("budget selection is stale"));
        tx.rollback().unwrap();
    }
}
