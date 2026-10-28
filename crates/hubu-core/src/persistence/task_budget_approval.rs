//! Human approval requests for opening and closing task budgets.
//!
//! An agent proposes a task budget or asks to close one; a human approves or
//! denies the request on the shared approval link. A proposal reserves
//! nothing until approved. A pending request past its expiry reads as
//! expired, and each command persists that before acting.
use super::task_budget::{
    close_open_task_budget, insert_open_task_budget, load_task_budget_where, open_task_budget_for,
    require_unallocated, trimmed_reason,
};
use super::*;
use crate::budget::{
    public_task_budget_approval_id, task_budget_approval_expires_at, task_budget_open_fingerprint,
    validate_task_id, OpenTaskBudgetRequest, ProposeTaskBudgetResponse,
    RequestTaskBudgetCloseRequest, RequestTaskBudgetCloseResponse,
    ResolveTaskBudgetApprovalRequest, ResolveTaskBudgetApprovalResponse, TaskBudgetApproval,
    TaskBudgetApprovalDecision, TaskBudgetApprovalKind, TaskBudgetApprovalSnapshot,
    TaskBudgetApprovalStatus, TaskBudgetError, TaskBudgetSnapshot, TaskBudgetState,
};
use hubu_common::ids::{TaskBudgetApprovalId, TaskBudgetId};
use sha2::{Digest, Sha256};

const APPROVAL_COLUMNS: &str = "id, kind, owner_user_id, agent_id, task_id, currency,
     amount_limit, task_budget_id, status, requested_at, requested_by, source, reason,
     request_fingerprint, expires_at, resolved_at, resolved_by";

impl SqliteGovernanceRepository {
    pub(super) fn init_task_budget_approval_schema(&mut self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_budget_approvals (
                id TEXT PRIMARY KEY,
                public_id TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL CHECK(kind IN ('open', 'close')),
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                task_id TEXT NOT NULL CHECK(length(task_id) BETWEEN 1 AND 512),
                currency TEXT NOT NULL,
                amount_limit INTEGER NOT NULL CHECK(amount_limit > 0),
                amount_scale INTEGER NOT NULL CHECK(amount_scale = 6),
                task_budget_id TEXT REFERENCES task_budgets(id),
                status TEXT NOT NULL
                    CHECK(status IN ('pending', 'approved', 'denied', 'expired')),
                requested_at TEXT NOT NULL,
                requested_by TEXT NOT NULL CHECK(length(requested_by) > 0),
                source TEXT NOT NULL CHECK(length(source) > 0),
                reason TEXT,
                request_fingerprint TEXT NOT NULL CHECK(length(request_fingerprint) > 0),
                expires_at TEXT NOT NULL,
                resolved_at TEXT,
                resolved_by TEXT,
                CHECK((status = 'pending') = (resolved_at IS NULL)),
                CHECK((status IN ('pending', 'expired')) = (resolved_by IS NULL)),
                CHECK(kind = 'open' OR task_budget_id IS NOT NULL),
                CHECK(kind = 'close' OR (status = 'approved') = (task_budget_id IS NOT NULL))
             );
             CREATE UNIQUE INDEX IF NOT EXISTS task_budget_approvals_one_pending_open
                ON task_budget_approvals(agent_id, task_id, currency)
                WHERE kind = 'open' AND status = 'pending';
             CREATE UNIQUE INDEX IF NOT EXISTS task_budget_approvals_one_pending_close
                ON task_budget_approvals(task_budget_id)
                WHERE kind = 'close' AND status = 'pending';
             CREATE INDEX IF NOT EXISTS task_budget_approvals_pending
                ON task_budget_approvals(owner_user_id) WHERE status = 'pending';
             CREATE TRIGGER IF NOT EXISTS task_budget_approvals_resolve_once
                BEFORE UPDATE ON task_budget_approvals
                WHEN OLD.status <> 'pending' OR NEW.status = 'pending'
                  OR NEW.id IS NOT OLD.id OR NEW.public_id IS NOT OLD.public_id
                  OR NEW.kind IS NOT OLD.kind
                  OR NEW.owner_user_id IS NOT OLD.owner_user_id
                  OR NEW.agent_id IS NOT OLD.agent_id
                  OR NEW.task_id IS NOT OLD.task_id OR NEW.currency IS NOT OLD.currency
                  OR NEW.amount_limit IS NOT OLD.amount_limit
                  OR NEW.amount_scale IS NOT OLD.amount_scale
                  OR (OLD.task_budget_id IS NOT NULL
                      AND NEW.task_budget_id IS NOT OLD.task_budget_id)
                  OR NEW.requested_at IS NOT OLD.requested_at
                  OR NEW.requested_by IS NOT OLD.requested_by
                  OR NEW.source IS NOT OLD.source OR NEW.reason IS NOT OLD.reason
                  OR NEW.request_fingerprint IS NOT OLD.request_fingerprint
                  OR NEW.expires_at IS NOT OLD.expires_at
                BEGIN SELECT RAISE(ABORT, 'task budget approvals resolve once'); END;
             CREATE TRIGGER IF NOT EXISTS task_budget_approvals_no_delete
                BEFORE DELETE ON task_budget_approvals
                BEGIN SELECT RAISE(ABORT, 'task budget approvals are retained'); END;",
        )?;
        Ok(())
    }

    /// Record an agent's proposal to open a task budget drawn from `parent`.
    ///
    /// Nothing is reserved until a human approves it. The limit must fit the
    /// parent's unallocated balance now, and is checked again on approval.
    /// An identical proposal replays the pending one, or the task budget
    /// already open for the task.
    pub fn propose_task_budget(
        &mut self,
        request: &OpenTaskBudgetRequest,
        parent: &BudgetWithBalance,
        now: DateTime<Utc>,
    ) -> Result<ProposeTaskBudgetResponse, TaskBudgetError> {
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
            return Ok(ProposeTaskBudgetResponse {
                approval: None,
                task_budget: Some(existing),
                replayed: true,
            });
        }
        if let Some(pending) =
            pending_open_proposal_for(&tx, &request.agent_id, &request.task_id, request.currency)?
        {
            if pending.owner_user_id != request.owner_user_id
                || pending.request_fingerprint != fingerprint
            {
                return Err(TaskBudgetError::OpenTaskBudgetConflict);
            }
            tx.commit()?;
            return Ok(ProposeTaskBudgetResponse {
                approval: Some(pending),
                task_budget: None,
                replayed: true,
            });
        }
        require_unallocated(
            &tx,
            &parent.budget.id,
            request.currency,
            request.amount_limit,
        )?;
        let approval = TaskBudgetApproval {
            id: TaskBudgetApprovalId::new(),
            kind: TaskBudgetApprovalKind::Open,
            owner_user_id: request.owner_user_id.clone(),
            agent_id: request.agent_id.clone(),
            task_id: request.task_id.clone(),
            currency: request.currency,
            amount_limit: request.amount_limit,
            task_budget_id: None,
            status: TaskBudgetApprovalStatus::Pending,
            requested_at: now,
            requested_by: request.actor.clone(),
            source: request.source.clone(),
            reason: trimmed_reason(request.reason.as_deref()),
            request_fingerprint: fingerprint,
            expires_at: task_budget_approval_expires_at(now),
            resolved_at: None,
            resolved_by: None,
        };
        insert_approval(&tx, &approval)?;
        tx.commit()?;
        Ok(ProposeTaskBudgetResponse {
            approval: Some(approval),
            task_budget: None,
            replayed: false,
        })
    }

    /// Record an agent's request to close an open task budget.
    ///
    /// While it is pending, new spend for the task is refused; authorizations
    /// already outstanding can still settle. Repeating the request replays
    /// the pending one, and a closed task budget is returned unchanged.
    pub fn request_task_budget_close(
        &mut self,
        request: &RequestTaskBudgetCloseRequest,
        now: DateTime<Utc>,
    ) -> Result<RequestTaskBudgetCloseResponse, TaskBudgetError> {
        self.expire_task_budget_approvals(now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task_budget = load_task_budget_where(
            &tx,
            "id = ?1 AND owner_user_id = ?2",
            params![
                request.task_budget_id.to_string(),
                request.owner_user_id.to_string()
            ],
        )?
        .ok_or(TaskBudgetError::UnknownTaskBudget)?;
        if task_budget.budget.state == TaskBudgetState::Closed {
            tx.commit()?;
            return Ok(RequestTaskBudgetCloseResponse {
                task_budget,
                approval: None,
                replayed: true,
            });
        }
        if let Some(pending) = pending_close_request_for(&tx, &task_budget.budget.id)? {
            tx.commit()?;
            return Ok(RequestTaskBudgetCloseResponse {
                task_budget,
                approval: Some(pending),
                replayed: true,
            });
        }
        let budget = &task_budget.budget;
        let approval = TaskBudgetApproval {
            id: TaskBudgetApprovalId::new(),
            kind: TaskBudgetApprovalKind::Close,
            owner_user_id: budget.owner_user_id.clone(),
            agent_id: budget.agent_id.clone(),
            task_id: budget.task_id.clone(),
            currency: budget.currency,
            amount_limit: budget.amount_limit,
            task_budget_id: Some(budget.id.clone()),
            status: TaskBudgetApprovalStatus::Pending,
            requested_at: now,
            requested_by: request.actor.clone(),
            source: request.source.clone(),
            reason: trimmed_reason(request.reason.as_deref()),
            request_fingerprint: task_budget_close_fingerprint(&budget.id),
            expires_at: task_budget_approval_expires_at(now),
            resolved_at: None,
            resolved_by: None,
        };
        insert_approval(&tx, &approval)?;
        tx.commit()?;
        Ok(RequestTaskBudgetCloseResponse {
            task_budget,
            approval: Some(approval),
            replayed: false,
        })
    }

    /// Apply a human decision to a pending task budget approval request.
    ///
    /// Approving a proposal opens the task budget from `parent`, which the
    /// caller evaluated under the budget manager lock; its limit must still
    /// fit the unallocated balance. Approving a close request closes the task
    /// budget once no authorization under it is outstanding. A refused
    /// approval leaves the request pending. Denying a close request leaves
    /// the task budget open, so spending resumes.
    pub fn resolve_task_budget_approval(
        &mut self,
        request: &ResolveTaskBudgetApprovalRequest,
        parent: Option<&BudgetWithBalance>,
        now: DateTime<Utc>,
    ) -> Result<ResolveTaskBudgetApprovalResponse, TaskBudgetError> {
        self.expire_task_budget_approvals(now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut approval = load_approval_where(
            &tx,
            "id = ?1 AND owner_user_id = ?2",
            params![
                request.approval_id.to_string(),
                request.owner_user_id.to_string()
            ],
        )?
        .ok_or(TaskBudgetError::UnknownApproval)?;
        let target = match request.decision {
            TaskBudgetApprovalDecision::Approve => TaskBudgetApprovalStatus::Approved,
            TaskBudgetApprovalDecision::Deny => TaskBudgetApprovalStatus::Denied,
        };
        if approval.status != TaskBudgetApprovalStatus::Pending {
            if approval.status != target {
                return Err(TaskBudgetError::ApprovalResolved {
                    status: approval.status.as_str(),
                });
            }
            let snapshot = approval_snapshot(&tx, approval)?;
            tx.commit()?;
            return Ok(ResolveTaskBudgetApprovalResponse {
                approval: snapshot,
                replayed: true,
            });
        }

        let task_budget = match (request.decision, approval.kind) {
            (TaskBudgetApprovalDecision::Deny, _) => None,
            (TaskBudgetApprovalDecision::Approve, TaskBudgetApprovalKind::Open) => {
                let parent = parent
                    .filter(|parent| {
                        parent.budget.agent_id == approval.agent_id
                            && parent.budget.currency == approval.currency
                    })
                    .ok_or(TaskBudgetError::NoAvailableBudget {
                        currency: approval.currency,
                    })?;
                if open_task_budget_for(
                    &tx,
                    &approval.agent_id,
                    &approval.task_id,
                    approval.currency,
                )?
                .is_some()
                {
                    return Err(TaskBudgetError::OpenTaskBudgetConflict);
                }
                let opened = insert_open_task_budget(
                    &tx,
                    &OpenTaskBudgetRequest {
                        owner_user_id: approval.owner_user_id.clone(),
                        agent_id: approval.agent_id.clone(),
                        task_id: approval.task_id.clone(),
                        currency: approval.currency,
                        amount_limit: approval.amount_limit,
                        actor: request.actor.clone(),
                        source: approval.source.clone(),
                        reason: approval.reason.clone(),
                    },
                    &parent.budget.id,
                    approval.request_fingerprint.clone(),
                    request.actor.clone(),
                    now,
                )?;
                approval.task_budget_id = Some(opened.budget.id.clone());
                Some(opened)
            }
            (TaskBudgetApprovalDecision::Approve, TaskBudgetApprovalKind::Close) => {
                let task_budget_id = approval.task_budget_id.clone().ok_or_else(|| {
                    StorageError::InvalidData("close request has no task budget".into())
                })?;
                let existing =
                    load_task_budget_where(&tx, "id = ?1", params![task_budget_id.to_string()])?
                        .ok_or(TaskBudgetError::UnknownTaskBudget)?;
                if existing.budget.state != TaskBudgetState::Open {
                    return Err(TaskBudgetError::TaskBudgetNotOpen);
                }
                Some(close_open_task_budget(&tx, existing, &request.actor, now)?)
            }
        };
        require_one_updated_row(
            tx.execute(
                "UPDATE task_budget_approvals
                 SET status = ?2, resolved_at = ?3, resolved_by = ?4, task_budget_id = ?5
                 WHERE id = ?1 AND status = 'pending'",
                params![
                    approval.id.to_string(),
                    target.as_str(),
                    now.to_rfc3339(),
                    request.actor,
                    approval.task_budget_id.as_ref().map(ToString::to_string),
                ],
            )?,
            "task budget approval changed while resolving",
        )?;
        approval.status = target;
        approval.resolved_at = Some(now);
        approval.resolved_by = Some(request.actor.clone());
        let task_budget = match task_budget {
            Some(task_budget) => Some(task_budget),
            None => approval_task_budget(&tx, &approval)?,
        };
        tx.commit()?;
        Ok(ResolveTaskBudgetApprovalResponse {
            approval: TaskBudgetApprovalSnapshot {
                approval,
                task_budget,
            },
            replayed: false,
        })
    }

    /// Persist the expiry of pending requests past their `expires_at`. It
    /// commits on its own, so a command refused afterwards keeps the sweep.
    pub fn expire_task_budget_approvals(&mut self, now: DateTime<Utc>) -> Result<(), StorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_stale_task_budget_approvals(&tx, now)?;
        tx.commit()?;
        Ok(())
    }

    /// One owned approval request by its public `tba_` identifier.
    pub fn load_task_budget_approval(
        &self,
        owner_user_id: &UserId,
        public_id: &str,
    ) -> Result<Option<TaskBudgetApprovalSnapshot>, StorageError> {
        load_approval_where(
            &self.conn,
            "public_id = ?1 AND owner_user_id = ?2",
            params![public_id, owner_user_id.to_string()],
        )?
        .map(|approval| approval_snapshot(&self.conn, approval))
        .transpose()
    }

    /// One owned approval request by its internal identifier.
    pub fn load_task_budget_approval_by_id(
        &self,
        owner_user_id: &UserId,
        id: &TaskBudgetApprovalId,
    ) -> Result<Option<TaskBudgetApproval>, StorageError> {
        load_approval_where(
            &self.conn,
            "id = ?1 AND owner_user_id = ?2",
            params![id.to_string(), owner_user_id.to_string()],
        )
    }

    /// Owned requests still waiting on a human at `now`, oldest first.
    pub fn list_pending_task_budget_approvals(
        &self,
        owner_user_id: &UserId,
        agent_id: Option<&AgentId>,
        now: DateTime<Utc>,
    ) -> Result<Vec<TaskBudgetApprovalSnapshot>, StorageError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {APPROVAL_COLUMNS} FROM task_budget_approvals
             WHERE owner_user_id = ?1 AND status = 'pending'
               AND (?2 IS NULL OR agent_id = ?2)
             ORDER BY requested_at, id"
        ))?;
        let rows = stmt.query_map(
            params![owner_user_id.to_string(), agent_id.map(ToString::to_string)],
            approval_from_row,
        )?;
        collect_rows(rows)?
            .into_iter()
            .filter(|approval| approval.status_at(now) == TaskBudgetApprovalStatus::Pending)
            .map(|approval| approval_snapshot(&self.conn, approval))
            .collect()
    }

    /// The close request waiting on a human for this task budget at `now`.
    pub fn pending_task_budget_close_request(
        &self,
        task_budget_id: &TaskBudgetId,
        now: DateTime<Utc>,
    ) -> Result<Option<TaskBudgetApproval>, StorageError> {
        Ok(pending_close_request_for(&self.conn, task_budget_id)?
            .filter(|approval| approval.status_at(now) == TaskBudgetApprovalStatus::Pending))
    }
}

/// Mark pending requests past their `expires_at` expired inside `tx`.
pub(super) fn expire_stale_task_budget_approvals(
    tx: &Connection,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    let mut stmt = tx.prepare(&format!(
        "SELECT {APPROVAL_COLUMNS} FROM task_budget_approvals WHERE status = 'pending'"
    ))?;
    let rows = stmt.query_map([], approval_from_row)?;
    for approval in collect_rows(rows)? {
        if approval.status_at(now) == TaskBudgetApprovalStatus::Expired {
            tx.execute(
                "UPDATE task_budget_approvals SET status = 'expired', resolved_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
                params![approval.id.to_string(), now.to_rfc3339()],
            )?;
        }
    }
    Ok(())
}

/// The pending proposal for a task, as stored (the caller sweeps expiry).
pub(super) fn pending_open_proposal_for(
    conn: &Connection,
    agent_id: &AgentId,
    task_id: &str,
    currency: Currency,
) -> Result<Option<TaskBudgetApproval>, StorageError> {
    load_approval_where(
        conn,
        "kind = 'open' AND status = 'pending'
         AND agent_id = ?1 AND task_id = ?2 AND currency = ?3",
        params![agent_id.to_string(), task_id, currency.to_string()],
    )
}

fn pending_close_request_for(
    conn: &Connection,
    task_budget_id: &TaskBudgetId,
) -> Result<Option<TaskBudgetApproval>, StorageError> {
    load_approval_where(
        conn,
        "kind = 'close' AND status = 'pending' AND task_budget_id = ?1",
        params![task_budget_id.to_string()],
    )
}

/// A human's direct close also approves the agent's pending close request.
pub(super) fn approve_pending_close_request(
    tx: &Connection,
    task_budget_id: &TaskBudgetId,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE task_budget_approvals
         SET status = 'approved', resolved_at = ?2, resolved_by = ?3
         WHERE kind = 'close' AND status = 'pending' AND task_budget_id = ?1",
        params![task_budget_id.to_string(), now.to_rfc3339(), actor],
    )?;
    Ok(())
}

/// Spend gate: the kind of request for this task still pending at `now`.
pub(super) fn pending_task_budget_approval_for_task(
    conn: &Connection,
    agent_id: &AgentId,
    task_id: &str,
    currency: Currency,
    now: DateTime<Utc>,
) -> Result<Option<TaskBudgetApprovalKind>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {APPROVAL_COLUMNS} FROM task_budget_approvals
         WHERE status = 'pending' AND agent_id = ?1 AND task_id = ?2 AND currency = ?3
         ORDER BY requested_at, id"
    ))?;
    let rows = stmt.query_map(
        params![agent_id.to_string(), task_id, currency.to_string()],
        approval_from_row,
    )?;
    Ok(collect_rows(rows)?
        .into_iter()
        .find(|approval| approval.status_at(now) == TaskBudgetApprovalStatus::Pending)
        .map(|approval| approval.kind))
}

fn task_budget_close_fingerprint(task_budget_id: &TaskBudgetId) -> String {
    let canonical = serde_json::to_vec(&("hubu-task-budget-close-v1", task_budget_id.to_string()))
        .expect("canonical close fields are always JSON serializable");
    format!("sha256:{:x}", Sha256::digest(canonical))
}

fn insert_approval(tx: &Connection, approval: &TaskBudgetApproval) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO task_budget_approvals
         (id, public_id, kind, owner_user_id, agent_id, task_id, currency, amount_limit,
          amount_scale, task_budget_id, status, requested_at, requested_by, source, reason,
          request_fingerprint, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 6, ?9, 'pending', ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            approval.id.to_string(),
            public_task_budget_approval_id(&approval.id),
            approval.kind.as_str(),
            approval.owner_user_id.to_string(),
            approval.agent_id.to_string(),
            approval.task_id,
            approval.currency.to_string(),
            approval.amount_limit.units(),
            approval.task_budget_id.as_ref().map(ToString::to_string),
            approval.requested_at.to_rfc3339(),
            approval.requested_by,
            approval.source,
            approval.reason,
            approval.request_fingerprint,
            approval.expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn approval_snapshot(
    conn: &Connection,
    approval: TaskBudgetApproval,
) -> Result<TaskBudgetApprovalSnapshot, StorageError> {
    let task_budget = approval_task_budget(conn, &approval)?;
    Ok(TaskBudgetApprovalSnapshot {
        approval,
        task_budget,
    })
}

fn approval_task_budget(
    conn: &Connection,
    approval: &TaskBudgetApproval,
) -> Result<Option<TaskBudgetSnapshot>, StorageError> {
    match &approval.task_budget_id {
        Some(id) => load_task_budget_where(conn, "id = ?1", params![id.to_string()]),
        None => Ok(None),
    }
}

fn load_approval_where(
    conn: &Connection,
    predicate: &str,
    params: impl rusqlite::Params,
) -> Result<Option<TaskBudgetApproval>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {APPROVAL_COLUMNS} FROM task_budget_approvals WHERE {predicate}"),
            params,
            approval_from_row,
        )
        .optional()?)
}

fn approval_from_row(row: &rusqlite::Row<'_>) -> Result<TaskBudgetApproval, rusqlite::Error> {
    let kind: String = row.get(1)?;
    let task_budget_id: Option<String> = row.get(7)?;
    let status: String = row.get(8)?;
    let resolved_at: Option<String> = row.get(15)?;
    let invalid = |error: StorageError| rusqlite::Error::ToSqlConversionFailure(Box::new(error));
    Ok(TaskBudgetApproval {
        id: parse_id(&row.get::<_, String>(0)?)?,
        kind: kind.parse().map_err(invalid)?,
        owner_user_id: parse_id(&row.get::<_, String>(2)?)?,
        agent_id: parse_id(&row.get::<_, String>(3)?)?,
        task_id: row.get(4)?,
        currency: parse_currency(&row.get::<_, String>(5)?)?,
        amount_limit: Amount::from_units(row.get(6)?),
        task_budget_id: parse_optional_id(task_budget_id)?,
        status: status.parse().map_err(invalid)?,
        requested_at: parse_timestamp(&row.get::<_, String>(9)?)?,
        requested_by: row.get(10)?,
        source: row.get(11)?,
        reason: row.get(12)?,
        request_fingerprint: row.get(13)?,
        expires_at: parse_timestamp(&row.get::<_, String>(14)?)?,
        resolved_at: resolved_at.as_deref().map(parse_timestamp).transpose()?,
        resolved_by: row.get(16)?,
    })
}
