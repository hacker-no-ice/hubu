use serde_json::{json, Map, Value};

pub(super) fn tool_definitions() -> Vec<Value> {
    let create_properties = json!({
        "schema_version": {"type":"integer","const":2},
        "spend_auth_token_id": {"type":"string","minLength":1,"maxLength":255},
        "input": {"type":"object"},
        "input_schema_version": {"type":"integer","minimum":1},
        "workload_type": {"type":"string","minLength":1},
        "provider": {"type":"string","minLength":1},
        "adapter": {"type":"string","minLength":1},
        "model": {"type":"string","minLength":1}
    });
    vec![
        json!({"name":"gongbu_create_execution","description":"Continue one authorized normalized operation into one Gongbu execution using its opaque continuation identifier and execution intent.","inputSchema":{"type":"object","additionalProperties":false,"required":["schema_version","spend_auth_token_id","input","input_schema_version","workload_type","provider","adapter","model"],"properties":create_properties}}),
        json!({"name":"gongbu_get_execution","description":"Get coarse status and redacted outcome for an execution.","inputSchema":id_schema("execution_id")}),
        json!({"name":"gongbu_list_artifacts","description":"List portable metadata for an execution's artifacts.","inputSchema":id_schema("execution_id")}),
        json!({"name":"gongbu_get_artifact","description":"Get portable base64 image content and safe metadata for an artifact.","inputSchema":id_schema("artifact_id")}),
    ]
}

pub(super) fn operation_status_definition() -> Value {
    json!({
        "name": "hubu_operation_status",
        "description": "Observe one durable operation by its safe public handle. Keep observing accepted nonterminal work instead of replacing it. A definitive denial is terminal; corrected work must be submitted as a new logical operation.",
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
            "openWorldHint": false
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
