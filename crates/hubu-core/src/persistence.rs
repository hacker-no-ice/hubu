use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, UserId};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;
use rusqlite::{params, Connection};

use crate::budget::{
    Budget, BudgetBalance, BudgetHold, BudgetHoldStatus, BudgetScope, BudgetStatus,
};
use crate::policy::Policy;
use crate::spend::{SpendAuthTokenRecord, SpendDecisionRecord};
use crate::storage::StorageError;

pub trait PolicyRepository {
    fn save_policy_assignment(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        policy: &Policy,
    ) -> Result<(), StorageError>;

    fn load_policy_assignments(&self) -> Result<Vec<PolicyAssignmentRecord>, StorageError>;
}

pub trait SpendRepository {
    fn save_spend_decision(&mut self, record: &SpendDecisionRecord) -> Result<(), StorageError>;
    fn save_spend_auth_token(&mut self, record: &SpendAuthTokenRecord) -> Result<(), StorageError>;
    fn update_spend_auth_token(
        &mut self,
        record: &SpendAuthTokenRecord,
    ) -> Result<(), StorageError>;
    fn load_spend_decisions(&self) -> Result<Vec<SpendDecisionRecord>, StorageError>;
    fn load_spend_auth_tokens(&self) -> Result<Vec<SpendAuthTokenRecord>, StorageError>;
}

pub trait BudgetRepository {
    fn expire_overdue_budget_holds(&mut self, now: DateTime<Utc>) -> Result<(), StorageError>;
    fn save_budget_with_balance(
        &mut self,
        budget: &Budget,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;
    fn save_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;
    fn update_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;
    fn load_budgets(&self) -> Result<Vec<Budget>, StorageError>;
    fn load_budget_balances(&self) -> Result<Vec<BudgetBalance>, StorageError>;
    fn load_budget_holds(&self) -> Result<Vec<BudgetHold>, StorageError>;
}

#[derive(Debug, Clone)]
pub struct PolicyAssignmentRecord {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub policy: Policy,
}

pub struct SqliteGovernanceRepository {
    conn: Connection,
}

impl SqliteGovernanceRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        let repository = Self { conn };
        repository.init()?;
        Ok(repository)
    }

    fn init(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS policy_assignments (
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                policy_id TEXT NOT NULL,
                policy_version TEXT NOT NULL,
                policy_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(owner_user_id, agent_id)
            );

            CREATE TABLE IF NOT EXISTS spend_decisions (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                request_json TEXT NOT NULL,
                evaluation_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS spend_auth_tokens (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                spend_decision_id TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                used_at TEXT,
                used_by_payment_id TEXT,
                revoked_at TEXT,
                FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
            );

            CREATE TABLE IF NOT EXISTS budgets (
                id TEXT PRIMARY KEY,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                amount_limit_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                starting_at TEXT NOT NULL,
                ending_before TEXT,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS budget_balances (
                budget_id TEXT PRIMARY KEY,
                consumed_amount_cents INTEGER NOT NULL,
                frozen_amount_cents INTEGER NOT NULL,
                remaining_amount_cents INTEGER NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(budget_id) REFERENCES budgets(id)
            );

            CREATE TABLE IF NOT EXISTS budget_holds (
                id TEXT PRIMARY KEY,
                budget_id TEXT NOT NULL,
                spend_decision_id TEXT NOT NULL UNIQUE,
                amount_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                FOREIGN KEY(budget_id) REFERENCES budgets(id),
                FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
            );

            CREATE TRIGGER IF NOT EXISTS spend_decisions_no_update
            BEFORE UPDATE ON spend_decisions
            BEGIN
                SELECT RAISE(ABORT, 'spend decisions are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS spend_decisions_no_delete
            BEFORE DELETE ON spend_decisions
            BEGIN
                SELECT RAISE(ABORT, 'spend decisions are immutable');
            END;
            ",
        )?;
        Ok(())
    }
}

impl PolicyRepository for SqliteGovernanceRepository {
    fn save_policy_assignment(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        policy: &Policy,
    ) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let policy_json = serde_json::to_string(policy)?;
        self.conn.execute(
            "INSERT INTO policy_assignments
             (owner_user_id, agent_id, policy_id, policy_version, policy_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(owner_user_id, agent_id) DO UPDATE SET
                policy_id = excluded.policy_id,
                policy_version = excluded.policy_version,
                policy_json = excluded.policy_json,
                updated_at = excluded.updated_at",
            params![
                owner_user_id.to_string(),
                agent_id.to_string(),
                policy.id,
                policy.version,
                policy_json,
                now,
            ],
        )?;
        Ok(())
    }

    fn load_policy_assignments(&self) -> Result<Vec<PolicyAssignmentRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT owner_user_id, agent_id, policy_json FROM policy_assignments
             ORDER BY updated_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let owner_user_id: String = row.get(0)?;
            let agent_id: String = row.get(1)?;
            let policy_json: String = row.get(2)?;
            let policy: Policy = serde_json::from_str(&policy_json)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            Ok(PolicyAssignmentRecord {
                owner_user_id: parse_id(&owner_user_id)?,
                agent_id: parse_id(&agent_id)?,
                policy,
            })
        })?;
        collect_rows(rows)
    }
}

impl SpendRepository for SqliteGovernanceRepository {
    fn save_spend_decision(&mut self, record: &SpendDecisionRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spend_decisions
             (id, owner_user_id, request_json, evaluation_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id.to_string(),
                record.owner_user_id.to_string(),
                serde_json::to_string(&record.request)?,
                serde_json::to_string(&record.evaluation)?,
                record.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn save_spend_auth_token(&mut self, record: &SpendAuthTokenRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spend_auth_tokens
             (id, owner_user_id, spend_decision_id, expires_at, used_at, used_by_payment_id, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id.to_string(),
                record.owner_user_id.to_string(),
                record.spend_decision_id.to_string(),
                record.expires_at.to_rfc3339(),
                record.used_at.map(|timestamp| timestamp.to_rfc3339()),
                record.used_by_payment_id.as_ref().map(ToString::to_string),
                record.revoked_at.map(|timestamp| timestamp.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    fn update_spend_auth_token(
        &mut self,
        record: &SpendAuthTokenRecord,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE spend_auth_tokens
             SET used_at = ?2, used_by_payment_id = ?3, revoked_at = ?4
             WHERE id = ?1",
            params![
                record.id.to_string(),
                record.used_at.map(|timestamp| timestamp.to_rfc3339()),
                record.used_by_payment_id.as_ref().map(ToString::to_string),
                record.revoked_at.map(|timestamp| timestamp.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    fn load_spend_decisions(&self) -> Result<Vec<SpendDecisionRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner_user_id, request_json, evaluation_json, created_at
             FROM spend_decisions
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let owner_user_id: String = row.get(1)?;
            let request_json: String = row.get(2)?;
            let evaluation_json: String = row.get(3)?;
            let created_at: String = row.get(4)?;
            Ok(SpendDecisionRecord {
                id: parse_id(&id)?,
                owner_user_id: parse_id(&owner_user_id)?,
                request: parse_json(&request_json)?,
                evaluation: parse_json(&evaluation_json)?,
                created_at: parse_timestamp(&created_at)?,
            })
        })?;
        collect_rows(rows)
    }

    fn load_spend_auth_tokens(&self) -> Result<Vec<SpendAuthTokenRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner_user_id, spend_decision_id, expires_at, used_at, used_by_payment_id, revoked_at
             FROM spend_auth_tokens
             ORDER BY expires_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let owner_user_id: String = row.get(1)?;
            let spend_decision_id: String = row.get(2)?;
            let expires_at: String = row.get(3)?;
            let used_at: Option<String> = row.get(4)?;
            let used_by_payment_id: Option<String> = row.get(5)?;
            let revoked_at: Option<String> = row.get(6)?;
            Ok(SpendAuthTokenRecord {
                id: parse_id(&id)?,
                owner_user_id: parse_id(&owner_user_id)?,
                spend_decision_id: parse_id(&spend_decision_id)?,
                expires_at: parse_timestamp(&expires_at)?,
                used_at: parse_optional_timestamp(used_at)?,
                used_by_payment_id: parse_optional_id(used_by_payment_id)?,
                revoked_at: parse_optional_timestamp(revoked_at)?,
            })
        })?;
        collect_rows(rows)
    }
}

impl BudgetRepository for SqliteGovernanceRepository {
    fn expire_overdue_budget_holds(&mut self, now: DateTime<Utc>) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        let expired_holds = {
            let mut stmt = sqlite_tx.prepare(
                "SELECT id, budget_id, amount_cents
                 FROM budget_holds
                 WHERE status = 'frozen' AND expires_at <= ?1
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![now.to_rfc3339()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (hold_id, budget_id, amount_cents) in expired_holds {
            sqlite_tx.execute(
                "UPDATE budget_holds
                 SET status = 'expired', updated_at = ?2
                 WHERE id = ?1 AND status = 'frozen'",
                params![hold_id, now.to_rfc3339()],
            )?;
            sqlite_tx.execute(
                "UPDATE budget_balances
                 SET frozen_amount_cents = frozen_amount_cents - ?2,
                     remaining_amount_cents = remaining_amount_cents + ?2,
                     updated_at = ?3
                 WHERE budget_id = ?1",
                params![budget_id, amount_cents, now.to_rfc3339()],
            )?;
        }

        sqlite_tx.commit()?;
        Ok(())
    }

    fn save_budget_with_balance(
        &mut self,
        budget: &Budget,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "INSERT INTO budgets
             (id, scope_type, scope_id, amount_limit_cents, currency, starting_at, ending_before, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                budget.id.to_string(),
                budget_scope_type(&budget.scope),
                budget_scope_id(&budget.scope),
                budget.amount_limit_cents,
                budget.currency.to_string(),
                budget.period.starting_at.to_rfc3339(),
                budget.period.ending_before.map(|timestamp| timestamp.to_rfc3339()),
                budget_status(&budget.status),
                budget.created_at.to_rfc3339(),
                budget.updated_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    fn save_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "INSERT INTO budget_holds
             (id, budget_id, spend_decision_id, amount_cents, currency, status, created_at, updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                hold.id.to_string(),
                hold.budget_id.to_string(),
                hold.spend_decision_id.to_string(),
                hold.amount_cents,
                hold.currency.to_string(),
                budget_hold_status(&hold.status),
                hold.created_at.to_rfc3339(),
                hold.updated_at.to_rfc3339(),
                hold.expires_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    fn update_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "UPDATE budget_holds SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![
                hold.id.to_string(),
                budget_hold_status(&hold.status),
                hold.updated_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    fn load_budgets(&self) -> Result<Vec<Budget>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scope_type, scope_id, amount_limit_cents, currency, starting_at,
                    ending_before, status, created_at, updated_at
             FROM budgets
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let scope_type: String = row.get(1)?;
            let scope_id: String = row.get(2)?;
            let currency: String = row.get(4)?;
            let starting_at: String = row.get(5)?;
            let ending_before: Option<String> = row.get(6)?;
            let status: String = row.get(7)?;
            let created_at: String = row.get(8)?;
            let updated_at: String = row.get(9)?;
            Ok(Budget {
                id: parse_id(&id)?,
                scope: parse_budget_scope(&scope_type, &scope_id)?,
                amount_limit_cents: row.get(3)?,
                currency: parse_currency(&currency)?,
                period: TimePeriod::new(
                    parse_timestamp(&starting_at)?,
                    parse_optional_timestamp(ending_before)?,
                )
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                status: parse_budget_status(&status)?,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })?;
        collect_rows(rows)
    }

    fn load_budget_balances(&self) -> Result<Vec<BudgetBalance>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT budget_id, consumed_amount_cents, frozen_amount_cents, remaining_amount_cents
             FROM budget_balances
             ORDER BY budget_id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let budget_id: String = row.get(0)?;
            Ok(BudgetBalance {
                budget_id: parse_id(&budget_id)?,
                consumed_amount_cents: row.get(1)?,
                frozen_amount_cents: row.get(2)?,
                remaining_amount_cents: row.get(3)?,
            })
        })?;
        collect_rows(rows)
    }

    fn load_budget_holds(&self) -> Result<Vec<BudgetHold>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, budget_id, spend_decision_id, amount_cents, currency, status,
                    created_at, updated_at, expires_at
             FROM budget_holds
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let budget_id: String = row.get(1)?;
            let spend_decision_id: String = row.get(2)?;
            let currency: String = row.get(4)?;
            let status: String = row.get(5)?;
            let created_at: String = row.get(6)?;
            let updated_at: String = row.get(7)?;
            let expires_at: String = row.get(8)?;
            Ok(BudgetHold {
                id: parse_id(&id)?,
                budget_id: parse_id(&budget_id)?,
                spend_decision_id: parse_id(&spend_decision_id)?,
                amount_cents: row.get(3)?,
                currency: parse_currency(&currency)?,
                status: parse_budget_hold_status(&status)?,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
                expires_at: parse_timestamp(&expires_at)?,
            })
        })?;
        collect_rows(rows)
    }
}

fn upsert_balance(conn: &Connection, balance: &BudgetBalance) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO budget_balances
         (budget_id, consumed_amount_cents, frozen_amount_cents, remaining_amount_cents, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(budget_id) DO UPDATE SET
            consumed_amount_cents = excluded.consumed_amount_cents,
            frozen_amount_cents = excluded.frozen_amount_cents,
            remaining_amount_cents = excluded.remaining_amount_cents,
            updated_at = excluded.updated_at",
        params![
            balance.budget_id.to_string(),
            balance.consumed_amount_cents,
            balance.frozen_amount_cents,
            balance.remaining_amount_cents,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn budget_scope_type(scope: &BudgetScope) -> &'static str {
    match scope {
        BudgetScope::User(_) => "user",
        BudgetScope::Agent(_) => "agent",
        BudgetScope::Task(_) => "task",
    }
}

fn budget_scope_id(scope: &BudgetScope) -> String {
    match scope {
        BudgetScope::User(id) => id.to_string(),
        BudgetScope::Agent(id) => id.to_string(),
        BudgetScope::Task(id) => id.to_string(),
    }
}

fn budget_status(status: &BudgetStatus) -> &'static str {
    match status {
        BudgetStatus::Active => "active",
        BudgetStatus::Exhausted => "exhausted",
        BudgetStatus::Expired => "expired",
        BudgetStatus::Revoked => "revoked",
    }
}

fn budget_hold_status(status: &BudgetHoldStatus) -> &'static str {
    match status {
        BudgetHoldStatus::Frozen => "frozen",
        BudgetHoldStatus::Settled => "settled",
        BudgetHoldStatus::Released => "released",
        BudgetHoldStatus::Expired => "expired",
    }
}

fn parse_budget_scope(scope_type: &str, scope_id: &str) -> Result<BudgetScope, rusqlite::Error> {
    match scope_type {
        "user" => Ok(BudgetScope::User(parse_id(scope_id)?)),
        "agent" => Ok(BudgetScope::Agent(parse_id(scope_id)?)),
        "task" => Ok(BudgetScope::Task(parse_id(scope_id)?)),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_budget_status(value: &str) -> Result<BudgetStatus, rusqlite::Error> {
    match value {
        "active" => Ok(BudgetStatus::Active),
        "exhausted" => Ok(BudgetStatus::Exhausted),
        "expired" => Ok(BudgetStatus::Expired),
        "revoked" => Ok(BudgetStatus::Revoked),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_budget_hold_status(value: &str) -> Result<BudgetHoldStatus, rusqlite::Error> {
    match value {
        "frozen" => Ok(BudgetHoldStatus::Frozen),
        "settled" => Ok(BudgetHoldStatus::Settled),
        "released" => Ok(BudgetHoldStatus::Released),
        "expired" => Ok(BudgetHoldStatus::Expired),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_optional_timestamp(
    value: Option<String>,
) -> Result<Option<DateTime<Utc>>, rusqlite::Error> {
    value.map(|value| parse_timestamp(&value)).transpose()
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

fn parse_optional_id<T>(value: Option<String>) -> Result<Option<T>, rusqlite::Error>
where
    T: FromStr,
{
    value.as_deref().map(parse_id).transpose()
}

fn parse_json<T>(value: &str) -> Result<T, rusqlite::Error>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>>,
) -> Result<Vec<T>, StorageError> {
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use hubu_common::ids::{BudgetHoldId, BudgetId, SpendAuthTokenId, SpendDecisionId};

    use super::*;
    use crate::policy::{
        condition::{Condition, Field, PolicyValue},
        Effect, Evaluation, Rule, RuleResult,
    };
    use crate::spend::SpendRequest;

    fn user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    fn agent_id() -> AgentId {
        "00000000-0000-4000-8000-000000000456".parse().unwrap()
    }

    fn policy() -> Policy {
        Policy {
            id: "demo_policy".to_string(),
            version: "v1".to_string(),
            owner_user_id: user_id(),
            default_effect: Effect::NeedsApproval,
            rules: vec![Rule {
                id: "allow_small".to_string(),
                effect: Effect::Allow,
                reason: "small spend".to_string(),
                when: Condition::Lte {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(5_000),
                },
            }],
        }
    }

    fn spend_request() -> SpendRequest {
        SpendRequest {
            amount_cents: 2_500,
            currency: Currency::Usd,
            owner_user_id: user_id(),
            agent_id: agent_id(),
            merchant: Some("Acme".to_string()),
            category: None,
            task_id: Some("task".to_string()),
        }
    }

    fn spend_decision() -> SpendDecisionRecord {
        SpendDecisionRecord {
            id: SpendDecisionId::new(),
            owner_user_id: user_id(),
            request: spend_request(),
            evaluation: Evaluation {
                policy_id: "demo_policy".to_string(),
                policy_version: "v1".to_string(),
                decision: Effect::Allow,
                reasons: vec!["small spend".to_string()],
                rule_results: vec![RuleResult {
                    rule_id: "allow_small".to_string(),
                    matched: true,
                    effect: Some(Effect::Allow),
                    reason: Some("small spend".to_string()),
                }],
            },
            created_at: Utc::now(),
        }
    }

    #[test]
    fn persists_policy_assignment_and_spend_records() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        repo.save_policy_assignment(&user_id(), &agent_id(), &policy())
            .unwrap();

        let decision = spend_decision();
        let token = SpendAuthTokenRecord {
            id: SpendAuthTokenId::new(),
            owner_user_id: user_id(),
            spend_decision_id: decision.id.clone(),
            expires_at: Utc::now() + Duration::minutes(5),
            used_at: None,
            used_by_payment_id: None,
            revoked_at: None,
        };

        repo.save_spend_decision(&decision).unwrap();
        repo.save_spend_auth_token(&token).unwrap();

        assert_eq!(repo.load_policy_assignments().unwrap().len(), 1);
        assert_eq!(repo.load_spend_decisions().unwrap().len(), 1);
        assert_eq!(repo.load_spend_auth_tokens().unwrap().len(), 1);
    }

    #[test]
    fn expire_overdue_budget_holds_returns_frozen_amount_to_remaining() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let decision = spend_decision();
        repo.save_spend_decision(&decision).unwrap();

        let budget = Budget::new(
            BudgetId::new(),
            BudgetScope::User(user_id()),
            10_000,
            Currency::Usd,
            TimePeriod::new(
                Utc::now() - Duration::hours(1),
                Some(Utc::now() + Duration::hours(1)),
            )
            .unwrap(),
        )
        .unwrap();
        let reserved_balance = BudgetBalance {
            budget_id: budget.id.clone(),
            consumed_amount_cents: 0,
            frozen_amount_cents: 2_500,
            remaining_amount_cents: 7_500,
        };
        let hold = BudgetHold {
            id: BudgetHoldId::new(),
            budget_id: budget.id.clone(),
            spend_decision_id: decision.id,
            amount_cents: 2_500,
            currency: Currency::Usd,
            status: BudgetHoldStatus::Frozen,
            created_at: Utc::now() - Duration::minutes(10),
            updated_at: Utc::now() - Duration::minutes(10),
            expires_at: Utc::now() - Duration::minutes(5),
        };

        repo.save_budget_with_balance(&budget, &reserved_balance)
            .unwrap();
        repo.save_budget_hold(&hold, &reserved_balance).unwrap();
        repo.expire_overdue_budget_holds(Utc::now()).unwrap();

        let reloaded_hold = repo.load_budget_holds().unwrap().pop().unwrap();
        let reloaded_balance = repo.load_budget_balances().unwrap().pop().unwrap();
        assert!(matches!(reloaded_hold.status, BudgetHoldStatus::Expired));
        assert_eq!(reloaded_balance.frozen_amount_cents, 0);
        assert_eq!(reloaded_balance.remaining_amount_cents, 10_000);
    }
}
