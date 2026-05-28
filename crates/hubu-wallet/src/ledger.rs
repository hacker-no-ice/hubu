use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use hubu_common::ids::{LedgerAccountId, LedgerEntryId, LedgerTransactionId};
use hubu_common::money::Currency;
use rusqlite::{params, Connection};

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("sqlite error")]
    Sqlite {
        #[from]
        source: rusqlite::Error,
    },
    #[error("ledger transaction must contain at least two entries")]
    TooFewEntries,
    #[error(
        "ledger transaction is not balanced for {currency}: debits={debits}, credits={credits}"
    )]
    Unbalanced {
        currency: Currency,
        debits: i64,
        credits: i64,
    },
    #[error("ledger entry amount must be positive")]
    NonPositiveAmount,
    #[error("unsupported currency in ledger store")]
    UnsupportedCurrency {
        #[from]
        source: hubu_common::money::ParseCurrencyError,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LedgerAccountKind {
    UserWalletCash,
    AgentSpendExpense,
    MerchantSettlement,
    RailFees,
}

impl LedgerAccountKind {
    fn as_str(self) -> &'static str {
        match self {
            LedgerAccountKind::UserWalletCash => "user_wallet_cash",
            LedgerAccountKind::AgentSpendExpense => "agent_spend_expense",
            LedgerAccountKind::MerchantSettlement => "merchant_settlement",
            LedgerAccountKind::RailFees => "rail_fees",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LedgerDirection {
    Debit,
    Credit,
}

impl LedgerDirection {
    fn as_str(self) -> &'static str {
        match self {
            LedgerDirection::Debit => "debit",
            LedgerDirection::Credit => "credit",
        }
    }

    fn from_str(value: &str) -> Result<Self, rusqlite::Error> {
        match value {
            "debit" => Ok(LedgerDirection::Debit),
            "credit" => Ok(LedgerDirection::Credit),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LedgerAccount {
    pub id: LedgerAccountId,
    pub name: String,
    pub kind: LedgerAccountKind,
    pub currency: Currency,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LedgerEntryDraft {
    pub account_id: LedgerAccountId,
    pub direction: LedgerDirection,
    pub amount_cents: i64,
    pub currency: Currency,
}

#[derive(Debug, Clone)]
pub struct LedgerTransaction {
    pub id: LedgerTransactionId,
    pub external_ref: Option<String>,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LedgerEntry {
    pub id: LedgerEntryId,
    pub transaction_id: LedgerTransactionId,
    pub account_id: LedgerAccountId,
    pub direction: LedgerDirection,
    pub amount_cents: i64,
    pub currency: Currency,
    pub created_at: DateTime<Utc>,
}

pub struct SqliteLedger {
    conn: Connection,
}

impl SqliteLedger {
    pub fn in_memory() -> Result<Self, LedgerError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(conn: Connection) -> Result<Self, LedgerError> {
        let ledger = Self { conn };
        ledger.init()?;
        Ok(ledger)
    }

    fn init(&self) -> Result<(), LedgerError> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ledger_accounts (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                currency TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ledger_transactions (
                id TEXT PRIMARY KEY,
                external_ref TEXT,
                description TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ledger_entries (
                id TEXT PRIMARY KEY,
                transaction_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                direction TEXT NOT NULL,
                amount_cents INTEGER NOT NULL CHECK(amount_cents > 0),
                currency TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(transaction_id) REFERENCES ledger_transactions(id),
                FOREIGN KEY(account_id) REFERENCES ledger_accounts(id)
            );

            CREATE TRIGGER IF NOT EXISTS ledger_transactions_no_update
            BEFORE UPDATE ON ledger_transactions
            BEGIN
                SELECT RAISE(ABORT, 'ledger transactions are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS ledger_transactions_no_delete
            BEFORE DELETE ON ledger_transactions
            BEGIN
                SELECT RAISE(ABORT, 'ledger transactions are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS ledger_entries_no_update
            BEFORE UPDATE ON ledger_entries
            BEGIN
                SELECT RAISE(ABORT, 'ledger entries are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS ledger_entries_no_delete
            BEFORE DELETE ON ledger_entries
            BEGIN
                SELECT RAISE(ABORT, 'ledger entries are immutable');
            END;
            ",
        )?;

        Ok(())
    }

    pub fn create_account(
        &self,
        name: impl Into<String>,
        kind: LedgerAccountKind,
        currency: Currency,
    ) -> Result<LedgerAccount, LedgerError> {
        let account = LedgerAccount {
            id: LedgerAccountId::new(),
            name: name.into(),
            kind,
            currency,
            created_at: Utc::now(),
        };

        self.conn.execute(
            "INSERT INTO ledger_accounts (id, name, kind, currency, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account.id.to_string(),
                account.name,
                account.kind.as_str(),
                account.currency.to_string(),
                account.created_at.to_rfc3339(),
            ],
        )?;

        Ok(account)
    }

    pub fn record_transaction(
        &mut self,
        external_ref: Option<String>,
        description: impl Into<String>,
        entries: Vec<LedgerEntryDraft>,
    ) -> Result<LedgerTransaction, LedgerError> {
        validate_entries_balance(&entries)?;

        let ledger_tx = LedgerTransaction {
            id: LedgerTransactionId::new(),
            external_ref,
            description: description.into(),
            created_at: Utc::now(),
        };

        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "INSERT INTO ledger_transactions (id, external_ref, description, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                ledger_tx.id.to_string(),
                ledger_tx.external_ref,
                ledger_tx.description,
                ledger_tx.created_at.to_rfc3339(),
            ],
        )?;

        for entry in entries {
            let id = LedgerEntryId::new();
            sqlite_tx.execute(
                "INSERT INTO ledger_entries
                 (id, transaction_id, account_id, direction, amount_cents, currency, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id.to_string(),
                    ledger_tx.id.to_string(),
                    entry.account_id.to_string(),
                    entry.direction.as_str(),
                    entry.amount_cents,
                    entry.currency.to_string(),
                    ledger_tx.created_at.to_rfc3339(),
                ],
            )?;
        }

        sqlite_tx.commit()?;

        Ok(ledger_tx)
    }

    pub fn entries_for_transaction(
        &self,
        transaction_id: &LedgerTransactionId,
    ) -> Result<Vec<LedgerEntry>, LedgerError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, direction, amount_cents, currency, created_at
             FROM ledger_entries
             WHERE transaction_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![transaction_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let account_id: String = row.get(1)?;
            let direction: String = row.get(2)?;
            let currency: String = row.get(4)?;
            let created_at: String = row.get(5)?;

            Ok(LedgerEntry {
                id: LedgerEntryId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                transaction_id: transaction_id.clone(),
                account_id: LedgerAccountId::from_str(&account_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                direction: LedgerDirection::from_str(&direction)?,
                amount_cents: row.get(3)?,
                currency: Currency::from_str(&currency)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?
                    .with_timezone(&Utc),
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_transactions(&self) -> Result<Vec<LedgerTransaction>, LedgerError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, external_ref, description, created_at
             FROM ledger_transactions
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let created_at: String = row.get(3)?;

            Ok(LedgerTransaction {
                id: LedgerTransactionId::from_str(&id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                external_ref: row.get(1)?,
                description: row.get(2)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?
                    .with_timezone(&Utc),
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    #[cfg(test)]
    fn raw_connection(&self) -> &Connection {
        &self.conn
    }
}

fn validate_entries_balance(entries: &[LedgerEntryDraft]) -> Result<(), LedgerError> {
    if entries.len() < 2 {
        return Err(LedgerError::TooFewEntries);
    }

    let mut totals: HashMap<Currency, (i64, i64)> = HashMap::new();

    for entry in entries {
        if entry.amount_cents <= 0 {
            return Err(LedgerError::NonPositiveAmount);
        }

        let total = totals.entry(entry.currency).or_insert((0, 0));
        match entry.direction {
            LedgerDirection::Debit => total.0 += entry.amount_cents,
            LedgerDirection::Credit => total.1 += entry.amount_cents,
        }
    }

    for (currency, (debits, credits)) in totals {
        if debits != credits {
            return Err(LedgerError::Unbalanced {
                currency,
                debits,
                credits,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_with_accounts() -> (SqliteLedger, LedgerAccount, LedgerAccount) {
        let ledger = SqliteLedger::in_memory().expect("ledger should initialize");
        let debit_account = ledger
            .create_account(
                "Agent spend expense",
                LedgerAccountKind::AgentSpendExpense,
                Currency::Usd,
            )
            .expect("debit account should be created");
        let credit_account = ledger
            .create_account(
                "Hubu wallet cash",
                LedgerAccountKind::UserWalletCash,
                Currency::Usd,
            )
            .expect("credit account should be created");

        (ledger, debit_account, credit_account)
    }

    #[test]
    fn records_balanced_double_entry_transaction() {
        let (mut ledger, debit_account, credit_account) = ledger_with_accounts();

        let transaction = ledger
            .record_transaction(
                Some("payment_1".to_string()),
                "mock payment",
                vec![
                    LedgerEntryDraft {
                        account_id: debit_account.id,
                        direction: LedgerDirection::Debit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        account_id: credit_account.id,
                        direction: LedgerDirection::Credit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                ],
            )
            .expect("balanced transaction should be recorded");

        let entries = ledger
            .entries_for_transaction(&transaction.id)
            .expect("entries should be readable");

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.direction == LedgerDirection::Debit)
                .map(|entry| entry.amount_cents)
                .sum::<i64>(),
            2_500
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.direction == LedgerDirection::Credit)
                .map(|entry| entry.amount_cents)
                .sum::<i64>(),
            2_500
        );
    }

    #[test]
    fn rejects_unbalanced_transaction() {
        let (mut ledger, debit_account, credit_account) = ledger_with_accounts();

        let error = ledger
            .record_transaction(
                None,
                "bad payment",
                vec![
                    LedgerEntryDraft {
                        account_id: debit_account.id,
                        direction: LedgerDirection::Debit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        account_id: credit_account.id,
                        direction: LedgerDirection::Credit,
                        amount_cents: 2_499,
                        currency: Currency::Usd,
                    },
                ],
            )
            .expect_err("unbalanced transaction should be rejected");

        assert!(matches!(error, LedgerError::Unbalanced { .. }));
    }

    #[test]
    fn ledger_entries_are_immutable_in_sqlite() {
        let (mut ledger, debit_account, credit_account) = ledger_with_accounts();

        let transaction = ledger
            .record_transaction(
                None,
                "mock payment",
                vec![
                    LedgerEntryDraft {
                        account_id: debit_account.id,
                        direction: LedgerDirection::Debit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        account_id: credit_account.id,
                        direction: LedgerDirection::Credit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                ],
            )
            .expect("balanced transaction should be recorded");

        let error = ledger
            .raw_connection()
            .execute(
                "UPDATE ledger_entries SET amount_cents = 1 WHERE transaction_id = ?1",
                params![transaction.id.to_string()],
            )
            .expect_err("ledger entry update should be blocked");

        assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
    }
}
