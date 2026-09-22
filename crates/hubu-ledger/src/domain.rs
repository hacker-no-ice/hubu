//! Canonical accounting model shared by wallet and provider execution. Budget
//! context is optional; transaction identity and balanced lines are independent.
use super::*;
use hubu_common::ids::{AgentAccountId, AgentId, BudgetId, BudgetVersionId, SpendDecisionId};
use rusqlite::OptionalExtension;
use serde_json::Value;

pub const CENT_AT_SCALE_18: i128 = 10_000_000_000_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExactMoney {
    pub amount: String,
    pub scale: u32,
    pub currency: Currency,
}
impl ExactMoney {
    pub fn scaled(&self) -> Result<i128, LedgerError> {
        let amount: i128 = self
            .amount
            .parse()
            .map_err(|_| invalid("invalid exact amount"))?;
        if amount < 0 || self.scale > 18 {
            return Err(invalid("amount must be nonnegative with scale <= 18"));
        }
        amount
            .checked_mul(10_i128.pow(18 - self.scale))
            .ok_or_else(|| invalid("exact amount overflow"))
    }
    pub fn budget_cents(&self) -> Result<i64, LedgerError> {
        let amount = self.scaled()?;
        i64::try_from(amount / CENT_AT_SCALE_18 + i128::from(amount % CENT_AT_SCALE_18 != 0))
            .map_err(|_| invalid("budget cents overflow"))
    }
    pub fn at_scale_18(amount: i128, currency: Currency) -> Self {
        Self {
            amount: amount.to_string(),
            scale: 18,
            currency,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovernedSpendContext {
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub budget_id: BudgetId,
    pub budget_version_id: BudgetVersionId,
    pub spend_decision_id: SpendDecisionId,
    pub operation_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    WalletPayment,
    ExternalProvider,
    Adjustment,
}
impl TransactionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WalletPayment => "wallet_payment",
            Self::ExternalProvider => "external_provider",
            Self::Adjustment => "adjustment",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionMetadata {
    pub kind: TransactionKind,
    pub source_key: String,
    pub agent_id: Option<AgentId>,
    pub agent_account_id: Option<AgentAccountId>,
    pub context: Option<GovernedSpendContext>,
    pub effective_cost: ExactMoney,
    pub budget_charge_delta_cents: Option<i64>,
    /// Signed coefficient at scale 18: budget charge minus exact expense delta.
    pub rounding_delta_amount: Option<String>,
    pub original_transaction_id: Option<LedgerTransactionId>,
    pub previous_transaction_id: Option<LedgerTransactionId>,
    pub provider: Option<String>,
    pub billing_merchant: Option<String>,
    pub purpose: Option<String>,
    pub source_evidence: Value,
    pub reason: Option<String>,
    pub adjustment_evidence: Option<String>,
    pub legacy_backfill: bool,
    pub missing_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionEntry {
    pub id: LedgerEntryId,
    pub account_id: LedgerAccountId,
    pub account_kind: LedgerAccountKind,
    pub direction: LedgerDirection,
    pub amount: ExactMoney,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionRecord {
    pub id: LedgerTransactionId,
    pub owner_user_id: UserId,
    pub external_ref: Option<String>,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub kind: TransactionKind,
    pub metadata: Option<TransactionMetadata>,
    /// Missing historical evidence remains explicit rather than inferred.
    pub missing_evidence: Vec<String>,
    pub entries: Vec<TransactionEntry>,
}

fn invalid(message: &str) -> LedgerError {
    LedgerError::InvalidData(message.to_string())
}

/// Installs or upgrades the deployed wallet tables in one transaction. Historical
/// IDs and cents remain intact; exact provider entries have NULL legacy cents.
pub fn initialize(conn: &Connection) -> Result<(), LedgerError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS ledger_accounts (
        id TEXT PRIMARY KEY, owner_user_id TEXT NOT NULL, name TEXT NOT NULL,
        kind TEXT NOT NULL, currency TEXT NOT NULL, created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS ledger_transactions (
        id TEXT PRIMARY KEY, owner_user_id TEXT NOT NULL, external_ref TEXT,
        description TEXT NOT NULL, created_at TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'wallet_payment');")?;
    let columns = |table: &str| -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
        let values = stmt.query_map([], |r| r.get(1))?.collect();
        values
    };
    if !columns("ledger_transactions")?.contains(&"kind".to_string()) {
        tx.execute("ALTER TABLE ledger_transactions ADD COLUMN kind TEXT NOT NULL DEFAULT 'wallet_payment'", [])?;
    }
    let entries = columns("ledger_entries")?;
    let migrate = !entries.is_empty() && !entries.contains(&"exact_amount".to_string());
    if migrate {
        tx.execute_batch(
            "DROP TRIGGER IF EXISTS ledger_entries_no_update;
            DROP TRIGGER IF EXISTS ledger_entries_no_delete;
            ALTER TABLE ledger_entries RENAME TO ledger_entries_legacy;",
        )?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS ledger_entries (
        id TEXT PRIMARY KEY, transaction_id TEXT NOT NULL REFERENCES ledger_transactions(id),
        owner_user_id TEXT NOT NULL, account_id TEXT NOT NULL REFERENCES ledger_accounts(id),
        direction TEXT NOT NULL CHECK(direction IN ('debit','credit')),
        amount_cents INTEGER CHECK(amount_cents IS NULL OR amount_cents > 0),
        currency TEXT NOT NULL, created_at TEXT NOT NULL,
        exact_amount TEXT NOT NULL, exact_scale INTEGER NOT NULL CHECK(exact_scale BETWEEN 0 AND 18));")?;
    if migrate {
        tx.execute_batch(
            "INSERT INTO ledger_entries SELECT id, transaction_id, owner_user_id,
            account_id, direction, amount_cents, currency, created_at, CAST(amount_cents AS TEXT), 2
            FROM ledger_entries_legacy; DROP TABLE ledger_entries_legacy;",
        )?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS ledger_transaction_metadata (
        transaction_id TEXT PRIMARY KEY REFERENCES ledger_transactions(id),
        source_key TEXT NOT NULL UNIQUE,
        previous_transaction_id TEXT UNIQUE REFERENCES ledger_transactions(id),
        record_json TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS ledger_transactions_no_update BEFORE UPDATE ON ledger_transactions BEGIN SELECT RAISE(ABORT, 'ledger transactions are immutable'); END;
        CREATE TRIGGER IF NOT EXISTS ledger_transactions_no_delete BEFORE DELETE ON ledger_transactions BEGIN SELECT RAISE(ABORT, 'ledger transactions are immutable'); END;
        CREATE TRIGGER IF NOT EXISTS ledger_entries_no_update BEFORE UPDATE ON ledger_entries BEGIN SELECT RAISE(ABORT, 'ledger entries are immutable'); END;
        CREATE TRIGGER IF NOT EXISTS ledger_entries_no_delete BEFORE DELETE ON ledger_entries BEGIN SELECT RAISE(ABORT, 'ledger entries are immutable'); END;
        CREATE TRIGGER IF NOT EXISTS ledger_metadata_no_update BEFORE UPDATE ON ledger_transaction_metadata BEGIN SELECT RAISE(ABORT, 'ledger metadata is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS ledger_metadata_no_delete BEFORE DELETE ON ledger_transaction_metadata BEGIN SELECT RAISE(ABORT, 'ledger metadata is immutable'); END;")?;
    tx.commit()?;
    Ok(())
}

/// Append evidence to a transaction without changing any historical posting.
/// This is also the migration path for authentic, uniquely linked wallet evidence.
pub fn attach_metadata(
    conn: &Connection,
    id: &LedgerTransactionId,
    metadata: &TransactionMetadata,
) -> Result<(), LedgerError> {
    let cost = metadata.effective_cost.scaled()?;
    if metadata.source_key.trim().is_empty() {
        return Err(invalid("source key is required"));
    }
    let current = load_transaction(conn, id)?.ok_or_else(|| invalid("missing transaction"))?;
    let mut debits = 0_i128;
    let mut credits = 0_i128;
    let mut expense = 0_i128;
    for entry in &current.entries {
        if entry.amount.currency != metadata.effective_cost.currency {
            return Err(invalid("evidence currency does not match entry"));
        }
        let amount = entry.amount.scaled()?;
        let (owner, currency): (String, String) = conn.query_row(
            "SELECT owner_user_id,currency FROM ledger_accounts WHERE id=?1",
            [entry.account_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if owner != current.owner_user_id.to_string()
            || currency != entry.amount.currency.to_string()
        {
            return Err(invalid(
                "account owner or currency does not match transaction",
            ));
        }
        match entry.direction {
            LedgerDirection::Debit => {
                debits = debits
                    .checked_add(amount)
                    .ok_or_else(|| invalid("debit sum overflow"))?
            }
            LedgerDirection::Credit => {
                credits = credits
                    .checked_add(amount)
                    .ok_or_else(|| invalid("credit sum overflow"))?
            }
        }
        if entry.account_kind == LedgerAccountKind::AgentSpendExpense {
            expense = expense
                .checked_add(if entry.direction == LedgerDirection::Debit {
                    amount
                } else {
                    -amount
                })
                .ok_or_else(|| invalid("expense sum overflow"))?;
        }
    }
    if current.entries.len() < 2 || debits != credits {
        return Err(invalid("canonical transaction is not balanced"));
    }
    let expected_charge = if metadata.kind == TransactionKind::Adjustment {
        let original_id = metadata
            .original_transaction_id
            .as_ref()
            .ok_or_else(|| invalid("adjustment original is required"))?;
        let previous_id = metadata
            .previous_transaction_id
            .as_ref()
            .ok_or_else(|| invalid("adjustment predecessor is required"))?;
        let original = load_transaction(conn, original_id)?
            .ok_or_else(|| invalid("missing adjustment original"))?;
        let previous = load_transaction(conn, previous_id)?
            .ok_or_else(|| invalid("missing adjustment predecessor"))?;
        let original_meta = original
            .metadata
            .as_ref()
            .ok_or_else(|| invalid("original evidence is missing"))?;
        let previous_meta = previous
            .metadata
            .as_ref()
            .ok_or_else(|| invalid("predecessor evidence is missing"))?;
        if original.owner_user_id != current.owner_user_id
            || previous.owner_user_id != current.owner_user_id
            || original.kind == TransactionKind::Adjustment
            || metadata.context != original_meta.context
            || (previous.id != original.id
                && previous_meta.original_transaction_id.as_ref() != Some(original_id))
            || previous_meta.effective_cost.currency != metadata.effective_cost.currency
            || previous_meta.effective_cost.scaled()?.checked_add(expense) != Some(cost)
        {
            return Err(invalid(
                "adjustment evidence does not match original and posted delta",
            ));
        }
        if metadata.context.is_some() {
            Some(
                metadata.effective_cost.budget_cents()?
                    - previous_meta.effective_cost.budget_cents()?,
            )
        } else {
            None
        }
    } else {
        if metadata.original_transaction_id.is_some()
            || metadata.previous_transaction_id.is_some()
            || expense != cost
        {
            return Err(invalid("expense evidence does not match posting"));
        }
        if metadata.context.is_some() {
            Some(metadata.effective_cost.budget_cents()?)
        } else {
            None
        }
    };
    if metadata.budget_charge_delta_cents != expected_charge {
        return Err(invalid("budget charge does not match exact expense"));
    }
    if let Some(charge) = expected_charge {
        let expected = i128::from(charge) * CENT_AT_SCALE_18 - expense;
        if metadata.rounding_delta_amount.as_deref() != Some(expected.to_string().as_str()) {
            return Err(invalid("rounding evidence does not match posting"));
        }
    }
    let kind: String = conn.query_row(
        "SELECT kind FROM ledger_transactions WHERE id=?1",
        [id.to_string()],
        |r| r.get(0),
    )?;
    if kind != metadata.kind.as_str() {
        return Err(invalid("transaction kind does not match evidence"));
    }
    if let Some(context) = &metadata.context {
        if context.operation_key.trim().is_empty()
            || metadata.agent_id.as_ref() != Some(&context.agent_id)
            || metadata.agent_account_id.as_ref() != Some(&context.agent_account_id)
        {
            return Err(invalid("agent context mismatch"));
        }
    }

    if metadata.context.is_some()
        && (metadata.budget_charge_delta_cents.is_none()
            || metadata.rounding_delta_amount.is_none())
    {
        return Err(invalid(
            "governed transaction requires budget charge and rounding evidence",
        ));
    }
    conn.execute("INSERT INTO ledger_transaction_metadata (transaction_id, source_key, previous_transaction_id, record_json) VALUES (?1, ?2, ?3, ?4)",
        params![id.to_string(), metadata.source_key, metadata.previous_transaction_id.as_ref().map(ToString::to_string), serde_json::to_string(metadata)?])?;
    Ok(())
}

type StoredTransactionHeader = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
);

pub fn load_transaction(
    conn: &Connection,
    id: &LedgerTransactionId,
) -> Result<Option<TransactionRecord>, LedgerError> {
    let row: Option<StoredTransactionHeader> = conn.query_row(
        "SELECT t.owner_user_id, t.external_ref, t.description, t.created_at, m.record_json, t.kind FROM ledger_transactions t LEFT JOIN ledger_transaction_metadata m ON m.transaction_id = t.id WHERE t.id = ?1",
        [id.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).optional()?;
    let Some((owner, external_ref, description, timestamp, metadata, kind)) = row else {
        return Ok(None);
    };
    let mut stmt = conn.prepare("SELECT e.id, e.account_id, a.kind, e.direction, e.exact_amount, e.exact_scale, e.currency FROM ledger_entries e JOIN ledger_accounts a ON a.id=e.account_id WHERE e.transaction_id=?1 ORDER BY e.id")?;
    let raw = stmt
        .query_map([id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, u32>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut entries = Vec::new();
    for (entry_id, account, kind, direction, amount, scale, currency) in raw {
        let kind = match kind.as_str() {
            "user_wallet_cash" => LedgerAccountKind::UserWalletCash,
            "agent_spend_expense" => LedgerAccountKind::AgentSpendExpense,
            "merchant_settlement" => LedgerAccountKind::MerchantSettlement,
            "rail_fees" => LedgerAccountKind::RailFees,
            "externally_billed_clearing" => LedgerAccountKind::ExternallyBilledClearing,
            _ => return Err(invalid("unknown account kind")),
        };
        entries.push(TransactionEntry {
            id: entry_id.parse().map_err(|_| invalid("invalid entry id"))?,
            account_id: account.parse().map_err(|_| invalid("invalid account id"))?,
            account_kind: kind,
            direction: LedgerDirection::from_str(&direction)?,
            amount: ExactMoney {
                amount,
                scale,
                currency: currency.parse()?,
            },
        });
    }
    Ok(Some(TransactionRecord {
        id: id.clone(),
        owner_user_id: owner.parse().map_err(|_| invalid("invalid owner id"))?,
        external_ref,
        description,
        created_at: DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|_| invalid("invalid timestamp"))?
            .with_timezone(&Utc),
        kind: match kind.as_str() {
            "wallet_payment" => TransactionKind::WalletPayment,
            "external_provider" => TransactionKind::ExternalProvider,
            "adjustment" => TransactionKind::Adjustment,
            _ => return Err(invalid("unknown transaction kind")),
        },
        missing_evidence: if metadata.is_none() {
            vec!["source_evidence".into(), "governed_context".into()]
        } else {
            vec![]
        },
        metadata: metadata.map(|s| serde_json::from_str(&s)).transpose()?,
        entries,
    }))
}

pub fn find_source(
    conn: &Connection,
    source_key: &str,
) -> Result<Option<TransactionRecord>, LedgerError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT transaction_id FROM ledger_transaction_metadata WHERE source_key=?1",
            [source_key],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| {
        let id = id.parse().map_err(|_| invalid("invalid transaction id"))?;
        load_transaction(conn, &id)
    })
    .transpose()
    .map(Option::flatten)
}

pub fn owner_transactions(
    conn: &Connection,
    owner: &UserId,
) -> Result<Vec<TransactionRecord>, LedgerError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM ledger_transactions WHERE owner_user_id=?1 ORDER BY created_at,id",
    )?;
    let ids = stmt
        .query_map([owner.to_string()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| {
            load_transaction(
                conn,
                &id.parse().map_err(|_| invalid("invalid transaction id"))?,
            )?
            .ok_or_else(|| invalid("missing transaction"))
        })
        .collect()
}

/// Atomically writes one canonical balanced pair through the caller's transaction.
/// A negative delta reverses the original account directions; it never invokes a rail.
pub struct PairPosting<'a> {
    pub owner: &'a UserId,
    pub debit_kind: LedgerAccountKind,
    pub credit_kind: LedgerAccountKind,
    pub signed_amount: i128,
    pub metadata: TransactionMetadata,
    pub external_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub fn post_pair(
    tx: &rusqlite::Transaction<'_>,
    posting: PairPosting<'_>,
) -> Result<TransactionRecord, LedgerError> {
    let PairPosting {
        owner,
        debit_kind,
        credit_kind,
        signed_amount,
        metadata,
        external_ref,
        created_at,
    } = posting;
    metadata.effective_cost.scaled()?;
    if debit_kind == credit_kind || metadata.kind == TransactionKind::WalletPayment {
        return Err(invalid(
            "exact pair requires distinct accounts and a provider/adjustment source",
        ));
    }
    let amount = signed_amount
        .checked_abs()
        .ok_or_else(|| invalid("expense delta overflow"))?;
    let id = LedgerTransactionId::new();
    tx.execute("INSERT INTO ledger_transactions (id,owner_user_id,external_ref,description,created_at,kind) VALUES (?1,?2,?3,?4,?5,?6)",params![id.to_string(),owner.to_string(),external_ref,metadata.kind.as_str(),created_at.to_rfc3339(),metadata.kind.as_str()])?;
    for (kind, mut direction) in [
        (debit_kind, LedgerDirection::Debit),
        (credit_kind, LedgerDirection::Credit),
    ] {
        if signed_amount < 0 {
            direction = match direction {
                LedgerDirection::Debit => LedgerDirection::Credit,
                LedgerDirection::Credit => LedgerDirection::Debit,
            };
        }
        let account: Option<String>=tx.query_row("SELECT id FROM ledger_accounts WHERE owner_user_id=?1 AND kind=?2 AND currency=?3 ORDER BY id LIMIT 1",params![owner.to_string(),kind.as_str(),metadata.effective_cost.currency.to_string()],|r|r.get(0)).optional()?;
        let account = match account {
            Some(id) => id,
            None => {
                let id = LedgerAccountId::new().to_string();
                tx.execute(
                    "INSERT INTO ledger_accounts VALUES (?1,?2,?3,?3,?4,?5)",
                    params![
                        id,
                        owner.to_string(),
                        kind.as_str(),
                        metadata.effective_cost.currency.to_string(),
                        created_at.to_rfc3339()
                    ],
                )?;
                id
            }
        };
        tx.execute("INSERT INTO ledger_entries (id,transaction_id,owner_user_id,account_id,direction,amount_cents,currency,created_at,exact_amount,exact_scale) VALUES (?1,?2,?3,?4,?5,NULL,?6,?7,?8,18)",params![LedgerEntryId::new().to_string(),id.to_string(),owner.to_string(),account,direction.as_str(),metadata.effective_cost.currency.to_string(),created_at.to_rfc3339(),amount.to_string()])?;
    }
    attach_metadata(tx, &id, &metadata)?;
    load_transaction(tx, &id)?.ok_or_else(|| invalid("missing posting"))
}
