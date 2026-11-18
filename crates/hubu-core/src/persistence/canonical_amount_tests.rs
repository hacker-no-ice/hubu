//! One-time upgrade from the released dual-written µ schema.
use super::tests::{persist_claimed_executor_spend_for_agent, precise_settlement_receipt, user_id};
use super::*;
use crate::spending_target::{SpendingTarget, SpendingTargetStatus};
use chrono::Duration;
use hubu_common::{ids::SpendingTargetId, time::TimePeriod};
use rusqlite::types::Value;
use std::path::Path;

const AMOUNTS: &[(&str, &str)] = &[
    ("budget_versions", "amount_limit"),
    ("budget_balances", "consumed_amount"),
    ("budget_balances", "frozen_amount"),
    ("budget_balances", "remaining_amount"),
    ("budget_holds", "amount"),
    ("spending_targets", "target_amount"),
    ("spend_executor_settlement_receipts", "authorized_max"),
    ("spend_executor_settlement_receipts", "budget_charge"),
    ("spend_executor_settlement_receipts", "released_amount"),
    ("spend_executor_settlement_receipts", "overrun_amount"),
];

fn rows(conn: &Connection, table: &str, columns: &str) -> Vec<Vec<Value>> {
    let mut stmt = conn
        .prepare(&format!("SELECT {columns} FROM {table} ORDER BY 1"))
        .unwrap();
    let count = stmt.column_count();
    stmt.query_map([], |row| (0..count).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn snapshot(conn: &Connection) -> Vec<Vec<Vec<Value>>> {
    [
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
    ]
    .iter()
    .map(|table| rows(conn, table, "*"))
    .collect()
}

fn guards(conn: &Connection) -> Vec<Vec<Value>> {
    rows(
        conn,
        "sqlite_master WHERE type IN ('index','trigger')",
        "name, sql",
    )
}

fn micro_profile(path: &Path) -> (AgentId, SpendExecutorClaimRecord) {
    let mut repo = SqliteGovernanceRepository::open(path).unwrap();
    let now = Utc::now();
    let agent = AgentId::new();
    let (live, _, _) = persist_claimed_executor_spend_for_agent(
        &mut repo,
        now + Duration::minutes(15),
        "live",
        agent.clone(),
    );
    let settled_agent = AgentId::new();
    let (settled, _, _) = persist_claimed_executor_spend_for_agent(
        &mut repo,
        now + Duration::minutes(15),
        "settled",
        settled_agent.clone(),
    );
    repo.settle_executor_claim_transactionally(
        &user_id(),
        &settled_agent,
        &settled.operation_key,
        PaymentId::new(),
        precise_settlement_receipt(31, 4),
        now,
    )
    .unwrap();
    repo.save_spending_target(&SpendingTarget {
        id: SpendingTargetId::new(),
        owner_user_id: user_id(),
        target_amount: Amount::from_units(12345),
        currency: Currency::Usd,
        period: TimePeriod::new(now, None).unwrap(),
        status: SpendingTargetStatus::Active,
        created_at: now,
        updated_at: now,
    })
    .unwrap();
    // Freeze the old writer's schema and projections around real lifecycle records.
    restore_micro_build_schema(&repo.conn);
    (agent, live)
}

#[test]
fn fresh_schema_is_canonical_and_reopens_without_rebuilding() {
    let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
    let schema = rows(&repo.conn, "sqlite_master", "name, sql");
    for (table, column) in AMOUNTS {
        assert!(table_has_column(&repo.conn, table, column).unwrap());
        assert!(!table_has_column(&repo.conn, table, &format!("{column}_cents")).unwrap());
    }
    repo.init().unwrap();
    assert_eq!(rows(&repo.conn, "sqlite_master", "name, sql"), schema);
}

#[test]
fn micro_profile_preserves_exact_values_guards_and_live_claims() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("micro.sqlite3");
    let (agent, live) = micro_profile(&path);
    let old = Connection::open(&path).unwrap();
    let expected: Vec<_> = AMOUNTS
        .iter()
        .map(|(table, col)| rows(&old, table, col))
        .collect();
    let objects = guards(&old);
    drop(old);
    let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
    for ((table, col), expected) in AMOUNTS.iter().zip(expected) {
        assert_eq!(rows(&repo.conn, table, col), expected, "{table}.{col}");
        assert!(!table_has_column(&repo.conn, table, &format!("{col}_cents")).unwrap());
        let not_null: i64 = repo
            .conn
            .query_row(
                &format!("SELECT [notnull] FROM pragma_table_info('{table}') WHERE name=?1"),
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(not_null, 1);
    }
    assert_eq!(guards(&repo.conn), objects);
    for sql in [
        "UPDATE budget_balances SET amount_scale=2",
        "UPDATE budget_holds SET amount=0",
        "UPDATE budget_balances SET frozen_amount=-1",
        "UPDATE budget_balances SET consumed_amount=0.5",
        "UPDATE budget_balances SET consumed_amount=NULL",
    ] {
        assert!(repo.conn.execute(sql, []).is_err(), "{sql}");
    }
    assert!(repo
        .conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
    let result = repo
        .settle_executor_claim_transactionally(
            &user_id(),
            &agent,
            &live.operation_key,
            PaymentId::new(),
            precise_settlement_receipt(31, 4),
            Utc::now(),
        )
        .unwrap();
    assert_eq!(result.balance.consumed_amount.units(), 3100);
    let state = snapshot(&repo.conn);
    drop(repo);
    assert_eq!(
        snapshot(&SqliteGovernanceRepository::open(&path).unwrap().conn),
        state
    );
}

#[test]
fn invalid_micro_profile_rolls_back_and_restores_foreign_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.sqlite3");
    micro_profile(&path);
    let conn = Connection::open(path).unwrap();
    conn.execute("UPDATE budget_balances SET consumed_amount=NULL", [])
        .unwrap();
    let before = snapshot(&conn);
    let objects = guards(&conn);
    let mut repo = SqliteGovernanceRepository { conn };
    assert!(repo.init().is_err());
    assert_eq!(snapshot(&repo.conn), before);
    assert_eq!(guards(&repo.conn), objects);
    assert_eq!(
        repo.conn
            .pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn concurrent_micro_profile_opens_serialize_the_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.sqlite3");
    micro_profile(&path);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                SqliteGovernanceRepository::open(path).unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn pre_micro_profile_is_rejected_without_any_schema_or_data_change() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE budgets(id TEXT PRIMARY KEY, amount_limit_cents INTEGER); INSERT INTO budgets VALUES('old',100)").unwrap();
    let schema = rows(&conn, "sqlite_master", "name, sql");
    let before = rows(&conn, "budgets", "*");
    let mut repo = SqliteGovernanceRepository { conn };
    assert!(repo
        .init()
        .unwrap_err()
        .to_string()
        .contains("upgrade through v0.2.4 or v0.2.5"));
    assert_eq!(rows(&repo.conn, "sqlite_master", "name, sql"), schema);
    assert_eq!(rows(&repo.conn, "budgets", "*"), before);
}

fn restore_micro_build_schema(conn: &Connection) {
    // Produce a fixture with the prior build's exact schema and conservative
    // projections, while retaining real IDs, lineage, claims and ledger rows.
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    let tables = [
        "budget_versions",
        "budget_balances",
        "budget_holds",
        "spending_targets",
        "spend_executor_settlement_receipts",
    ];
    let objects: Vec<(String, String, String)> = conn.prepare("SELECT name,type,sql FROM sqlite_master WHERE type IN ('index','trigger') AND sql IS NOT NULL AND tbl_name IN ('budget_versions','budget_balances','budget_holds','spending_targets','spend_executor_settlement_receipts')").unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<Result<_,_>>().unwrap();
    conn.execute_batch(include_str!("micro_build_fixture.sql"))
        .unwrap();
    for table in tables {
        let cols: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table}_micro_build)"))
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let values: Vec<String> = cols.iter().map(|col| match col.as_str() {
            "actual_vendor_cost_cents" => "min((budget_charge+9999)/10000,(authorized_max+9999)/10000)".into(),
            "released_amount_cents" if table == "spend_executor_settlement_receipts" => "(authorized_max+9999)/10000-min((budget_charge+9999)/10000,(authorized_max+9999)/10000)".into(),
            "amount_limit_cents" => "max(amount_limit/10000,1)".into(),
            "remaining_amount_cents" | "target_amount_cents" => format!("{}/10000", col.trim_end_matches("_cents")),
            _ if col.ends_with("_cents") => format!("({}+9999)/10000", col.trim_end_matches("_cents")),
            _ => col.clone(),
        }).collect();
        conn.execute_batch(&format!(
            "INSERT INTO {table}_micro_build ({}) SELECT {} FROM {table};",
            cols.join(","),
            values.join(",")
        ))
        .unwrap();
    }
    for (name, kind, _) in &objects {
        conn.execute_batch(&format!("DROP {kind} \"{name}\";"))
            .unwrap();
    }
    for table in tables {
        conn.execute_batch(&format!(
            "DROP TABLE {table}; ALTER TABLE {table}_micro_build RENAME TO {table};"
        ))
        .unwrap();
    }
    for (_, _, sql) in objects {
        conn.execute_batch(&sql).unwrap();
    }
    conn.pragma_update(None, "foreign_keys", true).unwrap();
}

#[test]
fn incomplete_micro_profile_is_rejected_before_schema_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.sqlite3");
    micro_profile(&path);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("ALTER TABLE budget_holds DROP COLUMN spend_exact_amount")
        .unwrap();
    let before = snapshot(&conn);
    let schema = rows(&conn, "sqlite_master", "name,sql");
    let mut repo = SqliteGovernanceRepository { conn };
    assert!(repo
        .init()
        .unwrap_err()
        .to_string()
        .contains("budget_holds.spend_exact_amount"));
    assert_eq!(snapshot(&repo.conn), before);
    assert_eq!(rows(&repo.conn, "sqlite_master", "name,sql"), schema);
}
