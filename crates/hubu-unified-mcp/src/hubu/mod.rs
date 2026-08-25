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
use routing::{
    public_spend_result, route_tool_call_v1, tool_result_v1, validate_model_spend_arguments,
};
pub use transport::RoutingConfig;

pub(crate) use catalog::tool_definitions;

pub(super) fn call_tool(server: &Server, id: Value, call: ToolCall) -> Value {
    let snapshot = server.snapshot();
    if let Err(rejection) = tool_availability(&call.name, BackendOwner::Hubu, &snapshot) {
        return backend_error_response(id, &call.name, BackendOwner::Hubu, rejection);
    }
    let Some(client) = server.backends.hubu.as_ref() else {
        return backend_error_response(
            id,
            &call.name,
            BackendOwner::Hubu,
            ToolRejection::Unconfigured,
        );
    };
    let name = call.name;
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
        match server.resolve_harness_operation(&identity, &name, &call.arguments) {
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
        "name": name,
        "arguments": call.arguments,
        "_meta": call.meta
    });
    match route_tool_call_v1(
        params,
        server.hubu_routing.trusted_client_approval,
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
            backend_error_response(id, &name, BackendOwner::Hubu, ToolRejection::Unavailable)
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
