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
    #[error("unknown lease profile `{0}`")]
    UnknownLeaseProfile(String),
    #[error("operation key cannot be empty")]
    EmptyOperationKey,
    #[error("operation key was already authorized with different spend scope")]
    OperationKeyConflict,
    #[error("spend auth token has already been claimed by another operation")]
    SpendAuthTokenAlreadyClaimed,
    #[error("unknown executor spend claim")]
    UnknownExecutorClaim,
    #[error("executor spend claim belongs to another operation")]
    ExecutorClaimOperationMismatch,
    #[error("executor spend claim is expired and requires reconciliation")]
    ExpiredExecutorClaim,
    #[error("executor spend claim is already finalized")]
    FinalizedExecutorClaim,
}
