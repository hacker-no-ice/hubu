use std::time::Duration;

use crate::{BackendClient, BackendOwner};
use serde_json::Value;

use super::response::{redact_backend_value, ForwardError};
use super::routing::HubuHttpRequestV1;

impl BackendClient {
    pub(super) fn execute_hubu(&self, request: HubuHttpRequestV1) -> anyhow::Result<Value> {
        debug_assert_eq!(self.owner, BackendOwner::Hubu);
        if !is_approved_http_route(request.method, &request.path) {
            return Err(ForwardError::InvalidRoute.into());
        }
        let is_read = request.method == "GET";
        let url = self
            .endpoint
            .join(request.path.trim_start_matches('/'))
            .map_err(|_| ForwardError::InvalidRoute)?;
        let mut builder = match request.method {
            "GET" => self.http.get(url),
            "POST" => self.http.post(url),
            _ => return Err(ForwardError::InvalidRoute.into()),
        };
        if request.path == "/spend/approval/prompt" {
            // The server holds the request for the 60-second owner dialog.
            builder = builder.timeout(Duration::from_secs(65));
        }
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let response = builder.send().map_err(|error| {
            if error.is_connect() || is_read {
                ForwardError::Unavailable
            } else {
                ForwardError::AmbiguousTransport
            }
        })?;
        let status = response.status();
        let body = response.json::<Value>().map_err(|error| {
            if is_read && (error.is_timeout() || error.is_body()) {
                ForwardError::Unavailable
            } else if is_read {
                ForwardError::InvalidResponse
            } else {
                ForwardError::AmbiguousTransport
            }
        })?;
        if !status.is_success() {
            let body = redact_backend_value(body, &self.bearer_token);
            let message = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
                .to_string();
            return Err(ForwardError::Application {
                status: status.as_u16(),
                message,
                error_code: body
                    .get("error_code")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                approval_status: body
                    .get("approval_status")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                retry_guidance: body.get("retry_guidance").cloned(),
            }
            .into());
        }
        Ok(body)
    }
}

pub(super) fn is_approved_http_route(method: &str, path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        (method, path),
        ("GET", "/health")
            | ("GET", "/registration/guidance")
            | ("POST", "/agents/register")
            | ("GET", "/policies/show")
            | ("POST", "/spend")
            | ("POST", "/spend/authorize")
            | ("POST", "/spend/void")
            | ("GET", "/spend/approval")
            | ("POST", "/spend/approval/prompt")
            | ("GET", "/agents")
            | ("GET", "/budgets")
            | ("GET", "/ledger")
            | ("GET", "/ledger/transactions")
            | ("GET", "/spend/authorizations")
            | ("GET", "/spend/authorizations/show")
    )
}
