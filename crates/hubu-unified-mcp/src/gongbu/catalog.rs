use serde_json::{json, Map, Value};

pub(super) fn tool_definitions() -> Vec<Value> {
    let create_properties = json!({
        "schema_version": {"type":"integer","const":2},
        "spend_auth_token_id": {"type":"string","minLength":1,"maxLength":255},
        "input": {"type":"object"},
        "input_schema_version": {"type":"integer","minimum":1},
        "target_id": {
            "type":"string",
            "pattern":"^gongbu:target:v1:[a-f0-9]{64}$"
        },
        "workload_type": {"type":"string","minLength":1},
        "provider": {"type":"string","minLength":1},
        "adapter": {"type":"string","minLength":1},
        "model": {"type":"string","minLength":1}
    });
    vec![
        json!({"name":"gongbu_list_execution_targets","description":"List the operator-approved execution targets and runtime image options currently selectable by agents. Returned targets omit credentials, endpoints, headers, and provider configuration revisions.","inputSchema":{"type":"object","additionalProperties":false,"properties":{}},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":true}}),
        json!({"name":"gongbu_create_execution","description":"Continue one authorized normalized operation into one Gongbu execution using its opaque continuation identifier and execution intent. Prefer a target_id returned by gongbu_list_execution_targets; the legacy raw target tuple remains accepted for compatibility.","inputSchema":{"type":"object","additionalProperties":false,"required":["schema_version","spend_auth_token_id","input","input_schema_version"],"oneOf":[{"required":["target_id"],"not":{"anyOf":[{"required":["workload_type"]},{"required":["provider"]},{"required":["adapter"]},{"required":["model"]}]}},{"required":["workload_type","provider","adapter","model"],"not":{"required":["target_id"]}}],"properties":create_properties}}),
        json!({"name":"gongbu_get_provider_catalog","description":"Get the production-validated managed provider profile catalog, including frozen pricing, capability policies, and non-network readiness evidence.","inputSchema":{"type":"object","additionalProperties":false,"properties":{}},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}),
        json!({"name":"gongbu_get_execution","description":"Get coarse status and redacted outcome for an execution.","inputSchema":id_schema("execution_id")}),
        json!({"name":"gongbu_get_redaction_attestation","description":"Get Gongbu's execution-bound FLUX redaction attestation containing only versioned safe fingerprints, booleans, and counters.","inputSchema":id_schema("execution_id"),"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}),
        json!({"name":"gongbu_list_artifacts","description":"List portable metadata for an execution's artifacts.","inputSchema":id_schema("execution_id")}),
        json!({"name":"gongbu_get_artifact","description":"Get portable base64 image content and safe metadata for an artifact.","inputSchema":id_schema("artifact_id")}),
    ]
}

pub(super) fn operation_status_definition() -> Value {
    json!({
        "name": "hubu_operation_status",
        "description": "Observe one durable operation by its safe public handle. Pending human approval is synchronized from Hubu before the status is returned. Keep observing accepted nonterminal work instead of replacing it. A definitive denial is terminal; corrected work must be submitted as a new logical operation.",
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "required": ["operation_handle"],
            "properties": {
                "operation_handle": {
                    "type": "string",
                    "minLength": 27,
                    "maxLength": 160,
                    "pattern": "^hubu:public-operation:v1:[a-f0-9]{32}$"
                }
            }
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": true
        }
    })
}

fn id_schema(field: &str) -> Value {
    let mut properties = Map::new();
    properties.insert(
        field.into(),
        json!({"type":"string","minLength":1,"maxLength":255,"pattern":"^[A-Za-z0-9_-]+$"}),
    );
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [field],
        "properties": properties
    })
}
