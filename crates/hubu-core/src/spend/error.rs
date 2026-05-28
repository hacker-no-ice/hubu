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
}
