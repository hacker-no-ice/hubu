fn settle_for_accounting(
    repo: &mut SqliteGovernanceRepository,
    cost: i64,
    scale: u32,
) -> ExecutorFinalizationResult {
    let now = Utc::now();
    let (claim, _, _) = persist_claimed_executor_spend(repo, now + Duration::minutes(15));
    repo.settle_executor_claim_transactionally(
        &claim.owner_user_id,
        &claim.agent_id,
        &claim.operation_key,
        PaymentId::new(),
        precise_settlement_receipt(cost, scale),
        now,
    )
    .unwrap()
}

#[test]
fn provider_accounting_is_balanced_precise_immutable_and_reconciles() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let settled = settle_for_accounting(&mut repo, 24_991, 3);
    let records = repo.provider_accounting_records(&user_id()).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.expense_delta_amount, "24991000000000000000");
    assert_eq!(record.rounding_delta_amount, "9000000000000000");
    assert_eq!(record.budget_charge_delta_cents, 2500);
    assert_eq!(record.billing_merchant.as_deref(), Some("Acme"));
    assert!(record.provider.is_some());
    assert!(record.purpose.is_some());
    assert_eq!(record.claim_id, settled.claim.id);
    let lines: Vec<(String, String, String)> = repo
        .conn
        .prepare(
            "SELECT account, direction, amount FROM provider_accounting_lines ORDER BY account",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        (
            "externally_billed_clearing".to_string(),
            "credit".to_string(),
            record.expense_delta_amount.clone()
        )
    );
    assert_eq!(
        lines[1],
        (
            "provider_spend_expense".to_string(),
            "debit".to_string(),
            record.expense_delta_amount.clone()
        )
    );
    assert!(repo
        .conn
        .execute(
            "UPDATE provider_accounting_journal SET budget_charge_delta_cents = 1",
            []
        )
        .is_err());
    assert!(repo
        .conn
        .execute("DELETE FROM provider_accounting_journal", [])
        .is_err());
    assert!(repo.conn.execute("INSERT INTO provider_accounting_lines VALUES ('bad','owner','budget','cash','debit','1',18,'usd')", []).is_err());
    let report = repo
        .reconcile_provider_budget(&user_id(), &settled.hold.budget_id)
        .unwrap();
    assert_eq!(report.consumed_amount_cents, 2500);
    assert_eq!(report.provider_budget_charges_cents, 2500);
    assert_eq!(report.other_or_unaccounted_consumption_cents, 0);
    assert!(repo
        .provider_accounting_records(&UserId::new())
        .unwrap()
        .is_empty());
    assert!(repo
        .reconcile_provider_budget(&UserId::new(), &settled.hold.budget_id)
        .is_err());
}

#[test]
fn provider_accounting_write_failure_rolls_back_entire_finalization() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let now = Utc::now();
    let (claim, token, hold) =
        persist_claimed_executor_spend(&mut repo, now + Duration::minutes(15));
    repo.conn.execute_batch("CREATE TRIGGER fail_provider_posting BEFORE INSERT ON provider_accounting_journal BEGIN SELECT RAISE(ABORT, 'injected posting failure'); END;").unwrap();
    assert!(repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &claim.operation_key,
            PaymentId::new(),
            settlement_receipt(2000),
            now
        )
        .is_err());
    assert!(repo
        .provider_accounting_records(&user_id())
        .unwrap()
        .is_empty());
    assert!(
        load_executor_settlement_receipt_by_claim_id(&repo.conn, &claim.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        load_executor_claim_by_id(&repo.conn, &claim.id)
            .unwrap()
            .unwrap()
            .status,
        SpendExecutorClaimStatus::Claimed
    );
    assert!(load_spend_auth_token_by_id(&repo.conn, &token.id)
        .unwrap()
        .unwrap()
        .used_at
        .is_none());
    let balance = load_budget_balance_by_id(&repo.conn, &hold.budget_id)
        .unwrap()
        .unwrap();
    assert_eq!(balance.consumed_amount_cents, 0);
    assert_eq!(balance.frozen_amount_cents, 2500);
    repo.conn
        .execute_batch("DROP TRIGGER fail_provider_posting;")
        .unwrap();
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &agent_id(),
        &claim.operation_key,
        PaymentId::new(),
        settlement_receipt(2000),
        now,
    )
    .unwrap();
    assert_eq!(
        repo.provider_accounting_records(&user_id()).unwrap().len(),
        1
    );
}

#[test]
fn provider_accounting_response_loss_restart_and_backfill_are_idempotent() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    let settled = settle_for_accounting(&mut repo, 19_991, 3);
    let record = repo
        .provider_accounting_records(&user_id())
        .unwrap()
        .remove(0);
    drop(repo);
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    let replay = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &settled.claim.operation_key,
            PaymentId::new(),
            precise_settlement_receipt(19_991, 3),
            Utc::now(),
        )
        .unwrap();
    assert!(replay.idempotent_replay);
    assert_eq!(
        repo.provider_accounting_records(&user_id()).unwrap(),
        vec![record]
    );
    // Simulate an old deployment which persisted settlement before this schema existed.
    repo.conn
        .execute_batch(
            "DROP VIEW provider_accounting_lines; DROP TABLE provider_accounting_journal;",
        )
        .unwrap();
    drop(repo);
    for _ in 0..2 {
        let repo = SqliteGovernanceRepository::open(file.path()).unwrap();
        let records = repo.provider_accounting_records(&user_id()).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].legacy_backfill);
        assert_eq!(records[0].effective_vendor_cost.amount, 19_991);
        assert_eq!(
            repo.reconcile_provider_budget(&user_id(), &settled.hold.budget_id)
                .unwrap()
                .other_or_unaccounted_consumption_cents,
            0
        );
    }
}

#[test]
fn provider_accounting_marks_missing_legacy_evidence_without_fabrication() {
    for missing in [
        "claim",
        "decision",
        "hold",
        "receipt",
        "hold_status",
        "token_link",
        "settlement_link",
    ] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let settled = settle_for_accounting(&mut repo, 2000, 2);
        repo.conn.execute_batch("PRAGMA foreign_keys = OFF; DROP VIEW provider_accounting_lines; DROP TABLE provider_accounting_journal;").unwrap();
        match missing {
            "claim" => {
                repo.conn
                    .execute("DELETE FROM spend_executor_claims", [])
                    .unwrap();
            }
            "decision" => {
                repo.conn
                    .execute_batch(
                        "DROP TRIGGER spend_decisions_no_delete; DELETE FROM spend_decisions;",
                    )
                    .unwrap();
            }
            "hold" => {
                repo.conn.execute("DELETE FROM budget_holds", []).unwrap();
            }
            "receipt" => {
                repo.conn.execute_batch("DROP TRIGGER spend_executor_settlement_receipts_no_delete; DELETE FROM spend_executor_settlement_receipts;").unwrap();
            }
            "hold_status" => {
                repo.conn
                    .execute("UPDATE budget_holds SET status = 'claimed'", [])
                    .unwrap();
            }
            "token_link" => {
                repo.conn
                    .execute(
                        "UPDATE spend_auth_tokens SET used_by_payment_id = ?1",
                        [PaymentId::new().to_string()],
                    )
                    .unwrap();
            }
            _ => {
                repo.conn
                    .execute(
                        "UPDATE spend_executor_claims SET settlement_id = ?1",
                        [PaymentId::new().to_string()],
                    )
                    .unwrap();
            }
        }
        repo.initialize_provider_accounting().unwrap();
        repo.initialize_provider_accounting().unwrap();
        assert!(
            repo.provider_accounting_records(&user_id())
                .unwrap()
                .is_empty(),
            "{missing}"
        );
        let gaps: i64 = repo
            .conn
            .query_row(
                "SELECT COUNT(*) FROM provider_accounting_legacy_gaps",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(gaps, 1, "{missing}");
        assert_eq!(
            load_budget_balance_by_id(&repo.conn, &settled.hold.budget_id)
                .unwrap()
                .unwrap()
                .consumed_amount_cents,
            2000
        );
    }
}

fn accounting_adjustment(
    record: &accounting::ProviderAccountingRecord,
    amount: i64,
    scale: u32,
) -> accounting::ProviderAccountingAdjustment {
    accounting::ProviderAccountingAdjustment {
        owner_user_id: record.owner_user_id.clone(),
        original_entry_id: record.original_entry_id.clone(),
        expected_previous_entry_id: record.id.clone(),
        operation_key: "invoice-correction-1".to_string(),
        corrected_vendor_cost: SpendExecutorVendorCost {
            amount,
            scale,
            currency: Currency::Usd,
        },
        reason: "Provider invoice correction".to_string(),
        evidence: "invoice://corrected".to_string(),
    }
}

#[test]
fn provider_accounting_adjustments_publish_cache_replay_and_preserve_receipts() {
    use std::sync::Mutex;
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let settled = settle_for_accounting(&mut repo, 19_991, 3);
    let original = repo
        .provider_accounting_records(&user_id())
        .unwrap()
        .remove(0);
    let repo = Arc::new(Mutex::new(repo));
    let mut manager = BudgetManager::new().with_repository(repo.clone());
    let command = accounting_adjustment(&original, 10_001, 3);
    let adjustment = manager.adjust_provider_accounting(command.clone()).unwrap();
    assert_eq!(adjustment.budget_charge_delta_cents, -999);
    assert_eq!(adjustment.expense_delta_amount, "-9990000000000000000");
    assert_eq!(adjustment.rounding_delta_amount, "0");
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .consumed_amount_cents,
        1001
    );
    assert_eq!(
        manager.adjust_provider_accounting(command.clone()).unwrap(),
        adjustment
    );
    let mut conflict = command.clone();
    conflict.reason = "changed".to_string();
    assert!(manager.adjust_provider_accounting(conflict).is_err());
    let mut stale = command;
    stale.operation_key = "stale".to_string();
    assert!(manager.adjust_provider_accounting(stale).is_err());
    let mut wrong_owner = accounting_adjustment(&adjustment, 0, 2);
    wrong_owner.owner_user_id = UserId::new();
    assert!(manager.adjust_provider_accounting(wrong_owner).is_err());
    let mut overrun = accounting_adjustment(&adjustment, 200_001, 3);
    overrun.operation_key = "overrun".to_string();
    let corrected = manager.adjust_provider_accounting(overrun).unwrap();
    assert_eq!(corrected.budget_charge_delta_cents, 19000);
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .remaining_amount_cents,
        -10001
    );
    let mut refund = accounting_adjustment(&corrected, 0, 18);
    refund.operation_key = "refund".to_string();
    manager.adjust_provider_accounting(refund).unwrap();
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .consumed_amount_cents,
        0
    );
    let mut repo = repo.lock().unwrap();
    let replay = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &settled.claim.operation_key,
            PaymentId::new(),
            precise_settlement_receipt(19_991, 3),
            Utc::now(),
        )
        .unwrap();
    assert!(replay.idempotent_replay);
    assert_eq!(replay.balance.consumed_amount_cents, 0);
    assert_eq!(replay.receipt, settled.receipt);
    assert_eq!(
        repo.provider_accounting_records(&user_id()).unwrap()[0],
        original
    );
    let report = repo
        .reconcile_provider_budget(&user_id(), &settled.hold.budget_id)
        .unwrap();
    assert_eq!(report.provider_budget_charges_cents, 0);
    assert_eq!(report.other_or_unaccounted_consumption_cents, 0);
}

#[test]
fn provider_accounting_adjustment_failure_preserves_cache_and_balance() {
    use std::sync::Mutex;
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let settled = settle_for_accounting(&mut repo, 2000, 2);
    let original = repo
        .provider_accounting_records(&user_id())
        .unwrap()
        .remove(0);
    let mut manager = BudgetManager::from_records(
        repo.load_budgets().unwrap(),
        repo.load_budget_versions().unwrap(),
        repo.load_budget_balances().unwrap(),
        repo.load_budget_holds().unwrap(),
    )
    .unwrap();
    repo.conn.execute_batch("CREATE TRIGGER fail_adjustment BEFORE INSERT ON provider_accounting_journal BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    let repo = Arc::new(Mutex::new(repo));
    manager = manager.with_repository(repo.clone());
    assert!(manager
        .adjust_provider_accounting(accounting_adjustment(&original, 1000, 2))
        .is_err());
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .consumed_amount_cents,
        2000
    );
    let repo = repo.lock().unwrap();
    assert_eq!(
        repo.reconcile_provider_budget(&user_id(), &settled.hold.budget_id)
            .unwrap()
            .provider_budget_charges_cents,
        2000
    );
    assert_eq!(
        repo.load_budget_balances().unwrap()[0].consumed_amount_cents,
        2000
    );
}

#[test]
fn provider_accounting_pending_release_and_unbilled_create_no_expense() {
    for release in [false, true] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let now = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(&mut repo, now);
        assert!(repo
            .provider_accounting_records(&user_id())
            .unwrap()
            .is_empty());
        if release {
            repo.reconcile_executor_claim_as_not_billed_transactionally(
                &claim.id,
                &user_id(),
                "provider-confirmation",
                "No bill",
                now,
            )
            .unwrap();
        } else {
            repo.release_executor_claim_transactionally(
                &user_id(),
                &agent_id(),
                &claim.operation_key,
                now - Duration::seconds(1),
            )
            .unwrap();
        }
        assert!(repo
            .provider_accounting_records(&user_id())
            .unwrap()
            .is_empty());
    }
}

#[test]
fn provider_accounting_vendor_billed_reconciliation_posts_once_and_preserves_extreme_precision() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let now = Utc::now();
    let (claim, _, hold) = persist_claimed_executor_spend(&mut repo, now);
    let receipt = precise_settlement_receipt(i64::MAX, 2);
    for _ in 0..2 {
        repo.reconcile_executor_claim_as_billed_transactionally(
            &claim.id,
            &user_id(),
            "invoice",
            "confirmed",
            PaymentId::new(),
            receipt.clone(),
            now,
        )
        .unwrap();
    }
    let records = repo.provider_accounting_records(&user_id()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].expense_delta_amount,
        (i128::from(i64::MAX) * 10_i128.pow(16)).to_string()
    );
    assert_eq!(records[0].rounding_delta_amount, "0");
    assert_eq!(
        repo.reconcile_provider_budget(&user_id(), &hold.budget_id)
            .unwrap()
            .other_or_unaccounted_consumption_cents,
        0
    );
}

#[test]
fn provider_accounting_concurrent_settlement_and_adjustment_replays_post_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounting.sqlite");
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    let now = Utc::now();
    let (claim, _, hold) = persist_claimed_executor_spend(&mut repo, now + Duration::minutes(15));
    let repositories: Vec<_> = (0..2)
        .map(|_| SqliteGovernanceRepository::open(&path).unwrap())
        .collect();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = repositories
        .into_iter()
        .map(|mut repo| {
            let claim = claim.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                repo.settle_executor_claim_transactionally(
                    &claim.owner_user_id,
                    &claim.agent_id,
                    &claim.operation_key,
                    PaymentId::new(),
                    settlement_receipt(2000),
                    now,
                )
                .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| result.idempotent_replay)
            .count(),
        1
    );
    let record = repo
        .provider_accounting_records(&user_id())
        .unwrap()
        .remove(0);
    let command = accounting_adjustment(&record, 11_001, 3);
    let repositories: Vec<_> = (0..2)
        .map(|_| SqliteGovernanceRepository::open(&path).unwrap())
        .collect();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = repositories
        .into_iter()
        .map(|mut repo| {
            let command = command.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                repo.adjust_provider_accounting(command).unwrap().0
            })
        })
        .collect();
    let adjusted: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(adjusted[0], adjusted[1]);
    drop(repo);
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(
        repo.adjust_provider_accounting(command).unwrap().0,
        adjusted[0]
    );
    assert_eq!(
        repo.provider_accounting_records(&user_id()).unwrap().len(),
        2
    );
    let report = repo
        .reconcile_provider_budget(&user_id(), &hold.budget_id)
        .unwrap();
    assert_eq!(report.consumed_amount_cents, 1101);
    assert_eq!(report.provider_budget_charges_cents, 1101);
    assert_eq!(report.other_or_unaccounted_consumption_cents, 0);
}
