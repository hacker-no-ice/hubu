mod support;

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
            "installation_id":"installation-96",
            "invocation_id":"invocation-96",
            "operation_key":"operation-96",
            "task_id":"linear:HUB-96"
        }}),
    );
    assert_eq!(
        authorized["result"]["structuredContent"]["spend_auth_token_id"],
        "hubu-spend-token-93"
    );
    let execution = mcp.call(6, "gongbu_create_execution", execution_arguments());
    assert_eq!(execution["result"]["isError"], false);
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
    assert!(!names
        .iter()
        .any(|name| name.starts_with("hubu_") && *name != "hubu_unified_capabilities"));

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
