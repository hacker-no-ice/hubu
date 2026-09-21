//! Integration tests for the ledger domain with governance fixtures.
use super::tests::{
    agent_id, persist_claimed_executor_spend, precise_settlement_receipt, settlement_receipt,
    user_id,
};
use super::*;
use crate::budget::BudgetManager;
use crate::ledger::{AccountingAdjustment, BudgetLedgerQuery, LedgerService};
use chrono::Duration;
use hubu_common::ids::AgentAccountId;
use hubu_ledger::{domain as ledger, ExactMoney, TransactionKind, TransactionRecord};
use std::sync::{Arc, Barrier, Mutex};

fn records(repo: &SqliteGovernanceRepository) -> Vec<TransactionRecord> {
    ledger::owner_transactions(&repo.conn, &user_id()).unwrap()
}
fn settle(
    repo: &mut SqliteGovernanceRepository,
    amount: i64,
    scale: u32,
) -> ExecutorFinalizationResult {
    let now = Utc::now();
    let (claim, _, _) = persist_claimed_executor_spend(repo, now + Duration::minutes(15));
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &agent_id(),
        &claim.operation_key,
        PaymentId::new(),
        precise_settlement_receipt(amount, scale),
        now,
    )
    .unwrap()
}
fn owner_fixture(repo: &SqliteGovernanceRepository) {
    repo.conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_accounts (id TEXT PRIMARY KEY, agent_id TEXT UNIQUE, owner_user_id TEXT);").unwrap();
    repo.conn
        .execute(
            "INSERT OR IGNORE INTO agent_accounts VALUES (?1,?2,?3)",
            params![
                AgentAccountId::new().to_string(),
                agent_id().to_string(),
                user_id().to_string()
            ],
        )
        .unwrap();
}
fn query(
    repo: &SqliteGovernanceRepository,
    budget: &BudgetId,
) -> crate::ledger::BudgetLedgerReport {
    LedgerService::transactions_for_budget(
        repo,
        BudgetLedgerQuery {
            owner_user_id: user_id(),
            agent_id: agent_id(),
            budget_id: budget.clone(),
        },
    )
    .unwrap()
}
fn adjustment(record: &TransactionRecord, amount: i64, scale: u32) -> AccountingAdjustment {
    AccountingAdjustment {
        owner_user_id: record.owner_user_id.clone(),
        original_transaction_id: record
            .metadata
            .as_ref()
            .unwrap()
            .original_transaction_id
            .clone()
            .unwrap_or_else(|| record.id.clone()),
        expected_previous_transaction_id: record.id.clone(),
        operation_key: "invoice-correction".into(),
        corrected_cost: ExactMoney {
            amount: amount.to_string(),
            scale,
            currency: Currency::Usd,
        },
        reason: "Corrected invoice".into(),
        evidence: "invoice://corrected".into(),
    }
}
fn remove_postings(repo: &SqliteGovernanceRepository) {
    repo.conn.execute_batch("DROP TRIGGER ledger_metadata_no_delete; DROP TRIGGER ledger_entries_no_delete; DROP TRIGGER ledger_transactions_no_delete; DELETE FROM ledger_transaction_metadata; DELETE FROM ledger_entries; DELETE FROM ledger_transactions;").unwrap();
}

#[test]
fn canonical_provider_postings_are_precise_balanced_immutable_and_owner_scoped() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    owner_fixture(&repo);
    let settled = settle(&mut repo, 24991, 3);
    let all = records(&repo);
    assert_eq!(all.len(), 1);
    let record = &all[0];
    let meta = record.metadata.as_ref().unwrap();
    assert_eq!(meta.kind, TransactionKind::ExternalProvider);
    assert_eq!(
        meta.rounding_delta_amount.as_deref(),
        Some("9000000000000000")
    );
    assert_eq!(meta.budget_charge_delta_cents, Some(2500));
    assert_eq!(record.entries.len(), 2);
    assert_eq!(record.entries[0].amount.amount, "24991000000000000000");
    assert_eq!(record.entries[0].amount, record.entries[1].amount);
    assert_ne!(record.entries[0].direction, record.entries[1].direction);
    assert!(record
        .entries
        .iter()
        .any(|e| e.account_kind == hubu_ledger::LedgerAccountKind::ExternallyBilledClearing));
    assert!(repo
        .conn
        .execute("UPDATE ledger_entries SET exact_amount='1'", [])
        .is_err());
    assert!(repo
        .conn
        .execute("DELETE FROM ledger_transactions", [])
        .is_err());
    assert!(repo
        .conn
        .execute(
            "UPDATE ledger_transaction_metadata SET record_json='{}'",
            []
        )
        .is_err());
    let report = query(&repo, &settled.hold.budget_id);
    assert_eq!(report.unaccounted_consumption_cents, 0);
    assert_eq!(report.recorded_budget_charges_cents, 2500);
    for (owner, agent) in [(UserId::new(), agent_id()), (user_id(), AgentId::new())] {
        assert!(LedgerService::transactions_for_budget(
            &repo,
            BudgetLedgerQuery {
                owner_user_id: owner,
                agent_id: agent,
                budget_id: settled.hold.budget_id.clone()
            }
        )
        .is_err());
    }
}

#[test]
fn canonical_posting_failure_rolls_back_every_settlement_write() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let now = Utc::now();
    let (claim, token, hold) =
        persist_claimed_executor_spend(&mut repo, now + Duration::minutes(15));
    repo.conn.execute_batch("CREATE TRIGGER fail_post BEFORE INSERT ON ledger_entries BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
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
    assert!(records(&repo).is_empty());
    assert!(
        load_executor_settlement_receipt_by_claim_id(&repo.conn, &claim.id)
            .unwrap()
            .is_none()
    );
    assert!(load_spend_auth_token_by_id(&repo.conn, &token.id)
        .unwrap()
        .unwrap()
        .used_at
        .is_none());
    assert_eq!(
        load_executor_claim_by_id(&repo.conn, &claim.id)
            .unwrap()
            .unwrap()
            .status,
        SpendExecutorClaimStatus::Claimed
    );
    let balance = load_budget_balance_by_id(&repo.conn, &hold.budget_id)
        .unwrap()
        .unwrap();
    assert_eq!(balance.consumed_amount_cents, 0);
    assert_eq!(balance.frozen_amount_cents, 2500);
}

#[test]
fn canonical_postings_replay_after_restart_and_backfill_without_reconsuming() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    let settled = settle(&mut repo, 19991, 3);
    let expected = records(&repo);
    drop(repo);
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    assert!(
        repo.settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &settled.claim.operation_key,
            PaymentId::new(),
            precise_settlement_receipt(19991, 3),
            Utc::now()
        )
        .unwrap()
        .idempotent_replay
    );
    assert_eq!(records(&repo), expected);
    remove_postings(&repo);
    drop(repo);
    for _ in 0..2 {
        let repo = SqliteGovernanceRepository::open(file.path()).unwrap();
        assert_eq!(records(&repo).len(), 1);
        assert!(records(&repo)[0].metadata.as_ref().unwrap().legacy_backfill);
        assert_eq!(
            repo.load_budget_balances().unwrap()[0].consumed_amount_cents,
            2000
        );
    }
}

#[test]
fn canonical_backfill_marks_incomplete_evidence_instead_of_inventing_postings() {
    for missing in [
        "claim",
        "decision",
        "hold",
        "receipt",
        "hold_status",
        "token_link",
    ] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        settle(&mut repo, 2000, 2);
        remove_postings(&repo);
        repo.conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        match missing {
            "claim" => {
                repo.conn
                    .execute("DELETE FROM spend_executor_claims", [])
                    .unwrap();
            }
            "decision" => {
                repo.conn
                    .execute_batch(
                        "DROP TRIGGER spend_decisions_no_delete;DELETE FROM spend_decisions;",
                    )
                    .unwrap();
            }
            "hold" => {
                repo.conn.execute("DELETE FROM budget_holds", []).unwrap();
            }
            "receipt" => {
                repo.conn.execute_batch("DROP TRIGGER spend_executor_settlement_receipts_no_delete;DELETE FROM spend_executor_settlement_receipts;").unwrap();
            }
            "hold_status" => {
                repo.conn
                    .execute("UPDATE budget_holds SET status='claimed'", [])
                    .unwrap();
            }
            _ => {
                repo.conn
                    .execute(
                        "UPDATE spend_auth_tokens SET used_by_payment_id=?1",
                        [PaymentId::new().to_string()],
                    )
                    .unwrap();
            }
        }
        repo.initialize_provider_accounting().unwrap();
        repo.initialize_provider_accounting().unwrap();
        assert!(records(&repo).is_empty(), "{missing}");
        assert_eq!(
            repo.conn
                .query_row("SELECT COUNT(*) FROM ledger_legacy_gaps", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            repo.load_budget_balances().unwrap()[0].consumed_amount_cents,
            2000
        );
    }
}

#[test]
fn ledger_corrections_append_publish_cache_and_leave_original_receipt_intact() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    owner_fixture(&repo);
    let settled = settle(&mut repo, 19991, 3);
    let original = records(&repo).remove(0);
    let shared = Arc::new(Mutex::new(repo));
    let mut manager = BudgetManager::new().with_repository(shared.clone());
    let command = adjustment(&original, 10001, 3);
    let corrected = LedgerService::adjust(&mut manager, command.clone()).unwrap();
    assert_eq!(
        corrected
            .metadata
            .as_ref()
            .unwrap()
            .budget_charge_delta_cents,
        Some(-999)
    );
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .consumed_amount_cents,
        1001
    );
    assert_eq!(
        LedgerService::adjust(&mut manager, command.clone()).unwrap(),
        corrected
    );
    let mut conflict = command.clone();
    conflict.reason = "other".into();
    assert!(LedgerService::adjust(&mut manager, conflict).is_err());
    let mut stale = command;
    stale.operation_key = "stale".into();
    assert!(LedgerService::adjust(&mut manager, stale).is_err());
    let mut wrong = adjustment(&corrected, 0, 2);
    wrong.owner_user_id = UserId::new();
    assert!(LedgerService::adjust(&mut manager, wrong).is_err());
    let mut overrun = adjustment(&corrected, 200001, 3);
    overrun.operation_key = "overrun".into();
    let overrun = LedgerService::adjust(&mut manager, overrun).unwrap();
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .remaining_amount_cents,
        -10001
    );
    let mut refund = adjustment(&overrun, 0, 18);
    refund.operation_key = "refund".into();
    LedgerService::adjust(&mut manager, refund).unwrap();
    let mut repo = shared.lock().unwrap();
    let replay = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &settled.claim.operation_key,
            PaymentId::new(),
            precise_settlement_receipt(19991, 3),
            Utc::now(),
        )
        .unwrap();
    assert_eq!(replay.balance.consumed_amount_cents, 0);
    assert_eq!(replay.receipt, settled.receipt);
    assert_eq!(
        records(&repo).iter().find(|r| r.id == original.id).unwrap(),
        &original
    );
    assert_eq!(
        query(&repo, &settled.hold.budget_id).unaccounted_consumption_cents,
        0
    );
}

#[test]
fn ledger_adjustment_failure_preserves_cache_and_database() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let settled = settle(&mut repo, 2000, 2);
    let record = records(&repo).remove(0);
    let mut manager = BudgetManager::from_records(
        repo.load_budgets().unwrap(),
        repo.load_budget_versions().unwrap(),
        repo.load_budget_balances().unwrap(),
        repo.load_budget_holds().unwrap(),
    )
    .unwrap();
    repo.conn.execute_batch("CREATE TRIGGER fail_meta BEFORE INSERT ON ledger_transaction_metadata BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let shared = Arc::new(Mutex::new(repo));
    manager = manager.with_repository(shared.clone());
    assert!(LedgerService::adjust(&mut manager, adjustment(&record, 1000, 2)).is_err());
    assert_eq!(
        manager
            .get_budget_by_id(&settled.hold.budget_id)
            .unwrap()
            .balance
            .consumed_amount_cents,
        2000
    );
    let repo = shared.lock().unwrap();
    assert_eq!(records(&repo).len(), 1);
    assert_eq!(
        repo.load_budget_balances().unwrap()[0].consumed_amount_cents,
        2000
    );
}

#[test]
fn ledger_pending_released_and_unbilled_claims_have_no_expense() {
    for release in [false, true] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let now = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(&mut repo, now);
        assert!(records(&repo).is_empty());
        if release {
            repo.reconcile_executor_claim_as_not_billed_transactionally(
                &claim.id,
                &user_id(),
                "provider",
                "not billed",
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
        assert!(records(&repo).is_empty());
    }
}

#[test]
fn ledger_extreme_exact_vendor_reconciliation_posts_once() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let now = Utc::now();
    let (claim, _, _) = persist_claimed_executor_spend(&mut repo, now);
    for _ in 0..2 {
        repo.reconcile_executor_claim_as_billed_transactionally(
            &claim.id,
            &user_id(),
            "invoice",
            "confirmed",
            PaymentId::new(),
            precise_settlement_receipt(i64::MAX, 2),
            now,
        )
        .unwrap();
    }
    assert_eq!(records(&repo).len(), 1);
    assert_eq!(
        records(&repo)[0].entries[0].amount.amount,
        (i128::from(i64::MAX) * 10_i128.pow(16)).to_string()
    );
}

#[test]
fn ledger_concurrent_finalization_and_restart_adjustment_post_once() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    let now = Utc::now();
    let (claim, _, _) = persist_claimed_executor_spend(&mut repo, now + Duration::minutes(15));
    let repositories: Vec<_> = (0..2)
        .map(|_| SqliteGovernanceRepository::open(file.path()).unwrap())
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
                    &user_id(),
                    &agent_id(),
                    &claim.operation_key,
                    PaymentId::new(),
                    settlement_receipt(2000),
                    now,
                )
                .unwrap()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|r| r.idempotent_replay)
            .count(),
        1
    );
    let record = records(&repo).remove(0);
    let command = adjustment(&record, 11001, 3);
    let first = repo.adjust_accounting(command.clone()).unwrap().0;
    drop(repo);
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    assert_eq!(repo.adjust_accounting(command).unwrap().0, first);
    assert_eq!(records(&repo).len(), 2);
}

#[test]
fn agent_budget_query_includes_wallet_provider_adjustments_and_truthful_unlinked_coverage() {
    use hubu_ledger::{
        LedgerAccountKind as Kind, LedgerDirection as Direction, LedgerEntryDraft, SqliteLedger,
    };
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
    owner_fixture(&repo);
    let settled = settle(&mut repo, 1, 3);
    let provider = records(&repo).remove(0);
    let mut wallet = SqliteLedger::open(file.path()).unwrap();
    let cash = wallet
        .create_account(user_id(), "cash", Kind::UserWalletCash, Currency::Usd)
        .unwrap();
    let expense = wallet
        .create_account(user_id(), "expense", Kind::AgentSpendExpense, Currency::Usd)
        .unwrap();
    let entries = || {
        vec![
            LedgerEntryDraft {
                owner_user_id: user_id(),
                account_id: expense.id.clone(),
                direction: Direction::Debit,
                amount_cents: 1000,
                currency: Currency::Usd,
            },
            LedgerEntryDraft {
                owner_user_id: user_id(),
                account_id: cash.id.clone(),
                direction: Direction::Credit,
                amount_cents: 1000,
                currency: Currency::Usd,
            },
        ]
    };
    let mut metadata = provider.metadata.clone().unwrap();
    metadata.kind = TransactionKind::WalletPayment;
    metadata.source_key = "payment:wallet-1".into();
    metadata.effective_cost = ExactMoney {
        amount: "1000".into(),
        scale: 2,
        currency: Currency::Usd,
    };
    metadata.budget_charge_delta_cents = Some(1000);
    metadata.rounding_delta_amount = Some("0".into());
    metadata.source_evidence = serde_json::json!({"payment_id":"wallet-1"});
    let wallet_tx = wallet
        .record_transaction_with_metadata(
            user_id(),
            Some("wallet-1".into()),
            "wallet expense",
            entries(),
            Some(metadata),
        )
        .unwrap();
    // Model the wallet lifecycle's successful consumption, still separate until HUB-210.
    repo.conn.execute("UPDATE budget_balances SET consumed_amount_cents=consumed_amount_cents+1000,remaining_amount_cents=remaining_amount_cents-1000 WHERE budget_id=?1",[settled.hold.budget_id.to_string()]).unwrap();
    wallet
        .record_transaction(
            user_id(),
            Some("legacy-unlinked".into()),
            "historical",
            entries(),
        )
        .unwrap();
    let report = query(&repo, &settled.hold.budget_id);
    assert_eq!(report.transactions.len(), 2);
    assert_eq!(report.consumed_amount_cents, 1001);
    assert_eq!(report.recorded_budget_charges_cents, 1001);
    assert_eq!(report.unaccounted_consumption_cents, 0);
    assert_eq!(report.owner_transactions_without_budget_context, 1);
    assert!(report.transactions.iter().any(|r| r.id == wallet_tx.id));
    let expense: i128 = report
        .transactions
        .iter()
        .map(|r| {
            r.metadata
                .as_ref()
                .unwrap()
                .effective_cost
                .scaled()
                .unwrap()
        })
        .sum();
    assert_eq!(expense, 10_001_000_000_000_000_000);
    assert_eq!(
        provider
            .metadata
            .as_ref()
            .unwrap()
            .rounding_delta_amount
            .as_deref(),
        Some("9000000000000000")
    );
    let wallet_record = report
        .transactions
        .into_iter()
        .find(|r| r.id == wallet_tx.id)
        .unwrap();
    let shared = Arc::new(Mutex::new(repo));
    let mut manager = BudgetManager::new().with_repository(shared.clone());
    assert!(LedgerService::adjust(&mut manager, adjustment(&wallet_record, 900, 2)).is_err());
    let correction = LedgerService::adjust(&mut manager, adjustment(&provider, 0, 3)).unwrap();
    assert_eq!(
        correction
            .metadata
            .as_ref()
            .unwrap()
            .budget_charge_delta_cents,
        Some(-1)
    );
    assert!(correction
        .entries
        .iter()
        .any(|e| e.account_kind == Kind::ExternallyBilledClearing));
    let repo = shared.lock().unwrap();
    let report = query(&repo, &settled.hold.budget_id);
    assert_eq!(report.transactions.len(), 3);
    assert_eq!(report.recorded_budget_charges_cents, 1000);
    assert_eq!(report.unaccounted_consumption_cents, 0);
    // Compatibility projection excludes exact provider postings and adjustments.
    assert_eq!(wallet.list_transactions().unwrap().len(), 2);
    assert_eq!(
        wallet.entries_for_transaction(&wallet_tx.id).unwrap()[0].amount_cents,
        1000
    );
}

#[test]
fn owned_empty_budget_has_valid_empty_ledger_and_pending_coverage() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    owner_fixture(&repo);
    let now = Utc::now();
    let (claim, _, hold) = persist_claimed_executor_spend(&mut repo, now + Duration::minutes(15));
    let report = query(&repo, &hold.budget_id);
    assert!(report.transactions.is_empty());
    assert_eq!(report.consumed_amount_cents, 0);
    assert_eq!(report.unaccounted_consumption_cents, 0);
    assert_eq!(report.pending_hold_count, 1);
    repo.release_executor_claim_transactionally(&user_id(), &agent_id(), &claim.operation_key, now)
        .unwrap();
    assert_eq!(query(&repo, &hold.budget_id).pending_hold_count, 0);
}

#[test]
fn backfill_sql_failure_cannot_commit_partial_or_unbalanced_postings() {
    for table in ["ledger_entries", "ledger_transaction_metadata"] {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        settle(&mut repo, 2000, 2);
        remove_postings(&repo);
        let condition = if table == "ledger_entries" {
            "WHEN (SELECT COUNT(*) FROM ledger_entries WHERE transaction_id=NEW.transaction_id)=1"
        } else {
            ""
        };
        repo.conn.execute_batch(&format!("CREATE TRIGGER fail_backfill BEFORE INSERT ON {table} {condition} BEGIN SELECT RAISE(ABORT,'injected migration write failure'); END;")).unwrap();
        assert!(repo.initialize_provider_accounting().is_err());
        assert!(records(&repo).is_empty());
        assert_eq!(
            repo.conn
                .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            repo.conn
                .query_row("SELECT COUNT(*) FROM ledger_legacy_gaps", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            repo.load_budget_balances().unwrap()[0].consumed_amount_cents,
            2000
        );
    }
}

#[test]
fn legacy_wallet_context_backfill_preserves_postings_and_rejects_corrupt_credit() {
    use hubu_ledger::{
        LedgerAccountKind as Kind, LedgerDirection as Direction, LedgerEntryDraft, SqliteLedger,
    };
    use hubu_wallet::{
        PaymentAttemptRepository, PaymentDestination, PaymentRailKind, PaymentRequest,
        PaymentResponse, PaymentStatus, SqlitePaymentAttemptRepository,
    };
    for corrupt in [false, true] {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut repo = SqliteGovernanceRepository::open(file.path()).unwrap();
        owner_fixture(&repo);
        let settled = settle(&mut repo, 2500, 2);
        let request = repo.load_spend_decisions().unwrap().remove(0).request;
        remove_postings(&repo);
        repo.conn.execute_batch("DROP TRIGGER spend_executor_settlement_receipts_no_delete; DELETE FROM spend_executor_settlement_receipts; UPDATE budget_holds SET executor_claim_id=NULL; DELETE FROM spend_executor_claims;").unwrap();
        let mut wallet = SqliteLedger::open(file.path()).unwrap();
        let cash = wallet
            .create_account(user_id(), "cash", Kind::UserWalletCash, Currency::Usd)
            .unwrap();
        let expense = wallet
            .create_account(user_id(), "expense", Kind::AgentSpendExpense, Currency::Usd)
            .unwrap();
        let payment = settled.receipt.as_ref().unwrap().settlement_id.clone();
        let wallet_tx = wallet
            .record_transaction(
                user_id(),
                Some(payment.to_string()),
                "legacy wallet payment",
                vec![
                    LedgerEntryDraft {
                        owner_user_id: user_id(),
                        account_id: expense.id,
                        direction: Direction::Debit,
                        amount_cents: 2500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        owner_user_id: user_id(),
                        account_id: cash.id,
                        direction: Direction::Credit,
                        amount_cents: 2500,
                        currency: Currency::Usd,
                    },
                ],
            )
            .unwrap();
        let original_ids: Vec<_> = wallet
            .entries_for_transaction(&wallet_tx.id)
            .unwrap()
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let payment_request = PaymentRequest {
            idempotency_key: "legacy-wallet".into(),
            spend_auth_token_id: settled.token.id.clone(),
            owner_user_id: user_id(),
            agent_id: agent_id(),
            agent_account_id: request.agent_account_id.clone(),
            amount_cents: 2500,
            currency: Currency::Usd,
            merchant: request.merchant,
            execution_scope: None,
            task_id: request.task_id,
            rail: PaymentRailKind::FiatMock,
            destination: PaymentDestination::FiatAccount {
                account_ref: "merchant".into(),
            },
            memo: None,
        };
        let response = PaymentResponse {
            payment_id: payment,
            owner_user_id: user_id(),
            agent_account_id: request.agent_account_id,
            status: PaymentStatus::Succeeded,
            amount_cents: 2500,
            currency: Currency::Usd,
            ledger_transaction_id: Some(wallet_tx.id.clone()),
            rail_reference: Some("confirmed".into()),
            failure_reason: None,
            created_at: Utc::now(),
        };
        SqlitePaymentAttemptRepository::open(file.path())
            .unwrap()
            .save_payment_attempt(&payment_request, &response)
            .unwrap();
        if corrupt {
            repo.conn.execute_batch("DROP TRIGGER ledger_entries_no_update; UPDATE ledger_entries SET amount_cents=2499,exact_amount='2499' WHERE direction='credit';").unwrap();
        }
        drop(repo);
        let repo = SqliteGovernanceRepository::open(file.path()).unwrap();
        let all = records(&repo);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, wallet_tx.id);
        assert_eq!(
            all[0]
                .entries
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>(),
            original_ids
        );
        let report = query(&repo, &settled.hold.budget_id);
        if corrupt {
            assert!(all[0].metadata.is_none());
            assert!(report.transactions.is_empty());
            assert_eq!(report.unaccounted_consumption_cents, 2500);
            assert_eq!(report.owner_transactions_without_budget_context, 1);
        } else {
            assert!(all[0].metadata.as_ref().unwrap().legacy_backfill);
            assert_eq!(report.transactions.len(), 1);
            assert_eq!(report.unaccounted_consumption_cents, 0);
        }
        assert_eq!(
            repo.load_budget_balances().unwrap()[0].consumed_amount_cents,
            2500
        );
    }
}
