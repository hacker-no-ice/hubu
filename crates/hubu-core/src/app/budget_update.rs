use chrono::{DateTime, Utc};
use hubu_common::ids::BudgetId;
use serde_json::json;

use crate::{
    budget::BudgetManager,
    persistence::{
        AppendBudgetVersionError, AppendBudgetVersionRequest, AppendBudgetVersionResult,
        BudgetVersionRepository,
    },
    telemetry::log_event,
};

const DEFAULT_BUDGET_UPDATE_SOURCE: &str = "hubu-core:budget-update";

/// Application command for changing the total limit of one stable logical
/// budget. Transport ownership checks and public-id parsing remain outside this
/// service.
#[derive(Debug, Clone)]
pub struct UpdateBudgetLimitRequest {
    pub budget_id: BudgetId,
    pub expected_revision: u64,
    pub amount_limit_cents: i64,
    pub actor: String,
    pub source: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BudgetUpdateServiceError {
    #[error(transparent)]
    Append(#[from] AppendBudgetVersionError),
}

/// Coordinates a storage-first budget limit update.
///
/// Callers must hold the budget-manager lock through this method. The
/// repository commits the immutable successor, CAS pointer, logical balance,
/// and status before the in-memory manager is changed, so any transaction
/// failure leaves memory untouched.
pub struct BudgetUpdateService;

impl BudgetUpdateService {
    pub fn update_limit<R>(
        &self,
        request: UpdateBudgetLimitRequest,
        effective_at: DateTime<Utc>,
        budget_manager: &mut BudgetManager,
        repository: &mut R,
    ) -> Result<AppendBudgetVersionResult, BudgetUpdateServiceError>
    where
        R: BudgetVersionRepository,
    {
        if request.amount_limit_cents <= 0 {
            return Err(AppendBudgetVersionError::AmountLimitMustBePositive.into());
        }
        if request.expected_revision == 0 {
            return Err(AppendBudgetVersionError::ExpectedRevisionMustBePositive.into());
        }
        let actor = request.actor.trim().to_string();
        if actor.is_empty() {
            return Err(AppendBudgetVersionError::MissingActor.into());
        }
        let source = match request.source {
            Some(source) => {
                let source = source.trim().to_string();
                if source.is_empty() {
                    return Err(AppendBudgetVersionError::MissingSource.into());
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
        budget_manager.apply_persisted_budget_version_append(
            result.applied_version.clone(),
            result.current.clone(),
        );
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
        Ok(result)
    }
}
