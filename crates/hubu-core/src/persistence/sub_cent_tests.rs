//! Sub-cent budget accounting edge cases (design §5.3) and cross-version replay.
use super::tests::{
    agent_id, persist_claimed_executor_spend_with_amounts, precise_settlement_receipt,
    spend_decision, user_id,
};
use super::*;
use crate::budget::manager::BudgetVersionProvenance;
use crate::budget::{
    BudgetManager, BudgetManagerError, CreateSingleBudgetRequest, ReserveBudgetRequest,
};
use crate::spend::SpendRequest;
use chrono::Duration;
use hubu_common::ids::SpendDecisionId;
use hubu_common::time::TimePeriod;
use std::sync::{Arc, Barrier, Mutex};

fn usd(value: &str) -> Amount {
    value.parse().unwrap()
}

fn in_memory_budget(manager: &mut BudgetManager, limit: &str) -> BudgetId {
    manager
        .create_single_budget_in_memory(
            CreateSingleBudgetRequest {
                agent_id: AgentId::new(),
                amount_limit: usd(limit),
                currency: Currency::Usd,
                period: TimePeriod::new(Utc::now() - Duration::hours(1), None).unwrap(),
            },
            BudgetVersionProvenance::new("test:owner", "test:sub-cent"),
        )
        .unwrap()
        .budget
        .id
}

fn reserve(
    manager: &mut BudgetManager,
    budget_id: &BudgetId,
    amount: &str,
) -> Result<crate::budget::ReserveBudgetResponse, BudgetManagerError> {
    manager.reserve_budget_at(
        ReserveBudgetRequest {
            budget_id: budget_id.clone(),
            spend_decision_id: SpendDecisionId::new(),
            amount: usd(amount),
            currency: Currency::Usd,
            expires_at: Utc::now() + Duration::minutes(5),
        },
        Utc::now(),
    )
}

/// Reserve and settle `calls` sequential spends of `cost` against a USD 1.00
/// budget, returning how many settled before the first denial.
fn sequential_calls(cost: &str, calls: usize) -> (usize, BudgetManagerError, BudgetBalance) {
    let mut manager = BudgetManager::new();
    let budget_id = in_memory_budget(&mut manager, "1.00");
    for settled in 0..calls {
        match reserve(&mut manager, &budget_id, cost) {
            Ok(reservation) => {
                manager
                    .settle_budget(&reservation.hold.id, usd(cost))
                    .unwrap();
            }
            Err(error) => {
                return (
                    settled,
                    error,
                    manager.get_budget_balance(&budget_id).unwrap(),
                )
            }
        }
    }
    panic!("every call settled");
}

#[test]
fn e1_one_thousand_tenth_of_a_cent_calls_fit_one_dollar() {
    let (settled, error, balance) = sequential_calls("0.001", 1_001);
    assert_eq!(settled, 1_000);
    assert!(matches!(
        error,
        BudgetManagerError::InsufficientRemainingBudget | BudgetManagerError::BudgetUnavailable(_)
    ));
    assert_eq!(balance.consumed_amount, usd("1.00"));
    assert_eq!(balance.remaining_amount, Amount::ZERO);
}

#[test]
fn e2_two_hundred_half_cent_calls_fit_one_dollar() {
    let (settled, _, balance) = sequential_calls("0.005", 201);
    assert_eq!(settled, 200);
    assert_eq!(balance.consumed_amount, usd("1.00"));
}

#[test]
fn e3_sub_cent_holds_sum_exactly_against_remaining() {
    let mut manager = BudgetManager::new();
    let budget_id = in_memory_budget(&mut manager, "0.01");
    for _ in 0..3 {
        reserve(&mut manager, &budget_id, "0.003").unwrap();
    }
    // 0.009 is held; a fourth 0.003 hold does not fit even though each rounds
    // to zero cents.
    assert!(matches!(
        reserve(&mut manager, &budget_id, "0.003"),
        Err(BudgetManagerError::InsufficientRemainingBudget)
    ));
    let balance = manager.get_budget_balance(&budget_id).unwrap();
    assert_eq!(balance.frozen_amount, usd("0.009"));
    assert_eq!(balance.remaining_amount, usd("0.001"));
}

fn claimed(
    repo: &mut SqliteGovernanceRepository,
    limit: &str,
    hold: &str,
    claim_expires_at: DateTime<Utc>,
) -> (SpendExecutorClaimRecord, BudgetHold) {
    let (claim, _, hold) = persist_claimed_executor_spend_with_amounts(
        repo,
        claim_expires_at,
        "sub-cent-operation",
        agent_id(),
        usd(limit),
        usd(hold),
        Currency::Usd,
    );
    (claim, hold)
}

fn settle(
    repo: &mut SqliteGovernanceRepository,
    claim: &SpendExecutorClaimRecord,
    amount: i64,
    scale: u32,
) -> Result<ExecutorFinalizationResult, StorageError> {
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &agent_id(),
        &claim.operation_key,
        PaymentId::new(),
        precise_settlement_receipt(amount, scale),
        Utc::now(),
    )
}

#[test]
fn e5_settlement_charges_exact_cost_and_releases_the_rest() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, _) = claimed(
        &mut repo,
        "1.00",
        "0.005",
        Utc::now() + Duration::minutes(15),
    );
    let settled = settle(&mut repo, &claim, 31, 4).unwrap();
    let receipt = settled.receipt.unwrap();
    assert_eq!(
        (
            receipt.receipt.actual_vendor_cost.amount,
            receipt.receipt.actual_vendor_cost.scale
        ),
        (31, 4)
    );
    assert_eq!(receipt.budget_charge, usd("0.0031"));
    assert_eq!(receipt.released_amount, usd("0.0019"));
    assert_eq!(settled.balance.consumed_amount, usd("0.0031"));
    assert_eq!(settled.balance.frozen_amount, Amount::ZERO);
    assert_eq!(settled.balance.remaining_amount, usd("0.9969"));
}

#[test]
fn e6_zero_cost_settlement_releases_the_full_hold() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, _) = claimed(
        &mut repo,
        "1.00",
        "0.005",
        Utc::now() + Duration::minutes(15),
    );
    let settled = settle(&mut repo, &claim, 0, 2).unwrap();
    let receipt = settled.receipt.unwrap();
    assert_eq!(receipt.budget_charge, Amount::ZERO);
    assert_eq!(receipt.released_amount, usd("0.005"));
    assert_eq!(settled.balance.consumed_amount, Amount::ZERO);
    assert_eq!(settled.balance.remaining_amount, usd("1.00"));
}

#[test]
fn e8_charge_equal_to_hold_settles_and_one_micro_over_is_rejected() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, _) = claimed(
        &mut repo,
        "1.00",
        "0.005",
        Utc::now() + Duration::minutes(15),
    );
    let error = settle(&mut repo, &claim, 5_001, 6).unwrap_err();
    assert!(
        error.to_string().contains("exceeds the authorized maximum"),
        "{error}"
    );
    let settled = settle(&mut repo, &claim, 5, 3).unwrap();
    assert_eq!(settled.receipt.unwrap().released_amount, Amount::ZERO);
    assert_eq!(settled.balance.consumed_amount, usd("0.005"));
}

#[test]
fn e9_reconciled_overrun_goes_negative_and_blocks_later_reservations() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, hold) = claimed(&mut repo, "0.005", "0.005", Utc::now());
    let reconciled = repo
        .reconcile_executor_claim_as_billed_transactionally(
            &claim.id,
            &user_id(),
            "vendor-invoice",
            "Invoice confirms the already-incurred charge.",
            PaymentId::new(),
            precise_settlement_receipt(51, 4),
            Utc::now(),
        )
        .unwrap();
    let receipt = reconciled.receipt.unwrap();
    assert_eq!(receipt.overrun_amount, usd("0.0001"));
    assert_eq!(
        reconciled.balance.remaining_amount,
        usd("0").checked_sub(usd("0.0001")).unwrap()
    );

    let mut manager = BudgetManager::from_records(
        repo.load_budgets().unwrap(),
        repo.load_budget_versions().unwrap(),
        repo.load_budget_balances().unwrap(),
        repo.load_budget_holds().unwrap(),
    )
    .unwrap();
    assert!(reserve(&mut manager, &hold.budget_id, "0.000001").is_err());
}

#[test]
fn e10_concurrent_reservations_admit_exactly_the_remaining_units() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hubu.sqlite3");
    let mut seed = SqliteGovernanceRepository::open(&path).unwrap();
    let mut manager = BudgetManager::new();
    let created = manager
        .create_single_budget_in_memory(
            CreateSingleBudgetRequest {
                agent_id: agent_id(),
                amount_limit: usd("0.009"),
                currency: Currency::Usd,
                period: TimePeriod::new(Utc::now() - Duration::hours(1), None).unwrap(),
            },
            BudgetVersionProvenance::new("test:owner", "test:sub-cent"),
        )
        .unwrap();
    seed.save_budget_with_balance(&created.budget, &created.version, &created.balance)
        .unwrap();
    drop(seed);

    let manager = Arc::new(Mutex::new(manager));
    let barrier = Arc::new(Barrier::new(3));
    let repositories: Vec<_> = (0..3)
        .map(|_| SqliteGovernanceRepository::open(&path).unwrap())
        .collect();
    let threads: Vec<_> = repositories
        .into_iter()
        .enumerate()
        .map(|(index, mut repo)| {
            let manager = Arc::clone(&manager);
            let barrier = Arc::clone(&barrier);
            let budget_id = created.budget.id.clone();
            std::thread::spawn(move || {
                let mut decision = spend_decision();
                decision.operation_key = format!("concurrent-{index}");
                // Never leave the barrier short if setup fails.
                let saved = repo.save_spend_decision(&decision);
                barrier.wait();
                saved.unwrap();
                let mut manager = manager.lock().unwrap();
                let reservation = manager.reserve_budget_at(
                    ReserveBudgetRequest {
                        budget_id,
                        spend_decision_id: decision.id,
                        amount: usd("0.0045"),
                        currency: Currency::Usd,
                        expires_at: Utc::now() + Duration::minutes(5),
                    },
                    Utc::now(),
                );
                if let Ok(reservation) = &reservation {
                    repo.save_budget_hold(&reservation.hold, &reservation.balance)
                        .unwrap();
                }
                reservation.is_ok()
            })
        })
        .collect();
    let admitted = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(|admitted| *admitted)
        .count();
    assert_eq!(admitted, 2);

    let repo = SqliteGovernanceRepository::open(&path).unwrap();
    let balance = load_budget_balance_by_id(&repo.conn, &created.budget.id)
        .unwrap()
        .unwrap();
    assert_eq!(balance.frozen_amount, usd("0.009"));
    assert_eq!(balance.remaining_amount, Amount::ZERO);
}

#[test]
fn e11_limit_update_compares_committed_usage_exactly() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, hold) = claimed(
        &mut repo,
        "0.005",
        "0.005",
        Utc::now() + Duration::minutes(15),
    );
    settle(&mut repo, &claim, 31, 4).unwrap();
    let append = |amount_limit: &str| AppendBudgetVersionRequest {
        budget_id: hold.budget_id.clone(),
        expected_revision: 1,
        amount_limit: usd(amount_limit),
        actor: "test:owner".to_string(),
        source: "test:sub-cent".to_string(),
        reason: None,
        effective_at: Utc::now(),
    };
    let error = repo.append_budget_version(&append("0.003")).unwrap_err();
    assert!(matches!(
        error,
        BudgetUpdateError::LimitBelowCommitted {
            requested_amount,
            committed_amount,
        } if requested_amount == usd("0.003") && committed_amount == usd("0.0031")
    ));
    let updated = repo.append_budget_version(&append("0.0031")).unwrap();
    assert_eq!(updated.current.version.amount_limit, usd("0.0031"));
    assert_eq!(updated.current.balance.remaining_amount, Amount::ZERO);
    let legacy_limit: i64 = repo
        .conn
        .query_row(
            "SELECT amount_limit_cents FROM budget_versions WHERE id = ?1",
            params![updated.current.version.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        legacy_limit, 1,
        "sub-cent limits project to the 1-cent minimum"
    );
}

#[test]
fn e14_expired_sub_cent_hold_returns_its_exact_amount() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let mut manager = BudgetManager::new();
    let created = manager
        .create_single_budget_in_memory(
            CreateSingleBudgetRequest {
                agent_id: agent_id(),
                amount_limit: usd("0.01"),
                currency: Currency::Usd,
                period: TimePeriod::new(Utc::now() - Duration::hours(1), None).unwrap(),
            },
            BudgetVersionProvenance::new("test:owner", "test:sub-cent"),
        )
        .unwrap();
    repo.save_budget_with_balance(&created.budget, &created.version, &created.balance)
        .unwrap();
    let decision = spend_decision();
    repo.save_spend_decision(&decision).unwrap();
    let reservation = manager
        .reserve_budget_at(
            ReserveBudgetRequest {
                budget_id: created.budget.id.clone(),
                spend_decision_id: decision.id,
                amount: usd("0.0035"),
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::seconds(1),
            },
            Utc::now(),
        )
        .unwrap();
    repo.save_budget_hold(&reservation.hold, &reservation.balance)
        .unwrap();
    let frozen_cents: i64 = repo
        .conn
        .query_row(
            "SELECT frozen_amount_cents FROM budget_balances WHERE budget_id = ?1",
            params![created.budget.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(frozen_cents, 1, "frozen projects up to whole cents");

    let later = Utc::now() + Duration::seconds(5);
    repo.expire_overdue_budget_holds(later).unwrap();
    let persisted = load_budget_balance_by_id(&repo.conn, &created.budget.id)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.frozen_amount, Amount::ZERO);
    assert_eq!(persisted.remaining_amount, usd("0.01"));
    let expired = manager.expire_overdue_budget_holds(later).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].balance.remaining_amount, usd("0.01"));
}

#[test]
fn wallet_settlement_consumes_the_rail_charge_and_releases_the_rest() {
    let mut manager = BudgetManager::new();
    let budget_id = in_memory_budget(&mut manager, "1.00");
    let reservation = reserve(&mut manager, &budget_id, "0.25").unwrap();
    assert!(matches!(
        manager.settle_budget(&reservation.hold.id, usd("0.26")),
        Err(BudgetManagerError::ChargeExceedsHold)
    ));
    let settled = manager
        .settle_budget(&reservation.hold.id, usd("0.20"))
        .unwrap();
    assert_eq!(settled.balance.consumed_amount, usd("0.20"));
    assert_eq!(settled.balance.frozen_amount, Amount::ZERO);
    assert_eq!(settled.balance.remaining_amount, usd("0.80"));
}

#[test]
fn whole_cent_budget_fingerprints_match_the_pre_upgrade_form() {
    let budget_id: BudgetId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
    // Recorded before accounting-scale amounts existed.
    assert_eq!(
        crate::budget::budget_update_request_fingerprint(
            &budget_id,
            7,
            usd("123.45"),
            "actor:user-1",
            "api",
            Some("quarterly increase"),
        ),
        "sha256:f88ff1fcc9cdf4e030b21aadacfc5cc079588a29df32fc12f9263b93bb43602a"
    );
    let period = TimePeriod::new(
        "2030-01-01T00:00:00Z".parse().unwrap(),
        Some("2030-01-02T00:00:00Z".parse().unwrap()),
    )
    .unwrap();
    let agent: AgentId = "00000000-0000-4000-8000-000000000999".parse().unwrap();
    let legacy_canonical = format!(
        "agent_id={agent}\namount_limit_cents=10000\ncurrency=usd\nstarting_at={}\nending_before={}",
        period.starting_at.to_rfc3339(),
        period.ending_before.unwrap().to_rfc3339(),
    );
    use sha2::Digest;
    assert_eq!(
        crate::budget::initial_budget_request_fingerprint(
            &agent,
            usd("100"),
            Currency::Usd,
            &period
        ),
        format!("sha256:{:x}", Sha256::digest(legacy_canonical.as_bytes()))
    );
    // Sub-cent limits use a distinct, explicitly scaled form.
    assert_ne!(
        crate::budget::initial_budget_request_fingerprint(
            &agent,
            usd("100.000001"),
            Currency::Usd,
            &period
        ),
        crate::budget::initial_budget_request_fingerprint(
            &agent,
            usd("100"),
            Currency::Usd,
            &period
        ),
    );
}

#[test]
fn spend_requests_keep_the_pre_upgrade_stored_form_and_replay() {
    let request = spend_decision().request;
    let stored = serde_json::to_value(&request).unwrap();
    // Whole-cent requests serialize exactly as before the upgrade.
    assert_eq!(stored["amount_cents"], 2_500);
    assert!(stored.get("amount").is_none());
    let pre_upgrade: SpendRequest = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(pre_upgrade.amount, usd("25"));
    assert!(pre_upgrade.replay_equivalent(&request));
    assert_eq!(serde_json::to_value(&pre_upgrade).unwrap(), stored);

    let mut sub_cent = request;
    sub_cent.amount = usd("0.005");
    let value = serde_json::to_value(&sub_cent).unwrap();
    assert_eq!(value["amount"], "0.005");
    assert!(value.get("amount_cents").is_none());
    assert_eq!(
        serde_json::from_value::<SpendRequest>(value.clone()).unwrap(),
        sub_cent
    );

    let mut both = value;
    both["amount_cents"] = serde_json::json!(1);
    assert!(serde_json::from_value::<SpendRequest>(both)
        .unwrap_err()
        .to_string()
        .contains("both"));
}
