//! Explicit public-handle continuation after a human spend approval.

use serde_json::{json, Value};

use crate::{
    backend_error_response, error_response, hubu, operation_registry, operation_status_result,
    success_response, tool_availability, BackendOwner, Server, ToolCall,
};

pub(crate) const TOOL_NAME: &str = "hubu_resume_operation";

pub(crate) fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Resume one approved operation by its public handle. The router replays only the immutable intent stored before human review; callers cannot replace the approved scope. Repeated calls observe or continue the same operation and never create a second logical operation.",
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "required": ["operation_handle"],
            "properties": {
                "operation_handle": {
                    "type": "string",
                    "minLength": 57,
                    "maxLength": 57,
                    "pattern": "^hubu:public-operation:v1:[a-f0-9]{32}$"
                }
            }
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": true,
            "x_hubu_human_approval": "already_resolved",
            "x_hubu_client_approval_mode": "auto",
            "x_hubu_runtime_approval": "none"
        }
    })
}

pub(crate) fn call_tool(server: &Server, id: Value, call: ToolCall) -> Value {
    if !server.operation_registry_available() {
        return error_response(
            id,
            -32000,
            "operation resume requires an available durable operation registry",
        );
    }
    let Some(arguments) = call.arguments.as_object() else {
        return error_response(id, -32602, "Invalid params");
    };
    if arguments.len() != 1 {
        return error_response(id, -32602, "Invalid params");
    }
    let Some(operation_handle) = arguments.get("operation_handle").and_then(Value::as_str) else {
        return error_response(id, -32602, "Invalid params");
    };

    // A decision made through the CLI or another owner-authorized surface may
    // not yet be reflected locally. This authoritative read never calls
    // Gongbu or a provider and resolution itself never resumes work.
    server.synchronize_pending_approval_for_handle(operation_handle);

    let preparation = match server.prepare_resume(operation_handle) {
        Ok(preparation) => preparation,
        Err(_) => {
            return error_response(
                id,
                -32000,
                "public operation handle is unknown or unavailable",
            )
        }
    };
    let plan = match preparation {
        operation_registry::ResumePreparation::Replay {
            origin,
            authoritative_result,
            status,
        } => {
            return success_response(id, resumed_result(&authoritative_result, &status, origin));
        }
        operation_registry::ResumePreparation::Status(status)
        | operation_registry::ResumePreparation::IntentUnavailable(status) => {
            return success_response(id, operation_status_result(&status));
        }
        operation_registry::ResumePreparation::Dispatch(plan) => plan,
    };

    server.refresh_hubu_capability_if_stale();
    let snapshot = server.snapshot();
    if let Err(rejection) = tool_availability(plan.hubu_tool_name(), BackendOwner::Hubu, &snapshot)
    {
        return backend_error_response(id, TOOL_NAME, BackendOwner::Hubu, rejection);
    }
    if plan.origin == operation_registry::ResumeOrigin::GovernedExecution {
        server.refresh_gongbu_capability_if_stale();
        let snapshot = server.snapshot();
        if let Err(rejection) =
            tool_availability("gongbu_create_execution", BackendOwner::Gongbu, &snapshot)
        {
            return backend_error_response(id, TOOL_NAME, BackendOwner::Gongbu, rejection);
        }
    }

    let hubu_response = hubu::call_resumed_spend(server, id.clone(), &plan);
    if hubu_response.get("error").is_some() {
        return hubu_response;
    }
    let Some(hubu_result) = hubu_response.pointer("/result/structuredContent").cloned() else {
        return error_response(
            id,
            -32000,
            "Hubu returned an invalid resumed operation result",
        );
    };
    let completion = match server.complete_resume(&plan, &hubu_result) {
        Ok(completion) => completion,
        Err(_) => {
            return error_response(
                id,
                -32000,
                &format!(
                    "Hubu completed the resumed mutation, but its durable router outcome may be ambiguous. Retry hubu_resume_operation with operation_handle {}; do not submit a replacement operation",
                    plan.operation.operation_handle
                ),
            );
        }
    };
    if completion.wake_operation_worker {
        server.wake_operation_worker();
    }

    success_response(
        id,
        resumed_result(
            &completion.authoritative_result,
            &completion.status,
            plan.origin,
        ),
    )
}

fn resumed_result(
    authoritative_result: &Value,
    status: &operation_registry::DurableOperationStatus,
    origin: operation_registry::ResumeOrigin,
) -> Value {
    let mut result = operation_status_result(status);
    let structured = result
        .get_mut("structuredContent")
        .and_then(Value::as_object_mut)
        .expect("operation status structured content is an object");
    let hubu_result = if origin == operation_registry::ResumeOrigin::GovernedExecution {
        crate::governed_execution::authorization_projection(authoritative_result)
    } else {
        authoritative_result.clone()
    };
    structured.insert("hubu_result".to_string(), hubu_result);
    result["content"][0]["text"] = Value::String(
        serde_json::to_string_pretty(&result["structuredContent"])
            .expect("resumed operation result serializes"),
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_accepts_only_a_public_operation_handle() {
        let definition = tool_definition();
        assert_eq!(definition["name"], TOOL_NAME);
        assert_eq!(
            definition["inputSchema"]["required"],
            json!(["operation_handle"])
        );
        assert_eq!(
            definition["annotations"]["idempotentHint"],
            Value::Bool(true)
        );
        assert_eq!(
            definition["annotations"]["x_hubu_client_approval_mode"],
            "auto"
        );
    }

    #[test]
    fn governed_replay_hides_continuation_while_direct_replay_recovers_it() {
        let status = operation_registry::DurableOperationStatus {
            operation_handle: "hubu:public-operation:v1:11111111111111111111111111111111".into(),
            state: "authorized".into(),
            execution_id: None,
            result_code: Some("authorization_allowed".into()),
            updated_at: "2026-08-28T00:00:00Z".into(),
        };
        let authoritative = json!({
            "decision":"allow",
            "auth_token_id":"private-continuation",
            "message":"private-continuation"
        });

        let direct = resumed_result(
            &authoritative,
            &status,
            operation_registry::ResumeOrigin::AuthorizeSpend,
        );
        assert_eq!(
            direct["structuredContent"]["hubu_result"]["auth_token_id"],
            "private-continuation"
        );

        let governed = resumed_result(
            &authoritative,
            &status,
            operation_registry::ResumeOrigin::GovernedExecution,
        );
        assert!(!governed.to_string().contains("private-continuation"));
        assert!(governed["structuredContent"]["hubu_result"]
            .get("auth_token_id")
            .is_none());
    }
}
