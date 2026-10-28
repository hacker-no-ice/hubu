//! Durable task-scoped budgets.
//!
//! A task budget row records the allocation; its spend is derived from the
//! parent budget's holds attributed to it plus the ledger's recorded budget
//! charges for the settled ones, so no settlement path maintains a second
//! balance.
use std::collections::HashMap;

use super::*;
use crate::budget::{
    public_task_budget_id, task_budget_open_fingerprint, validate_task_id, CloseTaskBudgetRequest,
    CloseTaskBudgetResponse, OpenTaskBudgetRequest, OpenTaskBudgetResponse, TaskBudget,
    TaskBudgetApprovalKind, TaskBudgetError, TaskBudgetSnapshot, TaskBudgetState, TaskBudgetUsage,
};
use hubu_common::ids::TaskBudgetId;
use hubu_ledger::TransactionMetadata;

use super::task_budget_approval::{
    approve_pending_close_request, pending_open_proposal_for, pending_task_budget_approval_for_task,
};

/// Reads the task budgets a spend reservation must respect. Writes go through
/// the inherent open/close commands, which run in their own transactions.
pub trait TaskBudgetRepository {
    /// The open task budget that spend for `task_id` must draw from, if any.
    fn open_task_budget_for_spend(
        &self,
        agent_id: &AgentId,
        task_id: &str,
        currency: Currency,
    ) -> Result<Option<TaskBudgetSnapshot>, StorageError>;

    /// Every open task budget allocated from `parent_budget_id`.
    fn open_task_budgets_for_parent(
        &self,
        parent_budget_id: &BudgetId,
    ) -> Result<Vec<TaskBudgetSnapshot>, StorageError>;

    /// The request for `task_id` still waiting on a human at `now`, if any:
    /// a proposal to open it, or a request to close its open task budget.
    /// Spend for the task is refused while either is pending.
    fn pending_task_budget_approval_for_spend(
        &self,
        agent_id: &AgentId,
        task_id: &str,
        currency: Currency,
        now: DateTime<Utc>,
    ) -> Result<Option<TaskBudgetApprovalKind>, StorageError>;
}

const TASK_BUDGET_COLUMNS: &str = "id, owner_user_id, agent_id, parent_budget_id, task_id,
     currency, amount_limit, state, opened_at, opened_by, source, reason,
     request_fingerprint, closed_at, closed_by";

impl SqliteGovernanceRepository {
    pub(super) fn init_task_budget_schema(&mut self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_budgets (
                id TEXT PRIMARY KEY,
                public_id TEXT NOT NULL UNIQUE,
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                parent_budget_id TEXT NOT NULL REFERENCES budgets(id),
                task_id TEXT NOT NULL CHECK(length(task_id) BETWEEN 1 AND 512),
                currency TEXT NOT NULL,
                amount_limit INTEGER NOT NULL CHECK(amount_limit > 0),
                amount_scale INTEGER NOT NULL CHECK(amount_scale = 6),
                state TEXT NOT NULL CHECK(state IN ('open', 'closed')),
                opened_at TEXT NOT NULL,
                opened_by TEXT NOT NULL CHECK(length(opened_by) > 0),
                source TEXT NOT NULL CHECK(length(source) > 0),
                reason TEXT,
                request_fingerprint TEXT NOT NULL CHECK(length(request_fingerprint) > 0),
                closed_at TEXT,
                closed_by TEXT,
                CHECK((state = 'open') = (closed_at IS NULL)),
                CHECK((closed_at IS NULL) = (closed_by IS NULL))
             );
             CREATE UNIQUE INDEX IF NOT EXISTS task_budgets_one_open_per_task
                ON task_budgets(agent_id, task_id, currency) WHERE state = 'open';
             CREATE INDEX IF NOT EXISTS task_budgets_open_by_parent
                ON task_budgets(parent_budget_id) WHERE state = 'open';
             CREATE TRIGGER IF NOT EXISTS task_budgets_close_only
                BEFORE UPDATE ON task_budgets
                WHEN OLD.state <> 'open' OR NEW.state <> 'closed'
                  OR NEW.id IS NOT OLD.id OR NEW.public_id IS NOT OLD.public_id
                  OR NEW.owner_user_id IS NOT OLD.owner_user_id
                  OR NEW.agent_id IS NOT OLD.agent_id
                  OR NEW.parent_budget_id IS NOT OLD.parent_budget_id
                  OR NEW.task_id IS NOT OLD.task_id OR NEW.currency IS NOT OLD.currency
                  OR NEW.amount_limit IS NOT OLD.amount_limit
                  OR NEW.amount_scale IS NOT OLD.amount_scale
                  OR NEW.opened_at IS NOT OLD.opened_at
                  OR NEW.request_fingerprint IS NOT OLD.request_fingerprint
                BEGIN SELECT RAISE(ABORT, 'task budgets only move from open to closed'); END;
             CREATE TRIGGER IF NOT EXISTS task_budgets_no_delete
                BEFORE DELETE ON task_budgets
                BEGIN SELECT RAISE(ABORT, 'task budgets are retained'); END;",
        )?;
        if !table_has_column(&self.conn, "budget_holds", "task_budget_id")? {
            self.conn.execute_batch(
                "ALTER TABLE budget_holds ADD COLUMN task_budget_id TEXT REFERENCES task_budgets(id);",
            )?;
        }
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS budget_holds_by_task_budget
                ON budget_holds(task_budget_id) WHERE task_budget_id IS NOT NULL;",
        )?;
        self.init_task_budget_approval_schema()
    }

    /// Open a task budget drawn from `parent`, the agent's available budget
    /// in the requested currency, evaluated by the caller under the budget
    /// manager lock. An identical request for an already open task budget
    /// replays it.
    pub fn open_task_budget(
        &mut self,
        request: &OpenTaskBudgetRequest,
        parent: &BudgetWithBalance,
        now: DateTime<Utc>,
    ) -> Result<OpenTaskBudgetResponse, TaskBudgetError> {
        validate_task_id(&request.task_id)?;
        if request.amount_limit <= Amount::ZERO {
            return Err(TaskBudgetError::AmountMustBePositive);
        }
        if parent.budget.agent_id != request.agent_id || parent.budget.currency != request.currency
        {
            return Err(TaskBudgetError::NoAvailableBudget {
                currency: request.currency,
            });
        }
        let fingerprint = task_budget_open_fingerprint(
            &request.agent_id,
            &request.task_id,
            request.currency,
            request.amount_limit,
        );
        self.expire_task_budget_approvals(now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) =
            open_task_budget_for(&tx, &request.agent_id, &request.task_id, request.currency)?
        {
            if existing.budget.owner_user_id != request.owner_user_id
                || existing.budget.request_fingerprint != fingerprint
            {
                return Err(TaskBudgetError::OpenTaskBudgetConflict);
            }
            tx.commit()?;
            return Ok(OpenTaskBudgetResponse {
                task_budget: existing,
                replayed: true,
            });
        }
        // The human approves or denies the agent's proposal instead.
        if pending_open_proposal_for(&tx, &request.agent_id, &request.task_id, request.currency)?
            .is_some()
        {
            return Err(TaskBudgetError::ProposalPending);
        }
        let task_budget = insert_open_task_budget(
            &tx,
            request,
            &parent.budget.id,
            fingerprint,
            request.actor.clone(),
            now,
        )?;
        tx.commit()?;
        Ok(OpenTaskBudgetResponse {
            task_budget,
            replayed: false,
        })
    }

    /// Close a task budget, returning its unused allocation to the parent.
    /// Closing is rejected while any attributed authorization is outstanding.
    pub fn close_task_budget(
        &mut self,
        request: &CloseTaskBudgetRequest,
        now: DateTime<Utc>,
    ) -> Result<CloseTaskBudgetResponse, TaskBudgetError> {
        self.expire_task_budget_approvals(now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load_task_budget_where(
            &tx,
            "id = ?1 AND owner_user_id = ?2",
            params![
                request.task_budget_id.to_string(),
                request.owner_user_id.to_string()
            ],
        )?
        .ok_or(TaskBudgetError::UnknownTaskBudget)?;
        if existing.budget.state == TaskBudgetState::Closed {
            tx.commit()?;
            return Ok(CloseTaskBudgetResponse {
                task_budget: existing,
                replayed: true,
            });
        }
        let task_budget = close_open_task_budget(&tx, existing, &request.actor, now)?;
        // A human closing directly also settles the agent's pending request.
        approve_pending_close_request(&tx, &task_budget.budget.id, &request.actor, now)?;
        tx.commit()?;
        Ok(CloseTaskBudgetResponse {
            task_budget,
            replayed: false,
        })
    }

    /// One owned task budget by its public `tbg_` identifier.
    pub fn load_task_budget(
        &self,
        owner_user_id: &UserId,
        public_id: &str,
    ) -> Result<Option<TaskBudgetSnapshot>, StorageError> {
        load_task_budget_where(
            &self.conn,
            "public_id = ?1 AND owner_user_id = ?2",
            params![public_id, owner_user_id.to_string()],
        )
    }

    /// The task budget a hold was reserved under.
    pub fn load_task_budget_by_id(
        &self,
        owner_user_id: &UserId,
        id: &TaskBudgetId,
    ) -> Result<Option<TaskBudgetSnapshot>, StorageError> {
        load_task_budget_where(
            &self.conn,
            "id = ?1 AND owner_user_id = ?2",
            params![id.to_string(), owner_user_id.to_string()],
        )
    }

    /// Owned task budgets, newest first; closed ones only when requested.
    pub fn list_task_budgets(
        &self,
        owner_user_id: &UserId,
        agent_id: Option<&AgentId>,
        include_closed: bool,
    ) -> Result<Vec<TaskBudgetSnapshot>, StorageError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_BUDGET_COLUMNS} FROM task_budgets
             WHERE owner_user_id = ?1
               AND (?2 IS NULL OR agent_id = ?2)
               AND (?3 OR state = 'open')
             ORDER BY opened_at DESC, id"
        ))?;
        let rows = stmt.query_map(
            params![
                owner_user_id.to_string(),
                agent_id.map(ToString::to_string),
                include_closed
            ],
            task_budget_from_row,
        )?;
        let budgets = collect_rows(rows)?;
        with_usage(&self.conn, budgets)
    }
}

impl TaskBudgetRepository for SqliteGovernanceRepository {
    fn open_task_budget_for_spend(
        &self,
        agent_id: &AgentId,
        task_id: &str,
        currency: Currency,
    ) -> Result<Option<TaskBudgetSnapshot>, StorageError> {
        open_task_budget_for(&self.conn, agent_id, task_id, currency)
    }

    fn open_task_budgets_for_parent(
        &self,
        parent_budget_id: &BudgetId,
    ) -> Result<Vec<TaskBudgetSnapshot>, StorageError> {
        open_task_budgets_for_parent(&self.conn, parent_budget_id)
    }

    fn pending_task_budget_approval_for_spend(
        &self,
        agent_id: &AgentId,
        task_id: &str,
        currency: Currency,
        now: DateTime<Utc>,
    ) -> Result<Option<TaskBudgetApprovalKind>, StorageError> {
        pending_task_budget_approval_for_task(&self.conn, agent_id, task_id, currency, now)
    }
}

/// Check the parent's unallocated balance and insert an open task budget.
pub(super) fn insert_open_task_budget(
    tx: &Connection,
    request: &OpenTaskBudgetRequest,
    parent_budget_id: &BudgetId,
    fingerprint: String,
    opened_by: String,
    now: DateTime<Utc>,
) -> Result<TaskBudgetSnapshot, TaskBudgetError> {
    require_unallocated(tx, parent_budget_id, request.currency, request.amount_limit)?;
    let budget = TaskBudget {
        id: TaskBudgetId::new(),
        owner_user_id: request.owner_user_id.clone(),
        agent_id: request.agent_id.clone(),
        parent_budget_id: parent_budget_id.clone(),
        task_id: request.task_id.clone(),
        currency: request.currency,
        amount_limit: request.amount_limit,
        state: TaskBudgetState::Open,
        opened_at: now,
        opened_by,
        source: request.source.clone(),
        reason: trimmed_reason(request.reason.as_deref()),
        request_fingerprint: fingerprint,
        closed_at: None,
        closed_by: None,
    };
    tx.execute(
        "INSERT INTO task_budgets
         (id, public_id, owner_user_id, agent_id, parent_budget_id, task_id, currency,
          amount_limit, amount_scale, state, opened_at, opened_by, source, reason,
          request_fingerprint)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 6, 'open', ?9, ?10, ?11, ?12, ?13)",
        params![
            budget.id.to_string(),
            public_task_budget_id(&budget.id),
            budget.owner_user_id.to_string(),
            budget.agent_id.to_string(),
            budget.parent_budget_id.to_string(),
            budget.task_id,
            budget.currency.to_string(),
            budget.amount_limit.units(),
            budget.opened_at.to_rfc3339(),
            budget.opened_by,
            budget.source,
            budget.reason,
            budget.request_fingerprint,
        ],
    )?;
    Ok(TaskBudgetSnapshot {
        budget,
        usage: TaskBudgetUsage::NONE,
    })
}

/// Fail unless `amount` fits the parent's remaining balance minus the unused
/// part of its open task budgets.
pub(super) fn require_unallocated(
    conn: &Connection,
    parent_budget_id: &BudgetId,
    currency: Currency,
    amount: Amount,
) -> Result<(), TaskBudgetError> {
    let balance = load_budget_balance_by_id(conn, parent_budget_id)?
        .ok_or(TaskBudgetError::NoAvailableBudget { currency })?;
    let allocated = open_task_budgets_for_parent(conn, parent_budget_id)?
        .iter()
        .try_fold(Amount::ZERO, |total, snapshot| {
            total.checked_add(snapshot.allocated_headroom())
        })
        .ok_or_else(|| StorageError::InvalidData("task allocation overflow".into()))?;
    let available = balance
        .remaining_amount
        .checked_sub(allocated)
        .ok_or_else(|| StorageError::InvalidData("task allocation overflow".into()))?;
    if available < amount {
        return Err(TaskBudgetError::InsufficientUnallocatedBudget {
            available: if available.is_negative() {
                Amount::ZERO
            } else {
                available
            },
            requested: amount,
        });
    }
    Ok(())
}

/// Close an open task budget once no attributed authorization is outstanding.
pub(super) fn close_open_task_budget(
    tx: &Connection,
    existing: TaskBudgetSnapshot,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<TaskBudgetSnapshot, TaskBudgetError> {
    if existing.usage.outstanding_hold_count > 0 {
        return Err(TaskBudgetError::OutstandingHolds {
            count: existing.usage.outstanding_hold_count,
        });
    }
    require_one_updated_row(
        tx.execute(
            "UPDATE task_budgets SET state = 'closed', closed_at = ?2, closed_by = ?3
             WHERE id = ?1 AND state = 'open'",
            params![existing.budget.id.to_string(), now.to_rfc3339(), actor],
        )?,
        "task budget changed while closing",
    )?;
    let mut task_budget = existing;
    task_budget.budget.state = TaskBudgetState::Closed;
    task_budget.budget.closed_at = Some(now);
    task_budget.budget.closed_by = Some(actor.to_string());
    Ok(task_budget)
}

pub(super) fn trimmed_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_string)
}

pub(super) fn open_task_budget_for(
    conn: &Connection,
    agent_id: &AgentId,
    task_id: &str,
    currency: Currency,
) -> Result<Option<TaskBudgetSnapshot>, StorageError> {
    load_task_budget_where(
        conn,
        "agent_id = ?1 AND task_id = ?2 AND currency = ?3 AND state = 'open'",
        params![agent_id.to_string(), task_id, currency.to_string()],
    )
}

pub(super) fn open_task_budgets_for_parent(
    conn: &Connection,
    parent_budget_id: &BudgetId,
) -> Result<Vec<TaskBudgetSnapshot>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_BUDGET_COLUMNS} FROM task_budgets
         WHERE parent_budget_id = ?1 AND state = 'open'
         ORDER BY opened_at, id"
    ))?;
    let rows = stmt.query_map([parent_budget_id.to_string()], task_budget_from_row)?;
    let budgets = collect_rows(rows)?;
    with_usage(conn, budgets)
}

pub(super) fn load_task_budget_where(
    conn: &Connection,
    predicate: &str,
    params: impl rusqlite::Params,
) -> Result<Option<TaskBudgetSnapshot>, StorageError> {
    let budget = conn
        .query_row(
            &format!("SELECT {TASK_BUDGET_COLUMNS} FROM task_budgets WHERE {predicate}"),
            params,
            task_budget_from_row,
        )
        .optional()?;
    budget
        .map(|budget| {
            let usage = task_budget_usage(conn, &budget)?;
            Ok(TaskBudgetSnapshot { budget, usage })
        })
        .transpose()
}

fn with_usage(
    conn: &Connection,
    budgets: Vec<TaskBudget>,
) -> Result<Vec<TaskBudgetSnapshot>, StorageError> {
    budgets
        .into_iter()
        .map(|budget| {
            let usage = task_budget_usage(conn, &budget)?;
            Ok(TaskBudgetSnapshot { budget, usage })
        })
        .collect()
}

fn task_budget_from_row(row: &rusqlite::Row<'_>) -> Result<TaskBudget, rusqlite::Error> {
    let state: String = row.get(7)?;
    let closed_at: Option<String> = row.get(13)?;
    Ok(TaskBudget {
        id: parse_id(&row.get::<_, String>(0)?)?,
        owner_user_id: parse_id(&row.get::<_, String>(1)?)?,
        agent_id: parse_id(&row.get::<_, String>(2)?)?,
        parent_budget_id: parse_id(&row.get::<_, String>(3)?)?,
        task_id: row.get(4)?,
        currency: parse_currency(&row.get::<_, String>(5)?)?,
        amount_limit: Amount::from_units(row.get(6)?),
        state: state
            .parse()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        opened_at: parse_timestamp(&row.get::<_, String>(8)?)?,
        opened_by: row.get(9)?,
        source: row.get(10)?,
        reason: row.get(11)?,
        request_fingerprint: row.get(12)?,
        closed_at: closed_at.as_deref().map(parse_timestamp).transpose()?,
        closed_by: row.get(14)?,
    })
}

/// Derive a task budget's spend from its attributed holds.
///
/// Outstanding holds count their reserved amount. A settled hold counts the
/// budget charge the ledger recorded for its spend decision, summed across
/// any corrections; without recorded charge evidence it conservatively counts
/// its full reserved amount. Released and expired holds count nothing.
fn task_budget_usage(
    conn: &Connection,
    budget: &TaskBudget,
) -> Result<TaskBudgetUsage, StorageError> {
    let (frozen_amount, outstanding): (Amount, i64) = conn.query_row(
        "SELECT COALESCE(SUM(amount), 0), COUNT(*) FROM budget_holds
         WHERE task_budget_id = ?1 AND status IN ('frozen', 'claimed')",
        [budget.id.to_string()],
        |row| Ok((accounting_units_from_row(row, 0)?, row.get(1)?)),
    )?;

    let mut settled = HashMap::<String, (Amount, Option<Amount>)>::new();
    let mut stmt = conn.prepare(
        "SELECT h.id, h.amount, m.record_json
         FROM budget_holds AS h
         LEFT JOIN ledger_transaction_metadata AS m
           ON json_extract(m.record_json, '$.context.spend_decision_id') = h.spend_decision_id
         WHERE h.task_budget_id = ?1 AND h.status = 'settled'",
    )?;
    let rows = stmt.query_map([budget.id.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            accounting_units_from_row(row, 1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (hold_id, hold_amount, record_json) = row?;
        let entry = settled.entry(hold_id).or_insert((hold_amount, None));
        let Some(record_json) = record_json else {
            continue;
        };
        let metadata: TransactionMetadata = serde_json::from_str(&record_json)?;
        let Some(units) = metadata
            .budget_charge_units()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?
        else {
            continue;
        };
        let charge = entry
            .1
            .unwrap_or(Amount::ZERO)
            .checked_add(Amount::from_units(units))
            .ok_or_else(|| StorageError::InvalidData("task budget charge overflow".into()))?;
        entry.1 = Some(charge);
    }
    let consumed_amount = settled
        .values()
        .try_fold(Amount::ZERO, |total, (hold_amount, charge)| {
            total.checked_add(charge.unwrap_or(*hold_amount))
        })
        .ok_or_else(|| StorageError::InvalidData("task budget charge overflow".into()))?;

    Ok(TaskBudgetUsage {
        frozen_amount,
        consumed_amount,
        outstanding_hold_count: u64::try_from(outstanding).unwrap_or_default(),
        settled_hold_count: settled.len() as u64,
    })
}
