use serde_json::{json, Value};

pub(crate) fn tool_definitions() -> Vec<Value> {
    vec![
        read_tool(
            "hubu_registration_guidance",
            "Read compact agent registration guidance.",
            json_schema(json!({})),
        ),
        approval_tool(
            "hubu_register_agent",
            "Submit the reviewed agent registration envelope from hubu_registration_guidance, or register by name and version for the active Hubu user.",
            json!({
                "oneOf": [
                    json_schema_required(json!({
                        "name": { "type": "string" },
                        "version": { "type": "string" }
                    }), &["name", "version"]),
                    json_schema_required(json!({
                        "protocol_version": { "type": "string" },
                        "identity": registration_payload_schema(),
                        "version": registration_payload_schema(),
                        "review": { "type": "object" },
                        "signature": {}
                    }), &["protocol_version", "identity", "version"])
                ],
                "type": "object"
            }),
        ),
        read_tool(
            "hubu_show_policy",
            "Show the current policy content and assignments.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        write_tool(
            "hubu_submit_spend",
            "Submit a mock-only spend request for demos and recover exact retries of that mock operation. Does not execute a real provider payment. Trusted harness metadata supplies normalized operation and optional task identity outside model arguments. Returns a public operation handle and decision-aware recovery guidance; a definitive denial is terminal and corrected work requires a new tool call. Human approval is only required when the decision is needs_approval.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount": spend_amount_input_schema(),
                "currency": spend_currency_input_schema(),
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "lease_profile": { "type": "string" }
            }), &["account_id", "amount", "reason"]),
        ),
        write_tool(
            "hubu_authorize_spend",
            "Authorize an agent spend request. Trusted harness metadata supplies normalized operation and optional task identity outside model arguments. Returns a public operation handle and decision-aware recovery guidance without exposing the private backend operation key; a definitive denial is terminal and corrected work requires a new tool call.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount": spend_amount_input_schema(),
                "currency": spend_currency_input_schema(),
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "lease_profile": { "type": "string" }
            }), &["account_id", "amount", "reason"]),
        ),
        read_tool(
            "hubu_get_spend_approval",
            "Get the durable pending, approved, or denied state and immutable human review payload for one spend approval request.",
            json_schema_required(json!({
                "approval_request_id": { "type": "string" }
            }), &["approval_request_id"]),
        ),
        approval_prompt_tool(
            "hubu_prompt_spend_approval",
            "Raise the server’s confirmation dialog for a pending spend approval. Call only after the owner says to approve in the conversation. The owner confirms on the Mac.",
            json_schema_required(json!({
                "approval_request_id": { "type": "string" }
            }), &["approval_request_id"]),
        ),
        read_tool(
            "hubu_list_agents",
            "List registered agents for the active Hubu user.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_budgets",
            "List scheduled, active, and exhausted budgets for the active Hubu user. Set include_all to include expired and revoked budgets.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        read_tool(
            "hubu_list_ledger",
            "List the active user's ledger transactions with exact costs and corrections. A budget filter requires agent_id; coverage describes gaps across the complete budget, independent of pagination.",
            json_schema(json!({
                "agent_id": { "type": "string" }, "account_id": { "type": "string" },
                "budget_id": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                "cursor": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_list_authorization_records",
            "List authorization records and their current lifecycle state. Follow next_cursor to retrieve subsequent pages.",
            json_schema(json!({
                "agent_id": { "type": "string" }, "account_id": { "type": "string" },
                "status": { "type": "string", "enum": ["authorized", "unknown", "needs_approval", "claimed", "settled", "released", "expired", "reconciliation_required"] },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }, "cursor": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_get_authorization_record",
            "Inspect one authorization record by its public authorization_id, including safe lifecycle and accounting references.",
            json_schema_required(json!({ "authorization_id": { "type": "string" } }), &["authorization_id"]),
        ),
    ]
}

fn registration_payload_schema() -> Value {
    json_schema_required(
        json!({
            "payload": { "type": "object" },
            "fingerprint": { "type": "string" }
        }),
        &["payload", "fingerprint"],
    )
}

fn json_schema(properties: Value) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false
    })
}

pub(crate) fn execution_scope_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "schema_version": {
                "type":"integer",
                "enum":[1,2],
                "description":"1 for target identities only; 2 also binds the execution target's model and image_size so policy can tier by them"
            },
            "provider": {"type":"string","minLength":1},
            "executor": {"type":"string","minLength":1},
            "capability": {"type":"string","minLength":1},
            "billing_merchant": {"type":"string","minLength":1},
            "model": {
                "type":"string",
                "pattern":"^[A-Za-z0-9._:/-]{1,128}$",
                "description":"Schema version 2 only (required there): the selected execution target's model, exactly as listed by gongbu_list_execution_targets"
            },
            "image_size": {
                "type":"string",
                "pattern":"^[a-z0-9._-]{1,32}$",
                "description":"Schema version 2 only: exactly the execution input's image_size; Gongbu rejects a mismatch"
            }
        },
        "required": ["schema_version","provider","executor","capability","billing_merchant"]
    })
}

/// Exact decimal spend amount in the spend currency.
pub(crate) fn spend_amount_input_schema() -> Value {
    json!({
        "type": "string",
        "pattern": "^\\d+(\\.\\d{1,6})?$",
        "description": "Exact decimal amount in the spend currency, at most 6 decimal places (for example \"0.005\")."
    })
}

/// Budgets are per currency: spend only counts against a budget in its own
/// currency.
pub(crate) fn spend_currency_input_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["usd", "usdc"],
        "description": "Spend currency (default usd). Spend only counts against a budget in this currency."
    })
}

fn json_schema_required(properties: Value, required: &[&str]) -> Value {
    let mut schema = json_schema(properties);
    schema["required"] = json!(required);
    schema
}

struct ToolAnnotations {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    human_approval: &'static str,
    client_approval_mode: &'static str,
    runtime_approval: &'static str,
}

fn read_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            human_approval: "none",
            client_approval_mode: "auto",
            runtime_approval: "none",
        },
    )
}

fn write_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: false,
            human_approval: "conditional",
            client_approval_mode: "auto",
            runtime_approval: "hubu_policy_needs_approval",
        },
    )
}

fn approval_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            human_approval: "required",
            client_approval_mode: "prompt_before_call",
            runtime_approval: "client_human_approval_required",
        },
    )
}

fn approval_prompt_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            human_approval: "required",
            client_approval_mode: "auto",
            runtime_approval: "hubu_owner_presence_required",
        },
    )
}

fn tool(name: &str, description: &str, input_schema: Value, annotations: ToolAnnotations) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": annotations.read_only,
            "destructiveHint": annotations.destructive,
            "idempotentHint": annotations.idempotent,
            "openWorldHint": true,
            "x_hubu_human_approval": annotations.human_approval,
            "x_hubu_client_approval_mode": annotations.client_approval_mode,
            "x_hubu_runtime_approval": annotations.runtime_approval
        }
    })
}
