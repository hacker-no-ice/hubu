use super::{BudgetLimitUpdateError, BudgetUpdateError, UpdateBudgetLimitRequest};
use chrono::{DateTime, Utc};
use serde_json::json;

use crate::{
    budget::BudgetManager,
    persistence::{AppendBudgetVersionRequest, BudgetVersionRepository},
    telemetry::log_event,
};

const DEFAULT_BUDGET_UPDATE_SOURCE: &str = "hubu-core:budget-update";

/// Owns storage-first budget administration and holds the repository lock
/// through committed-state publication.
///
/// Callers must hold the budget-manager lock through this method. The
/// repository commits the immutable successor, CAS pointer, logical balance,
/// and logical update timestamp before the in-memory manager is changed, so
/// any transaction failure leaves memory untouched.
#[derive(Debug, Clone)]
pub(crate) struct BudgetCoordinator {
    pub(super) repository:
        std::sync::Arc<std::sync::Mutex<crate::persistence::SqliteGovernanceRepository>>,
}

impl BudgetCoordinator {
    pub(super) fn adjust_provider_accounting(
        &self,
        manager: &mut BudgetManager,
        command: crate::persistence::accounting::ProviderAccountingAdjustment,
    ) -> Result<
        crate::persistence::accounting::ProviderAccountingRecord,
        crate::storage::StorageError,
    > {
        let mut repository = self.lock()?;
        let (record, state) = repository.adjust_provider_accounting(command)?;
        manager.apply_committed_state(state);
        Ok(record)
    }

    pub(super) fn create(
        &self,
        manager: &mut BudgetManager,
        request: super::CreateSingleBudgetRequest,
        provenance: super::BudgetVersionProvenance,
    ) -> Result<super::CreateSingleBudgetResponse, super::BudgetManagerError> {
        let mut repository = self.lock()?;
        let (result, state) = repository.create_budget(request, provenance)?;
        manager.apply_committed_state(state);
        Ok(result)
    }

    pub(super) fn revoke(
        &self,
        manager: &mut BudgetManager,
        budget_id: &hubu_common::ids::BudgetId,
        now: DateTime<Utc>,
    ) -> Result<super::BudgetWithBalance, super::BudgetManagerError> {
        let mut repository = self.lock()?;
        let (result, state) = repository.revoke_budget(budget_id, now)?;
        manager.apply_committed_state(state);
        Ok(result)
    }

    pub(super) fn update(
        &self,
        manager: &mut BudgetManager,
        request: UpdateBudgetLimitRequest,
        effective_at: DateTime<Utc>,
    ) -> Result<super::UpdateBudgetLimitResponse, BudgetLimitUpdateError> {
        let mut repository = self.lock().map_err(BudgetUpdateError::from)?;
        Self::update_limit(request, effective_at, manager, &mut *repository)
    }

    fn lock(
        &self,
    ) -> Result<
        std::sync::MutexGuard<'_, crate::persistence::SqliteGovernanceRepository>,
        crate::storage::StorageError,
    > {
        self.repository.lock().map_err(|_| {
            crate::storage::StorageError::InvalidData("governance store lock poisoned".into())
        })
    }

    pub(crate) fn update_limit<R>(
        request: UpdateBudgetLimitRequest,
        effective_at: DateTime<Utc>,
        budget_manager: &mut BudgetManager,
        repository: &mut R,
    ) -> Result<super::UpdateBudgetLimitResponse, BudgetLimitUpdateError>
    where
        R: BudgetVersionRepository,
    {
        if request.amount_limit_cents <= 0 {
            return Err(BudgetUpdateError::AmountLimitMustBePositive.into());
        }
        if request.expected_revision == 0 {
            return Err(BudgetUpdateError::ExpectedRevisionMustBePositive.into());
        }
        let actor = request.actor.trim().to_string();
        if actor.is_empty() {
            return Err(BudgetUpdateError::MissingActor.into());
        }
        let source = match request.source {
            Some(source) => {
                let source = source.trim().to_string();
                if source.is_empty() {
                    return Err(BudgetUpdateError::MissingSource.into());
                }
                source
            }
            None => DEFAULT_BUDGET_UPDATE_SOURCE.to_string(),
        };
        let reason = request.reason.and_then(|reason| {
            let reason = reason.trim().to_string();
            (!reason.is_empty()).then_some(reason)
        });
        let append = AppendBudgetVersionRequest {
            budget_id: request.budget_id,
            expected_revision: request.expected_revision,
            amount_limit_cents: request.amount_limit_cents,
            actor,
            source,
            reason,
            effective_at,
        };

        let result = repository.append_budget_version(&append)?;
        budget_manager.apply_committed_state(result.state);
        log_event(
            "info",
            "budget_limit_updated",
            json!({
                "budget_id": result.current.budget.id.to_string(),
                "applied_version_id": result.applied_version.id.to_string(),
                "applied_revision": result.applied_version.revision,
                "predecessor_revision": result.predecessor_revision,
                "current_version_id": result.current.version.id.to_string(),
                "current_revision": result.current.version.revision,
                "amount_limit_cents": result.applied_version.amount_limit_cents,
                "remaining_amount_cents": result.current.balance.remaining_amount_cents,
                "idempotent_replay": result.idempotent_replay,
            }),
        );
        Ok(super::UpdateBudgetLimitResponse {
            applied_version: result.applied_version,
            predecessor_revision: result.predecessor_revision,
            current: result.current,
            idempotent_replay: result.idempotent_replay,
        })
    }
}
