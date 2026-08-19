mod support;

use serde_json::{json, Value};
use support::{assert_bearer_isolated, tool_names, BackendKind, BackendStub, McpProcess};

const HUBU_TOKEN: &str = "hub93-hubu-credential-canary-4b1778";
const GONGBU_TOKEN: &str = "hub93-gongbu-credential-canary-9e30c1";

fn assert_backend_state(initialize: &Value, owner: &str, expected: &str) {
    assert_eq!(
        initialize["result"]["capabilities"]["experimental"]["hubu.dev/unified-mcp"]["backends"]
            [owner]["state"],
        expected
    );
}

fn execution_arguments() -> Value {
    json!({
        "schema_version":2,
        "spend_auth_token_id":"hubu-spend-token-93",
        "input":{"prompt":"deterministic circle","image_count":1},
        "input_schema_version":1,
        "workload_type":"image_generation",
        "provider":"fixture",
        "adapter":"fixture",
        "model":"fixture-v1"
    })
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn hubu_only_initialize_discovery_and_call() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), None);

    let initialize = mcp.initialize();
    assert_backend_state(&initialize, "hubu", "available");
    assert_backend_state(&initialize, "gongbu", "unconfigured");
    let tools = mcp.list_tools();
    let names = tool_names(&tools);
    assert!(names.contains(&"hubu_list_budgets"));
    assert!(!names.iter().any(|name| name.starts_with("gongbu_")));

    let response = mcp.call(3, "hubu_list_budgets", json!({}));
    assert_eq!(
        response["result"]["structuredContent"]["budgets"][0]["budget_id"],
        "hubu-state-marker"
    );
    assert_bearer_isolated(&hubu, HUBU_TOKEN, GONGBU_TOKEN);
    mcp.finish(&[HUBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn gongbu_only_initialize_discovery_and_read_call() {
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(None, Some((&gongbu, GONGBU_TOKEN)));

    let initialize = mcp.initialize();
    assert_backend_state(&initialize, "hubu", "unconfigured");
    assert_backend_state(&initialize, "gongbu", "available");
    let tools = mcp.list_tools();
    let names = tool_names(&tools);
    assert!(names.contains(&"gongbu_get_execution"));
    assert!(!names.contains(&"gongbu_create_execution"));
    assert!(!names
        .iter()
        .any(|name| name.starts_with("hubu_") && *name != "hubu_unified_capabilities"));

    let response = mcp.call(3, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
    let body: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "gongbu-state-marker");
    assert_bearer_isolated(&gongbu, GONGBU_TOKEN, HUBU_TOKEN);
    mcp.finish(&[GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn governed_hubu_to_gongbu_execution_fails_closed_without_hubu() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));

    let initialize = mcp.initialize();
    let capability =
        &initialize["result"]["capabilities"]["experimental"]["hubu.dev/unified-mcp"]["backends"];
    assert_eq!(
        capability["hubu"]["state"],
        "available",
        "unexpected initialization {initialize}; Hubu requests: {:?}; Gongbu requests: {:?}",
        hubu.requests(),
        gongbu.requests()
    );
    assert_eq!(
        capability["gongbu"]["state"],
        "available",
        "unexpected initialization {initialize}; Hubu requests: {:?}; Gongbu requests: {:?}",
        hubu.requests(),
        gongbu.requests()
    );
    let listed = mcp.list_tools();
    assert_eq!(
        tool_names(&listed).len(),
        33,
        "unexpected catalog {listed}; Hubu requests: {:?}; Gongbu requests: {:?}",
        hubu.requests(),
        gongbu.requests()
    );

    let authorized = mcp.call_with_meta(
        3,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"HUB-93 governed fixture"}),
        json!({"hubu.dev/platform-invocation":{
            "platform":"codex",
            "installation_id":"installation-93",
            "invocation_id":"invocation-93",
            "operation_key":"operation-93",
            "task_id":"linear:HUB-93"
        }}),
    );
    assert_eq!(
        authorized["result"]["structuredContent"]["spend_auth_token_id"],
        "hubu-spend-token-93"
    );
    let authorization_request = hubu
        .requests()
        .into_iter()
        .find(|request| request.path == "/spend/authorize")
        .unwrap();
    assert!(authorization_request
        .raw
        .contains("\"operation_key\":\"operation-93\""));

    let executed = mcp.call(4, "gongbu_create_execution", execution_arguments());
    assert_eq!(
        executed["result"]["isError"], false,
        "unexpected governed execution response: {executed}"
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);

    hubu.disconnect(true);
    let rejected = mcp.call(5, "gongbu_create_execution", execution_arguments());
    assert_eq!(rejected["error"]["data"]["code"], "backend_unavailable");
    assert_eq!(rejected["error"]["data"]["owner"], "gongbu");
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);

    assert_bearer_isolated(&hubu, HUBU_TOKEN, GONGBU_TOKEN);
    assert_bearer_isolated(&gongbu, GONGBU_TOKEN, HUBU_TOKEN);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn backend_outage_matrix_preserves_the_healthy_failure_domain() {
    for unavailable in [BackendKind::Hubu, BackendKind::Gongbu] {
        let hubu = BackendStub::start(BackendKind::Hubu);
        let gongbu = BackendStub::start(BackendKind::Gongbu);
        match unavailable {
            BackendKind::Hubu => hubu.disconnect(true),
            BackendKind::Gongbu => gongbu.disconnect(true),
        }
        let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
        let initialize = mcp.initialize();

        match unavailable {
            BackendKind::Hubu => {
                assert_backend_state(&initialize, "hubu", "unavailable");
                assert_backend_state(&initialize, "gongbu", "available");
                let failed = mcp.call(3, "hubu_list_budgets", json!({}));
                assert_eq!(failed["error"]["data"]["code"], "backend_unavailable");
                let healthy =
                    mcp.call(4, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
                assert!(healthy["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("gongbu-state-marker"));
            }
            BackendKind::Gongbu => {
                assert_backend_state(&initialize, "hubu", "available");
                assert_backend_state(&initialize, "gongbu", "unavailable");
                let failed = mcp.call(3, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
                assert_eq!(failed["error"]["data"]["code"], "backend_unavailable");
                let healthy = mcp.call(4, "hubu_list_budgets", json!({}));
                assert_eq!(
                    healthy["result"]["structuredContent"]["budgets"][0]["budget_id"],
                    "hubu-state-marker"
                );
            }
        }
        mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
    }
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn incompatible_version_matrix_preserves_the_healthy_backend() {
    for incompatible in [BackendKind::Hubu, BackendKind::Gongbu] {
        let hubu = BackendStub::start(BackendKind::Hubu);
        let gongbu = BackendStub::start(BackendKind::Gongbu);
        match incompatible {
            BackendKind::Hubu => hubu.respond_json(
                "GET",
                "/version",
                200,
                json!({
                    "product_version":"wrong",
                    "source_commit":hubu_unified_mcp::source_commit(),
                    "executor_contract":hubu_unified_mcp::EXECUTOR_CONTRACT_VERSION
                }),
            ),
            BackendKind::Gongbu => gongbu.respond_json(
                "GET",
                "/version",
                200,
                json!({
                    "product_version":hubu_unified_mcp::product_version(),
                    "source_commit":hubu_unified_mcp::source_commit(),
                    "api_schema_version":99,
                    "mcp_schema_version":2,
                    "mcp_protocol_version":hubu_unified_mcp::MCP_PROTOCOL_VERSION,
                    "hubu_executor_contract":hubu_unified_mcp::EXECUTOR_CONTRACT_VERSION
                }),
            ),
        }
        let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
        let initialize = mcp.initialize();
        let owner = match incompatible {
            BackendKind::Hubu => "hubu",
            BackendKind::Gongbu => "gongbu",
        };
        assert_backend_state(&initialize, owner, "incompatible");
        let failed = match incompatible {
            BackendKind::Hubu => mcp.call(3, "hubu_list_budgets", json!({})),
            BackendKind::Gongbu => {
                mcp.call(3, "gongbu_get_execution", json!({"execution_id":"exec-93"}))
            }
        };
        assert_eq!(failed["error"]["data"]["code"], "backend_incompatible");
        let healthy = match incompatible {
            BackendKind::Hubu => {
                mcp.call(4, "gongbu_get_execution", json!({"execution_id":"exec-93"}))
            }
            BackendKind::Gongbu => mcp.call(4, "hubu_list_budgets", json!({})),
        };
        assert!(
            healthy.get("result").is_some(),
            "healthy backend call failed: {healthy}; Hubu requests: {:?}; Gongbu requests: {:?}",
            hubu.requests(),
            gongbu.requests()
        );
        mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
    }
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn malformed_responses_and_backend_errors_are_redacted_and_isolated() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    hubu.respond_raw(
        "GET",
        "/budgets",
        500,
        &format!(r#"{{"error":"failure echoed {HUBU_TOKEN}"}}"#),
    );
    gongbu.respond_raw(
        "GET",
        "/v1/executions/exec-93",
        200,
        &format!("not-json-{GONGBU_TOKEN}"),
    );
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();

    let hubu_error = mcp.call(3, "hubu_list_budgets", json!({}));
    assert_eq!(hubu_error["error"]["code"], -32000);
    assert!(hubu_error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("<redacted>"));
    let gongbu_error = mcp.call(4, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
    assert_eq!(gongbu_error["result"]["isError"], true);
    assert!(gongbu_error["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("invalid_response"));

    gongbu.respond_json(
        "GET",
        "/v1/executions/exec-93",
        500,
        json!({"error":{"code":"internal","message":GONGBU_TOKEN}}),
    );
    let redacted = mcp.call(5, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
    assert!(redacted["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("gongbu_internal_error"));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn malformed_probe_is_unavailable_without_corrupting_other_backend_state() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    hubu.respond_raw("GET", "/version", 200, &format!("malformed-{HUBU_TOKEN}"));
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    let initialize = mcp.initialize();
    assert_backend_state(&initialize, "hubu", "unavailable");
    assert_backend_state(&initialize, "gongbu", "available");
    let response = mcp.call(3, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
    assert!(response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("gongbu-state-marker"));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}
