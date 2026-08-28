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

    let hubu_result = match hubu::dispatch_resolved_spend(
        server,
        plan.hubu_tool_name(),
        plan.hubu_arguments.clone(),
        &plan.operation,
    ) {
        Ok(result) => result,
        Err(error) if hubu::is_expired_resume_failure(&error) => {
            return terminalize_expired_resume(server, id, &plan.operation.operation_handle)
        }
        Err(error) => return error_response(id, -32000, &error.to_string()),
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

fn terminalize_expired_resume(server: &Server, id: Value, operation_handle: &str) -> Value {
    match server.fail_pre_execution_operation(
        operation_handle,
        "authorization_expired_before_resume",
    ) {
        Ok(status) => success_response(id, operation_status_result(&status)),
        Err(_) => error_response(
            id,
            -32000,
            &format!(
                "Hubu rejected an expired authorization, but its terminal router state could not be persisted. Retry hubu_resume_operation with operation_handle {operation_handle}; do not submit a replacement operation"
            ),
        ),
    }
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
    let authorization_expired =
        status.result_code.as_deref() == Some("authorization_expired_before_resume");
    let mut hubu_result =
        if origin == operation_registry::ResumeOrigin::GovernedExecution || authorization_expired {
            crate::governed_execution::authorization_projection(authoritative_result)
        } else {
            authoritative_result.clone()
        };
    if authorization_expired {
        let retry_guidance = structured
            .get("retry_guidance")
            .cloned()
            .expect("expired operation status includes retry guidance");
        if let Some(object) = hubu_result.as_object_mut() {
            object.insert("requires_human_approval".into(), Value::Bool(false));
            object.insert("retry_guidance".into(), retry_guidance);
        }
    }
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
    fn definitive_submit_expiry_terminalizes_the_same_public_handle() {
        let root = tempfile::tempdir().unwrap();
        let server = Server::new(crate::Config {
            operation_state_path: Some(root.path().join("expired-submit.sqlite3")),
            ..crate::Config::default()
        })
        .unwrap();
        let crate::OperationRegistryCapability::Available(registry) =
            server.operation_registry.as_ref()
        else {
            panic!("test registry should be available");
        };
        let operation_handle = {
            let mut registry = registry.lock().unwrap();
            let identity = operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
                "callId":"expired-submit-resume"
            })))
            .unwrap();
            let operation = registry
                .resolve_or_allocate(
                    &identity,
                    "hubu_submit_spend",
                    &json!({"account_id":"account-1","amount_cents":100,"reason":"test"}),
                )
                .unwrap();
            registry
                .record_authorization_result(
                    &operation.operation_handle,
                    &json!({
                        "decision":"needs_approval",
                        "decision_id":"expired-submit-approval",
                        "approval":{
                            "approval_request_id":"expired-submit-approval",
                            "status":"pending"
                        },
                        "operation_handle":operation.operation_handle
                    }),
                )
                .unwrap();
            registry
                .synchronize_approval_status("expired-submit-approval", "approved")
                .unwrap();
            operation.operation_handle
        };

        let response = terminalize_expired_resume(&server, json!(91), &operation_handle);
        assert_eq!(response["result"]["structuredContent"]["state"], "failed");
        assert_eq!(response["result"]["structuredContent"]["terminal"], true);
        assert_eq!(
            response["result"]["structuredContent"]["result"]["code"],
            "authorization_expired_before_resume"
        );
        assert_eq!(
            response["result"]["structuredContent"]["retry_guidance"]["action"],
            "create_new_operation"
        );
        let mut registry = registry.lock().unwrap();
        assert!(matches!(
            registry.prepare_resume(&operation_handle).unwrap(),
            operation_registry::ResumePreparation::Status(status)
                if status.terminal()
                    && status.result_code.as_deref()
                        == Some("authorization_expired_before_resume")
        ));
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

    #[test]
    fn governed_expiry_replaces_stale_backend_retry_guidance() {
        let status = operation_registry::DurableOperationStatus {
            operation_handle: "hubu:public-operation:v1:22222222222222222222222222222222".into(),
            state: "failed".into(),
            execution_id: None,
            result_code: Some("authorization_expired_before_resume".into()),
            updated_at: "2026-08-28T00:00:00Z".into(),
        };
        let authoritative = json!({
            "decision":"allow",
            "requires_human_approval":true,
            "authorization_expires_at":"2020-01-01T00:00:00Z",
            "retry_guidance":{
                "action":"replay_exactly",
                "message":"replay this exact immutable scope"
            }
        });

        let result = resumed_result(
            &authoritative,
            &status,
            operation_registry::ResumeOrigin::GovernedExecution,
        );
        let structured = &result["structuredContent"];
        assert_eq!(structured["terminal"], true);
        assert_eq!(structured["replacement_safe"], true);
        assert_eq!(
            structured["retry_guidance"]["action"],
            "create_new_operation"
        );
        assert_eq!(structured["hubu_result"]["requires_human_approval"], false);
        assert_eq!(
            structured["hubu_result"]["retry_guidance"],
            structured["retry_guidance"]
        );
        assert!(!result.to_string().contains("replay_exactly"));
    }

    #[test]
    fn direct_authorize_expiry_hides_the_token_and_stale_retry_guidance() {
        let status = operation_registry::DurableOperationStatus {
            operation_handle: "hubu:public-operation:v1:33333333333333333333333333333333".into(),
            state: "failed".into(),
            execution_id: None,
            result_code: Some("authorization_expired_before_resume".into()),
            updated_at: "2026-08-28T00:00:00Z".into(),
        };
        let authoritative = json!({
            "decision":"allow",
            "auth_token_id":"expired-direct-token",
            "authorization_expires_at":"2020-01-01T00:00:00Z",
            "reason":"expired-direct-token must not escape",
            "retry_guidance":{
                "action":"replay_exactly",
                "message":"replay expired-direct-token"
            }
        });

        let result = resumed_result(
            &authoritative,
            &status,
            operation_registry::ResumeOrigin::AuthorizeSpend,
        );
        let structured = &result["structuredContent"];
        assert_eq!(structured["terminal"], true);
        assert_eq!(structured["replacement_safe"], true);
        assert_eq!(
            structured["retry_guidance"]["action"],
            "create_new_operation"
        );
        assert_eq!(structured["hubu_result"]["requires_human_approval"], false);
        assert_eq!(
            structured["hubu_result"]["retry_guidance"],
            structured["retry_guidance"]
        );
        assert!(structured["hubu_result"].get("auth_token_id").is_none());
        assert!(!result.to_string().contains("expired-direct-token"));
        assert!(!result.to_string().contains("replay_exactly"));
    }
}
