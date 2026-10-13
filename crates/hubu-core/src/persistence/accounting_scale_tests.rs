//! Tests for the one-way accounting-scale (µ-unit) storage migration.
use super::tests::{
    agent_id, persist_claimed_executor_spend_for_agent,
    persist_claimed_executor_spend_with_operation_key, precise_settlement_receipt,
    settlement_receipt, user_id,
};
use super::*;
use crate::spending_target::{SpendingTarget, SpendingTargetStatus};
use chrono::Duration;
use hubu_common::ids::SpendingTargetId;
use hubu_common::time::TimePeriod;
use rusqlite::types::Value;
use std::path::Path;

/// Pairs of (table, authoritative column, legacy cent column).
const AMOUNT_PAIRS: &[(&str, &str, &str)] = &[
    ("budget_versions", "amount_limit", "amount_limit_cents"),
    (
        "budget_balances",
        "consumed_amount",
        "consumed_amount_cents",
    ),
    ("budget_balances", "frozen_amount", "frozen_amount_cents"),
    (
        "budget_balances",
        "remaining_amount",
        "remaining_amount_cents",
    ),
    ("budget_holds", "amount", "amount_cents"),
    ("spending_targets", "target_amount", "target_amount_cents"),
    (
        "spend_executor_settlement_receipts",
        "authorized_max",
        "authorized_max_cents",
    ),
    (
        "spend_executor_settlement_receipts",
        "budget_charge",
        "budget_charge_cents",
    ),
    (
        "spend_executor_settlement_receipts",
        "released_amount",
        "released_amount_cents",
    ),
    (
        "spend_executor_settlement_receipts",
        "overrun_amount",
        "overrun_amount_cents",
    ),
];

const SNAPSHOT_TABLES: &[&str] = &[
    "budgets",
    "budget_versions",
    "budget_current_versions",
    "budget_balances",
    "budget_holds",
    "spending_targets",
    "spend_executor_settlement_receipts",
    "spend_executor_claims",
    "ledger_transactions",
    "ledger_entries",
];

struct LegacyFixture {
    in_flight_agent: AgentId,
    in_flight: SpendExecutorClaimRecord,
    expired: SpendExecutorClaimRecord,
    settled_budget: BudgetId,
}

fn table_rows(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn snapshot(conn: &Connection) -> Vec<Vec<Vec<Value>>> {
    SNAPSHOT_TABLES
        .iter()
        .map(|table| table_rows(conn, table))
        .collect()
}

fn trigger_names(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn backups(path: &Path) -> Vec<std::path::PathBuf> {
    let prefix = format!(
        "{}.pre-accounting-scale.",
        path.file_name().unwrap().to_string_lossy()
    );
    let mut found: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| {
            candidate
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .collect();
    found.sort();
    found
}

/// Builds a database at the pre-accounting-scale schema with active, revoked
/// and scheduled budgets, in-flight and expired claims, precise and legacy
/// receipts, ledger postings and a spending target.
fn legacy_database(path: &Path) -> LegacyFixture {
    let now = Utc::now();
    let mut repo = SqliteGovernanceRepository::open(path).unwrap();
    let in_flight_agent = AgentId::new();
    let (in_flight, _, _) = persist_claimed_executor_spend_for_agent(
        &mut repo,
        now + Duration::minutes(15),
        "in-flight",
        in_flight_agent.clone(),
    );
    let (expired, _, _) = persist_claimed_executor_spend_for_agent(
        &mut repo,
        now - Duration::minutes(1),
        "expired-pending-reconciliation",
        AgentId::new(),
    );
    let precise_agent = AgentId::new();
    let (precise, _, hold) = persist_claimed_executor_spend_for_agent(
        &mut repo,
        now + Duration::minutes(15),
        "settled-precise",
        precise_agent.clone(),
    );
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &precise_agent,
        &precise.operation_key,
        PaymentId::new(),
        precise_settlement_receipt(31, 4),
        now,
    )
    .unwrap();
    let legacy_agent = AgentId::new();
    let (legacy, _, revoked_hold) = persist_claimed_executor_spend_for_agent(
        &mut repo,
        now + Duration::minutes(15),
        "settled-legacy-cents",
        legacy_agent.clone(),
    );
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &legacy_agent,
        &legacy.operation_key,
        PaymentId::new(),
        settlement_receipt(1_700),
        now,
    )
    .unwrap();
    repo.conn
        .execute(
            "UPDATE budgets SET administrative_state = 'revoked', updated_at = ?2 WHERE id = ?1",
            params![revoked_hold.budget_id.to_string(), now.to_rfc3339()],
        )
        .unwrap();

    let scheduled = Budget::new(
        BudgetId::new(),
        agent_id(),
        Currency::Usd,
        TimePeriod::new(now + Duration::days(2), Some(now + Duration::days(3))).unwrap(),
    );
    let scheduled_version =
        BudgetVersion::initial(&scheduled, 4_200, "test:owner", "test:scheduled", None).unwrap();
    repo.save_budget_with_balance(
        &scheduled,
        &scheduled_version,
        &BudgetBalance {
            budget_id: scheduled.id.clone(),
            consumed_amount_cents: 0,
            frozen_amount_cents: 0,
            remaining_amount_cents: 4_200,
        },
    )
    .unwrap();
    repo.save_spending_target(&SpendingTarget {
        id: SpendingTargetId::new(),
        owner_user_id: user_id(),
        target_amount_cents: 12_345,
        currency: Currency::Usd,
        period: TimePeriod::new(now, Some(now + Duration::days(30))).unwrap(),
        status: SpendingTargetStatus::Active,
        created_at: now,
        updated_at: now,
    })
    .unwrap();
    drop(repo);

    let conn = Connection::open(path).unwrap();
    strip_accounting_scale_columns(&conn);
    LegacyFixture {
        in_flight_agent,
        in_flight,
        expired,
        settled_budget: hold.budget_id,
    }
}

#[test]
fn legacy_database_migrates_exactly_with_backup_triggers_and_working_lifecycle() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hubu.sqlite3");
    let fixture = legacy_database(&path);
    let legacy = Connection::open(&path).unwrap();
    let legacy_triggers = trigger_names(&legacy);
    let legacy_ledger_rows = table_rows(&legacy, "ledger_entries").len();
    assert!(!table_has_column(&legacy, "budget_holds", "amount").unwrap());
    drop(legacy);

    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(
        backups(&path),
        vec![directory
            .path()
            .join("hubu.sqlite3.pre-accounting-scale.bak")]
    );
    let backup = Connection::open(&backups(&path)[0]).unwrap();
    assert!(!table_has_column(&backup, "budget_holds", "amount").unwrap());
    drop(backup);

    for (table, column, legacy_column) in AMOUNT_PAIRS {
        let (rows, mismatched): (i64, i64) = repo
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(*), COUNT(*) FILTER (
                         WHERE {column} IS NOT {legacy_column} * 10000
                            OR typeof({column}) != 'integer'
                            OR amount_scale IS NOT 6)
                     FROM {table}"
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(rows > 0, "{table} has fixture rows");
        assert_eq!(mismatched, 0, "{table}.{column}");
    }
    let hold_sources: Vec<(String, String, i64, Option<String>)> = repo
        .conn
        .prepare(
            "SELECT spend_asset, spend_exact_amount, spend_exact_scale, conversion_json
             FROM budget_holds",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(hold_sources
        .iter()
        .all(|source| source == &("usd".into(), "2500".into(), 2, None)));
    assert_eq!(trigger_names(&repo.conn), legacy_triggers);
    let update_error = repo
        .conn
        .execute("UPDATE budget_versions SET actor = 'changed'", [])
        .unwrap_err();
    assert!(update_error.to_string().contains("immutable"));
    let update_error = repo
        .conn
        .execute(
            "UPDATE spend_executor_settlement_receipts SET provider_request_id = 'changed'",
            [],
        )
        .unwrap_err();
    assert!(update_error.to_string().contains("immutable"));
    assert_eq!(
        table_rows(&repo.conn, "ledger_entries").len(),
        legacy_ledger_rows
    );

    let budgets = repo.load_budgets().unwrap();
    let versions = repo.load_budget_versions().unwrap();
    let balances = repo.load_budget_balances().unwrap();
    let holds = repo.load_budget_holds().unwrap();
    crate::budget::BudgetManager::from_records(budgets, versions, balances, holds).unwrap();
    let settled = load_budget_balance_by_id(&repo.conn, &fixture.settled_budget)
        .unwrap()
        .unwrap();
    assert_eq!(settled.consumed_amount_cents, 1);
    assert_eq!(settled.remaining_amount_cents, 9_999);
    assert_eq!(
        repo.load_spending_targets().unwrap()[0].target_amount_cents,
        12_345
    );

    let now = Utc::now();
    let settled_in_flight = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &fixture.in_flight_agent,
            &fixture.in_flight.operation_key,
            PaymentId::new(),
            precise_settlement_receipt(31, 4),
            now,
        )
        .unwrap();
    assert_eq!(settled_in_flight.balance.consumed_amount_cents, 1);
    assert_eq!(settled_in_flight.balance.frozen_amount_cents, 0);
    repo.reconcile_executor_claim_as_billed_transactionally(
        &fixture.expired.id,
        &user_id(),
        "vendor-invoice",
        "Invoice confirms the provider charge.",
        PaymentId::new(),
        precise_settlement_receipt(2_500, 2),
        now,
    )
    .unwrap();
    let (consumed, cents): (i64, i64) = repo
        .conn
        .query_row(
            "SELECT consumed_amount, consumed_amount_cents FROM budget_balances
             WHERE budget_id = (SELECT budget_id FROM budget_holds WHERE executor_claim_id = ?1)",
            params![fixture.expired.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((consumed, cents), (25_000_000, 2_500));
}

#[test]
fn reopening_a_migrated_database_is_a_no_op() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hubu.sqlite3");
    legacy_database(&path);
    let first = SqliteGovernanceRepository::open(&path).unwrap();
    let migrated = snapshot(&first.conn);
    let triggers = trigger_names(&first.conn);
    drop(first);

    let reopened = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(snapshot(&reopened.conn), migrated);
    assert_eq!(trigger_names(&reopened.conn), triggers);
    assert_eq!(backups(&path).len(), 1, "no second backup on reopen");
}

#[test]
fn overflowing_legacy_amount_aborts_without_partial_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hubu.sqlite3");
    legacy_database(&path);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TRIGGER budget_versions_no_update;
         UPDATE budget_versions SET amount_limit_cents = 1000000000000000
         WHERE rowid = (SELECT MIN(rowid) FROM budget_versions);",
    )
    .unwrap();
    let before = snapshot(&conn);
    drop(conn);

    let error = match SqliteGovernanceRepository::open(&path) {
        Ok(_) => panic!("overflowing amount must abort the migration"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("cannot migrate budget_versions.amount_limit_cents to accounting scale"),
        "{error}"
    );
    let conn = Connection::open(&path).unwrap();
    for (table, column, _) in AMOUNT_PAIRS {
        assert!(
            !table_has_column(&conn, table, column).unwrap(),
            "{table}.{column}"
        );
    }
    assert_eq!(snapshot(&conn), before);
    assert_eq!(
        backups(&path).len(),
        1,
        "backup precedes the failed migration"
    );
}

#[test]
fn fresh_database_gets_accounting_columns_without_a_backup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hubu.sqlite3");
    let repo = SqliteGovernanceRepository::open(&path).unwrap();
    for (table, column, _) in AMOUNT_PAIRS {
        assert!(
            table_has_column(&repo.conn, table, column).unwrap(),
            "{table}.{column}"
        );
    }
    assert!(backups(&path).is_empty());
}

#[test]
fn missing_or_sub_cent_authoritative_amounts_are_invalid_data() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (_, _, hold) = persist_claimed_executor_spend_with_operation_key(
        &mut repo,
        Utc::now() + Duration::minutes(15),
        "invalid-data",
    );
    repo.conn
        .execute(
            "UPDATE budget_balances SET consumed_amount = NULL WHERE budget_id = ?1",
            params![hold.budget_id.to_string()],
        )
        .unwrap();
    let error = repo.load_budget_balances().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("accounting-scale amount is missing"),
        "{error}"
    );

    repo.conn
        .execute(
            "UPDATE budget_balances SET consumed_amount = 1 WHERE budget_id = ?1",
            params![hold.budget_id.to_string()],
        )
        .unwrap();
    let error = repo.load_budget_balances().unwrap_err();
    assert!(error.to_string().contains("is not a whole cent"), "{error}");
}

#[test]
fn writes_dual_write_exact_units_and_legacy_cents() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let (claim, _, hold) = persist_claimed_executor_spend_with_operation_key(
        &mut repo,
        Utc::now() + Duration::minutes(15),
        "dual-write",
    );
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &agent_id(),
        &claim.operation_key,
        PaymentId::new(),
        precise_settlement_receipt(31, 4),
        Utc::now(),
    )
    .unwrap();
    for (table, column, legacy_column) in AMOUNT_PAIRS {
        let mismatched: i64 = repo
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table}
                     WHERE {column} IS NOT {legacy_column} * 10000 OR amount_scale IS NOT 6"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mismatched, 0, "{table}.{column}");
    }
    let balance = load_budget_balance_by_id(&repo.conn, &hold.budget_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            balance.consumed_amount_cents,
            balance.frozen_amount_cents,
            balance.remaining_amount_cents
        ),
        (1, 0, 9_999)
    );
}

/// Upgrade drill against a copy of a real database:
/// `HUBU_ACCOUNTING_SCALE_DRILL_DB=/path/to/copy.sqlite3 cargo test -p hubu-core \
///  accounting_scale_drill -- --ignored`. Never point it at a live profile.
#[test]
#[ignore = "requires HUBU_ACCOUNTING_SCALE_DRILL_DB pointing at a database copy"]
fn accounting_scale_drill() {
    let path = std::path::PathBuf::from(
        std::env::var("HUBU_ACCOUNTING_SCALE_DRILL_DB")
            .expect("set HUBU_ACCOUNTING_SCALE_DRILL_DB to a database copy"),
    );
    let before = Connection::open(&path).unwrap();
    let already_migrated = table_has_column(&before, "budget_holds", "amount").unwrap();
    let legacy_rows: Vec<_> = AMOUNT_PAIRS
        .iter()
        .map(|(table, _, legacy)| {
            before
                .query_row(
                    &format!("SELECT COUNT(*), COALESCE(SUM({legacy}), 0) FROM {table}"),
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap()
        })
        .collect();
    drop(before);

    let repo = SqliteGovernanceRepository::open(&path).unwrap();
    for ((table, column, legacy), (rows, legacy_sum)) in AMOUNT_PAIRS.iter().zip(&legacy_rows) {
        let (migrated_rows, sum): (i64, i64) = repo
            .conn
            .query_row(
                &format!("SELECT COUNT(*), COALESCE(SUM({column}), 0) FROM {table}"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(migrated_rows, *rows, "{table}");
        assert_eq!(sum, legacy_sum * 10_000, "{table}.{column} vs {legacy}");
        eprintln!("{table}.{column}: {rows} rows, Σ {legacy_sum} cents -> {sum} units");
    }
    crate::budget::BudgetManager::from_records(
        repo.load_budgets().unwrap(),
        repo.load_budget_versions().unwrap(),
        repo.load_budget_balances().unwrap(),
        repo.load_budget_holds().unwrap(),
    )
    .unwrap();
    repo.load_spending_targets().unwrap();
    let triggers = trigger_names(&repo.conn);
    drop(repo);
    let reopened = SqliteGovernanceRepository::open(&path).unwrap();
    assert_eq!(trigger_names(&reopened.conn), triggers);
    eprintln!(
        "already migrated: {already_migrated}; backups: {:?}",
        backups(&path)
    );
}
