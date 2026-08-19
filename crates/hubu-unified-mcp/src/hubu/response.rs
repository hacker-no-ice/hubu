use thiserror::Error;

use crate::Secret;

pub(super) fn redact_backend_message(
    message: &str,
    bearer_token: &Secret,
    configured_reconciliation: Option<&Secret>,
    used_reconciliation: Option<&Secret>,
) -> String {
    let mut redacted = message.replace(bearer_token.expose(), "<redacted>");
    for secret in [configured_reconciliation, used_reconciliation]
        .into_iter()
        .flatten()
    {
        if !secret.expose().is_empty() {
            redacted = redacted.replace(secret.expose(), "<redacted>");
        }
    }
    redacted
}

#[derive(Debug, Error)]
pub(super) enum ForwardError {
    #[error("Hubu backend is unavailable")]
    Unavailable,
    #[error("Hubu backend request failed after dispatch; mutation outcome may be ambiguous")]
    AmbiguousTransport,
    #[error("Hubu backend returned an invalid JSON response")]
    InvalidResponse,
    #[error("Hubu route is invalid")]
    InvalidRoute,
    #[error(
        "human reconciliation requires HUBU_RECONCILIATION_TOKEN or HUBU_RECONCILIATION_TOKEN_FILE"
    )]
    MissingReconciliationCapability,
    #[error("Hubu reconciliation credential is invalid")]
    InvalidReconciliationCapability,
    #[error("Hubu approval capability is not supported by the HUB-88 routing contract")]
    UnsupportedCapability,
    #[error("Hubu server returned HTTP {status}: {message}")]
    Application { status: u16, message: String },
}
