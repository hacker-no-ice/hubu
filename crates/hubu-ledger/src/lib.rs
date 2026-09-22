pub mod domain;
pub use domain::*;

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use hubu_common::ids::{LedgerAccountId, LedgerEntryId, LedgerTransactionId, UserId};
use hubu_common::money::Currency;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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
        debits: i128,
        credits: i128,
    },
    #[error("ledger entry amount must be positive")]
    NonPositiveAmount,
    #[error("ledger entry owner does not match transaction owner")]
    OwnerMismatch,
    #[error("unsupported currency in ledger store")]
    UnsupportedCurrency {
        #[from]
        source: hubu_common::money::ParseCurrencyError,
    },
    #[error("invalid ledger data: {0}")]
    InvalidData(String),
    #[error("invalid ledger metadata")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum LedgerAccountKind {
    UserWalletCash,
    AgentSpendExpense,
    MerchantSettlement,
    RailFees,
    ExternallyBilledClearing,
}

impl LedgerAccountKind {
    fn as_str(self) -> &'static str {
        match self {
            LedgerAccountKind::UserWalletCash => "user_wallet_cash",
            LedgerAccountKind::AgentSpendExpense => "agent_spend_expense",
            LedgerAccountKind::MerchantSettlement => "merchant_settlement",
            LedgerAccountKind::RailFees => "rail_fees",
            LedgerAccountKind::ExternallyBilledClearing => "externally_billed_clearing",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
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
    pub owner_user_id: UserId,
    pub name: String,
    pub kind: LedgerAccountKind,
    pub currency: Currency,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LedgerEntryDraft {
    pub owner_user_id: UserId,
    pub account_id: LedgerAccountId,
    pub direction: LedgerDirection,
    pub amount_cents: i64,
    pub currency: Currency,
}

#[derive(Debug, Clone)]
/// Compatibility projection for the existing cent-only wallet API.
/// Use `TransactionRecord` for the canonical domain model.
pub struct LedgerTransaction {
    pub id: LedgerTransactionId,
    pub owner_user_id: UserId,
    pub external_ref: Option<String>,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
/// Cent-only wallet compatibility projection; canonical lines are `TransactionEntry`.
pub struct LedgerEntry {
    pub id: LedgerEntryId,
    pub transaction_id: LedgerTransactionId,
    pub owner_user_id: UserId,
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
        domain::initialize(&self.conn)
    }

    /// Canonical owner-scoped transactions, including wallet, provider and adjustments.
    pub fn transactions_for_owner(
        &self,
        owner: &UserId,
    ) -> Result<Vec<TransactionRecord>, LedgerError> {
        domain::owner_transactions(&self.conn, owner)
    }

    pub fn create_account(
        &self,
        owner_user_id: UserId,
        name: impl Into<String>,
        kind: LedgerAccountKind,
        currency: Currency,
    ) -> Result<LedgerAccount, LedgerError> {
        let account = LedgerAccount {
            id: LedgerAccountId::new(),
            owner_user_id,
            name: name.into(),
            kind,
            currency,
            created_at: Utc::now(),
        };

        self.conn.execute(
            "INSERT INTO ledger_accounts (id, owner_user_id, name, kind, currency, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                account.id.to_string(),
                account.owner_user_id.to_string(),
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
        owner_user_id: UserId,
        external_ref: Option<String>,
        description: impl Into<String>,
        entries: Vec<LedgerEntryDraft>,
    ) -> Result<LedgerTransaction, LedgerError> {
        self.record_transaction_with_metadata(
            owner_user_id,
            external_ref,
            description,
            entries,
            None,
        )
    }

    pub fn record_transaction_with_metadata(
        &mut self,
        owner_user_id: UserId,
        external_ref: Option<String>,
        description: impl Into<String>,
        entries: Vec<LedgerEntryDraft>,
        metadata: Option<TransactionMetadata>,
    ) -> Result<LedgerTransaction, LedgerError> {
        validate_entries_balance(&entries)?;
        validate_entries_owner(&owner_user_id, &entries)?;
        if let Some(metadata) = &metadata {
            let debit_total: i128 = entries
                .iter()
                .filter(|entry| entry.direction == LedgerDirection::Debit)
                .map(|entry| i128::from(entry.amount_cents))
                .sum();
            if metadata.kind != TransactionKind::WalletPayment
                || metadata.effective_cost.scaled()?
                    != debit_total
                        .checked_mul(CENT_AT_SCALE_18)
                        .ok_or_else(|| LedgerError::InvalidData("exact debit overflow".into()))?
            {
                return Err(LedgerError::InvalidData(
                    "wallet metadata does not match posted expense".into(),
                ));
            }
            if metadata.context.is_some()
                && (metadata.budget_charge_delta_cents
                    != Some(metadata.effective_cost.budget_cents()?)
                    || metadata.rounding_delta_amount.as_deref() != Some("0"))
            {
                return Err(LedgerError::InvalidData(
                    "wallet budget charge does not match expense".into(),
                ));
            }
        }

        let ledger_tx = LedgerTransaction {
            id: LedgerTransactionId::new(),
            owner_user_id,
            external_ref,
            description: description.into(),
            created_at: Utc::now(),
        };

        let sqlite_tx = self.conn.transaction()?;
        for entry in &entries {
            let (owner, currency): (String, String) = sqlite_tx.query_row(
                "SELECT owner_user_id, currency FROM ledger_accounts WHERE id=?1",
                [entry.account_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if owner != ledger_tx.owner_user_id.to_string()
                || currency != entry.currency.to_string()
            {
                return Err(LedgerError::InvalidData(
                    "ledger account owner or currency mismatch".into(),
                ));
            }
        }

        sqlite_tx.execute(
            "INSERT INTO ledger_transactions (id, owner_user_id, external_ref, description, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                ledger_tx.id.to_string(),
                ledger_tx.owner_user_id.to_string(),
                ledger_tx.external_ref,
                ledger_tx.description,
                ledger_tx.created_at.to_rfc3339(),
            ],
        )?;

        for entry in entries {
            let id = LedgerEntryId::new();
            sqlite_tx.execute(
                "INSERT INTO ledger_entries
                 (id, transaction_id, owner_user_id, account_id, direction, amount_cents, currency, created_at, exact_amount, exact_scale)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CAST(?6 AS TEXT), 2)",
                params![
                    id.to_string(),
                    ledger_tx.id.to_string(),
                    entry.owner_user_id.to_string(),
                    entry.account_id.to_string(),
                    entry.direction.as_str(),
                    entry.amount_cents,
                    entry.currency.to_string(),
                    ledger_tx.created_at.to_rfc3339(),
                ],
            )?;
        }

        if let Some(metadata) = metadata {
            domain::attach_metadata(&sqlite_tx, &ledger_tx.id, &metadata)?;
        }
        sqlite_tx.commit()?;

        Ok(ledger_tx)
    }

    pub fn entries_for_transaction(
        &self,
        transaction_id: &LedgerTransactionId,
    ) -> Result<Vec<LedgerEntry>, LedgerError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner_user_id, account_id, direction, amount_cents, currency, created_at
             FROM ledger_entries
             WHERE transaction_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![transaction_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let owner_user_id: String = row.get(1)?;
            let account_id: String = row.get(2)?;
            let direction: String = row.get(3)?;
            let currency: String = row.get(5)?;
            let created_at: String = row.get(6)?;

            Ok(LedgerEntry {
                id: LedgerEntryId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                transaction_id: transaction_id.clone(),
                owner_user_id: UserId::from_str(&owner_user_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                account_id: LedgerAccountId::from_str(&account_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                direction: LedgerDirection::from_str(&direction)?,
                amount_cents: row.get(4)?,
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
            "SELECT id, owner_user_id, external_ref, description, created_at
             FROM ledger_transactions
             WHERE kind = 'wallet_payment'
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let owner_user_id: String = row.get(1)?;
            let created_at: String = row.get(4)?;

            Ok(LedgerTransaction {
                id: LedgerTransactionId::from_str(&id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                owner_user_id: UserId::from_str(&owner_user_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                external_ref: row.get(2)?,
                description: row.get(3)?,
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

    let mut totals: HashMap<Currency, (i128, i128)> = HashMap::new();

    for entry in entries {
        if entry.amount_cents <= 0 {
            return Err(LedgerError::NonPositiveAmount);
        }

        let total = totals.entry(entry.currency).or_insert((0, 0));
        match entry.direction {
            LedgerDirection::Debit => {
                total.0 = total
                    .0
                    .checked_add(i128::from(entry.amount_cents))
                    .ok_or_else(|| LedgerError::InvalidData("debit total overflow".into()))?
            }
            LedgerDirection::Credit => {
                total.1 = total
                    .1
                    .checked_add(i128::from(entry.amount_cents))
                    .ok_or_else(|| LedgerError::InvalidData("credit total overflow".into()))?
            }
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

fn validate_entries_owner(
    owner_user_id: &UserId,
    entries: &[LedgerEntryDraft],
) -> Result<(), LedgerError> {
    if entries
        .iter()
        .all(|entry| &entry.owner_user_id == owner_user_id)
    {
        Ok(())
    } else {
        Err(LedgerError::OwnerMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_with_accounts() -> (SqliteLedger, LedgerAccount, LedgerAccount) {
        let ledger = SqliteLedger::in_memory().expect("ledger should initialize");
        let owner_user_id = test_user_id();
        let debit_account = ledger
            .create_account(
                owner_user_id.clone(),
                "Agent spend expense",
                LedgerAccountKind::AgentSpendExpense,
                Currency::Usd,
            )
            .expect("debit account should be created");
        let credit_account = ledger
            .create_account(
                owner_user_id,
                "Hubu wallet cash",
                LedgerAccountKind::UserWalletCash,
                Currency::Usd,
            )
            .expect("credit account should be created");

        (ledger, debit_account, credit_account)
    }

    fn test_user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    #[test]
    fn records_balanced_double_entry_transaction() {
        let (mut ledger, debit_account, credit_account) = ledger_with_accounts();

        let transaction = ledger
            .record_transaction(
                test_user_id(),
                Some("payment_1".to_string()),
                "mock payment",
                vec![
                    LedgerEntryDraft {
                        owner_user_id: debit_account.owner_user_id.clone(),
                        account_id: debit_account.id,
                        direction: LedgerDirection::Debit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        owner_user_id: credit_account.owner_user_id.clone(),
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
        assert!(entries
            .iter()
            .all(|entry| entry.owner_user_id == test_user_id()));
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
                test_user_id(),
                None,
                "bad payment",
                vec![
                    LedgerEntryDraft {
                        owner_user_id: debit_account.owner_user_id.clone(),
                        account_id: debit_account.id,
                        direction: LedgerDirection::Debit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        owner_user_id: credit_account.owner_user_id.clone(),
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
                test_user_id(),
                None,
                "mock payment",
                vec![
                    LedgerEntryDraft {
                        owner_user_id: debit_account.owner_user_id.clone(),
                        account_id: debit_account.id,
                        direction: LedgerDirection::Debit,
                        amount_cents: 2_500,
                        currency: Currency::Usd,
                    },
                    LedgerEntryDraft {
                        owner_user_id: credit_account.owner_user_id.clone(),
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

#[cfg(test)]
mod migration_tests;
