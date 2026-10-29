use serde_json::{json, Value};

use crate::{BackendOwner, DOMAIN_TOOLS};

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
            "hubu_registration_guidance",
            "Read compact agent registration guidance.",
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
            "Show complete current policy content and every assignment; optionally include canonical YAML.",
            json_schema(json!({
                "include_yaml": { "type": "boolean", "default": false },
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
            "Create a budget owned by one agent. Budgets are per currency: spend only counts against a budget in its own currency. Requires a human click.",
            json_schema(json!({
                "amount": spend_amount_input_schema(),
                "amount_cents": {
                    "type": "integer",
                    "description": "Deprecated whole-cent limit; send `amount` instead, never both."
                },
                "currency": { "type": "string", "enum": ["usd", "usdc"] },
                "agent_id": { "type": "string" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_budget",
            "Administratively revoke a budget so it cannot reserve future spend. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" }
            })),
        ),
        idempotent_approval_tool(
            "hubu_update_budget",
            "Update the total cap of one stable logical budget from an explicitly pinned revision. The amount is the complete cap, not an added allowance. Requires a human click.",
            json_schema_required(json!({
                "budget_id": { "type": "string", "minLength": 1 },
                "expected_revision": { "type": "integer", "minimum": 1 },
                "amount_limit": spend_amount_input_schema(),
                "amount_limit_cents": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Deprecated whole-cent cap; send `amount_limit` instead, never both."
                },
                "reason": { "type": "string" }
            }), &["budget_id", "expected_revision"]),
        ),
        read_tool(
            "hubu_budget_history",
            "Read one stable logical budget and its immutable versions in ascending revision order.",
            json_schema_required(json!({
                "budget_id": { "type": "string", "minLength": 1 }
            }), &["budget_id"]),
        ),
        agent_write_tool(
            "hubu_open_task_budget",
            "Propose a task budget for one task, such as \"design this landing page for $3\". A human must approve it on the returned approval_request_id before it opens; until then it reserves nothing and this session's spend is denied. Once open, it reserves the amount and caps this session's spend; each request still goes through the normal approval policy. Repeating the same request returns the pending proposal or the open budget. Ask to close it when the task is done.",
            json_schema_required(json!({
                "agent_id": { "type": "string", "minLength": 1 },
                "task_id": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 512,
                    "description": "Short stable name for the task, such as landing-page"
                },
                "amount": spend_amount_input_schema(),
                "currency": spend_currency_input_schema(),
                "reason": { "type": "string" }
            }), &["agent_id", "task_id", "amount"]),
        ),
        agent_write_tool(
            "hubu_close_task_budget",
            "Ask a human to close a task budget when you believe its task is done. The human checks the work and approves or rejects the returned approval_request_id. While the request is pending, new spend in this session is refused; outstanding authorizations can still settle. Approval returns the unused amount to the agent budget; rejection keeps the task budget open.",
            json_schema_required(json!({
                "task_budget_id": { "type": "string", "minLength": 1 },
                "reason": {
                    "type": "string",
                    "description": "What was done, for the human who checks the task"
                }
            }), &["task_budget_id"]),
        ),
        read_tool(
            "hubu_list_task_budgets",
            "List open task budgets with spend so far (settled plus frozen) and remaining, plus proposals and close requests waiting for a human; include_all adds closed ones.",
            json_schema(json!({
                "agent_id": { "type": "string" },
                "include_all": { "type": "boolean", "default": false }
            })),
        ),
        approval_tool(
            "hubu_set_spending_target",
            "Set an advisory spending target for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "amount": spend_amount_input_schema(),
                "amount_cents": {
                    "type": "integer",
                    "description": "Deprecated whole-cent amount; send `amount` instead, never both."
                },
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
            "Submit a mock-only spend request for demos and recover exact retries of that mock operation. Does not execute a real provider payment. Trusted harness metadata supplies normalized operation and optional task identity outside model arguments. Returns a public operation handle and decision-aware recovery guidance; a definitive denial is terminal and corrected work requires a new tool call. Human approval is only required when the decision is needs_approval.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount": spend_amount_input_schema(),
                "amount_cents": {
                    "type": "integer",
                    "description": "Deprecated whole-cent amount; send `amount` instead, never both."
                },
                "currency": spend_currency_input_schema(),
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "lease_profile": { "type": "string" }
            }), &["account_id", "reason"]),
        ),
        write_tool(
            "hubu_authorize_spend",
            "Authorize an agent spend request. Trusted harness metadata supplies normalized operation and optional task identity outside model arguments. Returns a public operation handle and decision-aware recovery guidance without exposing the private backend operation key; a definitive denial is terminal and corrected work requires a new tool call.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount": spend_amount_input_schema(),
                "amount_cents": {
                    "type": "integer",
                    "description": "Deprecated whole-cent amount; send `amount` instead, never both."
                },
                "currency": spend_currency_input_schema(),
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "lease_profile": { "type": "string" }
            }), &["account_id", "reason"]),
        ),
        read_tool(
            "hubu_get_spend_approval",
            "Get the durable state and immutable human review payload for one spend approval request. Spend approval covers spend only; review a tba_ task budget proposal or close request with hubu_get_task_budget_approval.",
            json_schema_required(json!({
                "approval_request_id": { "type": "string" }
            }), &["approval_request_id"]),
        ),
        approval_resolution_tool(
            "hubu_resolve_spend_approval",
            "Resolve a pending spend approval request after the human explicitly chooses approve or deny in chat. The native client prompt confirms or cancels this formed resolver call; cancelling does not submit a denial. Spend approval covers spend only; tba_ task budget requests use hubu_resolve_task_budget_approval.",
            json_schema_required(json!({
                "approval_request_id": { "type": "string" },
                "decision": { "type": "string", "enum": ["approve", "deny"] }
            }), &["approval_request_id", "decision"]),
        ),
        read_tool(
            "hubu_get_task_budget_approval",
            "Get one tba_ task budget proposal or close request and its human review. Its summary says what approving does: opening reserves part of the agent budget for a task and closing returns the unused amount. A task budget request never authorizes spend.",
            json_schema_required(json!({
                "approval_request_id": { "type": "string" }
            }), &["approval_request_id"]),
        ),
        approval_resolution_tool(
            "hubu_resolve_task_budget_approval",
            "Resolve a pending tba_ task budget proposal or close request after the human explicitly chooses approve or deny in chat. Approving a proposal opens the task budget; approving a close request closes it and returns the unused amount, so the human should check the task is done first. It never authorizes spend. The native client prompt confirms or cancels this formed resolver call; cancelling does not submit a denial.",
            json_schema_required(json!({
                "approval_request_id": { "type": "string" },
                "decision": { "type": "string", "enum": ["approve", "deny"] }
            }), &["approval_request_id", "decision"]),
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
            "hubu_governance_summary",
            "Read governance counts, exact requested amounts per currency, coverage and snapshot backlog for the active owner. For 'past week', send explicit UTC occurred_from/occurred_before spanning the preceding seven days and state the actual returned bounds. Distinguish automatic_deny from human_declined. Lifecycle stages overlap; do not add their counts. Backlog includes older unresolved work regardless of period/stage filters. Qualify incomplete or unsupported history, including empty results; blocked requested amounts are not cost or savings.",
            super::governance::input_schema("hubu_governance_summary"),
        ),
        read_tool(
            "hubu_list_governance_records",
            "List supporting governance lifecycle facts with the same complete-filter summary, exact amounts and coverage. Reuse summary.snapshot.token with the same filters; follow next_cursor unchanged. Snapshot backlog includes pre-period unresolved work. Automatic denials and human declines are distinct; stages overlap and incomplete history cannot prove zero activity. This is a read; it does not approve, execute, retry or reconcile spending.",
            super::governance::input_schema("hubu_list_governance_records"),
        ),
        read_tool(
            "hubu_get_governance_record",
            "Inspect one public governance record and its complete-filter summary. Supply only its public lev_ record_id, plus the same filters and snapshot token used for discovery. Preserve evidence/coverage gaps and exact money strings. Claim acceptance does not prove provider work started; unsupported provider failure or pre-expiry uncertainty is not a zero count. This read has no spending or reconciliation side effects.",
            super::governance::input_schema("hubu_get_governance_record"),
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
                                "currency": { "type": "string", "enum": ["usd", "usdc"] }
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
        "description": "Exact decimal amount in the spend currency, at most 6 decimal places (for example \"0.005\"). Provide `amount` or the deprecated `amount_cents`, not both."
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

/// A write an agent may make without a human click: it only re-partitions
/// the agent's own budget and never authorizes spend.
fn agent_write_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            human_approval: "none",
            client_approval_mode: "auto",
            runtime_approval: "none",
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

fn idempotent_approval_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: true,
            human_approval: "required",
            client_approval_mode: "prompt_before_call",
            runtime_approval: "client_human_approval_required",
        },
    )
}

fn approval_resolution_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: true,
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
            "idempotentHint": annotations.idempotent,
            "openWorldHint": true,
            "x_hubu_human_approval": annotations.human_approval,
            "x_hubu_client_approval_mode": annotations.client_approval_mode,
            "x_hubu_runtime_approval": annotations.runtime_approval
        }
    })
}
