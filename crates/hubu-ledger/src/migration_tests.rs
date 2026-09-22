use super::*;

fn old_schema(conn: &Connection, direction: &str) -> (LedgerTransactionId, LedgerEntryId) {
    conn.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE ledger_accounts(id TEXT PRIMARY KEY,owner_user_id TEXT NOT NULL,name TEXT NOT NULL,kind TEXT NOT NULL,currency TEXT NOT NULL,created_at TEXT NOT NULL);
        CREATE TABLE ledger_transactions(id TEXT PRIMARY KEY,owner_user_id TEXT NOT NULL,external_ref TEXT,description TEXT NOT NULL,created_at TEXT NOT NULL);
        CREATE TABLE ledger_entries(id TEXT PRIMARY KEY,transaction_id TEXT NOT NULL,owner_user_id TEXT NOT NULL,account_id TEXT NOT NULL,direction TEXT NOT NULL,amount_cents INTEGER NOT NULL CHECK(amount_cents>0),currency TEXT NOT NULL,created_at TEXT NOT NULL,FOREIGN KEY(transaction_id) REFERENCES ledger_transactions(id),FOREIGN KEY(account_id) REFERENCES ledger_accounts(id));
        CREATE TRIGGER ledger_entries_no_update BEFORE UPDATE ON ledger_entries BEGIN SELECT RAISE(ABORT,'immutable'); END;
        CREATE TRIGGER ledger_entries_no_delete BEFORE DELETE ON ledger_entries BEGIN SELECT RAISE(ABORT,'immutable'); END;").unwrap();
    let owner = UserId::new();
    let tx = LedgerTransactionId::new();
    let entry = LedgerEntryId::new();
    let account = LedgerAccountId::new();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO ledger_accounts VALUES (?1,?2,'expense','agent_spend_expense','usd',?3)",
        params![account.to_string(), owner.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions VALUES (?1,?2,'payment-original','legacy payment',?3)",
        params![tx.to_string(), owner.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_entries VALUES (?1,?2,?3,?4,?5,1000,'usd',?6)",
        params![
            entry.to_string(),
            tx.to_string(),
            owner.to_string(),
            account.to_string(),
            direction,
            now
        ],
    )
    .unwrap();
    let cash = LedgerAccountId::new();
    conn.execute(
        "INSERT INTO ledger_accounts VALUES (?1,?2,'cash','user_wallet_cash','usd',?3)",
        params![cash.to_string(), owner.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_entries VALUES (?1,?2,?3,?4,'credit',1000,'usd',?5)",
        params![
            LedgerEntryId::new().to_string(),
            tx.to_string(),
            owner.to_string(),
            cash.to_string(),
            now
        ],
    )
    .unwrap();
    (tx, entry)
}
#[test]
fn deployed_wallet_schema_upgrades_preserving_ids_cents_and_immutability() {
    let conn = Connection::open_in_memory().unwrap();
    let (tx, entry) = old_schema(&conn, "debit");
    domain::initialize(&conn).unwrap();
    domain::initialize(&conn).unwrap();
    let record = domain::load_transaction(&conn, &tx).unwrap().unwrap();
    assert_eq!(record.id, tx);
    assert!(record.entries.iter().any(|row| row.id == entry));
    assert_eq!(record.entries[0].amount.amount, "1000");
    assert_eq!(record.entries[0].amount.scale, 2);
    assert!(record.metadata.is_none());
    assert_eq!(
        conn.query_row("SELECT amount_cents FROM ledger_entries", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1000
    );
    assert!(conn
        .execute("UPDATE ledger_entries SET amount_cents=1", [])
        .is_err());
    assert!(conn.execute("DELETE FROM ledger_transactions", []).is_err());
}
#[test]
fn failed_schema_migration_rolls_back_tables_values_and_triggers() {
    let conn = Connection::open_in_memory().unwrap();
    old_schema(&conn, "invalid-old-direction");
    assert!(domain::initialize(&conn).is_err());
    assert_eq!(
        conn.query_row("SELECT direction FROM ledger_entries", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "invalid-old-direction"
    );
    assert!(conn
        .prepare("SELECT exact_amount FROM ledger_entries")
        .is_err());
    assert!(conn
        .prepare("SELECT kind FROM ledger_transactions")
        .is_err());
    assert!(conn.execute("DELETE FROM ledger_entries", []).is_err());
}
#[test]
fn compatibility_posting_rejects_foreign_accounts_and_handles_large_balanced_totals() {
    let mut ledger = SqliteLedger::in_memory().unwrap();
    let owner = UserId::new();
    let foreign = ledger
        .create_account(
            UserId::new(),
            "foreign",
            LedgerAccountKind::UserWalletCash,
            Currency::Usd,
        )
        .unwrap();
    let debit = ledger
        .create_account(
            owner.clone(),
            "expense",
            LedgerAccountKind::AgentSpendExpense,
            Currency::Usd,
        )
        .unwrap();
    let entry = |account_id, direction| LedgerEntryDraft {
        owner_user_id: owner.clone(),
        account_id,
        direction,
        amount_cents: i64::MAX,
        currency: Currency::Usd,
    };
    assert!(ledger
        .record_transaction(
            owner.clone(),
            None,
            "bad",
            vec![
                entry(debit.id.clone(), LedgerDirection::Debit),
                entry(foreign.id, LedgerDirection::Credit)
            ]
        )
        .is_err());
    let credit = ledger
        .create_account(
            owner.clone(),
            "cash",
            LedgerAccountKind::UserWalletCash,
            Currency::Usd,
        )
        .unwrap();
    ledger
        .record_transaction(
            owner.clone(),
            None,
            "large balanced",
            vec![
                entry(debit.id.clone(), LedgerDirection::Debit),
                entry(debit.id, LedgerDirection::Debit),
                entry(credit.id.clone(), LedgerDirection::Credit),
                entry(credit.id, LedgerDirection::Credit),
            ],
        )
        .unwrap();
    assert_eq!(ledger.list_transactions().unwrap().len(), 1);
}
