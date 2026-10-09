use crate::storage::StorageError;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistrationError {
    #[error("missing agent identity or version fingerprint")]
    MissingFingerprint,

    #[error("identity fingerprint already belongs to a different entity")]
    IdentityConflict,

    #[error("version fingerprint resolves to an existing version that is different")]
    VersionConflict,

    #[error("agent is suspended")]
    SuspendedAgent,

    #[error("agent account is suspended")]
    SuspendedAccount,

    #[error("invalid rename: {0}")]
    InvalidRename(&'static str),

    #[error("agent not found")]
    AgentNotFound,

    #[error(
        "agent identity changed since the rename was prepared; review the current name and retry"
    )]
    RenameConflict,

    #[error("rename does not change the agent identity")]
    NoIdentityChange,

    #[error("renamed identity fingerprint already belongs to a different agent")]
    IdentityFingerprintCollision,

    #[error("agent name is already used by another agent of this owner")]
    AgentNameConflict,

    #[error("agent name is reserved as a previous name of another agent of this owner")]
    AgentNameReserved,

    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for RegistrationError {
    fn from(error: rusqlite::Error) -> Self {
        StorageError::from(error).into()
    }
}

impl From<serde_json::Error> for RegistrationError {
    fn from(error: serde_json::Error) -> Self {
        StorageError::from(error).into()
    }
}
