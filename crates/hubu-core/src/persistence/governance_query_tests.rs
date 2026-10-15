use super::*;
use crate::app::{
    AuthorizeSpendRequest, HumanApprovalDecision, SpendApprovalService, SpendAuthorizationOutcome,
};
use crate::budget::{BudgetManager, CreateSingleBudgetRequest};
use crate::policy::{Effect, Policy};
use crate::spend::SpendManager;
use hubu_common::models::UserContext;

struct Harness {
    repo: SqliteGovernanceRepository,
    spend: SpendManager,
    budgets: BudgetManager,
    request: AuthorizeSpendRequest,
    selected: SelectedSpendPolicy,
}
impl Harness {
    fn new(effect: Effect) -> Self {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let account = AgentAccountId::new();
        seed_lifecycle_test_identity(&mut repo, &owner, &agent, &account);
        repo.save_policy_assignment(
            &owner,
            &PolicyAssignmentScope::UserDefault,
            &Policy {
                id: "authored-policy".into(),
                version: "v1".into(),
                owner_user_id: owner.clone(),
                default_effect: effect,
                rules: vec![],
            },
        )
        .unwrap();
        let selected = repo.load_policy_assignments().unwrap().remove(0).into();
        let mut budgets = BudgetManager::new();
        let b = budgets
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent.clone(),
                amount_limit: "10".parse().unwrap(),
                currency: Currency::Usd,
                period: TimePeriod::new(Utc::now() - chrono::Duration::days(3), None).unwrap(),
            })
            .unwrap();
        repo.save_budget_with_balance(&b.budget, &b.version, &b.balance)
            .unwrap();
        Self {
            repo,
            spend: SpendManager::new(),
            budgets,
            selected,
            request: AuthorizeSpendRequest {
                operation_key: "private-key".into(),
                user: UserContext::new(owner),
                agent_id: agent,
                agent_account_id: account,
                amount: "0.005".parse().unwrap(),
                currency: Currency::Usd,
                merchant: Some("private merchant".into()),
                execution_scope: None,
                task_id: Some("private task".into()),
                reason: "private prompt".into(),
                lease_profile: "default".into(),
            },
        }
    }
    fn owner(&self) -> &UserId {
        &self.request.user.user_id
    }
    fn capture(&mut self, at: DateTime<Utc>) {
        SpendApprovalService
            .authorize_selected_at(
                self.request.clone(),
                &self.selected,
                &mut self.spend,
                &mut self.budgets,
                &mut self.repo,
                at,
            )
            .unwrap();
    }
    fn summary(&self) -> GovernanceSummary {
        self.repo
            .governance_summary(self.owner(), &GovernanceQuery::default())
            .unwrap()
    }
    /// Persist a real source decision with an explicit authoritative transition
    /// time. The service's evaluation clock is intentionally wall time, so time
    /// boundary tests use the same source repository seam as capture producers.
    fn source_policy_at(&mut self, time: DateTime<Utc>) -> SpendDecisionRecord {
        let request = SpendRequest {
            owner_user_id: self.owner().clone(),
            agent_id: self.request.agent_id.clone(),
            agent_account_id: self.request.agent_account_id.clone(),
            amount: self.request.amount,
            currency: self.request.currency,
            merchant: self.request.merchant.clone(),
            execution_scope: self.request.execution_scope.clone(),
            category: None,
            task_id: self.request.task_id.clone(),
            reason: self.request.reason.clone(),
            lease_profile: self.request.lease_profile.clone(),
        };
        let revision = match self
            .repo
            .admit_spend_attempt(
                &request.owner_user_id,
                &self.request.operation_key,
                &request,
                "fixture",
                time,
            )
            .unwrap()
        {
            SpendAttemptAdmission::Admitted { revision } => revision,
            _ => panic!("source fixture must be new"),
        };
        let record = SpendDecisionRecord {
            id: hubu_common::ids::SpendDecisionId::new(),
            owner_user_id: self.owner().clone(),
            operation_key: self.request.operation_key.clone(),
            revision,
            actor: self.owner().to_string(),
            evaluation: crate::policy::evaluate_policy(&request, self.selected.policy()).unwrap(),
            request,
            created_at: time,
        };
        self.repo
            .save_automatic_policy_transition(&record, Some(&self.selected))
            .unwrap();
        self.spend = SpendManager::from_records(
            self.repo.load_spend_decisions().unwrap(),
            self.repo.load_spend_auth_tokens().unwrap(),
        );
        record
    }
    fn source_denial_at(&mut self, time: DateTime<Utc>) {
        let record = self.source_policy_at(time);
        self.repo
            .save_automatic_denied_transition(
                &record,
                Some(&self.selected),
                None,
                AdmissionDenialReason::Policy,
                &[],
                time,
                time,
            )
            .unwrap();
    }
}
fn at(raw: &str) -> DateTime<Utc> {
    raw.parse().unwrap()
}
fn frozen(token: &str) -> GovernanceQuery {
    GovernanceQuery {
        snapshot: Some(token.into()),
        ..Default::default()
    }
}

#[test]
fn corrected_revision_denial_then_success_counts_two_attempts_one_operation() {
    let mut h = Harness::new(Effect::Deny);
    h.capture(Utc::now() - chrono::Duration::seconds(2));
    let mut policy = h.selected.policy().clone();
    policy.default_effect = Effect::Allow;
    let owner = h.owner().clone();
    h.repo
        .save_policy_assignment(&owner, &PolicyAssignmentScope::UserDefault, &policy)
        .unwrap();
    h.selected = h.repo.load_policy_assignments().unwrap().remove(0).into();
    h.request.amount = "0.004".parse().unwrap();
    h.capture(Utc::now() - chrono::Duration::seconds(1));
    let summary = h.summary();
    assert_eq!(summary.distinct_attempts, 2);
    assert_eq!(summary.distinct_operations, 1);
    assert_eq!(summary.fact_count, 5);
    assert_eq!(summary.automatic_denials[0].distinct_attempts, 1);
    assert_eq!(summary.requested.per_currency[0].units, 9_000);
    assert_eq!(
        summary.automatic_denied_requested.per_currency[0].units,
        5_000
    );
    assert!(summary
        .stages
        .iter()
        .any(|s| s.outcome == GovernanceOutcome::AutomaticAllow && s.fact_count == 1));
}

#[test]
fn multiple_decisive_rules_count_one_primary_denied_attempt() {
    use crate::policy::condition::{Condition, Field};
    let mut h = Harness::new(Effect::Allow);
    let mut policy = h.selected.policy().clone();
    policy.rules = ["rule-a", "rule-b"]
        .into_iter()
        .map(|id| crate::policy::Rule {
            id: id.into(),
            effect: Effect::Deny,
            when: Condition::Exists {
                field: Field::Merchant,
            },
            reason: "private rule prose".into(),
        })
        .collect();
    let owner = h.owner().clone();
    h.repo
        .save_policy_assignment(&owner, &PolicyAssignmentScope::UserDefault, &policy)
        .unwrap();
    h.selected = h.repo.load_policy_assignments().unwrap().remove(0).into();
    h.capture(Utc::now() - chrono::Duration::seconds(1));
    let summary = h.summary();
    assert_eq!(
        summary.automatic_denials,
        vec![GovernanceDenialCount {
            reason: GovernanceDenialReason::Policy,
            distinct_attempts: 1
        }]
    );
    assert_eq!(
        summary.automatic_denied_requested.per_currency[0].units,
        5_000
    );
}

#[test]
fn source_capture_replay_full_totals_pages_and_shared_detail_snapshot() {
    let mut h = Harness::new(Effect::Deny);
    let time = Utc::now() - chrono::Duration::seconds(1);
    h.capture(time);
    let first = h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.summary.fact_count, 2);
    assert_eq!(first.summary.distinct_attempts, 1);
    assert_eq!(first.summary.distinct_operations, 1);
    assert_eq!(
        first.summary.automatic_denials,
        vec![GovernanceDenialCount {
            reason: GovernanceDenialReason::Policy,
            distinct_attempts: 1
        }]
    );
    assert_eq!(
        first.summary.automatic_denied_requested.per_currency[0].units,
        5_000
    );
    assert!(first.summary.stage_counts_overlap);
    h.capture(time); // Real idempotent source replay adds no evidence.
    let query = GovernanceQuery {
        cursor: first.next_cursor.clone(),
        limit: Some(1),
        ..Default::default()
    };
    let second = h.repo.governance_list(h.owner(), &query).unwrap();
    assert_eq!(second.summary, first.summary);
    assert_ne!(
        second.facts[0].event_public_id,
        first.facts[0].event_public_id
    );
    assert_eq!(first.facts[0].outcome, GovernanceOutcome::AutomaticDeny);
    assert_eq!(second.facts[0].outcome, GovernanceOutcome::PolicyDeny);
    let query = frozen(&first.summary.snapshot.token);
    assert_eq!(
        h.repo.governance_summary(h.owner(), &query).unwrap(),
        first.summary
    );
    let detail = h
        .repo
        .governance_detail(h.owner(), &first.facts[0].event_public_id, &query)
        .unwrap()
        .unwrap();
    assert_eq!(detail.fact, first.facts[0]);
    assert_eq!(detail.summary, first.summary);
    let public = format!("{first:?}");
    for secret in [
        "private-key",
        "private merchant",
        "private task",
        "private prompt",
        &h.owner().to_string(),
        &h.request.agent_id.to_string(),
        &h.request.agent_account_id.to_string(),
    ] {
        assert!(!public.contains(secret), "leaked {secret}");
    }
}

#[test]
fn strict_fractional_time_filter_uses_instants_and_excludes_late_recorded_older_fact() {
    let mut h = Harness::new(Effect::Deny);
    let t = at("2026-10-10T00:00:00.123456789Z");
    h.source_denial_at(t);
    let query = GovernanceQuery {
        occurred_from: Some(at("2026-10-09T17:00:00.123456789-07:00")),
        occurred_before: Some(t + chrono::Duration::nanoseconds(1)),
        ..Default::default()
    };
    let first = h.repo.governance_list(h.owner(), &query).unwrap();
    assert_eq!(first.summary.fact_count, 2);
    assert_eq!(
        h.repo
            .governance_summary(
                h.owner(),
                &GovernanceQuery {
                    occurred_from: Some(t - chrono::Duration::seconds(1)),
                    occurred_before: Some(t),
                    ..Default::default()
                }
            )
            .unwrap()
            .fact_count,
        0
    );
    h.request.operation_key = "late-recorded-older".into();
    h.source_denial_at(t - chrono::Duration::seconds(1));
    let old = h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                snapshot: Some(first.summary.snapshot.token.clone()),
                ..query.clone()
            },
        )
        .unwrap();
    assert_eq!(old.summary, first.summary);
    assert_eq!(h.summary().distinct_operations, 2);
    assert!(h
        .repo
        .governance_summary(
            h.owner(),
            &GovernanceQuery {
                occurred_from: Some(t),
                occurred_before: Some(t),
                ..Default::default()
            }
        )
        .is_err());
}

#[test]
fn backlog_uses_preperiod_evidence_and_frozen_boundary_not_current_status() {
    let mut h = Harness::new(Effect::NeedsApproval);
    let open = Utc::now() - chrono::Duration::days(1);
    h.source_policy_at(open);
    let query = GovernanceQuery {
        occurred_from: Some(Utc::now() - chrono::Duration::hours(1)),
        categories: vec![GovernanceCategory::AutomatedAdmission],
        ..Default::default()
    };
    let first = h.repo.governance_list(h.owner(), &query).unwrap();
    assert_eq!(first.summary.fact_count, 0);
    assert_eq!(first.summary.backlog.pending_human_approvals, 1);
    let all_first = h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let id = h.repo.load_spend_decisions().unwrap().remove(0).id;
    assert!(matches!(
        SpendApprovalService
            .resolve_human_approval_at(
                h.request.user.clone(),
                &id,
                HumanApprovalDecision::Deny,
                &mut h.spend,
                &mut h.budgets,
                &mut h.repo,
                Utc::now()
            )
            .unwrap(),
        SpendAuthorizationOutcome::Rejected(_)
    ));
    let frozen_query = GovernanceQuery {
        snapshot: Some(first.summary.snapshot.token.clone()),
        ..query
    };
    let after = h.repo.governance_list(h.owner(), &frozen_query).unwrap();
    assert_eq!(after.summary, first.summary);
    let all_next = h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                cursor: all_first.next_cursor,
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all_next.summary, all_first.summary);
    assert_eq!(
        h.repo
            .governance_detail(
                h.owner(),
                &all_first.facts[0].event_public_id,
                &frozen(&all_first.summary.snapshot.token)
            )
            .unwrap()
            .unwrap()
            .summary,
        all_first.summary
    );
    let latest = h.summary();
    assert_eq!(latest.backlog.pending_human_approvals, 0);
    assert!(latest
        .stages
        .iter()
        .any(|s| s.outcome == GovernanceOutcome::HumanDeclined && s.fact_count == 1));
    assert!(latest.automatic_denials.is_empty());
}

#[test]
fn selectors_are_validated_even_empty_and_account_canonicalizes_agent() {
    let mut h = Harness::new(Effect::Deny);
    let agent = format!("agt_{}", h.request.agent_id.public_suffix());
    let account = format!("aga_{}", h.request.agent_account_id.public_suffix());
    let q = GovernanceQuery {
        account_public_id: Some(account.clone()),
        ..Default::default()
    };
    let s = h.repo.governance_summary(h.owner(), &q).unwrap();
    assert_eq!(s.scope.agent_public_id, Some(agent.clone()));
    assert_eq!(s.fact_count, 0);
    let canonical = GovernanceQuery {
        agent_public_id: Some(agent),
        ..q.clone()
    };
    assert_eq!(
        h.repo
            .governance_summary(
                h.owner(),
                &GovernanceQuery {
                    snapshot: Some(s.snapshot.token.clone()),
                    ..canonical
                }
            )
            .unwrap(),
        s
    );
    assert!(s.snapshot.captured_evidence_as_of.is_none());
    assert_eq!(s.snapshot.watermark, 0);
    for q in [
        GovernanceQuery {
            agent_public_id: Some("agt_unknown".into()),
            ..Default::default()
        },
        GovernanceQuery {
            account_public_id: Some("aga_unknown".into()),
            ..Default::default()
        },
    ] {
        assert!(h.repo.governance_summary(h.owner(), &q).is_err());
    }
    let foreign = UserId::new();
    let foreign_agent = AgentId::new();
    let foreign_account = AgentAccountId::new();
    seed_lifecycle_test_identity(&mut h.repo, &foreign, &foreign_agent, &foreign_account);
    assert!(h.repo.governance_summary(&foreign, &q).is_err());
    let other_agent = AgentId::new();
    let other_account = AgentAccountId::new();
    let now = Utc::now().to_rfc3339();
    h.repo.conn.execute("INSERT INTO agent_identities(id,pub_id,fingerprint,display_name,owner_user_id,agent_type,agent_status,created_at,updated_at) VALUES(?1,?2,?3,'Other',?4,'autonomous_agent','active',?5,?5)",params![other_agent.to_string(),format!("agt_{}",other_agent.public_suffix()),format!("fp:{other_agent}"),h.owner().to_string(),now]).unwrap();
    h.repo.conn.execute("INSERT INTO agent_accounts(id,pub_id,agent_id,owner_user_id,account_status,created_at,updated_at) VALUES(?1,?2,?3,?4,'active',?5,?5)",params![other_account.to_string(),format!("aga_{}",other_account.public_suffix()),other_agent.to_string(),h.owner().to_string(),now]).unwrap();
    assert!(h
        .repo
        .governance_summary(
            h.owner(),
            &GovernanceQuery {
                agent_public_id: Some(format!("agt_{}", other_agent.public_suffix())),
                ..q
            }
        )
        .is_err());
}

#[test]
fn strict_tokens_verify_owner_anchor_metadata_scope_and_matching_cursor() {
    let mut h = Harness::new(Effect::Deny);
    h.capture(Utc::now() - chrono::Duration::seconds(1));
    let page = h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let original: serde_json::Value = serde_json::from_str(&page.summary.snapshot.token).unwrap();
    let mutate = |key: &str, value: serde_json::Value| {
        let mut token = original.clone();
        token[key] = value;
        token.to_string()
    };
    for raw in [
        "{".into(),
        "x".repeat(MAX_SNAPSHOT_BYTES + 1),
        mutate("version", 2.into()),
        mutate("extra", true.into()),
        mutate("watermark", u64::MAX.into()),
        mutate("binding", "wrong".into()),
    ] {
        assert!(h.repo.governance_summary(h.owner(), &frozen(&raw)).is_err());
    }
    for field in ["event_id", "recorded_at"] {
        let mut token = original.clone();
        token["boundary"][field] = if field == "event_id" {
            "lev_foreign".into()
        } else {
            "2001-01-01T00:00:00Z".into()
        };
        assert!(h
            .repo
            .governance_summary(h.owner(), &frozen(&token.to_string()))
            .is_err());
    }
    let mut cursor: serde_json::Value =
        serde_json::from_str(page.next_cursor.as_ref().unwrap()).unwrap();
    cursor["after"]["occurred_at"] = "2000-01-01T00:00:00Z".into();
    assert!(h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                cursor: Some(cursor.to_string()),
                ..Default::default()
            }
        )
        .is_err());
    let changed = GovernanceQuery {
        snapshot: Some(page.summary.snapshot.token.clone()),
        categories: vec![GovernanceCategory::InitialPolicy],
        ..Default::default()
    };
    assert!(h.repo.governance_summary(h.owner(), &changed).is_err());
    let empty = h
        .repo
        .governance_detail(
            h.owner(),
            "lev_unknown",
            &frozen(&page.summary.snapshot.token),
        )
        .unwrap();
    assert!(empty.is_none());
}

/// Pure evaluator fixtures are never appended to the durable store and cannot
/// bypass source validation. Persistence tests above use actual source writes.
fn synthetic_record(revision: u64, units: i64, currency: Currency) -> LifecycleRecord {
    LifecycleRecord {
        sequence: revision,
        event_id: format!("lev_fixture{revision}"),
        agent_public_id: "agt_fixture".into(),
        account_public_id: "aga_fixture".into(),
        recorded_at: at("2026-10-10T00:00:01Z"),
        event: LifecycleEventInput {
            schema_version: 1,
            correlation: LifecycleCorrelation {
                owner_user_id: UserId::new(),
                agent_id: AgentId::new(),
                agent_account_id: AgentAccountId::new(),
                operation_key: "private".into(),
                attempt_revision: revision,
                decision_id: None,
                correction_of_revision: None,
            },
            source: LifecycleSource::AutomatedAdmission,
            occurred_at: at("2026-10-10T00:00:00Z"),
            requested_amount: LifecycleEvidence::Known {
                value: LifecycleAmount {
                    amount: Amount::from_units(units),
                    currency,
                },
            },
            execution_scope: LifecycleEvidence::Unavailable {
                reason: EvidenceUnavailableReason::NotCaptured,
            },
            fact: LifecycleFact::AutomatedAdmission {
                result: AutomatedAdmissionOutcome::Denied {
                    reason: AdmissionDenialReason::Policy,
                },
            },
            explanation: LifecycleExplanation::default(),
        },
    }
}
#[test]
fn exact_requested_totals_are_wide_per_currency_and_deduplicate_attempts() {
    let a = synthetic_record(1, i64::MAX, Currency::Usd);
    let mut b = synthetic_record(2, i64::MAX, Currency::Usd);
    b.event.correlation.agent_id = a.event.correlation.agent_id.clone();
    let c = synthetic_record(3, 1, Currency::Usdc);
    let totals = requested_total([&a, &a, &b, &c].into_iter()).unwrap();
    assert_eq!(totals.known_attempts, 3);
    assert_eq!(totals.unknown_attempts, 0);
    assert_eq!(
        totals.per_currency,
        vec![
            GovernanceCurrencyTotal {
                currency: Currency::Usd,
                units: 2 * i128::from(i64::MAX)
            },
            GovernanceCurrencyTotal {
                currency: Currency::Usdc,
                units: 1
            }
        ]
    );
}

#[test]
fn per_category_support_does_not_claim_execution_failures_and_empty_is_partial() {
    let h = Harness::new(Effect::Deny);
    let summary = h.summary();
    assert!(!summary.legacy_coverage.assessed);
    assert!(summary.record_coverage.iter().all(|c| !c.complete_history));
    assert!(
        !summary
            .record_coverage
            .iter()
            .find(|c| c.category == GovernanceCategory::ExecutionFailure)
            .unwrap()
            .supported
    );
}

#[test]
fn inventory_missing_scope_and_broader_gaps_never_become_selected_zero() {
    let record = synthetic_record(1, 1, Currency::Usd);
    let agent = record.event.correlation.agent_id.clone();
    let account = record.event.correlation.agent_account_id.clone();
    let scope = Scope {
        public: GovernanceScope {
            agent_public_id: Some("agt_fixture".into()),
            account_public_id: Some("aga_fixture".into()),
        },
        agent: Some(agent.clone()),
        account: Some(account.clone()),
    };
    let mut snapshot = LifecycleSnapshot {
        schema_version: 1,
        watermark: 4,
        events: vec![],
        capture_markers: vec![],
        coverage_inventories: vec![LifecycleCoverageInventory {
            sequence: 4,
            observed_at: record.recorded_at,
            recorded_at: record.recorded_at,
            collector_version: 1,
            scopes: vec![LifecycleCoverageScopeObservation {
                scope: LifecycleCoverageScope::OwnerUnattributed,
                gaps: vec![LifecycleCoverageGap {
                    source: LifecycleSource::PolicyEngine,
                    metric: LifecycleCount::PolicyDeny,
                }],
                backlog_gaps: vec![LifecycleCoverageBacklog::HumanApproval],
            }],
        }],
    };
    let missing = legacy_coverage(&scope, &snapshot);
    assert!(!missing.assessed);
    assert!(missing.broader_attribution_unknown);
    assert!(missing.unlinked_metrics.is_empty());
    assert!(!missing.unlinked_human_approval_backlog);
    snapshot.coverage_inventories[0]
        .scopes
        .push(LifecycleCoverageScopeObservation {
            scope: LifecycleCoverageScope::Account {
                agent_id: agent,
                account_id: account,
            },
            gaps: vec![LifecycleCoverageGap {
                source: LifecycleSource::Reconciliation,
                metric: LifecycleCount::ReconciliationRequired,
            }],
            backlog_gaps: vec![LifecycleCoverageBacklog::FinancialReconciliation],
        });
    let selected = legacy_coverage(&scope, &snapshot);
    assert!(selected.assessed);
    assert!(selected.broader_attribution_unknown);
    assert_eq!(
        selected.unlinked_metrics,
        vec![LifecycleCount::ReconciliationRequired]
    );
    assert!(selected.unlinked_reconciliation_backlog);
}

#[test]
fn backlog_resolution_dominates_late_open_and_ignores_future_occurrence() {
    use hubu_common::ids::SpendDecisionId;
    let mut open = synthetic_record(2, 1, Currency::Usd);
    let mut closure = open.clone();
    let mut future = open.clone();
    let subject = SpendDecisionId::new();
    open.event.fact = LifecycleFact::HumanEscalation {
        approval_request_id: subject.clone(),
    };
    closure.sequence = 1;
    closure.event.fact = LifecycleFact::HumanResolutionConfirmed {
        approval_request_id: subject,
        resolution: HumanResolution::Declined,
        confirmed_by_user_id: UserId::new(),
    };
    future.sequence = 3;
    future.event.occurred_at = at("2026-10-10T00:00:02Z");
    future.event.fact = LifecycleFact::HumanEscalation {
        approval_request_id: SpendDecisionId::new(),
    };
    let records = vec![closure, open, future];
    let scoped = records.iter().collect::<Vec<_>>();
    let snapshot = LifecycleSnapshot {
        schema_version: 1,
        watermark: 3,
        events: records.clone(),
        capture_markers: vec![],
        coverage_inventories: vec![],
    };
    let scope = Scope {
        public: GovernanceScope {
            agent_public_id: None,
            account_public_id: None,
        },
        agent: None,
        account: None,
    };
    let summary = summarize(
        &scope,
        &snapshot,
        &scoped,
        &records,
        GovernanceSnapshot {
            token: "test".into(),
            watermark: 3,
            captured_evidence_as_of: Some(at("2026-10-10T00:00:01Z")),
        },
    )
    .unwrap();
    assert_eq!(summary.backlog.pending_human_approvals, 0);
}

#[test]
fn safe_detail_preserves_financial_resolution_and_release_meaning() {
    use hubu_common::ids::{SpendDecisionId, SpendExecutorClaimId};
    let mut record = synthetic_record(1, 1, Currency::Usd);
    record.event.source = LifecycleSource::Reconciliation;
    record.event.fact = LifecycleFact::ReconciliationResolved {
        claim_id: SpendExecutorClaimId::new(),
        outcome: ReconciliationResolution::VendorDidNotBill,
    };
    let detail = safe_fact(&record);
    assert_eq!(detail.source, LifecycleSource::Reconciliation);
    assert_eq!(
        detail.reconciliation_resolution,
        Some(ReconciliationResolution::VendorDidNotBill)
    );
    assert_eq!(detail.authorization_release_reason, None);
    record.event.fact = LifecycleFact::AuthorizationReleased {
        authorization_id: SpendDecisionId::new(),
        reason: AuthorizationReleaseReason::ExecutorRejectedBeforeClaim,
    };
    let detail = safe_fact(&record);
    assert_eq!(
        detail.authorization_release_reason,
        Some(AuthorizationReleaseReason::ExecutorRejectedBeforeClaim)
    );
    assert_eq!(detail.reconciliation_resolution, None);
}

#[test]
fn later_inventory_does_not_rewrite_empty_snapshot_or_obey_event_window() {
    let mut h = Harness::new(Effect::Deny);
    let query = GovernanceQuery {
        occurred_from: Some(at("2001-01-01T00:00:00Z")),
        occurred_before: Some(at("2001-01-02T00:00:00Z")),
        ..Default::default()
    };
    let before = h.repo.governance_summary(h.owner(), &query).unwrap();
    assert_eq!(before.snapshot.watermark, 0);
    assert_eq!(before.snapshot.captured_evidence_as_of, None);
    assert!(!before.legacy_coverage.assessed);
    let owner = h.owner().clone();
    h.repo
        .observe_lifecycle_coverage_inventory_at(&owner, Utc::now())
        .unwrap();
    let after = h.repo.governance_summary(h.owner(), &query).unwrap();
    assert!(after.snapshot.watermark > 0);
    assert!(after.snapshot.captured_evidence_as_of.is_some());
    assert_eq!(after.fact_count, 0);
    assert!(after.legacy_coverage.assessed);
    assert!(after.legacy_coverage.unlinked_metrics.is_empty());
    assert!(after.record_coverage.iter().all(|c| !c.complete_history));
    let frozen = h
        .repo
        .governance_summary(
            h.owner(),
            &GovernanceQuery {
                snapshot: Some(before.snapshot.token.clone()),
                ..query
            },
        )
        .unwrap();
    assert_eq!(frozen, before);
}

#[test]
fn caller_validation_and_stored_corruption_have_distinct_redacted_errors() {
    let mut h = Harness::new(Effect::Deny);
    let error = h
        .repo
        .governance_list(
            h.owner(),
            &GovernanceQuery {
                cursor: Some("{PRIVATE_CLIENT_TOKEN".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(error, GovernanceReadError::InvalidQuery));
    assert_eq!(error.code(), "governance_invalid_query");
    assert!(!error.to_string().contains("PRIVATE_CLIENT_TOKEN"));

    let foreign = UserId::new();
    let agent = AgentId::new();
    let account = AgentAccountId::new();
    seed_lifecycle_test_identity(&mut h.repo, &foreign, &agent, &account);
    let error = h
        .repo
        .governance_summary(
            h.owner(),
            &GovernanceQuery {
                account_public_id: Some(format!("aga_{}", account.public_suffix())),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(error, GovernanceReadError::InvalidQuery));

    // Intentional disk-corruption fixture: bypassing append validation here
    // models a malformed stored payload, never a supported producer operation.
    h.repo.conn.execute("INSERT INTO spend_lifecycle_records(owner_user_id,semantic_identity,event_id,payload_json,recorded_at) VALUES(?1,'corrupt-fixture','lev_corrupt',?2,?3)",params![h.owner().to_string(),r#"{"record_type":"event","input":"PRIVATE_STORED_PAYLOAD"}"#,Utc::now().to_rfc3339()]).unwrap();
    let error = h
        .repo
        .governance_summary(h.owner(), &GovernanceQuery::default())
        .unwrap_err();
    assert!(matches!(
        error,
        GovernanceReadError::Unavailable(StorageError::Json(_))
    ));
    assert_eq!(error.code(), "governance_unavailable");
    assert_eq!(error.to_string(), "governance evidence is unavailable");
    assert!(!error.to_string().contains("PRIVATE_STORED_PAYLOAD"));
}

#[test]
fn inconsistent_captured_amount_is_unavailable_instead_of_invalid_query() {
    let first = synthetic_record(1, 1, Currency::Usd);
    let mut inconsistent = first.clone();
    inconsistent.event.requested_amount = LifecycleEvidence::Known {
        value: LifecycleAmount {
            amount: Amount::from_units(2),
            currency: Currency::Usd,
        },
    };
    let error = requested_total([&first, &inconsistent].into_iter()).unwrap_err();
    assert!(matches!(
        error,
        GovernanceReadError::Unavailable(StorageError::InvalidData(_))
    ));
    assert_eq!(error.code(), "governance_unavailable");
}
