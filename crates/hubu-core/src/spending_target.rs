use std::collections::HashMap;

use chrono::{DateTime, Utc};
use hubu_common::{
    ids::{SpendingTargetId, UserId},
    money::Currency,
    time::TimePeriod,
};

#[derive(Debug, Clone)]
pub struct SpendingTarget {
    pub id: SpendingTargetId,
    pub owner_user_id: UserId,
    pub target_amount_cents: i64,
    pub currency: Currency,
    pub period: TimePeriod,
    pub status: SpendingTargetStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SpendingTargetStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone)]
pub struct CreateSpendingTargetRequest {
    pub owner_user_id: UserId,
    pub target_amount_cents: i64,
    pub currency: Currency,
    pub period: TimePeriod,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum SpendingTargetError {
    #[error("spending target amount must be positive")]
    AmountMustBePositive,

    #[error("spending target period overlaps an active target")]
    OverlappingTargetPeriod,

    #[error("unknown spending target")]
    UnknownTarget,

    #[error("spending target is not active")]
    TargetNotActive,
}

pub struct SpendingTargetManager {
    targets: HashMap<SpendingTargetId, SpendingTarget>,
    target_ids_by_user_id: HashMap<UserId, Vec<SpendingTargetId>>,
}

impl SpendingTargetManager {
    pub fn new() -> Self {
        Self {
            targets: HashMap::new(),
            target_ids_by_user_id: HashMap::new(),
        }
    }

    pub fn from_records(targets: Vec<SpendingTarget>) -> Self {
        let mut manager = Self::new();
        for target in targets {
            manager.index_target(&target);
            manager.targets.insert(target.id.clone(), target);
        }
        manager
    }

    pub fn create_target(
        &mut self,
        request: CreateSpendingTargetRequest,
    ) -> Result<SpendingTarget, SpendingTargetError> {
        if request.target_amount_cents <= 0 {
            return Err(SpendingTargetError::AmountMustBePositive);
        }
        if self.targets.values().any(|target| {
            target.owner_user_id == request.owner_user_id
                && target.currency == request.currency
                && target.status == SpendingTargetStatus::Active
                && periods_overlap(&target.period, &request.period)
        }) {
            return Err(SpendingTargetError::OverlappingTargetPeriod);
        }

        let now = Utc::now();
        let target = SpendingTarget {
            id: SpendingTargetId::new(),
            owner_user_id: request.owner_user_id,
            target_amount_cents: request.target_amount_cents,
            currency: request.currency,
            period: request.period,
            status: SpendingTargetStatus::Active,
            created_at: now,
            updated_at: now,
        };
        self.index_target(&target);
        self.targets.insert(target.id.clone(), target.clone());
        Ok(target)
    }

    pub fn get_targets_by_user_id(&self, user_id: &UserId) -> Vec<SpendingTarget> {
        self.target_ids_by_user_id
            .get(user_id)
            .into_iter()
            .flatten()
            .filter_map(|target_id| self.targets.get(target_id).cloned())
            .collect()
    }

    pub fn revoke_target(
        &mut self,
        target_id: &SpendingTargetId,
    ) -> Result<SpendingTarget, SpendingTargetError> {
        let target = self
            .targets
            .get_mut(target_id)
            .ok_or(SpendingTargetError::UnknownTarget)?;
        if target.status != SpendingTargetStatus::Active {
            return Err(SpendingTargetError::TargetNotActive);
        }
        target.status = SpendingTargetStatus::Revoked;
        target.updated_at = Utc::now();
        Ok(target.clone())
    }

    fn index_target(&mut self, target: &SpendingTarget) {
        self.target_ids_by_user_id
            .entry(target.owner_user_id.clone())
            .or_default()
            .push(target.id.clone());
    }
}

impl Default for SpendingTargetManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn periods_overlap(left: &TimePeriod, right: &TimePeriod) -> bool {
    let left_starts_before_right_ends = right
        .ending_before
        .map_or(true, |right_end| left.starting_at < right_end);
    let right_starts_before_left_ends = left
        .ending_before
        .map_or(true, |left_end| right.starting_at < left_end);

    left_starts_before_right_ends && right_starts_before_left_ends
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn request(user_id: UserId, starting_at: DateTime<Utc>) -> CreateSpendingTargetRequest {
        CreateSpendingTargetRequest {
            owner_user_id: user_id,
            target_amount_cents: 10_000,
            currency: Currency::Usd,
            period: TimePeriod::new(starting_at, Some(starting_at + Duration::days(30)))
                .expect("period should be valid"),
        }
    }

    #[test]
    fn creates_and_lists_a_user_spending_target() {
        let mut manager = SpendingTargetManager::new();
        let user_id = UserId::new();
        let created = manager
            .create_target(request(user_id.clone(), Utc::now()))
            .expect("target should create");

        let targets = manager.get_targets_by_user_id(&user_id);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, created.id);
        assert_eq!(targets[0].target_amount_cents, 10_000);
    }

    #[test]
    fn rejects_overlapping_targets_for_the_same_user_and_currency() {
        let mut manager = SpendingTargetManager::new();
        let user_id = UserId::new();
        let starting_at = Utc::now();
        manager
            .create_target(request(user_id.clone(), starting_at))
            .expect("first target should create");

        let error = manager
            .create_target(request(user_id, starting_at + Duration::days(1)))
            .expect_err("overlapping target should fail");
        assert_eq!(error, SpendingTargetError::OverlappingTargetPeriod);
    }

    #[test]
    fn revoked_target_does_not_block_replacement() {
        let mut manager = SpendingTargetManager::new();
        let user_id = UserId::new();
        let starting_at = Utc::now();
        let created = manager
            .create_target(request(user_id.clone(), starting_at))
            .expect("target should create");
        manager
            .revoke_target(&created.id)
            .expect("target should revoke");

        manager
            .create_target(request(user_id, starting_at))
            .expect("replacement target should create");
    }
}
