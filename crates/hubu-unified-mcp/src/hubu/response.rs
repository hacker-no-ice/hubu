use serde_json::Value;
use thiserror::Error;

use crate::Secret;

pub(super) fn redact_backend_message(
    message: &str,
    bearer_token: &Secret,
    configured_approval: Option<&Secret>,
    used_approval: Option<&Secret>,
    configured_reconciliation: Option<&Secret>,
    used_reconciliation: Option<&Secret>,
) -> String {
    let mut redacted = message.to_string();
    if !bearer_token.expose().is_empty() {
        redacted = redacted.replace(bearer_token.expose(), "<redacted>");
    }
    for secret in [
        configured_approval,
        used_approval,
        configured_reconciliation,
        used_reconciliation,
    ]
    .into_iter()
    .flatten()
    {
        if !secret.expose().is_empty() {
            redacted = redacted.replace(secret.expose(), "<redacted>");
        }
    }
    redacted
}

pub(super) fn redact_backend_value(
    mut value: Value,
    bearer_token: &Secret,
    configured_approval: Option<&Secret>,
    used_approval: Option<&Secret>,
    configured_reconciliation: Option<&Secret>,
    used_reconciliation: Option<&Secret>,
) -> Value {
    redact_value_strings(
        &mut value,
        bearer_token,
        configured_approval,
        used_approval,
        configured_reconciliation,
        used_reconciliation,
    );
    value
}

fn redact_value_strings(
    value: &mut Value,
    bearer_token: &Secret,
    configured_approval: Option<&Secret>,
    used_approval: Option<&Secret>,
    configured_reconciliation: Option<&Secret>,
    used_reconciliation: Option<&Secret>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                redact_value_strings(
                    value,
                    bearer_token,
                    configured_approval,
                    used_approval,
                    configured_reconciliation,
                    used_reconciliation,
                );
            }
        }
        Value::Object(object) => {
            let entries = std::mem::take(object);
            for (key, mut child) in entries {
                let key = redact_backend_message(
                    &key,
                    bearer_token,
                    configured_approval,
                    used_approval,
                    configured_reconciliation,
                    used_reconciliation,
                );
                redact_value_strings(
                    &mut child,
                    bearer_token,
                    configured_approval,
                    used_approval,
                    configured_reconciliation,
                    used_reconciliation,
                );
                object.entry(key).or_insert(child);
            }
        }
        Value::String(message) => {
            *message = redact_backend_message(
                message,
                bearer_token,
                configured_approval,
                used_approval,
                configured_reconciliation,
                used_reconciliation,
            );
        }
        _ => {}
    }
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
    #[error("human spend approval requires HUBU_APPROVAL_TOKEN or HUBU_APPROVAL_TOKEN_FILE")]
    MissingApprovalCapability,
    #[error("Hubu approval credential is invalid")]
    InvalidApprovalCapability,
    #[error(
        "human reconciliation requires HUBU_RECONCILIATION_TOKEN or HUBU_RECONCILIATION_TOKEN_FILE"
    )]
    MissingReconciliationCapability,
    #[error("Hubu reconciliation credential is invalid")]
    InvalidReconciliationCapability,
    #[error("Hubu server returned HTTP {status}: {message}")]
    Application {
        status: u16,
        message: String,
        error_code: Option<String>,
        details: Option<Value>,
        retry_guidance: Option<Value>,
    },
}
