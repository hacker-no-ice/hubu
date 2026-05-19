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
}