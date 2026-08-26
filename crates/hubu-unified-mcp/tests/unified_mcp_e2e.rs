mod support;

use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    thread,
    time::{Duration, Instant},
};
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
    execution_arguments_for("hubu-spend-token-93", "deterministic circle")
}

fn execution_arguments_for(token: &str, prompt: &str) -> Value {
    json!({
        "schema_version":2,
        "spend_auth_token_id":token,
        "input":{"prompt":prompt,"image_count":1},
        "input_schema_version":1,
        "workload_type":"image_generation",
        "provider":"fixture",
        "adapter":"fixture",
        "model":"fixture-v1"
    })
}

fn private_operation_key(mcp: &McpProcess, auth_token_id: &str) -> String {
    Connection::open(mcp.operation_state_path())
        .unwrap()
        .query_row(
            "SELECT operation_key FROM harness_operations WHERE auth_token_id = ?1",
            [auth_token_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn execution_response(operation_key: &str, status: &str) -> Value {
    execution_response_for(operation_key, status, "exec-93")
}

fn execution_response_for(operation_key: &str, status: &str, execution_id: &str) -> Value {
    json!({
        "schema_version":2,
        "execution_id":execution_id,
        "operation_key":operation_key,
        "status":status,
        "outcome":if status == "succeeded" { Some("gongbu-state-marker") } else { None },
        "failure":null,
        "authorization":{"amount_minor":25,"currency":"USD"},
        "created_at":"2026-08-18T00:00:00Z",
        "updated_at":"2026-08-18T00:00:01Z",
        "started_at":null,
        "completed_at":null
    })
}

fn observation_response(operation_key: &str, status: &str) -> Value {
    observation_response_for(operation_key, status, "exec-93")
}

fn observation_response_for(operation_key: &str, status: &str, execution_id: &str) -> Value {
    let mut response = execution_response_for(operation_key, status, execution_id);
    response["schema_version"] = json!(1);
    response
}

fn routing_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/unified-mcp-routing-v1.json"
    ))
    .unwrap()
}

fn wait_for_backend_states(
    mcp: &mut McpProcess,
    next_id: &mut u64,
    expected_hubu: &str,
    expected_gongbu: &str,
) -> Value {
    for _ in 0..6 {
        let response = mcp.call(*next_id, "hubu_unified_capabilities", json!({}));
        *next_id += 1;
        let backends = &response["result"]["structuredContent"]["backends"];
        if backends["hubu"]["state"] == expected_hubu
            && backends["gongbu"]["state"] == expected_gongbu
        {
            return response;
        }
    }
    panic!("backend states did not stabilize as {expected_hubu}/{expected_gongbu}")
}

fn wait_for_operation_state(
    mcp: &mut McpProcess,
    next_id: &mut u64,
    operation_handle: &str,
    expected: &str,
) -> Value {
    for _ in 0..1500 {
        let response = mcp.call(
            *next_id,
            "hubu_operation_status",
            json!({"operation_handle": operation_handle}),
        );
        *next_id += 1;
        if response["result"]["structuredContent"]["state"] == expected {
            return response;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("operation did not reach {expected}")
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn durable_operations_cover_codex_claude_and_safe_transient_replay() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    hubu.respond_sequence_json(
        "POST",
        "/spend/authorize",
        [
            (
                200,
                json!({"decision":"allow","decision_id":"codex-decision","auth_token_id":"codex-durable-token","authorization_expires_at":"2099-01-01T00:00:00Z"}),
            ),
            (
                200,
                json!({"decision":"allow","decision_id":"claude-decision","auth_token_id":"claude-durable-token","authorization_expires_at":"2099-01-01T00:00:00Z"}),
            ),
        ],
    );
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();

    let codex = mcp.call_with_meta(
        20,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"codex durable"}),
        json!({"callId":"codex-durable-call"}),
    );
    let codex_handle = codex["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let codex_key = private_operation_key(&mcp, "codex-durable-token");
    gongbu.respond_sequence_json(
        "POST",
        "/v2/executions",
        [
            (500, json!({"error":{"code":"temporary"}})),
            (
                200,
                execution_response_for(&codex_key, "succeeded", "exec-codex"),
            ),
        ],
    );
    let accepted = mcp.call(
        21,
        "gongbu_create_execution",
        execution_arguments_for("codex-durable-token", "codex prompt"),
    );
    assert_eq!(accepted["result"]["structuredContent"]["terminal"], false);
    let mut next_id = 1000;
    wait_for_operation_state(&mut mcp, &mut next_id, &codex_handle, "succeeded");
    let codex_posts = gongbu
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/v2/executions")
        .collect::<Vec<_>>();
    assert_eq!(codex_posts.len(), 2);
    let bodies = codex_posts
        .iter()
        .map(|request| request.raw.split_once("\r\n\r\n").unwrap().1)
        .collect::<Vec<_>>();
    assert_eq!(
        bodies[0], bodies[1],
        "transient replay must be byte-identical"
    );

    let claude = mcp.call_with_meta(
        22,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"claude durable"}),
        json!({"claudecode/toolUseId":"toolu_durable_claude"}),
    );
    let claude_handle = claude["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let claude_key = private_operation_key(&mcp, "claude-durable-token");
    gongbu.respond_json(
        "POST",
        "/v2/executions",
        200,
        execution_response_for(&claude_key, "succeeded", "exec-claude"),
    );
    mcp.call(
        23,
        "gongbu_create_execution",
        execution_arguments_for("claude-durable-token", "claude prompt"),
    );
    wait_for_operation_state(&mut mcp, &mut next_id, &claude_handle, "succeeded");

    let rows = Connection::open(mcp.operation_state_path())
        .unwrap()
        .prepare(
            "SELECT platform, operation_state FROM harness_operations
             WHERE operation_handle IN (?1, ?2) ORDER BY platform",
        )
        .unwrap()
        .query_map([codex_handle, claude_handle], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("claude-code".into(), "succeeded".into()),
            ("codex".into(), "succeeded".into())
        ]
    );
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &codex_key, &claude_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn ambiguous_provider_outcome_reconciles_then_fails_without_replacement_permission() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        30,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"ambiguous provider"}),
        json!({"callId":"ambiguous-provider-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    let ambiguous = execution_response(&operation_key, "reconciliation_required");
    gongbu.respond_json("POST", "/v2/executions", 200, ambiguous.clone());
    gongbu.respond_json(
        "GET",
        "/v1/executions/exec-93",
        200,
        observation_response(&operation_key, "reconciliation_required"),
    );
    mcp.call(31, "gongbu_create_execution", execution_arguments());
    let mut next_id = 2000;
    let failed = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "failed");
    assert_eq!(
        failed["result"]["structuredContent"]["result"]["code"],
        "reconciliation_exhausted"
    );
    assert_eq!(
        failed["result"]["structuredContent"]["replacement_safe"],
        false
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    assert_eq!(gongbu.request_count("GET", "/v1/executions/exec-93"), 5);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn transient_dispatch_exhaustion_reaches_safe_terminal_failure() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        40,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"retry exhaustion"}),
        json!({"callId":"dispatch-exhaustion-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_sequence_json(
        "POST",
        "/v2/executions",
        (0..5).map(|_| (500, json!({"error":{"code":"temporary"}}))),
    );
    mcp.call(41, "gongbu_create_execution", execution_arguments());
    let mut next_id = 3000;
    let failed = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "failed");
    assert_eq!(
        failed["result"]["structuredContent"]["result"]["code"],
        "dispatch_retry_exhausted"
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 5);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn malformed_successful_create_response_replays_exact_request() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        45,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"ambiguous create response"}),
        json!({"callId":"malformed-create-response-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_sequence_json(
        "POST",
        "/v2/executions",
        [
            (200, json!("truncated-success-response")),
            (200, execution_response(&operation_key, "succeeded")),
        ],
    );

    mcp.call(46, "gongbu_create_execution", execution_arguments());
    let mut next_id = 3500;
    let succeeded = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "succeeded");
    assert_eq!(
        succeeded["result"]["structuredContent"]["result"]["code"],
        "execution_succeeded"
    );
    let create_requests = gongbu
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/v2/executions")
        .collect::<Vec<_>>();
    assert_eq!(create_requests.len(), 2);
    assert_eq!(
        create_requests[0].raw.split_once("\r\n\r\n").unwrap().1,
        create_requests[1].raw.split_once("\r\n\r\n").unwrap().1
    );
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn invalid_successful_observations_retry_without_create_replay() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        47,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"invalid observation recovery"}),
        json!({"callId":"invalid-observation-recovery-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_json(
        "POST",
        "/v2/executions",
        200,
        execution_response(&operation_key, "executing"),
    );
    gongbu.respond_sequence_json(
        "GET",
        "/v1/executions/exec-93",
        [
            (200, json!("truncated-success-response")),
            (
                200,
                observation_response(&operation_key, "unsupported_status"),
            ),
            (200, observation_response(&operation_key, "succeeded")),
        ],
    );

    mcp.call(48, "gongbu_create_execution", execution_arguments());
    let mut next_id = 3600;
    let succeeded = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "succeeded");
    assert_eq!(
        succeeded["result"]["structuredContent"]["result"]["code"],
        "execution_succeeded"
    );
    assert_eq!(
        succeeded["result"]["structuredContent"]["replacement_safe"],
        false
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    assert_eq!(gongbu.request_count("GET", "/v1/executions/exec-93"), 3);
    let persisted: (String, String, Option<String>, u32) =
        Connection::open(mcp.operation_state_path())
            .unwrap()
            .query_row(
                "SELECT gongbu_execution_id, operation_state, gongbu_request_json,
                        observation_failures
                 FROM harness_operations WHERE operation_handle = ?1",
                [&handle],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
    assert_eq!(persisted, ("exec-93".into(), "succeeded".into(), None, 0));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn invalid_observation_exhaustion_is_terminal_without_create_replay() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        49,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"invalid observation exhaustion"}),
        json!({"callId":"invalid-observation-exhaustion-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_json(
        "POST",
        "/v2/executions",
        200,
        execution_response(&operation_key, "executing"),
    );
    gongbu.respond_sequence_json(
        "GET",
        "/v1/executions/exec-93",
        (0..5).map(|_| (200, json!("malformed-observation"))),
    );

    mcp.call(50, "gongbu_create_execution", execution_arguments());
    let mut next_id = 3700;
    let failed = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "failed");
    assert_eq!(
        failed["result"]["structuredContent"]["result"]["code"],
        "observation_retry_exhausted"
    );
    assert_eq!(
        failed["result"]["structuredContent"]["replacement_safe"],
        false
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    assert_eq!(gongbu.request_count("GET", "/v1/executions/exec-93"), 5);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn unreadable_permanent_http_error_is_not_retried() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        51,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"unreadable permanent error"}),
        json!({"callId":"unreadable-permanent-error-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_bytes(
        "POST",
        "/v2/executions",
        401,
        "application/json",
        vec![b'x'; 1024 * 1024 + 1],
    );

    mcp.call(52, "gongbu_create_execution", execution_arguments());
    let mut next_id = 3800;
    let failed = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "failed");
    assert_eq!(
        failed["result"]["structuredContent"]["result"]["code"],
        "gongbu_authentication_failed"
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn permanent_dispatch_failure_is_terminal_without_retry() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let authorized = mcp.call_with_meta(
        50,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"permanent failure"}),
        json!({"callId":"permanent-failure-call"}),
    );
    let handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_json(
        "POST",
        "/v2/executions",
        400,
        json!({"error":{"code":"invalid_request","message":"private backend detail"}}),
    );
    mcp.call(51, "gongbu_create_execution", execution_arguments());
    let mut next_id = 4000;
    let failed = wait_for_operation_state(&mut mcp, &mut next_id, &handle, "failed");
    assert_eq!(
        failed["result"]["structuredContent"]["result"]["code"],
        "execution_request_invalid"
    );
    assert_eq!(
        failed["result"]["structuredContent"]["replacement_safe"],
        false
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    assert!(!failed.to_string().contains("private backend detail"));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

fn assert_public_spend_result(response: &Value) -> &Value {
    assert!(
        response.get("error").is_none(),
        "unexpected response: {response}"
    );
    let result = &response["result"]["structuredContent"];
    assert!(result["operation_handle"]
        .as_str()
        .unwrap()
        .starts_with("hubu:public-operation:v1:"));
    assert_eq!(
        result["agent_guidance"]["on_ambiguous_result"],
        "redeliver_exact_call"
    );
    assert_eq!(
        result["agent_guidance"]["replacement_call"],
        "do_not_submit"
    );
    let serialized = response.to_string();
    assert!(!serialized.contains("operation_key"));
    result
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn normalized_spend_wire_lifecycle_survives_redelivery_collision_and_restart() {
    const PRIVATE_RESPONSE_CANARY: &str = "backend-private-operation-canary";
    let hubu = BackendStub::start(BackendKind::Hubu);
    let state = tempfile::tempdir().unwrap();
    let state_path = state.path().join("operations.sqlite3");
    let backend_result = json!({
        "operation_key": PRIVATE_RESPONSE_CANARY,
        "task_id": "backend-private-task",
        "decision": "allow",
        "decision_id": "decision-125",
        "auth_token_id": "authorization-125",
        "authorization_expires_at": "2099-08-24T00:00:00Z",
        "requires_human_approval": false
    });
    let mut distinct_authorization = backend_result.clone();
    distinct_authorization["auth_token_id"] = json!("authorization-125-distinct");
    hubu.respond_sequence_json(
        "POST",
        "/spend/authorize",
        [(200, backend_result.clone()), (200, distinct_authorization)],
    );
    let mut submitted = backend_result.clone();
    submitted["auth_token_id"] = json!("authorization-submit-125");
    let mut distinct_submission = backend_result;
    distinct_submission["auth_token_id"] = json!("authorization-submit-125-distinct");
    hubu.respond_sequence_json(
        "POST",
        "/spend",
        [(200, submitted), (200, distinct_submission)],
    );
    let arguments = json!({
        "account_id":"account-125",
        "amount_cents":25,
        "reason":"HUB-125 wire lifecycle"
    });

    let mut first =
        McpProcess::start_with_operation_state(Some((&hubu, HUBU_TOKEN)), None, &state_path);
    first.initialize();
    let codex = first.call_with_meta(
        10,
        "hubu_authorize_spend",
        arguments.clone(),
        json!({"callId":"codex-call-125"}),
    );
    let codex_result = assert_public_spend_result(&codex);
    assert_eq!(codex_result["auth_token_id"], "authorization-125");
    assert_eq!(codex_result["task_id"], "backend-private-task");
    let codex_handle = codex_result["operation_handle"].clone();
    assert_eq!(hubu.request_count("POST", "/spend/authorize"), 1);
    let first_request_body = hubu
        .requests()
        .into_iter()
        .find(|request| request.path == "/spend/authorize")
        .and_then(|request| request.raw.split("\r\n\r\n").nth(1).map(str::to_owned))
        .map(|body| serde_json::from_str::<Value>(&body).unwrap())
        .unwrap();
    let first_private_key = first_request_body["operation_key"].clone();
    assert_eq!(first_request_body["task_id"], Value::Null);
    assert!(first_request_body.get("_meta").is_none());

    let exact_redelivery = first.call_with_meta(
        11,
        "hubu_authorize_spend",
        arguments.clone(),
        json!({"callId":"codex-call-125"}),
    );
    assert_eq!(exact_redelivery["result"], codex["result"]);
    assert_eq!(hubu.request_count("POST", "/spend/authorize"), 1);

    let collision = first.call_with_meta(
        12,
        "hubu_authorize_spend",
        json!({"account_id":"account-125","amount_cents":30,"reason":"changed"}),
        json!({"callId":"codex-call-125"}),
    );
    assert!(collision["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refusing backend access"));
    assert_eq!(hubu.request_count("POST", "/spend/authorize"), 1);

    for (id, protected) in [
        (13, "operation_key"),
        (14, "task_id"),
        (15, "operation_handle"),
    ] {
        let mut spoofed = arguments.clone();
        spoofed[protected] = json!("model-owned");
        let rejected = first.call_with_meta(
            id,
            "hubu_authorize_spend",
            spoofed,
            json!({"callId":format!("spoof-{protected}")}),
        );
        assert!(rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("trusted platform state"));
    }
    assert_eq!(hubu.request_count("POST", "/spend/authorize"), 1);

    let distinct = first.call_with_meta(
        16,
        "hubu_authorize_spend",
        arguments.clone(),
        json!({"callId":"codex-call-125-distinct"}),
    );
    let distinct_result = assert_public_spend_result(&distinct);
    assert_ne!(distinct_result["operation_handle"], codex_handle);
    assert_eq!(hubu.request_count("POST", "/spend/authorize"), 2);
    let second_private_key = hubu
        .requests()
        .into_iter()
        .filter(|request| request.path == "/spend/authorize")
        .nth(1)
        .and_then(|request| request.raw.split("\r\n\r\n").nth(1).map(str::to_owned))
        .map(|body| serde_json::from_str::<Value>(&body).unwrap()["operation_key"].clone())
        .unwrap();
    assert_ne!(second_private_key, first_private_key);

    let claude = first.call_with_meta(
        17,
        "hubu_submit_spend",
        arguments.clone(),
        json!({"claudecode/toolUseId":"toolu_hub_125"}),
    );
    let claude_result = assert_public_spend_result(&claude);
    let claude_handle = claude_result["operation_handle"].clone();
    assert_eq!(hubu.request_count("POST", "/spend"), 1);
    let claude_request_body = hubu
        .requests()
        .into_iter()
        .find(|request| request.path == "/spend")
        .and_then(|request| request.raw.split("\r\n\r\n").nth(1).map(str::to_owned))
        .map(|body| serde_json::from_str::<Value>(&body).unwrap())
        .unwrap();
    assert_eq!(claude_request_body["task_id"], Value::Null);
    assert!(claude_request_body.get("_meta").is_none());

    let claude_redelivery = first.call_with_meta(
        18,
        "hubu_submit_spend",
        arguments.clone(),
        json!({"claudecode/toolUseId":"toolu_hub_125"}),
    );
    assert_eq!(claude_redelivery["result"], claude["result"]);
    assert_eq!(hubu.request_count("POST", "/spend"), 1);

    let claude_collision = first.call_with_meta(
        19,
        "hubu_submit_spend",
        json!({"account_id":"account-125","amount_cents":30,"reason":"changed"}),
        json!({"claudecode/toolUseId":"toolu_hub_125"}),
    );
    assert!(claude_collision["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refusing backend access"));
    assert_eq!(hubu.request_count("POST", "/spend"), 1);

    let mut claude_spoofed = arguments.clone();
    claude_spoofed["operation_key"] = json!("model-owned");
    let claude_spoof = first.call_with_meta(
        20,
        "hubu_submit_spend",
        claude_spoofed,
        json!({"claudecode/toolUseId":"toolu_hub_125_spoof"}),
    );
    assert!(claude_spoof["error"]["message"]
        .as_str()
        .unwrap()
        .contains("trusted platform state"));
    assert_eq!(hubu.request_count("POST", "/spend"), 1);

    let claude_distinct = first.call_with_meta(
        21,
        "hubu_submit_spend",
        arguments.clone(),
        json!({"claudecode/toolUseId":"toolu_hub_125_distinct"}),
    );
    let claude_distinct_result = assert_public_spend_result(&claude_distinct);
    assert_ne!(claude_distinct_result["operation_handle"], claude_handle);
    assert_eq!(hubu.request_count("POST", "/spend"), 2);
    first.finish(&[HUBU_TOKEN, PRIVATE_RESPONSE_CANARY]);

    let mut restarted =
        McpProcess::start_with_operation_state(Some((&hubu, HUBU_TOKEN)), None, &state_path);
    restarted.initialize();
    let recovered = restarted.call_with_meta(
        30,
        "hubu_authorize_spend",
        arguments.clone(),
        json!({"callId":"codex-call-125"}),
    );
    assert_eq!(recovered["result"], codex["result"]);
    assert_eq!(hubu.request_count("POST", "/spend/authorize"), 2);
    let claude_recovered = restarted.call_with_meta(
        31,
        "hubu_submit_spend",
        arguments,
        json!({"claudecode/toolUseId":"toolu_hub_125"}),
    );
    assert_eq!(claude_recovered["result"], claude["result"]);
    assert_eq!(hubu.request_count("POST", "/spend"), 2);
    restarted.finish(&[HUBU_TOKEN, PRIVATE_RESPONSE_CANARY]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn private_gongbu_continuation_binds_replays_restarts_and_redacts_recursively() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let state = tempfile::tempdir().unwrap();
    let state_path = state.path().join("operations.sqlite3");
    let mut first = McpProcess::start_with_operation_state(
        Some((&hubu, HUBU_TOKEN)),
        Some((&gongbu, GONGBU_TOKEN)),
        &state_path,
    );
    first.initialize();
    let authorized = first.call_with_meta(
        3,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"HUB-126 binding"}),
        json!({"callId":"hub-126-authorization"}),
    );
    let public_handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation_key = private_operation_key(&first, "hubu-spend-token-93");
    let mut failed = execution_response(&operation_key, "failed");
    failed["failure"] = json!({
        "code":"provider_failed",
        "message":format!("nested private identity: {operation_key}")
    });
    gongbu.respond_json("POST", "/v2/executions", 200, failed);

    let created = first.call(4, "gongbu_create_execution", execution_arguments());
    assert_eq!(created["result"]["isError"], false);
    assert_eq!(
        created["result"]["structuredContent"]["operation_handle"],
        public_handle
    );
    assert_eq!(
        created["result"]["structuredContent"]["replacement_safe"],
        false
    );
    let created_text = created["result"]["content"][0]["text"].as_str().unwrap();
    assert!(created_text.contains(&public_handle));
    assert!(!created_text.contains("operation_key") && !created_text.contains(&operation_key));

    let replay = first.call(5, "gongbu_create_execution", execution_arguments());
    assert_eq!(replay["result"]["isError"], false);
    let mut next_id = 100;
    let terminal = wait_for_operation_state(&mut first, &mut next_id, &public_handle, "failed");
    assert_eq!(
        terminal["result"]["structuredContent"]["result"]["code"],
        "execution_failed"
    );
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    assert!(!terminal.to_string().contains(&operation_key));

    let mut changed = execution_arguments();
    changed["model"] = json!("spoofed-model");
    let conflict = first.call(6, "gongbu_create_execution", changed);
    assert!(conflict["error"]["message"]
        .as_str()
        .unwrap()
        .contains("different execution intent"));
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);

    for (id, protected) in [
        (7, "operation_key"),
        (8, "endpoint"),
        (9, "credentials"),
        (10, "retry"),
        (11, "task_id"),
    ] {
        let mut spoofed = execution_arguments();
        spoofed["input"][protected] = json!("model-owned");
        let rejected = first.call(id, "gongbu_create_execution", spoofed);
        assert_eq!(rejected["result"]["isError"], true);
        assert!(rejected["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("protected_override"));
    }
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);

    first.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);

    gongbu.respond_json(
        "GET",
        "/v1/executions/exec-93",
        200,
        observation_response(&operation_key, "succeeded"),
    );
    let mut restarted = McpProcess::start_with_operation_state(
        Some((&hubu, HUBU_TOKEN)),
        Some((&gongbu, GONGBU_TOKEN)),
        &state_path,
    );
    restarted.initialize();
    let recovered = restarted.call(13, "gongbu_create_execution", execution_arguments());
    assert_eq!(recovered["result"]["isError"], false);
    assert_eq!(recovered["result"]["structuredContent"]["state"], "failed");
    assert_eq!(gongbu.request_count("POST", "/v2/executions"), 1);
    let status = restarted.call(
        14,
        "gongbu_get_execution",
        json!({"execution_id":"exec-93"}),
    );
    let serialized = status.to_string();
    assert!(serialized.contains(&public_handle));
    assert!(!serialized.contains("operation_key") && !serialized.contains(&operation_key));

    let persisted: (String, String, Option<String>, String, Option<String>) =
        Connection::open(&state_path)
            .unwrap()
            .query_row(
                "SELECT gongbu_execution_id, gongbu_status, gongbu_outcome,
                    operation_state, gongbu_request_json
             FROM harness_operations WHERE auth_token_id = ?1",
                ["hubu-spend-token-93"],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
    assert_eq!(
        persisted,
        (
            "exec-93".into(),
            "succeeded".into(),
            Some("gongbu-state-marker".into()),
            "failed".into(),
            None
        )
    );
    restarted.finish(&[HUBU_TOKEN, GONGBU_TOKEN, &operation_key]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn exact_hub_88_catalog_and_representative_governed_artifact_flow() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));

    let initialized = mcp.initialize();
    assert_backend_state(&initialized, "hubu", "available");
    assert_backend_state(&initialized, "gongbu", "available");

    let fixture = routing_fixture();
    let expected = fixture["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    let listed = mcp.list_tools();
    let mut listed_names = tool_names(&listed);
    listed_names.sort_unstable();
    assert_eq!(listed_names, expected);

    let capabilities = mcp.call(3, "hubu_unified_capabilities", json!({}));
    let observed = capabilities["result"]["structuredContent"]["tools"]
        .as_array()
        .unwrap();
    assert_eq!(observed.len(), expected.len());
    for (actual, expected) in observed.iter().zip(fixture["tools"].as_array().unwrap()) {
        assert_eq!(actual["name"], expected["name"]);
        assert_eq!(actual["owner"], expected["owner"]);
        assert_eq!(actual["available"], true);
    }

    let budgets = mcp.call(4, "hubu_list_budgets", json!({}));
    assert_eq!(
        budgets["result"]["structuredContent"]["budgets"][0]["budget_id"],
        "hubu-state-marker"
    );
    let authorized = mcp.call_with_meta(
        5,
        "hubu_authorize_spend",
        json!({"account_id":"account-93","amount_cents":25,"reason":"HUB-96 no-spend canary"}),
        json!({"hubu.dev/platform-invocation":{
            "platform":"codex",
            "invocation_id":"invocation-96",
            "task_id":"linear:HUB-96"
        }}),
    );
    assert_eq!(
        authorized["result"]["structuredContent"]["spend_auth_token_id"],
        "hubu-spend-token-93"
    );
    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_json(
        "POST",
        "/v2/executions",
        200,
        execution_response(&operation_key, "succeeded"),
    );
    gongbu.respond_json(
        "GET",
        "/v1/executions/exec-93",
        200,
        observation_response(&operation_key, "succeeded"),
    );
    let execution = mcp.call(6, "gongbu_create_execution", execution_arguments());
    assert_eq!(execution["result"]["isError"], false);
    let serialized = execution.to_string();
    assert!(!serialized.contains("operation_key") && !serialized.contains(&operation_key));
    let public_handle = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap();
    let mut next_id = 100;
    wait_for_operation_state(&mut mcp, &mut next_id, public_handle, "succeeded");
    let execution = mcp.call(7, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
    assert_eq!(execution["result"]["isError"], false);
    let artifacts = mcp.call(
        8,
        "gongbu_list_artifacts",
        json!({"execution_id":"exec-93"}),
    );
    let artifact_list: Value =
        serde_json::from_str(artifacts["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(artifact_list["artifacts"][0]["metadata"]
        .get("storage_path")
        .is_none());
    assert_eq!(
        artifact_list["artifacts"][0]["metadata"]["provider_token"],
        "[REDACTED]"
    );
    let artifact = mcp.call(
        9,
        "gongbu_get_artifact",
        json!({"artifact_id":"artifact-93"}),
    );
    assert_eq!(artifact["result"]["isError"], false);
    assert_eq!(artifact["result"]["content"][1]["type"], "image");
    assert_eq!(artifact["result"]["content"][1]["mimeType"], "image/png");
    assert_eq!(artifact["result"]["content"][1]["data"], "iVBORw0KGgo=");

    assert_bearer_isolated(&hubu, HUBU_TOKEN, GONGBU_TOKEN);
    assert_bearer_isolated(&gongbu, GONGBU_TOKEN, HUBU_TOKEN);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn backend_transport_stop_and_recovery_are_observed_on_refresh() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize();
    let mut next_id = 3;

    hubu.disconnect(true);
    let unavailable = wait_for_backend_states(&mut mcp, &mut next_id, "unavailable", "available");
    assert_eq!(
        unavailable["result"]["structuredContent"]["backends"]["hubu"]["state"],
        "unavailable"
    );
    assert_eq!(
        unavailable["result"]["structuredContent"]["backends"]["gongbu"]["state"],
        "available"
    );
    hubu.disconnect(false);
    let recovered = wait_for_backend_states(&mut mcp, &mut next_id, "available", "available");
    assert_eq!(
        recovered["result"]["structuredContent"]["backends"]["hubu"]["state"],
        "available"
    );

    gongbu.disconnect(true);
    let unavailable = wait_for_backend_states(&mut mcp, &mut next_id, "available", "unavailable");
    assert_eq!(
        unavailable["result"]["structuredContent"]["backends"]["gongbu"]["state"],
        "unavailable"
    );
    assert_eq!(
        unavailable["result"]["structuredContent"]["backends"]["hubu"]["state"],
        "available"
    );
    gongbu.disconnect(false);
    let recovered = wait_for_backend_states(&mut mcp, &mut next_id, "available", "available");
    assert_eq!(
        recovered["result"]["structuredContent"]["backends"]["gongbu"]["state"],
        "available"
    );

    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

fn assert_list_changed(notification: &Value) {
    assert_eq!(
        notification,
        &json!({
            "jsonrpc":"2.0",
            "method":"notifications/tools/list_changed"
        })
    );
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn initialized_lifecycle_establishes_the_notification_baseline() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));

    mcp.initialize_protocol();
    hubu.disconnect(true);
    thread::sleep(Duration::from_millis(100));
    mcp.assert_no_notification(Duration::from_millis(1_200));

    mcp.send_notification("notifications/initialized");
    mcp.assert_no_notification(Duration::from_millis(1_200));

    hubu.disconnect(false);
    assert_list_changed(&mcp.notification(Duration::from_secs(3)));
    mcp.assert_no_notification(Duration::from_millis(2_200));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn out_of_order_initialized_does_not_start_the_monitor() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));

    mcp.send_notification("notifications/initialized");
    let barrier = mcp.request(json!({"jsonrpc":"2.0","id":90,"method":"ping"}));
    assert_eq!(barrier["result"], json!({}));
    hubu.disconnect(true);
    thread::sleep(Duration::from_millis(100));
    mcp.assert_no_notification(Duration::from_millis(1_200));

    mcp.initialize_protocol();
    mcp.send_notification("notifications/initialized");
    let barrier = mcp.request(json!({"jsonrpc":"2.0","id":91,"method":"ping"}));
    assert_eq!(barrier["result"], json!({}));
    mcp.assert_no_notification(Duration::from_millis(1_200));

    hubu.disconnect(false);
    assert_list_changed(&mcp.notification(Duration::from_secs(3)));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn concurrent_monitor_and_request_refresh_are_single_flight_per_backend() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize_with_monitor();

    let baseline_health_requests = hubu.request_count("GET", "/health");
    hubu.delay_response("GET", "/health", Duration::from_millis(800));
    let deadline = Instant::now() + Duration::from_secs(2);
    while hubu.request_count("GET", "/health") == baseline_health_requests {
        assert!(Instant::now() < deadline, "monitor probe did not start");
        thread::sleep(Duration::from_millis(10));
    }

    // The in-flight monitor already captured a healthy response. Change the
    // next response so an overlapping request would otherwise race it.
    hubu.respond_json("GET", "/health", 503, json!({"status":"stopped"}));

    let started = Instant::now();
    let healthy = mcp.call(
        92,
        "gongbu_get_execution",
        json!({"execution_id":"exec-93"}),
    );
    assert!(healthy.get("result").is_some());
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "Hubu probe delayed the independent Gongbu read"
    );

    let coalesced = mcp.call(93, "hubu_list_budgets", json!({}));
    assert_eq!(
        coalesced["result"]["structuredContent"]["budgets"][0]["budget_id"], "hubu-state-marker",
        "request refresh did not reuse the in-flight healthy Hubu probe: {coalesced}"
    );

    assert_list_changed(&mcp.notification(Duration::from_secs(3)));
    mcp.assert_no_notification(Duration::from_millis(2_200));
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn outage_backoff_is_shared_with_requests_and_independent_per_backend() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize_with_monitor();

    hubu.disconnect(true);
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));

    let mut observed_health = hubu.request_count("GET", "/health");
    for _ in 0..3 {
        let deadline = Instant::now() + Duration::from_secs(6);
        while hubu.request_count("GET", "/health") == observed_health {
            assert!(
                Instant::now() < deadline,
                "Hubu outage probe did not repeat"
            );
            thread::sleep(Duration::from_millis(10));
        }
        observed_health = hubu.request_count("GET", "/health");
    }

    // Hubu remains deeply backed off, but Gongbu must still observe its own
    // readiness transition on the normal independent cadence.
    let baseline_readyz = gongbu.request_count("GET", "/readyz");
    gongbu.respond_json("GET", "/readyz", 503, json!({"status":"not_ready"}));
    let deadline = Instant::now() + Duration::from_secs(2);
    while gongbu.request_count("GET", "/readyz") == baseline_readyz {
        assert!(
            Instant::now() < deadline,
            "Gongbu probe was delayed by Hubu outage backoff"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // The fourth Hubu failure has a minimum 6.4 second jittered backoff. Wait
    // past the one-second base interval, then prove routine traffic shares that
    // longer deadline instead of starting another Hubu probe.
    thread::sleep(Duration::from_millis(1_250));
    let backed_off_health = hubu.request_count("GET", "/health");
    let _ = mcp.list_tools();
    let rejected = mcp.call(94, "hubu_list_budgets", json!({}));
    assert_eq!(rejected["error"]["data"]["code"], "backend_unavailable");
    assert_eq!(
        hubu.request_count("GET", "/health"),
        backed_off_health,
        "request-triggered refresh bypassed Hubu outage backoff"
    );

    // A forced diagnostic observes recovery and shortens Hubu's next deadline
    // back to the healthy cadence. The sleeping monitor must be woken so the
    // next background probe is not delayed by the obsolete outage deadline.
    hubu.disconnect(false);
    let recovered = mcp.call(95, "hubu_unified_capabilities", json!({}));
    assert_eq!(
        recovered["result"]["structuredContent"]["backends"]["hubu"]["state"],
        "available"
    );
    assert_list_changed(&mcp.notification(Duration::from_secs(1)));
    let recovered_health = hubu.request_count("GET", "/health");
    let deadline = Instant::now() + Duration::from_secs(2);
    while hubu.request_count("GET", "/health") == recovered_health {
        assert!(
            Instant::now() < deadline,
            "monitor did not wake after forced recovery shortened the probe deadline"
        );
        thread::sleep(Duration::from_millis(10));
    }
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn catalog_transitions_emit_exactly_once_and_preserve_the_healthy_backend() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    mcp.initialize_with_monitor();

    hubu.disconnect(true);
    let healthy = mcp.call(
        100,
        "gongbu_get_execution",
        json!({"execution_id":"exec-93"}),
    );
    assert!(healthy["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("gongbu-state-marker"));
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));
    mcp.assert_no_notification(Duration::from_millis(2_200));

    hubu.disconnect(false);
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));
    mcp.assert_no_notification(Duration::from_millis(2_200));

    gongbu.respond_json("GET", "/readyz", 503, json!({"status":"not_ready"}));
    let healthy = mcp.call(101, "hubu_list_budgets", json!({}));
    assert_eq!(
        healthy["result"]["structuredContent"]["budgets"][0]["budget_id"], "hubu-state-marker",
        "healthy Hubu call failed during Gongbu readiness transition: {healthy}"
    );
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));
    mcp.assert_no_notification(Duration::from_millis(2_200));

    gongbu.respond_json("GET", "/readyz", 200, json!({"status":"ready"}));
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));

    gongbu.respond_json(
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
    );
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));
    let healthy = mcp.call(102, "hubu_list_budgets", json!({}));
    assert!(healthy.get("result").is_some());
    mcp.assert_no_notification(Duration::from_millis(2_200));

    gongbu.respond_json(
        "GET",
        "/version",
        200,
        json!({
            "product_version":hubu_unified_mcp::product_version(),
            "source_commit":hubu_unified_mcp::source_commit(),
            "api_schema_version":2,
            "mcp_schema_version":2,
            "mcp_protocol_version":hubu_unified_mcp::MCP_PROTOCOL_VERSION,
            "hubu_executor_contract":hubu_unified_mcp::EXECUTOR_CONTRACT_VERSION
        }),
    );
    assert_list_changed(&mcp.notification(Duration::from_secs(2)));
    mcp.assert_no_notification(Duration::from_millis(2_200));

    assert_bearer_isolated(&hubu, HUBU_TOKEN, GONGBU_TOKEN);
    assert_bearer_isolated(&gongbu, GONGBU_TOKEN, HUBU_TOKEN);
    mcp.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with deterministic build stamps"]
fn hubu_only_initialize_discovery_and_call() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let mut mcp = McpProcess::start(Some((&hubu, HUBU_TOKEN)), None);

    let initialize = mcp.initialize();
    assert_backend_state(&initialize, "hubu", "available");
    assert_backend_state(&initialize, "gongbu", "unconfigured");
    let baseline_health = hubu.request_count("GET", "/health");
    let baseline_version = hubu.request_count("GET", "/version");
    let tools = mcp.list_tools();
    let names = tool_names(&tools);
    assert!(names.contains(&"hubu_list_budgets"));
    assert!(!names.iter().any(|name| name.starts_with("gongbu_")));

    let response = mcp.call(3, "hubu_list_budgets", json!({}));
    assert_eq!(
        response["result"]["structuredContent"]["budgets"][0]["budget_id"],
        "hubu-state-marker"
    );
    assert_eq!(hubu.request_count("GET", "/health"), baseline_health);
    assert_eq!(hubu.request_count("GET", "/version"), baseline_version);
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
    let baseline_livez = gongbu.request_count("GET", "/livez");
    let baseline_readyz = gongbu.request_count("GET", "/readyz");
    let baseline_version = gongbu.request_count("GET", "/version");
    let tools = mcp.list_tools();
    let names = tool_names(&tools);
    assert!(names.contains(&"gongbu_get_execution"));
    assert!(!names.contains(&"gongbu_create_execution"));
    assert!(!names.iter().any(|name| name.starts_with("hubu_")
        && !matches!(*name, "hubu_unified_capabilities" | "hubu_operation_status")));

    let response = mcp.call(3, "gongbu_get_execution", json!({"execution_id":"exec-93"}));
    let body: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "gongbu-state-marker");
    assert_eq!(gongbu.request_count("GET", "/livez"), baseline_livez);
    assert_eq!(gongbu.request_count("GET", "/readyz"), baseline_readyz);
    assert_eq!(gongbu.request_count("GET", "/version"), baseline_version);
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
        34,
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
            "invocation_id":"invocation-93",
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
        .contains("\"operation_key\":\"hubu:operation:v1:codex:"));

    let operation_key = private_operation_key(&mcp, "hubu-spend-token-93");
    gongbu.respond_json(
        "POST",
        "/v2/executions",
        200,
        execution_response(&operation_key, "pending"),
    );

    let executed = mcp.call(4, "gongbu_create_execution", execution_arguments());
    assert_eq!(
        executed["result"]["isError"], false,
        "unexpected governed execution response: {executed}"
    );
    for _ in 0..100 {
        if gongbu.request_count("POST", "/v2/executions") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
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
