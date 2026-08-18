use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use hubu_common::execution_scope::ExecutionScope;
use hubu_common::ids::{
    AgentAccountId, AgentId, LedgerTransactionId, PaymentId, SpendAuthTokenId, UserId,
};
use hubu_common::money::Currency;
use rusqlite::{params, Connection};

use crate::payment::{
    PaymentDestination, PaymentRailKind, PaymentRequest, PaymentResponse, PaymentStatus,
};

#[derive(Debug, thiserror::Error)]
pub enum PaymentAttemptStorageError {
    #[error("sqlite error")]
    Sqlite {
        #[from]
        source: rusqlite::Error,
    },
    #[error("payment attempt execution scope is invalid")]
    Json(#[from] serde_json::Error),
}

pub trait PaymentAttemptRepository {
    fn save_payment_attempt(
        &mut self,
        request: &PaymentRequest,
        response: &PaymentResponse,
    ) -> Result<(), PaymentAttemptStorageError>;

    fn list_payment_attempts(
        &self,
    ) -> Result<Vec<PaymentAttemptRecord>, PaymentAttemptStorageError>;
}

#[derive(Debug, Clone)]
pub struct PaymentAttemptRecord {
    pub payment_id: PaymentId,
    pub idempotency_key: String,
    pub spend_auth_token_id: SpendAuthTokenId,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub execution_scope: Option<ExecutionScope>,
    pub task_id: Option<String>,
    pub rail: PaymentRailKind,
    pub destination: PaymentDestination,
    pub memo: Option<String>,
    pub status: PaymentStatus,
    pub ledger_transaction_id: Option<LedgerTransactionId>,
    pub rail_reference: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl PaymentAttemptRecord {
    pub fn request(&self) -> PaymentRequest {
        PaymentRequest {
            idempotency_key: self.idempotency_key.clone(),
            spend_auth_token_id: self.spend_auth_token_id.clone(),
            owner_user_id: self.owner_user_id.clone(),
            agent_id: self.agent_id.clone(),
            agent_account_id: self.agent_account_id.clone(),
            amount_cents: self.amount_cents,
            currency: self.currency,
            merchant: self.merchant.clone(),
            execution_scope: self.execution_scope.clone(),
            task_id: self.task_id.clone(),
            rail: self.rail,
            destination: self.destination.clone(),
            memo: self.memo.clone(),
        }
    }

    pub fn response(&self) -> PaymentResponse {
        PaymentResponse {
            payment_id: self.payment_id.clone(),
            owner_user_id: self.owner_user_id.clone(),
            agent_account_id: self.agent_account_id.clone(),
            status: self.status,
            amount_cents: self.amount_cents,
            currency: self.currency,
            ledger_transaction_id: self.ledger_transaction_id.clone(),
            rail_reference: self.rail_reference.clone(),
            failure_reason: self.failure_reason.clone(),
            created_at: self.created_at,
        }
    }
}

pub struct SqlitePaymentAttemptRepository {
    conn: Connection,
}

impl SqlitePaymentAttemptRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PaymentAttemptStorageError> {
        let repository = Self {
            conn: Connection::open(path)?,
        };
        repository.init()?;
        Ok(repository)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, PaymentAttemptStorageError> {
        let repository = Self {
            conn: Connection::open_in_memory()?,
        };
        repository.init()?;
        Ok(repository)
    }

    fn init(&self) -> Result<(), PaymentAttemptStorageError> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS payment_attempts (
                payment_id TEXT PRIMARY KEY,
                idempotency_key TEXT NOT NULL,
                spend_auth_token_id TEXT NOT NULL,
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                agent_account_id TEXT NOT NULL,
                amount_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                merchant TEXT,
                execution_scope_json TEXT,
                task_id TEXT,
                rail TEXT NOT NULL,
                destination_type TEXT NOT NULL,
                destination_ref TEXT NOT NULL,
                memo TEXT,
                status TEXT NOT NULL,
                ledger_transaction_id TEXT,
                rail_reference TEXT,
                failure_reason TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS payment_attempts_idempotency_key_idx
            ON payment_attempts(idempotency_key);
            ",
        )?;
        let has_scope_column = self
            .conn
            .prepare("PRAGMA table_info(payment_attempts)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|column| column == "execution_scope_json");
        if !has_scope_column {
            self.conn.execute(
                "ALTER TABLE payment_attempts ADD COLUMN execution_scope_json TEXT",
                [],
            )?;
        }
        Ok(())
    }
}

impl PaymentAttemptRepository for SqlitePaymentAttemptRepository {
    fn save_payment_attempt(
        &mut self,
        request: &PaymentRequest,
        response: &PaymentResponse,
    ) -> Result<(), PaymentAttemptStorageError> {
        let (destination_type, destination_ref) = destination_parts(&request.destination);
        self.conn.execute(
            "INSERT OR IGNORE INTO payment_attempts
             (payment_id, idempotency_key, spend_auth_token_id, owner_user_id, agent_id, agent_account_id,
              amount_cents, currency, merchant, execution_scope_json, task_id, rail, destination_type, destination_ref,
              memo, status, ledger_transaction_id, rail_reference, failure_reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                response.payment_id.to_string(),
                request.idempotency_key,
                request.spend_auth_token_id.to_string(),
                request.owner_user_id.to_string(),
                request.agent_id.to_string(),
                request.agent_account_id.to_string(),
                request.amount_cents,
                request.currency.to_string(),
                request.merchant,
                request.execution_scope.as_ref().map(serde_json::to_string).transpose()?,
                request.task_id,
                request.rail.as_ref(),
                destination_type,
                destination_ref,
                request.memo,
                payment_status(response.status),
                response.ledger_transaction_id.as_ref().map(ToString::to_string),
                response.rail_reference,
                response.failure_reason,
                response.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn list_payment_attempts(
        &self,
    ) -> Result<Vec<PaymentAttemptRecord>, PaymentAttemptStorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT payment_id, idempotency_key, spend_auth_token_id, owner_user_id, agent_id,
                    agent_account_id, amount_cents, currency, merchant, execution_scope_json, task_id, rail, destination_type, destination_ref,
                    memo, status, ledger_transaction_id, rail_reference, failure_reason, created_at
             FROM payment_attempts
             ORDER BY created_at ASC, payment_id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let payment_id: String = row.get(0)?;
            let spend_auth_token_id: String = row.get(2)?;
            let owner_user_id: String = row.get(3)?;
            let agent_id: String = row.get(4)?;
            let agent_account_id: String = row.get(5)?;
            let currency: String = row.get(7)?;
            let execution_scope_json: Option<String> = row.get(9)?;
            let rail: String = row.get(11)?;
            let destination_type: String = row.get(12)?;
            let destination_ref: String = row.get(13)?;
            let status: String = row.get(15)?;
            let ledger_transaction_id: Option<String> = row.get(16)?;
            let created_at: String = row.get(19)?;
            Ok(PaymentAttemptRecord {
                payment_id: parse_id(&payment_id)?,
                idempotency_key: row.get(1)?,
                spend_auth_token_id: parse_id(&spend_auth_token_id)?,
                owner_user_id: parse_id(&owner_user_id)?,
                agent_id: parse_id(&agent_id)?,
                agent_account_id: parse_id(&agent_account_id)?,
                amount_cents: row.get(6)?,
                currency: parse_currency(&currency)?,
                merchant: row.get(8)?,
                execution_scope: execution_scope_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                task_id: row.get(10)?,
                rail: parse_rail(&rail)?,
                destination: parse_destination(&destination_type, &destination_ref)?,
                memo: row.get(14)?,
                status: parse_payment_status(&status)?,
                ledger_transaction_id: ledger_transaction_id
                    .as_deref()
                    .map(parse_id)
                    .transpose()?,
                rail_reference: row.get(17)?,
                failure_reason: row.get(18)?,
                created_at: parse_timestamp(&created_at)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn destination_parts(destination: &PaymentDestination) -> (&'static str, String) {
    match destination {
        PaymentDestination::FiatAccount { account_ref } => ("fiat_account", account_ref.clone()),
        PaymentDestination::StablecoinWallet { chain, address } => {
            ("stablecoin_wallet", format!("{chain}:{address}"))
        }
    }
}

fn parse_destination(
    destination_type: &str,
    destination_ref: &str,
) -> Result<PaymentDestination, rusqlite::Error> {
    match destination_type {
        "fiat_account" => Ok(PaymentDestination::FiatAccount {
            account_ref: destination_ref.to_string(),
        }),
        "stablecoin_wallet" => {
            let (chain, address) = destination_ref
                .split_once(':')
                .ok_or(rusqlite::Error::InvalidQuery)?;
            Ok(PaymentDestination::StablecoinWallet {
                chain: chain.to_string(),
                address: address.to_string(),
            })
        }
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn payment_status(status: PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::Succeeded => "succeeded",
        PaymentStatus::Failed => "failed",
    }
}

fn parse_payment_status(value: &str) -> Result<PaymentStatus, rusqlite::Error> {
    match value {
        "succeeded" => Ok(PaymentStatus::Succeeded),
        "failed" => Ok(PaymentStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_rail(value: &str) -> Result<PaymentRailKind, rusqlite::Error> {
    match value {
        "fiat_mock" => Ok(PaymentRailKind::FiatMock),
        "stablecoin_mock" => Ok(PaymentRailKind::StablecoinMock),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_currency(value: &str) -> Result<Currency, rusqlite::Error> {
    Currency::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_id<T>(value: &str) -> Result<T, rusqlite::Error>
where
    T: FromStr,
{
    T::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubu_common::execution_scope::{
        ExecutionScope, ScopeIdentity, EXECUTION_SCOPE_SCHEMA_VERSION,
    };

    fn identity(id: &str, name: &str) -> ScopeIdentity {
        ScopeIdentity {
            id: id.into(),
            display_name: name.into(),
        }
    }

    fn scope() -> ExecutionScope {
        ExecutionScope {
            schema_version: EXECUTION_SCOPE_SCHEMA_VERSION,
            provider: identity(
                "provider:google:gemini-developer",
                "Google Gemini Developer API",
            ),
            executor: identity("executor:gongbu:image", "Gongbu image executor"),
            capability: identity("capability:image:generate", "Generate image"),
            billing_merchant: identity("merchant:google", "Google"),
        }
    }

    #[test]
    fn execution_scope_round_trips_across_restart() {
        let path =
            std::env::temp_dir().join(format!("hubu-wallet-scope-{}.sqlite", PaymentId::new()));
        let request = PaymentRequest {
            idempotency_key: "scope-restart".into(),
            spend_auth_token_id: SpendAuthTokenId::new(),
            owner_user_id: UserId::new(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            amount_cents: 25,
            currency: Currency::Usd,
            merchant: None,
            execution_scope: Some(scope()),
            task_id: Some("task".into()),
            rail: PaymentRailKind::FiatMock,
            destination: PaymentDestination::FiatAccount {
                account_ref: "merchant".into(),
            },
            memo: None,
        };
        let response = PaymentResponse {
            payment_id: PaymentId::new(),
            owner_user_id: request.owner_user_id.clone(),
            agent_account_id: request.agent_account_id.clone(),
            status: PaymentStatus::Succeeded,
            amount_cents: 25,
            currency: Currency::Usd,
            ledger_transaction_id: None,
            rail_reference: Some("rail".into()),
            failure_reason: None,
            created_at: Utc::now(),
        };
        SqlitePaymentAttemptRepository::open(&path)
            .unwrap()
            .save_payment_attempt(&request, &response)
            .unwrap();
        let restarted = SqlitePaymentAttemptRepository::open(&path).unwrap();
        let restored = restarted.list_payment_attempts().unwrap();
        assert_eq!(
            restored[0].request().execution_scope,
            request.execution_scope
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_payment_schema_adds_nullable_scope_column() {
        let path = std::env::temp_dir().join(format!(
            "hubu-wallet-legacy-scope-{}.sqlite",
            PaymentId::new()
        ));
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE payment_attempts (payment_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL, spend_auth_token_id TEXT NOT NULL, owner_user_id TEXT NOT NULL, agent_id TEXT NOT NULL, agent_account_id TEXT NOT NULL, amount_cents INTEGER NOT NULL, currency TEXT NOT NULL, merchant TEXT, task_id TEXT, rail TEXT NOT NULL, destination_type TEXT NOT NULL, destination_ref TEXT NOT NULL, memo TEXT, status TEXT NOT NULL, ledger_transaction_id TEXT, rail_reference TEXT, failure_reason TEXT, created_at TEXT NOT NULL);").unwrap();
        let repository = SqlitePaymentAttemptRepository::open(&path).unwrap();
        let columns = repository
            .conn
            .prepare("PRAGMA table_info(payment_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns
            .iter()
            .any(|column| column == "execution_scope_json"));
        let _ = std::fs::remove_file(path);
    }
}
