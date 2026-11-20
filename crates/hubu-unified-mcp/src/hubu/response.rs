use serde_json::Value;
use thiserror::Error;

use crate::Secret;

pub(super) fn redact_backend_value(mut value: Value, bearer_token: &Secret) -> Value {
    redact_value_strings(&mut value, bearer_token.expose());
    value
}

fn redact_value_strings(value: &mut Value, bearer_token: &str) {
    match value {
        Value::Array(values) => {
            for value in values {
                redact_value_strings(value, bearer_token);
            }
        }
        Value::Object(object) => {
            for (key, mut child) in std::mem::take(object) {
                let key = key.replace(bearer_token, "<redacted>");
                redact_value_strings(&mut child, bearer_token);
                object.entry(key).or_insert(child);
            }
        }
        Value::String(message) => {
            *message = message.replace(bearer_token, "<redacted>");
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
    #[error("Hubu server returned HTTP {status}: {message}")]
    Application {
        status: u16,
        message: String,
        error_code: Option<String>,
        approval_status: Option<String>,
        retry_guidance: Option<Value>,
    },
}
