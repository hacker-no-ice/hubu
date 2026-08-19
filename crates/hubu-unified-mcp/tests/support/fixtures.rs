use serde_json::json;
use std::collections::HashMap;

use hubu_unified_mcp::{
    product_version, source_commit, EXECUTOR_CONTRACT_VERSION, MCP_PROTOCOL_VERSION,
};

use super::backend::{BackendKind, StubResponse};

pub(super) fn default_responses(kind: BackendKind) -> HashMap<(String, String), StubResponse> {
    let mut responses = HashMap::new();
    let mut insert = |method: &str, path: &str, response: StubResponse| {
        responses.insert((method.to_owned(), path.to_owned()), response);
    };
    match kind {
        BackendKind::Hubu => {
            insert(
                "GET",
                "/health",
                StubResponse::json(200, json!({"status":"ok"})),
            );
            insert(
                "GET",
                "/version",
                StubResponse::json(
                    200,
                    json!({
                        "product_version": product_version(),
                        "source_commit": source_commit(),
                        "executor_contract": EXECUTOR_CONTRACT_VERSION
                    }),
                ),
            );
            insert(
                "GET",
                "/budgets",
                StubResponse::json(200, json!({"budgets":[{"budget_id":"hubu-state-marker"}]})),
            );
            insert(
                "POST",
                "/spend/authorize",
                StubResponse::json(
                    200,
                    json!({
                        "decision":"allow",
                        "spend_auth_token_id":"hubu-spend-token-93",
                        "requires_human_approval":false
                    }),
                ),
            );
        }
        BackendKind::Gongbu => {
            insert(
                "GET",
                "/livez",
                StubResponse::json(200, json!({"status":"live"})),
            );
            insert(
                "GET",
                "/readyz",
                StubResponse::json(200, json!({"status":"ready"})),
            );
            insert(
                "GET",
                "/version",
                StubResponse::json(
                    200,
                    json!({
                        "product_version": product_version(),
                        "source_commit": source_commit(),
                        "api_schema_version":2,
                        "mcp_schema_version":2,
                        "mcp_protocol_version":MCP_PROTOCOL_VERSION,
                        "hubu_executor_contract":EXECUTOR_CONTRACT_VERSION
                    }),
                ),
            );
            let execution = json!({
                "schema_version":2,
                "execution_id":"exec-93",
                "operation_key":"operation-93",
                "status":"succeeded",
                "outcome":"gongbu-state-marker",
                "failure":null,
                "authorization":{"amount_minor":25,"currency":"USD"},
                "created_at":"2026-08-18T00:00:00Z",
                "updated_at":"2026-08-18T00:00:01Z",
                "started_at":"2026-08-18T00:00:00Z",
                "completed_at":"2026-08-18T00:00:01Z"
            });
            insert(
                "GET",
                "/v1/executions/exec-93",
                StubResponse::json(200, execution.clone()),
            );
            insert("POST", "/v2/executions", StubResponse::json(200, execution));
        }
    }
    responses
}
