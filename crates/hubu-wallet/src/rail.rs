use chrono::{DateTime, Utc};

use crate::payment::{PaymentRailKind, PaymentRequest, PaymentStatus};

#[derive(Debug, thiserror::Error)]
pub enum PaymentRailError {
    #[error("payment rail `{rail}` is unavailable")]
    Unavailable { rail: PaymentRailKind },
}

#[derive(Debug, Clone)]
pub struct RailPaymentResult {
    pub status: PaymentStatus,
    pub rail_reference: Option<String>,
    pub failure_reason: Option<String>,
    pub executed_at: DateTime<Utc>,
}

pub trait PaymentRail {
    fn execute(&self, request: &PaymentRequest) -> Result<RailPaymentResult, PaymentRailError>;
}

#[derive(Debug, Clone, Default)]
pub struct MockPaymentRail;

impl PaymentRail for MockPaymentRail {
    fn execute(&self, request: &PaymentRequest) -> Result<RailPaymentResult, PaymentRailError> {
        if request.merchant.as_deref() == Some("fail") {
            return Ok(RailPaymentResult {
                status: PaymentStatus::Failed,
                rail_reference: None,
                failure_reason: Some("mock rail declined merchant".to_string()),
                executed_at: Utc::now(),
            });
        }

        Ok(RailPaymentResult {
            status: PaymentStatus::Succeeded,
            rail_reference: Some(format!(
                "{}_{}",
                request.rail.as_ref(),
                request.idempotency_key
            )),
            failure_reason: None,
            executed_at: Utc::now(),
        })
    }
}
