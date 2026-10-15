use super::*;
use crate::policy::Effect;
use hubu_common::ids::{AgentAccountId, SpendDecisionId};

struct Fixture {
    owner: UserId,
    agent: AgentId,
    account: AgentAccountId,
}
impl Fixture {
    fn new(repo: &mut SqliteGovernanceRepository) -> Self {
        let f = Self {
            owner: UserId::new(),
            agent: AgentId::new(),
            account: AgentAccountId::new(),
        };
        seed_lifecycle_test_identity(repo, &f.owner, &f.agent, &f.account);
        f
    }
    fn legacy(&self, repo: &mut SqliteGovernanceRepository, effect: Effect) -> SpendDecisionRecord {
        let request = SpendRequest {
            owner_user_id: self.owner.clone(),
            agent_id: self.agent.clone(),
            agent_account_id: self.account.clone(),
            amount: Amount::from_units(5000),
            currency: Currency::Usd,
            merchant: None,
            execution_scope: None,
            category: None,
            task_id: None,
            reason: "private legacy reason".into(),
            lease_profile: "default".into(),
        };
        let policy = Policy {
            id: "legacy".into(),
            version: "v1".into(),
            owner_user_id: self.owner.clone(),
            rules: vec![],
            default_effect: effect,
        };
        let record = SpendDecisionRecord {
            id: SpendDecisionId::new(),
            owner_user_id: self.owner.clone(),
            operation_key: format!("private-op-{}", SpendDecisionId::new()),
            revision: 1,
            actor: "legacy".into(),
            evaluation: crate::policy::evaluate_policy(&request, &policy).unwrap(),
            request,
            created_at: Utc::now(),
        };
        repo.admit_spend_attempt(
            &self.owner,
            &record.operation_key,
            &record.request,
            "legacy",
            record.created_at,
        )
        .unwrap();
        repo.save_spend_decision(&record).unwrap();
        record
    }
    fn inventory(&self, repo: &SqliteGovernanceRepository) -> LifecycleCoverageInventory {
        repo.lifecycle_snapshot(&self.owner, None)
            .unwrap()
            .coverage_inventories
            .pop()
            .unwrap()
    }
}
fn append(
    repo: &mut SqliteGovernanceRepository,
    record: &SpendDecisionRecord,
    fact: LifecycleFact,
    source: LifecycleSource,
) {
    repo.append_lifecycle_event(&LifecycleEventInput {
        schema_version: LIFECYCLE_SCHEMA_VERSION,
        correlation: LifecycleCorrelation {
            owner_user_id: record.owner_user_id.clone(),
            agent_id: record.request.agent_id.clone(),
            agent_account_id: record.request.agent_account_id.clone(),
            operation_key: record.operation_key.clone(),
            attempt_revision: record.revision,
            decision_id: Some(record.id.clone()),
            correction_of_revision: None,
        },
        source,
        occurred_at: record.created_at,
        requested_amount: LifecycleEvidence::Known {
            value: LifecycleAmount {
                amount: record.request.amount,
                currency: record.request.currency,
            },
        },
        execution_scope: LifecycleEvidence::Unavailable {
            reason: EvidenceUnavailableReason::NotCaptured,
        },
        fact,
        explanation: LifecycleExplanation::default(),
    })
    .unwrap();
}
fn account_scope<'a>(
    inventory: &'a LifecycleCoverageInventory,
    f: &Fixture,
) -> &'a LifecycleCoverageScopeObservation {
    inventory.scopes.iter().find(|s|matches!(&s.scope,LifecycleCoverageScope::Account{agent_id,account_id} if agent_id==&f.agent && account_id==&f.account)).unwrap()
}

#[test]
fn empty_inventory_is_explicit_unchanged_poll_is_inert_and_facts_stay_empty() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::new(&mut repo);
    assert!(repo
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .coverage_inventories
        .is_empty());
    let at = Utc::now();
    let sequence = repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, at)
        .unwrap()
        .unwrap();
    let first = f.inventory(&repo);
    assert_eq!(first.sequence, sequence);
    assert_eq!(first.observed_at, at);
    assert_eq!(first.scopes.len(), 3);
    assert!(first
        .scopes
        .iter()
        .all(|s| s.gaps.is_empty() && s.backlog_gaps.is_empty()));
    assert!(repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, at + chrono::Duration::days(1))
        .unwrap()
        .is_none());
    assert_eq!(f.inventory(&repo), first);
    assert!(repo
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .events
        .is_empty());
    assert!(repo
        .observe_lifecycle_coverage_inventory_at(&UserId::new(), at)
        .is_err());
}

#[test]
fn inventory_state_can_return_a_to_b_to_a_and_old_watermarks_remain_frozen() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::new(&mut repo);
    let a = repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap()
        .unwrap();
    let decision = f.legacy(&mut repo, Effect::NeedsApproval);
    let b = repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap()
        .unwrap();
    assert!(b > a);
    let pending = f.inventory(&repo);
    let scope = account_scope(&pending, &f);
    assert!(scope.gaps.contains(&LifecycleCoverageGap {
        source: LifecycleSource::HumanApproval,
        metric: LifecycleCount::HumanEscalation
    }));
    assert_eq!(
        scope.backlog_gaps,
        vec![LifecycleCoverageBacklog::HumanApproval]
    );
    append(
        &mut repo,
        &decision,
        LifecycleFact::InitialPolicy {
            decision: InitialPolicyDecision::NeedsApproval,
        },
        LifecycleSource::PolicyEngine,
    );
    append(
        &mut repo,
        &decision,
        LifecycleFact::HumanEscalation {
            approval_request_id: decision.id.clone(),
        },
        LifecycleSource::HumanApproval,
    );
    let second_a = repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap()
        .unwrap();
    assert!(second_a > b);
    assert!(account_scope(&f.inventory(&repo), &f).gaps.is_empty());
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, Some(b))
            .unwrap()
            .coverage_inventories
            .last()
            .unwrap(),
        &pending
    );
    assert!(repo
        .lifecycle_snapshot(&f.owner, Some(a))
        .unwrap()
        .coverage_inventories[0]
        .scopes
        .iter()
        .all(|s| s.gaps.is_empty()));
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, None)
            .unwrap()
            .events
            .len(),
        2
    );
}

#[test]
fn human_terminal_changes_legacy_metadata_without_rewriting_a_snapshot_or_inventing_events() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::new(&mut repo);
    let decision = f.legacy(&mut repo, Effect::NeedsApproval);
    let before = repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap()
        .unwrap();
    let pending = f.inventory(&repo);
    repo.record_spend_attempt_outcome(
        &decision,
        SpendAuthorizationDecision::Denied,
        &["arbitrary prose that is never interpreted".into()],
        Utc::now(),
    )
    .unwrap();
    repo.observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap();
    let closed = f.inventory(&repo);
    assert!(account_scope(&closed, &f)
        .gaps
        .contains(&LifecycleCoverageGap {
            source: LifecycleSource::HumanApproval,
            metric: LifecycleCount::HumanDeclined
        }));
    assert!(!account_scope(&closed, &f)
        .gaps
        .iter()
        .any(|g| g.source == LifecycleSource::AutomatedAdmission));
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, Some(before))
            .unwrap()
            .coverage_inventories
            .last()
            .unwrap(),
        &pending
    );
    assert!(repo
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .events
        .is_empty());
}

#[test]
fn evaluation_placeholder_is_not_a_genuine_human_request() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::new(&mut repo);
    let mut request = f.legacy(&mut repo, Effect::Allow).request;
    request.reason = "new placeholder".into();
    repo.admit_spend_attempt(&f.owner, "placeholder", &request, "test", Utc::now())
        .unwrap();
    repo.observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap();
    let inventory = f.inventory(&repo);
    assert!(!inventory
        .scopes
        .iter()
        .flat_map(|s| &s.gaps)
        .any(|g| matches!(
            g.metric,
            LifecycleCount::HumanEscalation
                | LifecycleCount::HumanApproved
                | LifecycleCount::HumanDeclined
                | LifecycleCount::ExecutionFailed
        )));
    assert!(inventory.scopes.iter().all(|s| s.backlog_gaps.is_empty()));
}

#[test]
fn foreign_accounts_fall_back_to_owned_agent_and_foreign_agents_to_owner_only() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let a = Fixture::new(&mut repo);
    let b = Fixture::new(&mut repo);
    b.legacy(&mut repo, Effect::NeedsApproval);
    repo.observe_lifecycle_coverage_inventory_at(&a.owner, Utc::now())
        .unwrap();
    assert!(a.inventory(&repo).scopes.iter().all(|s| s.gaps.is_empty()));
    let mismatch = Fixture {
        owner: a.owner.clone(),
        agent: a.agent.clone(),
        account: b.account.clone(),
    };
    mismatch.legacy(&mut repo, Effect::NeedsApproval);
    repo.observe_lifecycle_coverage_inventory_at(&a.owner, Utc::now())
        .unwrap();
    let inv = a.inventory(&repo);
    assert!(account_scope(&inv, &a).gaps.is_empty());
    assert!(inv.scopes.iter().any(|s|matches!(&s.scope,LifecycleCoverageScope::AgentUnattributed{agent_id} if agent_id==&a.agent)&&!s.gaps.is_empty()));
    assert!(!inv.scopes.iter().any(|s|matches!(&s.scope,LifecycleCoverageScope::Account{account_id,..} if account_id==&b.account)));
    let foreign = Fixture {
        owner: a.owner.clone(),
        agent: b.agent.clone(),
        account: b.account.clone(),
    };
    foreign.legacy(&mut repo, Effect::Deny);
    repo.observe_lifecycle_coverage_inventory_at(&a.owner, Utc::now())
        .unwrap();
    let inv = a.inventory(&repo);
    assert!(inv
        .scopes
        .iter()
        .any(
            |s| matches!(s.scope, LifecycleCoverageScope::OwnerUnattributed) && !s.gaps.is_empty()
        ));
}

#[test]
fn failed_inventory_insert_rolls_back_and_immutable_observations_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inventory.sqlite");
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    let f = Fixture::new(&mut repo);
    repo.conn.execute_batch("CREATE TRIGGER fail_inventory BEFORE INSERT ON spend_lifecycle_records WHEN json_extract(NEW.payload_json,'$.record_type')='coverage_inventory' BEGIN SELECT RAISE(ABORT,'test inventory failure'); END;").unwrap();
    assert!(repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .is_err());
    assert!(repo
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .coverage_inventories
        .is_empty());
    repo.conn
        .execute_batch("DROP TRIGGER fail_inventory")
        .unwrap();
    repo.observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap();
    let before = f.inventory(&repo);
    assert!(repo
        .conn
        .execute(
            "UPDATE spend_lifecycle_records SET payload_json=payload_json WHERE sequence=?1",
            [before.sequence]
        )
        .is_err());
    assert!(repo
        .conn
        .execute(
            "DELETE FROM spend_lifecycle_records WHERE sequence=?1",
            [before.sequence]
        )
        .is_err());
    drop(repo);
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(f.inventory(&repo), before);
    assert!(repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap()
        .is_none());
}

#[test]
fn inventory_and_fact_lookup_indexes_are_used() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let f = Fixture::new(&mut repo);
    repo.observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap();
    for (sql,index) in [("EXPLAIN QUERY PLAN SELECT sequence FROM spend_lifecycle_records WHERE owner_user_id=?1 AND json_extract(payload_json,'$.record_type')='coverage_inventory' ORDER BY sequence DESC LIMIT 1","spend_lifecycle_inventory_owner_sequence"),("EXPLAIN QUERY PLAN SELECT 1 FROM spend_lifecycle_records WHERE owner_user_id=?1 AND json_extract(payload_json,'$.record_type')='event' AND json_extract(payload_json,'$.input.correlation.decision_id')='decision' AND json_extract(payload_json,'$.input.fact.kind')='initial_policy' LIMIT 1","spend_lifecycle_fact_decision_kind")] {
        let mut stmt=repo.conn.prepare(sql).unwrap();let plans=stmt.query_map([f.owner.to_string()],|r|r.get::<_,String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();assert!(plans.iter().any(|p|p.contains(index)),"{plans:?}");
    }
}

#[test]
fn financial_gaps_use_exact_eligibility_and_remain_frozen_after_real_reconciliation() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let expiry = Utc::now() - chrono::Duration::seconds(1);
    let (claim, _, _) = super::super::tests::persist_claimed_executor_spend(&mut repo, expiry);
    let decision = repo.load_spend_decisions().unwrap().remove(0);
    seed_lifecycle_test_identity(
        &mut repo,
        &claim.owner_user_id,
        &claim.agent_id,
        &decision.request.agent_account_id,
    );
    repo.conn
        .execute(
            "UPDATE spend_executor_claims SET claimed_at=?2 WHERE id=?1",
            params![
                claim.id.to_string(),
                (expiry - chrono::Duration::minutes(15)).to_rfc3339()
            ],
        )
        .unwrap();
    let f = Fixture {
        owner: claim.owner_user_id.clone(),
        agent: claim.agent_id.clone(),
        account: decision.request.agent_account_id.clone(),
    };
    repo.observe_lifecycle_coverage_inventory_at(
        &f.owner,
        expiry - chrono::Duration::nanoseconds(1),
    )
    .unwrap();
    assert!(!account_scope(&f.inventory(&repo), &f)
        .gaps
        .iter()
        .any(|g| g.metric == LifecycleCount::ReconciliationRequired));
    let at_expiry = repo
        .observe_lifecycle_coverage_inventory_at(&f.owner, expiry)
        .unwrap()
        .unwrap();
    let open = f.inventory(&repo);
    let scope = account_scope(&open, &f);
    assert!(scope.gaps.contains(&LifecycleCoverageGap {
        source: LifecycleSource::Executor,
        metric: LifecycleCount::ExecutionStarted
    }));
    assert!(scope.gaps.contains(&LifecycleCoverageGap {
        source: LifecycleSource::Reconciliation,
        metric: LifecycleCount::ReconciliationRequired
    }));
    assert_eq!(
        scope.backlog_gaps,
        vec![LifecycleCoverageBacklog::FinancialReconciliation]
    );
    assert!(repo
        .lifecycle_snapshot(&f.owner, None)
        .unwrap()
        .events
        .is_empty());
    repo.reconcile_executor_claim_as_not_billed_transactionally(
        &claim.id,
        &f.owner,
        "confirmed-provider-reference",
        "confirmed no charge",
        Utc::now(),
    )
    .unwrap();
    repo.observe_lifecycle_coverage_inventory_at(&f.owner, Utc::now())
        .unwrap();
    let closed = f.inventory(&repo);
    let scope = account_scope(&closed, &f);
    assert!(!scope.gaps.iter().any(|g| matches!(
        g.metric,
        LifecycleCount::ExecutionFailed
            | LifecycleCount::ReconciliationRequired
            | LifecycleCount::ReconciliationResolved
            | LifecycleCount::ExecutionReleased
    )));
    assert!(scope.backlog_gaps.is_empty());
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, Some(at_expiry))
            .unwrap()
            .coverage_inventories
            .last()
            .unwrap(),
        &open
    );
    assert_eq!(
        repo.lifecycle_snapshot(&f.owner, None)
            .unwrap()
            .events
            .len(),
        3
    );
}

#[test]
fn unavailable_identity_catalog_never_becomes_an_assessed_empty_inventory() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    repo.observe_all_lifecycle_coverage_inventories_at(Utc::now())
        .unwrap();
    assert_eq!(
        repo.conn
            .query_row("SELECT COUNT(*) FROM spend_lifecycle_records", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
