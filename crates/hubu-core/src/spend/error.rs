use crate::policy::error::PolicyValidationError;

#[derive(Debug, thiserror::Error)]
pub enum SpendError {
    #[error("invalid policy")]
    PolicyValidation {
        #[from]
        source: PolicyValidationError,
    },
    #[error("unknown spend auth token")]
    UnknownSpendAuthToken,
    #[error("spend auth token is expired")]
    ExpiredSpendAuthToken,
    #[error("spend auth token has already been used")]
    UsedSpendAuthToken,
    #[error("spend auth token has been revoked")]
    RevokedSpendAuthToken,
    #[error("spend decision record is missing")]
    MissingSpendDecision,
    #[error("spend decision did not allow payment")]
    SpendDecisionNotAllowed,
    #[error("payment request does not match authorized spend")]
    PaymentRequestMismatch,
    #[error("request is outside the resolved user context")]
    UserScopeMismatch,
    #[error("unknown workload profile `{0}`")]
    UnknownWorkloadProfile(String),
    #[error("executor execution id cannot be empty")]
    EmptyExecutorExecutionId,
    #[error("spend auth token has already been claimed by another execution")]
    SpendAuthTokenAlreadyClaimed,
    #[error("unknown executor spend claim")]
    UnknownExecutorClaim,
    #[error("executor spend claim belongs to another execution")]
    ExecutorClaimExecutionMismatch,
    #[error("executor spend claim is expired and requires reconciliation")]
    ExpiredExecutorClaim,
    #[error("executor spend claim is already finalized")]
    FinalizedExecutorClaim,
}
