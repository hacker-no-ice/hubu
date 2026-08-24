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
use routing::{route_tool_call_v1, tool_result_v1};
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
        let identity = match crate::operation_registry::NormalizedHarnessIdentity::from_meta(
            call.meta.as_ref(),
        ) {
            Ok(identity) => identity,
            Err(error) => return error_response(id, -32000, &error.to_string()),
        };
        match server.resolve_harness_operation(&identity, &name, &call.arguments) {
            Ok(operation) => Some(operation),
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
        operation,
        |request| client.execute_hubu(request, &server.hubu_routing),
    ) {
        Ok(result) => success_response(id, result),
        Err(error)
            if matches!(
                error.downcast_ref::<ForwardError>(),
                Some(ForwardError::Unavailable)
            ) =>
        {
            server.mark_hubu_unavailable();
            backend_error_response(id, &name, BackendOwner::Hubu, ToolRejection::Unavailable)
        }
        Err(error) => error_response(id, -32000, &error.to_string()),
    }
}

#[cfg(test)]
mod tests;
