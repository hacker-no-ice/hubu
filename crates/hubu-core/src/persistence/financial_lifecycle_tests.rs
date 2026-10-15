use super::*;
use crate::spend::{LifecycleBacklogTransition, LifecycleCount};
fn debug_state(value: impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

fn fixture(
    repo: &mut SqliteGovernanceRepository,
    expires: DateTime<Utc>,
) -> (SpendExecutorClaimRecord, SpendAuthTokenRecord, BudgetHold) {
    fixture_currency(repo, expires, Currency::Usd)
}
fn fixture_currency(
    repo: &mut SqliteGovernanceRepository,
    expires: DateTime<Utc>,
    currency: Currency,
) -> (SpendExecutorClaimRecord, SpendAuthTokenRecord, BudgetHold) {
    let (mut claim, token, hold) = super::super::tests::persist_claimed_executor_spend_with_amounts(
        repo,
        expires,
        "gongbu-operation-transactional",
        super::super::tests::agent_id(),
        Amount::from_units(100_000_000),
        Amount::from_units(25_000_000),
        currency,
    );
    let decision = repo.load_spend_decisions().unwrap().remove(0);
    seed_lifecycle_test_identity(
        repo,
        &claim.owner_user_id,
        &claim.agent_id,
        &decision.request.agent_account_id,
    );
    claim.claimed_at = expires - chrono::Duration::minutes(15);
    repo.conn
        .execute(
            "UPDATE spend_executor_claims SET claimed_at=?2 WHERE id=?1",
            params![claim.id.to_string(), claim.claimed_at.to_rfc3339()],
        )
        .unwrap();
    (claim, token, hold)
}
fn snapshot(
    repo: &SqliteGovernanceRepository,
    claim: &SpendExecutorClaimRecord,
) -> LifecycleSnapshot {
    repo.lifecycle_snapshot(&claim.owner_user_id, None).unwrap()
}
fn categories(s: &LifecycleSnapshot) -> Vec<LifecycleCount> {
    s.events
        .iter()
        .map(|e| e.event.fact.count_category())
        .collect()
}
fn fail_kind(repo: &SqliteGovernanceRepository, kind: &str) {
    repo.conn.execute_batch(&format!("CREATE TRIGGER fail_financial_evidence BEFORE INSERT ON spend_lifecycle_records WHEN json_extract(NEW.payload_json,'$.input.fact.kind')='{kind}' BEGIN SELECT RAISE(ABORT,'injected financial evidence fault'); END;")).unwrap();
}
fn unclaim(
    repo: &SqliteGovernanceRepository,
    claim: &SpendExecutorClaimRecord,
    hold: &BudgetHold,
) -> BudgetHold {
    repo.conn
        .execute(
            "DELETE FROM spend_executor_claims WHERE id=?1",
            [claim.id.to_string()],
        )
        .unwrap();
    repo.conn
        .execute(
            "UPDATE budget_holds SET status='frozen',executor_claim_id=NULL WHERE id=?1",
            [hold.id.to_string()],
        )
        .unwrap();
    let mut hold = hold.clone();
    hold.status = BudgetHoldStatus::Frozen;
    hold.executor_claim_id = None;
    hold
}

#[test]
fn idle_observation_exact_boundary_is_evidence_only_and_restart_stable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("observed.sqlite");
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    let expiry = Utc::now() - chrono::Duration::seconds(1);
    let (claim, _, _) = fixture(&mut repo, expiry);
    let balances = repo.load_budget_balances().unwrap();
    let holds = repo.load_budget_holds().unwrap();
    repo.observe_financial_lifecycle_at(expiry - chrono::Duration::nanoseconds(1))
        .unwrap();
    assert!(snapshot(&repo, &claim).events.is_empty());
    repo.observe_financial_lifecycle_at(expiry).unwrap();
    let first = snapshot(&repo, &claim);
    assert_eq!(
        categories(&first),
        vec![LifecycleCount::ReconciliationRequired]
    );
    assert_eq!(first.events[0].event.occurred_at, expiry);
    assert_eq!(
        first.events[0].event.source,
        LifecycleSource::Reconciliation
    );
    assert!(first
        .capture_markers
        .iter()
        .all(|m| m.effective_at >= expiry));
    assert_eq!(
        debug_state(repo.load_budget_balances().unwrap()),
        debug_state(balances)
    );
    assert_eq!(
        debug_state(repo.load_budget_holds().unwrap()),
        debug_state(holds)
    );
    let mut second = SqliteGovernanceRepository::open(&path).unwrap();
    second.observe_financial_lifecycle_at(Utc::now()).unwrap();
    assert_eq!(snapshot(&second, &claim).events, first.events);
    assert_eq!(snapshot(&second, &claim).watermark, first.watermark);
    assert_eq!(
        repo.load_executor_claims().unwrap()[0].status,
        SpendExecutorClaimStatus::Claimed
    );
}

#[test]
fn reconciliation_without_poll_atomically_preserves_entry_and_closure_and_frozen_history() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let expiry = Utc::now() - chrono::Duration::seconds(1);
    let (claim, _, _) = fixture(&mut repo, expiry);
    let frozen = snapshot(&repo, &claim).watermark;
    repo.reconcile_executor_claim_as_not_billed_transactionally(
        &claim.id,
        &claim.owner_user_id,
        "provider:confirmed",
        "no charge confirmed",
        Utc::now(),
    )
    .unwrap();
    let after = snapshot(&repo, &claim);
    assert_eq!(
        categories(&after),
        vec![
            LifecycleCount::ReconciliationRequired,
            LifecycleCount::ExecutionReleased,
            LifecycleCount::ReconciliationResolved
        ]
    );
    assert_eq!(after.events[0].event.occurred_at, expiry);
    let persisted = repo.load_executor_claims().unwrap().remove(0);
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    capture_required(&tx, &persisted, Utc::now()).unwrap();
    tx.commit().unwrap();
    assert_eq!(snapshot(&repo, &claim).events, after.events);
    assert!(repo
        .lifecycle_snapshot(&claim.owner_user_id, Some(frozen))
        .unwrap()
        .events
        .is_empty());
    assert!(matches!(
        after.events.last().unwrap().event.fact.backlog_transition(),
        Some(LifecycleBacklogTransition::Resolve(_))
    ));
    repo.reconcile_executor_claim_as_not_billed_transactionally(
        &claim.id,
        &claim.owner_user_id,
        "provider:confirmed",
        "no charge confirmed",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(snapshot(&repo, &claim).watermark, after.watermark);
}

#[test]
fn evidence_failure_rolls_back_reconciliation_and_retry_is_one_pair() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, token, hold) = fixture(&mut repo, Utc::now() - chrono::Duration::seconds(1));
    let balances = repo.load_budget_balances().unwrap();
    fail_kind(&repo, "reconciliation_resolved");
    assert!(repo
        .reconcile_executor_claim_as_not_billed_transactionally(
            &claim.id,
            &claim.owner_user_id,
            "provider:confirmed",
            "no bill",
            Utc::now()
        )
        .is_err());
    assert_eq!(
        repo.load_executor_claims().unwrap()[0].status,
        SpendExecutorClaimStatus::Claimed
    );
    assert_eq!(
        debug_state(&repo.load_spend_auth_tokens().unwrap()[0]),
        debug_state(token)
    );
    assert_eq!(
        debug_state(&repo.load_budget_holds().unwrap()[0]),
        debug_state(hold)
    );
    assert_eq!(
        debug_state(repo.load_budget_balances().unwrap()),
        debug_state(balances)
    );
    assert!(snapshot(&repo, &claim).events.is_empty());
    repo.conn
        .execute_batch("DROP TRIGGER fail_financial_evidence")
        .unwrap();
    repo.reconcile_executor_claim_as_not_billed_transactionally(
        &claim.id,
        &claim.owner_user_id,
        "provider:confirmed",
        "no bill",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(snapshot(&repo, &claim).events.len(), 3);
}

#[test]
fn normal_preexpiry_release_never_opens_financial_backlog() {
    for currency in [Currency::Usd, Currency::Usdc] {
        assert_normal_preexpiry_release(currency);
    }
}
fn assert_normal_preexpiry_release(currency: Currency) {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, _, _) = fixture_currency(
        &mut repo,
        Utc::now() + chrono::Duration::minutes(10),
        currency,
    );
    let before = snapshot(&repo, &claim).watermark;
    fail_kind(&repo, "execution_finalized");
    assert!(repo
        .release_executor_claim_transactionally(
            &claim.owner_user_id,
            &claim.agent_id,
            &claim.operation_key,
            Utc::now()
        )
        .is_err());
    assert_eq!(snapshot(&repo, &claim).watermark, before);
    repo.conn
        .execute_batch("DROP TRIGGER fail_financial_evidence")
        .unwrap();
    repo.release_executor_claim_transactionally(
        &claim.owner_user_id,
        &claim.agent_id,
        &claim.operation_key,
        Utc::now(),
    )
    .unwrap();
    repo.observe_financial_lifecycle_at(claim.expires_at + chrono::Duration::seconds(1))
        .unwrap();
    assert_eq!(
        categories(&snapshot(&repo, &claim)),
        vec![LifecycleCount::ExecutionReleased]
    );
    let finalized = repo.load_executor_claims().unwrap().remove(0);
    assert!(validate_reconciliation_entry(&finalized, claim.expires_at).is_err());

    let final_event = snapshot(&repo, &claim).events.remove(0);
    assert!(
        matches!(final_event.event.requested_amount, LifecycleEvidence::Known { value } if value.currency == currency)
    );
}

#[test]
fn claim_and_preclaim_void_evidence_faults_roll_back_and_replay_is_inert() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (mut claim, _token, hold) = fixture(&mut repo, Utc::now() + chrono::Duration::minutes(10));
    let _ = unclaim(&repo, &claim, &hold);
    claim.claimed_at = Utc::now();
    let balance = repo.load_budget_balances().unwrap().remove(0);
    fail_kind(&repo, "execution_started");
    assert!(repo
        .save_executor_claim_with_budget_hold(&claim, &hold, &balance)
        .is_err());
    assert!(repo.load_executor_claims().unwrap().is_empty());
    assert!(matches!(
        repo.load_budget_holds().unwrap()[0].status,
        BudgetHoldStatus::Frozen
    ));
    repo.conn
        .execute_batch("DROP TRIGGER fail_financial_evidence")
        .unwrap();
    repo.save_executor_claim_with_budget_hold(&claim, &hold, &balance)
        .unwrap();
    repo.save_executor_claim_with_budget_hold(&claim, &hold, &balance)
        .unwrap();
    assert_eq!(
        categories(&snapshot(&repo, &claim)),
        vec![LifecycleCount::ExecutionStarted]
    );
    // A separate unclaimed authorization uses its own operation and fact slot.
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, token, hold) = fixture(&mut repo, Utc::now() + chrono::Duration::minutes(10));
    let _ = unclaim(&repo, &claim, &hold);
    let account = repo
        .load_spend_decisions()
        .unwrap()
        .remove(0)
        .request
        .agent_account_id;
    fail_kind(&repo, "authorization_released");
    assert!(repo
        .void_unclaimed_authorization(
            &claim.owner_user_id,
            &account,
            &token.id,
            &claim.operation_key
        )
        .is_err());
    assert_eq!(repo.load_spend_auth_tokens().unwrap()[0].revoked_at, None);
    assert!(matches!(
        repo.load_budget_holds().unwrap()[0].status,
        BudgetHoldStatus::Frozen
    ));
    repo.conn
        .execute_batch("DROP TRIGGER fail_financial_evidence")
        .unwrap();
    repo.void_unclaimed_authorization(
        &claim.owner_user_id,
        &account,
        &token.id,
        &claim.operation_key,
    )
    .unwrap();
    let after = snapshot(&repo, &claim);
    assert_eq!(
        categories(&after),
        vec![LifecycleCount::AuthorizationReleased]
    );
    assert!(
        repo.void_unclaimed_authorization(
            &claim.owner_user_id,
            &account,
            &token.id,
            &claim.operation_key
        )
        .unwrap()
        .idempotent_replay
    );
    assert_eq!(snapshot(&repo, &claim).watermark, after.watermark);
}

#[test]
fn unused_expiry_records_actual_release_time_and_used_legacy_hold_skips_evidence() {
    for used in [false, true] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let expires = Utc::now() - chrono::Duration::seconds(3);
        let (claim, token, hold) = fixture(&mut repo, expires);
        let _ = unclaim(&repo, &claim, &hold);
        if used {
            repo.conn
                .execute(
                    "UPDATE spend_auth_tokens SET used_at=?2 WHERE id=?1",
                    params![token.id.to_string(), expires.to_rfc3339()],
                )
                .unwrap();
        }
        let observed = Utc::now();
        repo.expire_overdue_budget_holds(observed).unwrap();
        let result = snapshot(&repo, &claim);
        if used {
            assert!(result.events.is_empty());
        } else {
            assert_eq!(
                categories(&result),
                vec![LifecycleCount::AuthorizationReleased]
            );
            assert_eq!(result.events[0].event.occurred_at, observed);
        }
        assert!(matches!(
            repo.load_budget_holds().unwrap()[0].status,
            BudgetHoldStatus::Expired
        ));
        assert_eq!(
            repo.load_budget_balances().unwrap()[0].frozen_amount,
            Amount::ZERO
        );
    }
}

#[test]
fn billed_receipt_accounting_and_evidence_commit_or_roll_back_together() {
    for (currency, reconciled) in [
        (Currency::Usd, false),
        (Currency::Usd, true),
        (Currency::Usdc, false),
        (Currency::Usdc, true),
    ] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let now = Utc::now();
        let expiry = if reconciled {
            now - chrono::Duration::seconds(1)
        } else {
            now + chrono::Duration::minutes(10)
        };
        let (claim, token, hold) = fixture_currency(&mut repo, expiry, currency);
        let balance = debug_state(repo.load_budget_balances().unwrap());
        let mut receipt = super::super::tests::precise_settlement_receipt(5_001, 6);
        receipt.actual_vendor_cost.currency = currency;
        receipt.price_model_snapshot["currency"] = serde_json::json!(currency.to_string());
        let settle = |repo: &mut SqliteGovernanceRepository| {
            if reconciled {
                repo.reconcile_executor_claim_as_billed_transactionally(
                    &claim.id,
                    &claim.owner_user_id,
                    "invoice:1",
                    "confirmed invoice",
                    PaymentId::new(),
                    receipt.clone(),
                    now,
                )
            } else {
                repo.settle_executor_claim_transactionally(
                    &claim.owner_user_id,
                    &claim.agent_id,
                    &claim.operation_key,
                    PaymentId::new(),
                    receipt.clone(),
                    now,
                )
            }
        };
        fail_kind(
            &repo,
            if reconciled {
                "reconciliation_resolved"
            } else {
                "execution_finalized"
            },
        );
        assert!(settle(&mut repo).is_err());
        assert_eq!(
            repo.load_executor_claims().unwrap()[0].status,
            SpendExecutorClaimStatus::Claimed
        );
        assert_eq!(
            debug_state(&repo.load_spend_auth_tokens().unwrap()[0]),
            debug_state(&token)
        );
        assert_eq!(
            debug_state(&repo.load_budget_holds().unwrap()[0]),
            debug_state(&hold)
        );
        assert_eq!(debug_state(repo.load_budget_balances().unwrap()), balance);
        assert!(
            load_executor_settlement_receipt_by_claim_id(&repo.conn, &claim.id)
                .unwrap()
                .is_none()
        );
        let postings: i64 = repo
            .conn
            .query_row("SELECT COUNT(*) FROM ledger_transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(postings, 0);
        assert!(snapshot(&repo, &claim).events.is_empty());
        repo.conn
            .execute_batch("DROP TRIGGER fail_financial_evidence")
            .unwrap();
        let settled = settle(&mut repo).unwrap();
        let after = snapshot(&repo, &claim);
        assert_eq!(after.events.len(), if reconciled { 3 } else { 1 });
        assert_eq!(settled.receipt.unwrap().receipt, receipt);
        assert_eq!(settled.balance.consumed_amount.units(), 5_001);
        assert_eq!(settled.balance.remaining_amount.units(), 99_994_999);
        assert!(after.events.iter().all(|event| matches!(&event.event.requested_amount, LifecycleEvidence::Known { value } if value.currency == currency)));
        let ledger =
            hubu_ledger::domain::owner_transactions(&repo.conn, &claim.owner_user_id).unwrap();
        assert_eq!(ledger.len(), 1);
        let metadata = ledger[0].metadata.as_ref().unwrap();
        assert_eq!(metadata.effective_cost.currency, currency);
        assert_eq!(metadata.budget_charge_units().unwrap(), Some(5_001));
        assert!(ledger[0]
            .entries
            .iter()
            .all(|entry| entry.amount.currency == currency));
        let postings: i64 = repo
            .conn
            .query_row("SELECT COUNT(*) FROM ledger_transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(postings, 1);
        assert!(settle(&mut repo).unwrap().idempotent_replay);
        assert_eq!(snapshot(&repo, &claim).watermark, after.watermark);
    }
}

#[test]
fn required_fact_rejects_preclaim_preexpiry_and_wrong_source_but_accepts_late_history() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let expiry = Utc::now() - chrono::Duration::seconds(1);
    let (claim, _, _) = fixture(&mut repo, expiry);
    let mut input = claim_event(
        &repo.conn,
        &claim,
        LifecycleSource::Reconciliation,
        LifecycleFact::ReconciliationRequired {
            claim_id: claim.id.clone(),
        },
        expiry,
    )
    .unwrap()
    .unwrap();
    input.occurred_at = claim.claimed_at - chrono::Duration::nanoseconds(1);
    assert!(repo.append_lifecycle_event(&input).is_err());
    input.occurred_at = expiry - chrono::Duration::nanoseconds(1);
    assert!(repo.append_lifecycle_event(&input).is_err());
    input.occurred_at = expiry;
    input.source = LifecycleSource::Executor;
    assert!(repo.append_lifecycle_event(&input).is_err());
    input.source = LifecycleSource::Reconciliation;
    let mut impossible = claim.clone();
    impossible.claimed_at = expiry + chrono::Duration::nanoseconds(1);
    assert!(validate_reconciliation_entry(&impossible, Utc::now()).is_err());
    assert!(
        validate_reconciliation_entry(&claim, expiry - chrono::Duration::nanoseconds(1)).is_err()
    );
    repo.append_lifecycle_event(&input).unwrap();
}

#[test]
fn ineligible_legacy_claim_does_not_starve_other_observations() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let expiry = Utc::now() - chrono::Duration::seconds(1);
    let (valid, _, _) = fixture(&mut repo, expiry);
    let (invalid, _, _) = super::super::tests::persist_claimed_executor_spend_with_operation_key(
        &mut repo,
        expiry,
        "ineligible-legacy-operation",
    );
    assert!(invalid.claimed_at > invalid.expires_at);
    repo.observe_financial_lifecycle_at(Utc::now()).unwrap();
    assert_eq!(
        categories(&snapshot(&repo, &valid)),
        vec![LifecycleCount::ReconciliationRequired]
    );
}
