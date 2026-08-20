use std::{fs, io};

use crate::{BackendClient, BackendOwner, Secret};
use serde_json::Value;

use super::response::{redact_backend_message, ForwardError};
use super::routing::{HubuHttpRequestV1, HubuRequestCapabilityV1};

const DEFAULT_RECONCILIATION_TOKEN_FILE: &str = "hubu.reconciliation-token";
const RECONCILIATION_CAPABILITY_HEADER: &str = "X-Hubu-Reconciliation-Capability";

#[derive(Clone, Debug)]
pub struct RoutingConfig {
    pub(super) trusted_client_approval: bool,
    reconciliation_capability: Option<Secret>,
    pub(crate) reconciliation_capability_file: String,
}

impl RoutingConfig {
    pub fn new(trusted_client_approval: bool, reconciliation_capability: Option<String>) -> Self {
        Self {
            trusted_client_approval,
            reconciliation_capability: reconciliation_capability.map(Secret),
            reconciliation_capability_file: DEFAULT_RECONCILIATION_TOKEN_FILE.to_string(),
        }
    }

    fn reconciliation_capability(&self) -> Result<Secret, ForwardError> {
        if let Some(capability) = &self.reconciliation_capability {
            if capability.expose().trim().is_empty() {
                return Err(ForwardError::InvalidReconciliationCapability);
            }
            return Ok(capability.clone());
        }
        match fs::read_to_string(&self.reconciliation_capability_file) {
            Ok(contents) if contents.trim().is_empty() => {
                Err(ForwardError::InvalidReconciliationCapability)
            }
            Ok(contents) => Ok(Secret(contents.trim().to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(ForwardError::MissingReconciliationCapability)
            }
            Err(_) => Err(ForwardError::InvalidReconciliationCapability),
        }
    }
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self::new(false, None)
    }
}

impl BackendClient {
    pub(super) fn execute_hubu(
        &self,
        request: HubuHttpRequestV1,
        routing: &RoutingConfig,
    ) -> anyhow::Result<Value> {
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
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let mut used_reconciliation_capability = None;
        match request.capability {
            HubuRequestCapabilityV1::None => {}
            HubuRequestCapabilityV1::Reconciliation => {
                let capability = routing.reconciliation_capability()?;
                builder = builder.header(RECONCILIATION_CAPABILITY_HEADER, capability.expose());
                used_reconciliation_capability = Some(capability);
            }
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
            let message = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request failed");
            let message = redact_backend_message(
                message,
                &self.bearer_token,
                routing.reconciliation_capability.as_ref(),
                used_reconciliation_capability.as_ref(),
            );
            return Err(ForwardError::Application {
                status: status.as_u16(),
                message,
            }
            .into());
        }
        Ok(body)
    }
}

fn is_approved_http_route(method: &str, path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        (method, path),
        ("GET", "/health")
            | ("GET", "/registration/guidance")
            | ("GET", "/users")
            | ("POST", "/init")
            | ("POST", "/agents/register")
            | ("POST", "/policies")
            | ("GET", "/policies/show")
            | ("GET", "/policies/export")
            | ("GET", "/policies/history")
            | ("GET", "/policies/diff")
            | ("POST", "/budgets")
            | ("POST", "/budgets/series")
            | ("POST", "/budgets/revoke")
            | ("POST", "/budgets/replace")
            | ("POST", "/user/spending-target")
            | ("POST", "/user/spending-target/revoke")
            | ("GET", "/user/spending-target")
            | ("POST", "/spend")
            | ("POST", "/spend/authorize")
            | ("GET", "/agents")
            | ("GET", "/budgets")
            | ("GET", "/ledger")
            | ("GET", "/spend/executor/claim")
            | ("GET", "/spend/executor/reconciliation")
            | ("POST", "/spend/executor/settle")
            | ("POST", "/spend/executor/release")
    )
}
