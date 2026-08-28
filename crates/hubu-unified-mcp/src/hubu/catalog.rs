use serde_json::{json, Value};

use crate::{BackendOwner, DOMAIN_TOOLS};

const HUBU_APPROVAL_PROFILE_VERSION: &str = "hubu-mcp-client-approval-v1";

pub(crate) fn is_approved_tool(name: &str) -> bool {
    DOMAIN_TOOLS
        .iter()
        .any(|(candidate, owner)| *owner == BackendOwner::Hubu && *candidate == name)
}

pub(crate) fn tool_definitions() -> Vec<Value> {
    all_tool_definitions()
        .into_iter()
        .filter(|tool| tool["name"].as_str().is_some_and(is_approved_tool))
        .collect()
}

fn all_tool_definitions() -> Vec<Value> {
    vec![
        read_tool(
            "hubu_health",
            "Check whether the local Hubu server is reachable.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_registration_guidance",
            "Read compact agent registration guidance.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_client_approval_profile",
            "Read Hubu's generic MCP client approval profile for configuring agent harnesses.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_users",
            "List registered human users and public user ids.",
            json_schema(json!({})),
        ),
        approval_tool(
            "hubu_register_human",
            "Register or select the active human user. Requires a human click.",
            json_schema(json!({
                "username": { "type": "string" },
                "display_name": { "type": "string" },
                "email": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_register_agent",
            "Register an agent for an explicit Hubu user. Requires a human click.",
            json_schema(json!({
                "owner_user_id": { "type": "string" },
                "name": { "type": "string" },
                "version": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_add_policy",
            "Compatibility alias that declaratively applies and assigns a spending policy. Requires a human click.",
            json_schema(json!({
                "policy_yaml": { "type": "string" },
                "daily_limit_cents": { "type": "integer" }
            })),
        ),
        approval_tool(
            "hubu_apply_policy",
            "Declaratively reconcile a policy resource and assignment with optional compare-and-set. Requires a human click.",
            json_schema_required(json!({
                "policy_yaml": { "type": "string" },
                "declarative_key": { "type": "string" },
                "display_name": { "type": "string" },
                "agent_id": { "type": "string" },
                "expected_revision": { "type": "integer" },
                "expected_hash": { "type": "string" }
            }), &["policy_yaml"]),
        ),
        read_tool(
            "hubu_show_policy",
            "Show complete current policy content and every assignment without database access.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_export_policy",
            "Export the complete current policy as YAML with resource metadata and assignments.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_policy_history",
            "Inspect immutable policy revisions, payload hashes, actors, sources, and timestamps.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_policy_diff",
            "Compare two immutable policy revisions; to_revision defaults to current.",
            json_schema_required(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" },
                "from_revision": { "type": "integer" },
                "to_revision": { "type": "integer" }
            }), &["from_revision"]),
        ),
        approval_tool(
            "hubu_create_budget",
            "Create a budget owned by one agent. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "agent_id": { "type": "string" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_create_recurring_budget",
            "Create a recurring budget series owned by one agent. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "agent_id": { "type": "string" },
                "recurrence": {
                    "type": "string",
                    "enum": ["daily", "monthly", "yearly"]
                },
                "period_count": { "type": "integer" },
                "starting_at": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_budget",
            "Revoke an active budget. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_replace_budget",
            "Replace an active budget with a new forward-looking allowance. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" },
                "amount_cents": { "type": "integer" }
            })),
        ),
        approval_tool(
            "hubu_set_spending_target",
            "Set an advisory spending target for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_spending_target",
            "Revoke an advisory spending target for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "target_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_show_spending_targets",
            "Show advisory spending targets and current agent budget allocations for the active Hubu user.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        write_tool(
            "hubu_submit_spend",
            "Submit an agent spend request. Trusted harness metadata supplies normalized operation and optional task identity outside model arguments. Returns a public operation handle and decision-aware recovery guidance; a definitive denial is terminal and corrected work requires a new tool call. Human approval is only required when the decision is needs_approval.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "lease_profile": { "type": "string" }
            }), &["account_id", "amount_cents", "reason"]),
        ),
        write_tool(
            "hubu_authorize_spend",
            "Authorize an agent spend request. Trusted harness metadata supplies normalized operation and optional task identity outside model arguments. Returns a public operation handle and decision-aware recovery guidance without exposing the private backend operation key; a definitive denial is terminal and corrected work requires a new tool call.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "lease_profile": { "type": "string" }
            }), &["account_id", "amount_cents", "reason"]),
        ),
        read_tool(
            "hubu_list_agents",
            "List registered agents for the active Hubu user.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_budgets",
            "List active budgets for the active Hubu user.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        read_tool(
            "hubu_list_ledger",
            "List local ledger transactions.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_get_executor_claim",
            "Look up executor claim status, spend scope, hold balance, and reconciliation evidence.",
            json_schema_required(json!({
                "claim_id": { "type": "string" }
            }), &["claim_id"]),
        ),
        read_tool(
            "hubu_list_claims_requiring_reconciliation",
            "List expired executor claims whose budget remains frozen pending human review.",
            json_schema(json!({})),
        ),
        approval_tool(
            "hubu_reconcile_vendor_billed_claim",
            "Confirm after human review that an expired claim was billed and settle its frozen hold. Requires a human click.",
            json_schema_required(json!({
                "claim_id": { "type": "string" },
                "provider_reference": { "type": "string" },
                "evidence": { "type": "string" },
                "receipt": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "actual_vendor_cost": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "amount": { "type": "integer", "minimum": 0 },
                                "scale": { "type": "integer", "minimum": 0, "maximum": 18 },
                                "currency": { "type": "string", "enum": ["usd"] }
                            },
                            "required": ["amount", "scale", "currency"]
                        },
                        "actual_vendor_cost_cents": { "type": "integer", "minimum": 0 },
                        "provider_request_id": { "type": "string" },
                        "price_model_snapshot": {
                            "type": "object",
                            "description": "The complete immutable pricing snapshot captured before provider work."
                        },
                        "artifact_reference": { "type": "string" }
                    },
                    "required": ["provider_request_id", "price_model_snapshot", "artifact_reference"],
                    "oneOf": [
                        { "required": ["actual_vendor_cost"], "not": { "required": ["actual_vendor_cost_cents"] } },
                        { "required": ["actual_vendor_cost_cents"], "not": { "required": ["actual_vendor_cost"] } }
                    ]
                }
            }), &["claim_id", "provider_reference", "evidence", "receipt"]),
        ),
        approval_tool(
            "hubu_reconcile_vendor_did_not_bill_claim",
            "Confirm after human review that an expired claim was not billed and release its frozen hold. Requires a human click.",
            json_schema_required(json!({
                "claim_id": { "type": "string" },
                "provider_reference": { "type": "string" },
                "evidence": { "type": "string" }
            }), &["claim_id", "provider_reference", "evidence"]),
        ),
    ]
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
            "schema_version": {"type":"integer","const":1},
            "provider": {"type":"string","minLength":1},
            "executor": {"type":"string","minLength":1},
            "capability": {"type":"string","minLength":1},
            "billing_merchant": {"type":"string","minLength":1}
        },
        "required": ["schema_version","provider","executor","capability","billing_merchant"]
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
            human_approval: "required",
            client_approval_mode: "prompt_before_call",
            runtime_approval: "client_human_approval_required",
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
            "idempotentHint": false,
            "openWorldHint": true,
            "x_hubu_human_approval": annotations.human_approval,
            "x_hubu_client_approval_mode": annotations.client_approval_mode,
            "x_hubu_runtime_approval": annotations.runtime_approval
        }
    })
}

pub(super) fn approval_profile() -> Value {
    let mut definitions = tool_definitions();
    definitions.push(crate::governed_execution::tool_definition());
    let names_matching = |client_mode: &str, runtime_approval: Option<&str>| {
        definitions
            .iter()
            .filter(|tool| {
                tool["annotations"]["x_hubu_client_approval_mode"] == client_mode
                    && runtime_approval.is_none_or(|runtime| {
                        tool["annotations"]["x_hubu_runtime_approval"] == runtime
                    })
            })
            .map(|tool| tool["name"].clone())
            .collect::<Vec<_>>()
    };
    json!({
        "protocol_version": HUBU_APPROVAL_PROFILE_VERSION,
        "summary": "Configure agent harnesses to auto-call Hubu read and spend tools, prompt before setup/admin tools, and rely on Hubu policy for needs_approval spend outcomes.",
        "client_policy": {
            "auto_approve_tools": names_matching("auto", None),
            "prompt_before_call_tools": names_matching("prompt_before_call", None),
            "hubu_policy_conditional_tools": names_matching("auto", Some("hubu_policy_needs_approval"))
        },
        "response_contract": {
            "needs_approval_field": "requires_human_approval",
            "needs_approval_meaning": "Hubu policy required human review and no payment was executed.",
            "agent_action": "Stop the spend workflow and surface approval_reason plus the structured response to the human."
        },
        "annotation_fields": {
            "client_pre_call": "x_hubu_client_approval_mode",
            "runtime_policy": "x_hubu_runtime_approval",
            "legacy_hubu_field": "x_hubu_human_approval"
        },
        "tools": [
            {
                "names": names_matching("auto", Some("none")),
                "x_hubu_client_approval_mode": "auto",
                "x_hubu_runtime_approval": "none"
            },
            {
                "names": names_matching("auto", Some("hubu_policy_needs_approval")),
                "x_hubu_client_approval_mode": "auto",
                "x_hubu_runtime_approval": "hubu_policy_needs_approval"
            },
            {
                "names": names_matching("prompt_before_call", Some("client_human_approval_required")),
                "x_hubu_client_approval_mode": "prompt_before_call",
                "x_hubu_runtime_approval": "client_human_approval_required"
            }
        ]
    })
}
