//! USDC as a budget currency: budgets are per currency, with no conversion.
use super::tests::{agent_id, persist_claimed_executor_spend_with_amounts, user_id};
use super::*;
use crate::budget::manager::BudgetVersionProvenance;
use crate::budget::{
    BudgetManager, BudgetManagerError, CreateSingleBudgetRequest, ReserveBudgetRequest,
};
use crate::policy::model::Policy;
use crate::spend::{SpendExecutorSettlementReceipt, SpendExecutorVendorCost};
use chrono::Duration;
use hubu_common::ids::SpendDecisionId;
use hubu_common::time::TimePeriod;

fn units(value: &str) -> Amount {
    value.parse().unwrap()
}

fn budget(
    manager: &mut BudgetManager,
    agent: &AgentId,
    currency: Currency,
    limit: &str,
) -> BudgetId {
    manager
        .create_single_budget_in_memory(
            CreateSingleBudgetRequest {
                agent_id: agent.clone(),
                amount_limit: units(limit),
                currency,
                period: TimePeriod::new(Utc::now() - Duration::hours(1), None).unwrap(),
            },
            BudgetVersionProvenance::new("test:owner", "test:usdc"),
        )
        .unwrap()
        .budget
        .id
}

fn reserve(
    manager: &mut BudgetManager,
    budget_id: &BudgetId,
    currency: Currency,
    amount: &str,
) -> Result<crate::budget::ReserveBudgetResponse, BudgetManagerError> {
    manager.reserve_budget_at(
        ReserveBudgetRequest {
            budget_id: budget_id.clone(),
            spend_decision_id: SpendDecisionId::new(),
            amount: units(amount),
            currency,
            expires_at: Utc::now() + Duration::minutes(5),
        },
        Utc::now(),
    )
}

fn usdc_receipt(atomic_units: i64) -> SpendExecutorSettlementReceipt {
    SpendExecutorSettlementReceipt {
        actual_vendor_cost: SpendExecutorVendorCost {
            amount: atomic_units,
            scale: 6,
            currency: Currency::Usdc,
        },
        provider_request_id: "x402-request".to_string(),
        price_model_snapshot: serde_json::json!({
            "provider": "x402-api",
            "model": "per-call",
            "currency": "usdc",
        }),
        artifact_reference: "artifact://x402".to_string(),
    }
}

#[test]
fn a_usdc_budget_runs_one_thousand_tenth_of_a_cent_calls() {
    let mut manager = BudgetManager::new();
    let agent = AgentId::new();
    let budget_id = budget(&mut manager, &agent, Currency::Usdc, "1");
    for _ in 0..1_000 {
        let reservation = reserve(&mut manager, &budget_id, Currency::Usdc, "0.001").unwrap();
        assert_eq!(reservation.hold.currency, Currency::Usdc);
        manager
            .settle_budget(&reservation.hold.id, units("0.001"))
            .unwrap();
    }
    assert!(reserve(&mut manager, &budget_id, Currency::Usdc, "0.001").is_err());
    let balance = manager.get_budget_balance(&budget_id).unwrap();
    assert_eq!(balance.consumed_amount, units("1"));
}

#[test]
fn an_x402_hold_of_max_amount_required_is_exact() {
    let mut manager = BudgetManager::new();
    let agent = AgentId::new();
    let budget_id = budget(&mut manager, &agent, Currency::Usdc, "0.01");
    // maxAmountRequired = 5000 atomic USDC units = 0.005 USDC = 5000 µUSDC.
    let reservation = reserve(&mut manager, &budget_id, Currency::Usdc, "0.005").unwrap();
    assert_eq!(reservation.hold.amount.units(), 5_000);
    assert_eq!(reservation.balance.remaining_amount, units("0.005"));
}

#[test]
fn spend_only_counts_against_a_budget_in_its_own_currency() {
    let mut manager = BudgetManager::new();
    let agent = AgentId::new();
    let usd_budget = budget(&mut manager, &agent, Currency::Usd, "100");
    // With only a USD budget there is no USDC budget to spend from.
    assert_eq!(
        manager
            .available_budget_id_for_agent_at(&agent, Currency::Usdc, Utc::now())
            .unwrap(),
        None
    );
    assert!(matches!(
        reserve(&mut manager, &usd_budget, Currency::Usdc, "1"),
        Err(BudgetManagerError::CurrencyMismatch)
    ));
    // A USDC budget can run alongside the USD one in the same period.
    let usdc_budget = budget(&mut manager, &agent, Currency::Usdc, "5");
    assert_eq!(
        manager
            .available_budget_id_for_agent_at(&agent, Currency::Usdc, Utc::now())
            .unwrap(),
        Some(usdc_budget.clone())
    );
    reserve(&mut manager, &usdc_budget, Currency::Usdc, "2").unwrap();
    assert_eq!(
        manager
            .get_budget_balance(&usd_budget)
            .unwrap()
            .remaining_amount,
        units("100"),
        "USDC spend never touches the USD budget"
    );
}

#[test]
fn executor_settles_usdc_against_a_usdc_hold_and_posts_usdc_ledger_lines() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    repo.conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_accounts
             (id TEXT PRIMARY KEY, agent_id TEXT UNIQUE, owner_user_id TEXT);",
        )
        .unwrap();
    repo.conn
        .execute(
            "INSERT INTO agent_accounts VALUES (?1, ?2, ?3)",
            params![
                hubu_common::ids::AgentAccountId::new().to_string(),
                agent_id().to_string(),
                user_id().to_string()
            ],
        )
        .unwrap();
    let (claim, _, hold) = persist_claimed_executor_spend_with_amounts(
        &mut repo,
        Utc::now() + Duration::minutes(15),
        "x402-call",
        agent_id(),
        units("1"),
        units("0.005"),
        Currency::Usdc,
    );
    let settled = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &claim.operation_key,
            PaymentId::new(),
            usdc_receipt(3_100),
            Utc::now(),
        )
        .unwrap();
    let receipt = settled.receipt.unwrap();
    assert_eq!(receipt.currency, Currency::Usdc);
    assert_eq!(receipt.budget_charge, units("0.0031"));
    assert_eq!(receipt.released_amount, units("0.0019"));
    assert_eq!(settled.balance.consumed_amount, units("0.0031"));

    let transactions = hubu_ledger::domain::owner_transactions(&repo.conn, &user_id()).unwrap();
    assert_eq!(transactions.len(), 1);
    let posting = &transactions[0];
    assert_eq!(
        posting.metadata.as_ref().unwrap().effective_cost.currency,
        Currency::Usdc
    );
    assert!(posting
        .entries
        .iter()
        .all(|entry| entry.amount.currency == Currency::Usdc));
    let report = crate::ledger::LedgerService::transactions_for_budget(
        &repo,
        crate::ledger::BudgetLedgerQuery {
            owner_user_id: user_id(),
            agent_id: agent_id(),
            budget_id: hold.budget_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(report.recorded_budget_charges, units("0.0031"));
    assert_eq!(report.unaccounted_consumption, Amount::ZERO);
}

#[test]
fn a_receipt_in_another_currency_than_the_hold_is_rejected() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, _, _) = persist_claimed_executor_spend_with_amounts(
        &mut repo,
        Utc::now() + Duration::minutes(15),
        "usd-hold",
        agent_id(),
        units("1"),
        units("0.005"),
        Currency::Usd,
    );
    let error = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent_id(),
            &claim.operation_key,
            PaymentId::new(),
            usdc_receipt(3_100),
            Utc::now(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("actual vendor cost currency does not match the authorization"),
        "{error}"
    );
}

#[test]
fn executor_receipts_parse_usdc_and_still_reject_unknown_currencies() {
    let receipt: SpendExecutorSettlementReceipt = serde_json::from_value(serde_json::json!({
        "actual_vendor_cost": {"amount": 5000, "scale": 6, "currency": "USDC"},
        "provider_request_id": "x402-request",
        "price_model_snapshot": {"provider": "x402", "model": "call", "currency": "usdc"},
        "artifact_reference": "artifact://x402"
    }))
    .unwrap();
    assert_eq!(receipt.actual_vendor_cost.currency, Currency::Usdc);
    assert!(
        serde_json::from_value::<SpendExecutorSettlementReceipt>(serde_json::json!({
            "actual_vendor_cost": {"amount": 1, "scale": 18, "currency": "eth"},
            "provider_request_id": "r",
            "price_model_snapshot": {"provider": "p", "model": "m", "currency": "eth"},
            "artifact_reference": "a"
        }))
        .is_err()
    );
}

#[test]
fn amount_caps_compare_in_the_request_currency_and_can_be_scoped_by_currency() {
    let policy = Policy::from_yaml_str(
        r#"
id: x402
version: v1
default_effect: deny
rules:
  - id: allow_small_usdc_calls
    effect: allow
    reason: x402 calls up to half a cent
    when:
      op: all
      conditions:
        - op: eq
          field: currency
          value:
            currency: usdc
        - op: lte
          field: amount
          value:
            money: "0.005"
"#,
    )
    .unwrap();
    let mut request = super::tests::spend_decision().request;
    request.currency = Currency::Usdc;
    request.amount = units("0.005");
    let rule = &policy.rules[0].when;
    assert!(rule.eval(&request));
    request.amount = units("0.005001");
    assert!(!rule.eval(&request));
    // The same amount in USD does not match the USDC-scoped cap.
    request.currency = Currency::Usd;
    request.amount = units("0.005");
    assert!(!rule.eval(&request));
    // A sub-cent cap warns that USD wallet payments cannot match it.
    let warnings = policy.amount_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("finer than one cent"),
        "{}",
        warnings[0]
    );
    assert!(
        Policy::from_yaml_str(include_str!("../../../../policies/starter-policy.yaml"))
            .unwrap()
            .amount_warnings()
            .is_empty()
    );
}
