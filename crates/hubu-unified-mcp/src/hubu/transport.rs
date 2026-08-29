use std::{fs, io};

use crate::{BackendClient, BackendOwner, Secret};
use serde_json::Value;

use super::response::{redact_backend_value, ForwardError};
use super::routing::{validate_public_budget_id, HubuHttpRequestV1, HubuRequestCapabilityV1};

const DEFAULT_APPROVAL_TOKEN_FILE: &str = "hubu.approval-token";
const APPROVAL_CAPABILITY_HEADER: &str = "X-Hubu-Approval-Capability";
const DEFAULT_RECONCILIATION_TOKEN_FILE: &str = "hubu.reconciliation-token";
const RECONCILIATION_CAPABILITY_HEADER: &str = "X-Hubu-Reconciliation-Capability";

#[derive(Clone, Debug)]
pub struct RoutingConfig {
    pub(super) trusted_client_approval: bool,
    pub(super) trusted_spend_approval: bool,
    approval_capability: Option<Secret>,
    pub(crate) approval_capability_file: String,
    reconciliation_capability: Option<Secret>,
    pub(crate) reconciliation_capability_file: String,
}

impl RoutingConfig {
    pub fn new(trusted_client_approval: bool, reconciliation_capability: Option<String>) -> Self {
        Self::new_with_spend_approval(
            trusted_client_approval,
            false,
            None,
            reconciliation_capability,
        )
    }

    pub fn new_with_spend_approval(
        trusted_client_approval: bool,
        trusted_spend_approval: bool,
        approval_capability: Option<String>,
        reconciliation_capability: Option<String>,
    ) -> Self {
        Self {
            trusted_client_approval,
            trusted_spend_approval,
            approval_capability: approval_capability.map(Secret),
            approval_capability_file: DEFAULT_APPROVAL_TOKEN_FILE.to_string(),
            reconciliation_capability: reconciliation_capability.map(Secret),
            reconciliation_capability_file: DEFAULT_RECONCILIATION_TOKEN_FILE.to_string(),
        }
    }

    fn approval_capability(&self) -> Result<Secret, ForwardError> {
        if let Some(capability) = &self.approval_capability {
            if capability.expose().trim().is_empty() {
                return Err(ForwardError::InvalidApprovalCapability);
            }
            return Ok(capability.clone());
        }
        match fs::read_to_string(&self.approval_capability_file) {
            Ok(contents) if contents.trim().is_empty() => {
                Err(ForwardError::InvalidApprovalCapability)
            }
            Ok(contents) => Ok(Secret(contents.trim().to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(ForwardError::MissingApprovalCapability)
            }
            Err(_) => Err(ForwardError::InvalidApprovalCapability),
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
        let mut used_approval_capability = None;
        let mut used_reconciliation_capability = None;
        match request.capability {
            HubuRequestCapabilityV1::None => {}
            HubuRequestCapabilityV1::Approval => {
                let capability = routing.approval_capability()?;
                builder = builder.header(APPROVAL_CAPABILITY_HEADER, capability.expose());
                used_approval_capability = Some(capability);
            }
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
            let body = redact_backend_value(
                body,
                &self.bearer_token,
                routing.approval_capability.as_ref(),
                used_approval_capability.as_ref(),
                routing.reconciliation_capability.as_ref(),
                used_reconciliation_capability.as_ref(),
            );
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
                details: body.get("details").cloned(),
                retry_guidance: body.get("retry_guidance").cloned(),
            }
            .into());
        }
        Ok(body)
    }
}

pub(super) fn is_approved_http_route(method: &str, path: &str) -> bool {
    let (path, has_query) = path
        .split_once('?')
        .map_or((path, false), |(path, _)| (path, true));
    !has_query && matches!(method, "GET" | "POST") && is_budget_versions_route(path)
        || matches!(
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
                | ("POST", "/user/spending-target")
                | ("POST", "/user/spending-target/revoke")
                | ("GET", "/user/spending-target")
                | ("POST", "/spend")
                | ("POST", "/spend/authorize")
                | ("GET", "/spend/approval")
                | ("POST", "/spend/approval/resolve")
                | ("GET", "/agents")
                | ("GET", "/budgets")
                | ("GET", "/ledger")
                | ("GET", "/spend/executor/claim")
                | ("GET", "/spend/executor/reconciliation")
                | ("POST", "/spend/executor/settle")
                | ("POST", "/spend/executor/release")
        )
}

fn is_budget_versions_route(path: &str) -> bool {
    let Some(remainder) = path.strip_prefix("/budgets/") else {
        return false;
    };
    let Some(budget_id) = remainder.strip_suffix("/versions") else {
        return false;
    };
    !budget_id.is_empty()
        && !budget_id.contains('/')
        && validate_public_budget_id(budget_id).is_ok()
}
