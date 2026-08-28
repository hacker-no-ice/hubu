mod catalog;
mod response;
mod routing;
mod transport;

use serde_json::{json, Value};

use crate::{
    backend_error_response, error_response, success_response, tool_availability, BackendOwner,
    Server, ToolCall, ToolRejection,
};

use response::ForwardError;
pub(crate) use routing::{denied_retry_guidance, DENIED_OPERATION_GUIDANCE};
use routing::{
    public_spend_result, route_tool_call_v1, tool_result_v1, validate_model_spend_arguments,
};
pub use transport::RoutingConfig;

pub(crate) use catalog::{execution_scope_input_schema, tool_definitions};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct ResumeSpendFailure {
    authorization_expired: bool,
    message: String,
}

pub(crate) fn is_expired_resume_failure(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ResumeSpendFailure>()
        .is_some_and(|failure| failure.authorization_expired)
}

pub(super) fn call_tool(server: &Server, id: Value, call: ToolCall) -> Value {
    call_tool_with_operation_identity(server, id, call, None)
}

pub(super) fn call_governed_authorization(
    server: &Server,
    id: Value,
    authorization: Value,
    canonical_request: &Value,
    meta: Option<Value>,
) -> Value {
    call_tool_with_operation_identity(
        server,
        id,
        ToolCall {
            name: "hubu_authorize_spend".into(),
            arguments: authorization,
            meta,
        },
        Some((crate::governed_execution::TOOL_NAME, canonical_request)),
    )
}

/// Dispatch a previously registered spend operation without allocating, marking, or
/// persisting registry state. The caller must atomically complete the resume plan.
pub(crate) fn dispatch_resolved_spend(
    server: &Server,
    tool_name: &str,
    arguments: Value,
    operation: &crate::operation_registry::OperationResolution,
) -> anyhow::Result<Value> {
    if !matches!(tool_name, "hubu_authorize_spend" | "hubu_submit_spend") {
        anyhow::bail!("unsupported resumed Hubu spend route");
    }
    validate_model_spend_arguments(&arguments)?;
    let snapshot = server.snapshot();
    if let Err(rejection) = tool_availability(tool_name, BackendOwner::Hubu, &snapshot) {
        anyhow::bail!(
            "resumed Hubu spend route is unavailable: {}",
            rejection.reason_code()
        );
    }
    let client = server
        .backends
        .hubu
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("resumed Hubu spend route is unavailable"))?;
    let params = json!({
        "name": tool_name,
        "arguments": arguments,
    });
    match route_tool_call_v1(
        params,
        server.hubu_routing.trusted_client_approval,
        server.hubu_routing.trusted_spend_approval,
        Some(operation.clone()),
        |request| client.execute_hubu(request, &server.hubu_routing),
    ) {
        Ok(result) => {
            let response = result
                .get("structuredContent")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(public_spend_result(
                response,
                &operation.operation_handle,
                operation.operation_key.as_deref(),
            ))
        }
        Err(error) => {
            if matches!(
                error.downcast_ref::<ForwardError>(),
                Some(ForwardError::Unavailable)
            ) {
                server.mark_hubu_unavailable();
            }
            let ambiguous = matches!(
                error.downcast_ref::<ForwardError>(),
                Some(ForwardError::AmbiguousTransport | ForwardError::InvalidResponse)
            );
            let authorization_expired = matches!(
                error.downcast_ref::<ForwardError>(),
                Some(ForwardError::Application {
                    status: 400,
                    error_code: Some(error_code),
                    ..
                }) if error_code == "spend_auth_token_expired"
            );
            Err(ResumeSpendFailure {
                authorization_expired,
                message: resume_failure_message(&error.to_string(), operation, ambiguous),
            }
            .into())
        }
    }
}

fn resume_failure_message(
    message: &str,
    operation: &crate::operation_registry::OperationResolution,
    ambiguous: bool,
) -> String {
    let message = operation.operation_key.as_deref().map_or_else(
        || message.to_string(),
        |operation_key| message.replace(operation_key, "<private operation redacted>"),
    );
    if ambiguous {
        format!(
            "{message}. Operation handle: {}. Retry hubu_resume_operation with this same public handle; do not submit a replacement operation",
            operation.operation_handle
        )
    } else {
        message
    }
}

fn call_tool_with_operation_identity(
    server: &Server,
    id: Value,
    call: ToolCall,
    operation_identity: Option<(&str, &Value)>,
) -> Value {
    let snapshot = server.snapshot();
    if let Err(rejection) = tool_availability(&call.name, BackendOwner::Hubu, &snapshot) {
        let public_name = operation_identity.map_or(call.name.as_str(), |(name, _)| name);
        return backend_error_response(id, public_name, BackendOwner::Hubu, rejection);
    }
    let Some(client) = server.backends.hubu.as_ref() else {
        let public_name = operation_identity.map_or(call.name.as_str(), |(name, _)| name);
        return backend_error_response(
            id,
            public_name,
            BackendOwner::Hubu,
            ToolRejection::Unconfigured,
        );
    };
    let name = call.name;
    let public_name = operation_identity.map_or(name.as_str(), |(name, _)| name);
    if name == "hubu_client_approval_profile"
        && call
            .arguments
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
    {
        return success_response(id, tool_result_v1(catalog::approval_profile()));
    }
    let operation = if matches!(name.as_str(), "hubu_submit_spend" | "hubu_authorize_spend") {
        if let Err(error) = validate_model_spend_arguments(&call.arguments) {
            return error_response(id, -32000, &error.to_string());
        }
        let identity = match crate::operation_registry::NormalizedHarnessIdentity::from_meta(
            call.meta.as_ref(),
        ) {
            Ok(identity) => identity,
            Err(error) => return error_response(id, -32000, &error.to_string()),
        };
        let (identity_name, identity_arguments) =
            operation_identity.unwrap_or((name.as_str(), &call.arguments));
        match server.resolve_harness_operation(&identity, identity_name, identity_arguments) {
            Ok(operation) => {
                if let Some(result) = operation.recorded_result.clone() {
                    return success_response(id, tool_result_v1(result));
                }
                match server.mark_harness_operation_dispatch_started(&operation.operation_handle) {
                    Ok(Some(result)) => return success_response(id, tool_result_v1(result)),
                    Ok(None) => {}
                    Err(error) => return error_response(id, -32000, &error.to_string()),
                }
                Some(operation)
            }
            Err(error) => return error_response(id, -32000, &error.to_string()),
        }
    } else {
        None
    };
    let params = json!({
        "name": &name,
        "arguments": call.arguments,
        "_meta": call.meta
    });
    match route_tool_call_v1(
        params,
        server.hubu_routing.trusted_client_approval,
        server.hubu_routing.trusted_spend_approval,
        operation.clone(),
        |request| client.execute_hubu(request, &server.hubu_routing),
    ) {
        Ok(result) => {
            if let Some(operation) = operation.as_ref() {
                let response = result
                    .get("structuredContent")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let response = public_spend_result(
                    response,
                    &operation.operation_handle,
                    operation.operation_key.as_deref(),
                );
                return match server
                    .record_harness_operation_result(&operation.operation_handle, &response)
                {
                    Ok(authoritative_response) => {
                        success_response(id, tool_result_v1(authoritative_response))
                    }
                    Err(error) => {
                        let message =
                            operation_failure_message(&error.to_string(), operation, true);
                        error_response(id, -32000, &message)
                    }
                };
            }
            success_response(id, result)
        }
        Err(error)
            if matches!(
                error.downcast_ref::<ForwardError>(),
                Some(ForwardError::Unavailable)
            ) =>
        {
            server.mark_hubu_unavailable();
            backend_error_response(
                id,
                public_name,
                BackendOwner::Hubu,
                ToolRejection::Unavailable,
            )
        }
        Err(error) => {
            let message = operation.as_ref().map_or_else(
                || error.to_string(),
                |operation| {
                    operation_failure_message(
                        &error.to_string(),
                        operation,
                        matches!(
                            error.downcast_ref::<ForwardError>(),
                            Some(ForwardError::AmbiguousTransport | ForwardError::InvalidResponse)
                        ),
                    )
                },
            );
            error_response(id, -32000, &message)
        }
    }
}

fn operation_failure_message(
    message: &str,
    operation: &crate::operation_registry::OperationResolution,
    ambiguous: bool,
) -> String {
    let message = operation.operation_key.as_deref().map_or_else(
        || message.to_string(),
        |operation_key| message.replace(operation_key, "<private operation redacted>"),
    );
    if ambiguous {
        format!(
            "{message}. Operation handle: {}. Redeliver this exact harness call with the same call identity; do not submit a replacement spend call",
            operation.operation_handle
        )
    } else {
        message
    }
}

#[cfg(test)]
mod tests;
