use crate::policy::error::PolicyValidationError;

#[derive(Debug, thiserror::Error)]
pub enum SpendError {
    #[error("invalid policy")]
    PolicyValidation {
        #[from]
        source: PolicyValidationError,
    },
}
