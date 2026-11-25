use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{json, Value};

use super::routing::{public_spend_result, route_tool_call_v1, spend_response_with_approval_hint};
use crate::{
    capability::{BackendReport, BackendState, CapabilitySnapshot, ContractVersions},
    operation_registry::OperationResolution,
    *,
};

fn resolved_operation() -> OperationResolution {
    OperationResolution {
        operation_key: Some("hubu:operation:v1:test:fixed".into()),
        operation_handle: "hubu:public-operation:v1:fixed".into(),
        task_id: Some("linear:HUB-124".into()),
        recorded_result: None,
    }
}

fn server_with_backends(hubu_endpoint: &str, gongbu_endpoint: Option<&str>) -> Server {
    let config = Config {
        hubu: Some(
            BackendConfig::new(BackendOwner::Hubu, hubu_endpoint, "hubu-token-canary").unwrap(),
        ),
        gongbu: gongbu_endpoint.map(|endpoint| {
            BackendConfig::new(BackendOwner::Gongbu, endpoint, "gongbu-token-canary").unwrap()
        }),
        ..Config::default()
    };
    let gongbu_state = if config.gongbu.is_some() {
        BackendState::Available
    } else {
        BackendState::Unconfigured
    };
    let snapshot = CapabilitySnapshot {
        generated_at: "2026-08-18T00:00:00.000Z".into(),
        hubu: test_backend_report(BackendState::Available, false),
        gongbu: test_backend_report(gongbu_state, true),
    };
    let transition_state = TransitionState::new(&snapshot);
    let now = Instant::now();
    Server {
        backends: BackendClients::new(config).unwrap(),
        snapshot: Arc::new(Mutex::new(snapshot)),
        transition_state: Arc::new(transition_state),
        capability_poll_interval: DEFAULT_CAPABILITY_POLL_INTERVAL,
        operation_tick: DEFAULT_OPERATION_TICK,
        governed_execution_wait: DEFAULT_GOVERNED_EXECUTION_WAIT,
        probe_timings: Arc::new(Mutex::new(ProbeTimings {
            hubu: BackendProbeTiming::new(now, DEFAULT_CAPABILITY_POLL_INTERVAL, false, 7),
            gongbu: BackendProbeTiming::new(now, DEFAULT_CAPABILITY_POLL_INTERVAL, false, 11),
        })),
        probe_schedule_waker: Arc::new(Mutex::new(None)),
        operation_worker_waker: Arc::new(Mutex::new(None)),
        operation_registry: Arc::new(OperationRegistryCapability::Available(Mutex::new(
            crate::operation_registry::OperationRegistry::open_in_memory().unwrap(),
        ))),
        meta_shape_diagnostics: false,
        save_folders_path: None,
        approval_portal_addr: DEFAULT_APPROVAL_PORTAL_ADDR,
        use_capability_snapshot_for_test: true,
    }
}

fn test_backend_report(state: BackendState, gongbu: bool) -> BackendReport {
    BackendReport {
        state,
        product_version: Some(product_version().into()),
        source_commit: Some("a".repeat(40)),
        api_schema_version: gongbu.then_some(2),
        mcp_schema_version: gongbu.then_some(2),
        contract_versions: ContractVersions {
            executor: Some(EXECUTOR_CONTRACT_VERSION.into()),
        },
        reason_code: (state != BackendState::Available).then_some("configuration_missing"),
    }
}

fn tool_call(server: &Server, name: &str, arguments: Value, meta: Option<Value>) -> Value {
    server.call_tool_from_snapshot(
        json!(7),
        json!({"name": name, "arguments": arguments, "_meta": meta}),
    )
}

fn one_shot_http_server(
    status: u16,
    body: &'static str,
) -> (String, Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = header_end + 4;
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(str::trim)
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + content_length {
                    break;
                }
            }
        }
        sender.send(String::from_utf8(bytes).unwrap()).unwrap();
        write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (endpoint, receiver, handle)
}

fn disconnect_after_request_server() -> (String, Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buffer = [0_u8; 2048];
        let count = stream.read(&mut buffer).unwrap();
        sender
            .send(String::from_utf8(buffer[..count].to_vec()).unwrap())
            .unwrap();
    });
    (endpoint, receiver, handle)
}

#[test]
fn configured_catalog_matches_the_owned_hubu_contract() {
    let server = server_with_backends("http://127.0.0.1:1", None);
    let actual = server.list_tools_for_snapshot();
    assert_eq!(actual[0], capability_tool());
    assert_eq!(&actual[1..3], hubu_feedback::tool_definitions().as_slice());
    assert_eq!(actual[3], gongbu::operation_status_definition());
    assert_eq!(actual[4], crate::resume_operation::tool_definition());

    let expected = super::catalog::tool_definitions();
    assert_eq!(&actual[5..], expected.as_slice());
    assert_eq!(expected.len(), 12);
    assert!(!actual
        .iter()
        .any(|tool| tool["name"] == "hubu_replace_budget"));
    let get = actual
        .iter()
        .find(|tool| tool["name"] == "hubu_get_spend_approval")
        .unwrap();
    assert_eq!(
        get["inputSchema"]["required"],
        json!(["approval_request_id"])
    );
    assert_eq!(get["inputSchema"]["additionalProperties"], false);
    assert_eq!(get["annotations"]["readOnlyHint"], true);
    assert_eq!(get["annotations"]["x_hubu_client_approval_mode"], "auto");
    let resolve = actual
        .iter()
        .find(|tool| tool["name"] == "hubu_prompt_spend_approval")
        .unwrap();
    assert_eq!(
        resolve["inputSchema"]["required"],
        json!(["approval_request_id"])
    );
    assert_eq!(resolve["inputSchema"]["additionalProperties"], false);
    assert_eq!(resolve["annotations"]["destructiveHint"], true);
    assert_eq!(resolve["annotations"]["idempotentHint"], false);
    assert_eq!(
        resolve["annotations"]["x_hubu_client_approval_mode"],
        "auto"
    );
    assert!(!actual.iter().any(|tool| tool["name"]
        .as_str()
        .is_some_and(|name| name.starts_with("gongbu_"))));
}

#[test]
fn spend_tools_accept_exact_amounts_and_currency() {
    let tools = super::catalog::tool_definitions();
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name}"))
    };
    for name in ["hubu_authorize_spend", "hubu_submit_spend"] {
        let schema = &tool(name)["inputSchema"];
        assert_eq!(
            schema["required"],
            json!(["account_id", "amount", "reason"]),
            "{name}"
        );
        assert_eq!(schema["properties"]["amount"]["type"], "string", "{name}");
        assert_eq!(
            schema["properties"]["amount"]["pattern"], "^\\d+(\\.\\d{1,6})?$",
            "{name}"
        );
        assert!(schema["properties"].get("amount_cents").is_none(), "{name}");
        assert_eq!(
            schema["properties"]["currency"]["enum"],
            json!(["usd", "usdc"]),
            "{name}"
        );
    }
}

#[test]
fn combined_catalog_exposes_both_approved_sets_under_readiness_gates() {
    let server = server_with_backends("http://127.0.0.1:1", Some("http://127.0.0.1:2"));
    let tools = server.list_tools_for_snapshot();
    assert_eq!(tools.len(), 24);
    assert!(tools.contains(&gongbu::operation_status_definition()));
    for definition in super::catalog::tool_definitions()
        .into_iter()
        .chain(gongbu::tool_definitions())
    {
        assert!(tools.contains(&definition), "{}", definition["name"]);
    }

    {
        let mut snapshot = server
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.gongbu.state = BackendState::Degraded;
        snapshot.gongbu.reason_code = Some("backend_not_ready");
    }
    let degraded = server.list_tools_for_snapshot();
    assert_eq!(degraded.len(), 22);
    assert!(!degraded
        .iter()
        .any(|tool| tool["name"] == "gongbu_create_execution"));
    assert!(degraded
        .iter()
        .any(|tool| tool["name"] == "gongbu_get_execution"));
    assert!(degraded
        .iter()
        .any(|tool| tool["name"] == crate::resume_operation::TOOL_NAME));

    {
        let mut snapshot = server
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.gongbu.state = BackendState::Available;
        snapshot.gongbu.reason_code = None;
        snapshot.hubu.state = BackendState::Unavailable;
        snapshot.hubu.reason_code = Some("health_unavailable");
    }
    let hubu_down = server.list_tools_for_snapshot();
    assert_eq!(hubu_down.len(), 9);
    assert!(!hubu_down
        .iter()
        .any(|tool| tool["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("hubu_")
                && !matches!(
                    name,
                    "hubu_unified_capabilities"
                        | "hubu_operation_status"
                        | "hubu_feedback_guidance"
                        | "hubu_prepare_feedback"
                ))));
    assert!(!hubu_down
        .iter()
        .any(|tool| tool["name"] == "gongbu_create_execution"));
}

#[test]
fn approved_hubu_routes_prepare_exact_static_requests() {
    let empty = json!({});
    let spend = json!({"account_id":"account-1","amount": "0.25","reason":"test"});
    let approval_request_id = "11111111-1111-4111-8111-111111111111";
    let cases = [
        (
            "hubu_registration_guidance",
            empty.clone(),
            "GET",
            "/registration/guidance",
        ),
        (
            "hubu_register_agent",
            empty.clone(),
            "POST",
            "/agents/register",
        ),
        ("hubu_show_policy", empty.clone(), "GET", "/policies/show"),
        ("hubu_submit_spend", spend.clone(), "POST", "/spend"),
        ("hubu_authorize_spend", spend, "POST", "/spend/authorize"),
        (
            "hubu_get_spend_approval",
            json!({"approval_request_id":approval_request_id}),
            "GET",
            "/spend/approval?approval_request_id=11111111-1111-4111-8111-111111111111",
        ),
        (
            "hubu_prompt_spend_approval",
            json!({"approval_request_id":approval_request_id}),
            "POST",
            "/spend/approval/prompt",
        ),
        ("hubu_list_agents", empty.clone(), "GET", "/agents"),
        ("hubu_list_budgets", empty.clone(), "GET", "/budgets"),
        (
            "hubu_list_authorization_records",
            empty.clone(),
            "GET",
            "/spend/authorizations",
        ),
        (
            "hubu_get_authorization_record",
            json!({"authorization_id":"public-authorization_record"}),
            "GET",
            "/spend/authorizations/show?authorization_id=public-authorization_record",
        ),
        (
            "hubu_list_ledger",
            empty.clone(),
            "GET",
            "/ledger/transactions",
        ),
    ];
    assert_eq!(cases.len(), 12);
    for (name, arguments, method, path) in cases {
        let params = json!({
            "name": name,
            "arguments": arguments,
        });
        let mut captured = None;
        let operation =
            matches!(name, "hubu_submit_spend" | "hubu_authorize_spend").then(resolved_operation);
        let result = route_tool_call_v1(params, operation, |request| {
            captured = Some(request);
            Ok(json!({"status":"ok"}))
        })
        .unwrap();
        let request = captured.expect("routed Hubu tool should make one request");
        assert_eq!(request.method, method, "{name}");
        assert_eq!(request.path, path, "{name}");
        assert!(request
            .body
            .as_ref()
            .is_none_or(|body| body.get("_meta").is_none()));
        if matches!(name, "hubu_submit_spend" | "hubu_authorize_spend") {
            assert_eq!(
                request.body.as_ref().unwrap()["operation_key"],
                "hubu:operation:v1:test:fixed"
            );
            assert_eq!(request.body.as_ref().unwrap()["task_id"], "linear:HUB-124");
        }
        if name == "hubu_prompt_spend_approval" {
            assert_eq!(
                request.body.as_ref().unwrap(),
                &json!({
                    "approval_request_id": approval_request_id
                })
            );
        }
        assert_eq!(result["structuredContent"]["status"], "ok");
    }
}

#[test]
fn approved_query_variants_match_owned_routing_contract() {
    let cases = [
        (
            "hubu_show_policy",
            json!({"policy_id":"policy-1"}),
            "/policies/show?policy_id=policy-1",
        ),
        (
            "hubu_show_policy",
            json!({"agent_id":"agent-1&other=value"}),
            "/policies/show?agent_id=agent-1%26other%3Dvalue",
        ),
        (
            "hubu_list_budgets",
            json!({"include_all":true}),
            "/budgets?all=true",
        ),
    ];
    for (name, arguments, expected_path) in cases {
        let mut captured = None;
        route_tool_call_v1(
            json!({"name":name,"arguments":arguments}),
            None,
            |request| {
                captured = Some(request);
                Ok(json!({}))
            },
        )
        .unwrap();
        assert_eq!(captured.unwrap().path, expected_path, "{name}");
    }

    let error = route_tool_call_v1(
        json!({
            "name":"hubu_show_policy",
            "arguments":{"policy_id":"policy-1","agent_id":"agent-1"}
        }),
        None,
        |_| unreachable!(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("pass only one"));
}

#[test]
fn routed_success_preserves_metadata_auth_and_spend_result_shape() {
    let (endpoint, request, handle) =
        one_shot_http_server(200, r#"{"decision":"needs_approval","payment":null}"#);
    let server = server_with_backends(&endpoint, None);
    let arguments = json!({"account_id":"account-1","amount": "0.25","reason":"review"});
    let meta = json!({"hubu.dev/platform-invocation":{
        "platform":"codex-controlled",
        "invocation_id":"invocation-1",
        "task_id":"linear:HUB-124"
    }});
    let owned_routing_result = route_tool_call_v1(
        json!({
            "name":"hubu_authorize_spend",
            "arguments":arguments.clone(),
            "_meta":meta.clone()
        }),
        Some(resolved_operation()),
        |_| Ok(json!({"decision":"needs_approval","payment":null})),
    )
    .unwrap();
    let response = tool_call(&server, "hubu_authorize_spend", arguments, Some(meta));
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with("POST /spend/authorize HTTP/1.1"));
    assert!(raw.contains("authorization: Bearer hubu-token-canary"));
    assert!(!raw.contains("gongbu-token-canary"));
    let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(body["operation_key"]
        .as_str()
        .unwrap()
        .starts_with("hubu:operation:v1:codex-controlled:"));
    assert_eq!(body["task_id"], "linear:HUB-124");
    assert!(body.get("_meta").is_none());
    assert!(body.get("platform").is_none());
    assert_eq!(
        response["result"]["structuredContent"]["decision"],
        owned_routing_result["structuredContent"]["decision"]
    );
    assert!(response["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .starts_with("hubu:public-operation:v1:"));
    assert_eq!(
        response["result"]["structuredContent"]["requires_human_approval"],
        true
    );
    assert_eq!(
        response["result"]["content"][0]["text"],
        serde_json::to_string_pretty(&response["result"]["structuredContent"]).unwrap()
    );
}

#[test]
fn resolved_spend_dispatch_uses_supplied_identity_without_registry_persistence() {
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"decision":"allow","operation_key":"hubu:operation:v1:test:fixed","message":"completed hubu:operation:v1:test:fixed"}"#,
    );
    let server = server_with_backends(&endpoint, None);
    let operation = resolved_operation();

    let response = super::dispatch_resolved_spend(
        &server,
        "hubu_authorize_spend",
        json!({"account_id":"account-1","amount": "0.25","reason":"resume"}),
        &operation,
    )
    .unwrap();
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["operation_key"], "hubu:operation:v1:test:fixed");
    assert_eq!(body["task_id"], "linear:HUB-124");
    assert_eq!(response["operation_handle"], operation.operation_handle);
    assert_eq!(response["decision"], "allow");
    assert!(!response
        .to_string()
        .contains("hubu:operation:v1:test:fixed"));
    assert!(server
        .durable_operation_status("hubu:public-operation:v1:fixed")
        .is_err());
}

#[test]
fn resumed_spend_expiry_requires_the_machine_readable_backend_code() {
    for (body, expected_expired) in [
        (
            r#"{"error":"spend authorization rejected payment request: spend auth token is expired","error_code":"spend_auth_token_expired","retry_guidance":null}"#,
            true,
        ),
        (
            r#"{"error":"spend authorization rejected payment request: spend auth token is expired","retry_guidance":null}"#,
            false,
        ),
        (
            r#"{"error":"some other application rejection","error_code":"some_other_code"}"#,
            false,
        ),
    ] {
        let (endpoint, request, handle) = one_shot_http_server(400, body);
        let server = server_with_backends(&endpoint, None);
        let error = super::dispatch_resolved_spend(
            &server,
            "hubu_submit_spend",
            json!({"account_id":"account-1","amount": "0.25","reason":"resume"}),
            &resolved_operation(),
        )
        .unwrap_err();
        request.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert_eq!(super::is_expired_resume_failure(&error), expected_expired);
    }
}

#[test]
fn spend_approval_read_is_automatic_and_redacts_private_operation_identity() {
    let approval_request_id = "11111111-1111-4111-8111-111111111111";
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"approval_request_id":"11111111-1111-4111-8111-111111111111","status":"pending","review":{"operation_key":"private-root","reason":"review private-root","details":[{"operation_key":"private-nested","message":"do not expose private-nested"}]}}"#,
    );
    let server = server_with_backends(&endpoint, None);

    let response = tool_call(
        &server,
        "hubu_get_spend_approval",
        json!({"approval_request_id":approval_request_id}),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with(
        "GET /spend/approval?approval_request_id=11111111-1111-4111-8111-111111111111 HTTP/1.1"
    ));
    assert!(raw.contains("authorization: Bearer hubu-token-canary"));
    assert!(!raw.contains("x-hubu-approval-capability"));
    assert!(!raw.contains("unused-approval-capability"));
    assert_eq!(
        response["result"]["structuredContent"]["approval_request_id"],
        approval_request_id
    );
    assert_eq!(response["result"]["structuredContent"]["status"], "pending");
    assert_eq!(
        response["result"]["structuredContent"]["review_url"],
        format!("http://127.0.0.1:8790/#approvals?request={approval_request_id}"),
    );
    let serialized = response.to_string();
    assert!(!serialized.contains("operation_key"));
    assert!(!serialized.contains("private-root"));
    assert!(!serialized.contains("private-nested"));
    assert!(serialized.contains("<private operation redacted>"));
}

#[test]
fn standalone_spend_pending_result_and_exact_replay_include_review_url() {
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"decision":"needs_approval","approval":{"approval_request_id":"approval:fixture","status":"pending"}}"#,
    );
    let mut server = server_with_backends(&endpoint, None);
    server.approval_portal_addr = "[::1]:4567".parse().unwrap();

    let spend = || {
        tool_call(
            &server,
            "hubu_authorize_spend",
            json!({"account_id":"account-1","amount":"0.25","reason":"review"}),
            Some(json!({"callId":"standalone-review-link"})),
        )
    };
    for response in [spend(), spend()] {
        let result = &response["result"];
        assert_eq!(
            result["structuredContent"]["review_url"],
            "http://[::1]:4567/#approvals?request=approval%3Afixture",
        );
        let text_result: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text_result, result["structuredContent"]);
    }
    assert!(request
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .starts_with("POST /spend/authorize "));
    handle.join().unwrap();
}

#[test]
fn spend_approval_response_identity_conflicts_fail_closed() {
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"approval_request_id":"99999999-9999-4999-8999-999999999999","status":"pending","review":{}}"#,
    );
    let server = server_with_backends(&endpoint, None);

    let response = tool_call(
        &server,
        "hubu_get_spend_approval",
        json!({"approval_request_id":"88888888-8888-4888-8888-888888888888"}),
        None,
    );
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "Hubu returned a conflicting approval identity"
    );
    assert!(!response.to_string().contains("99999999"));
}

#[test]
fn spend_approval_prompt_uses_agent_bearer_and_redacts_private_identifiers() {
    let approval_request_id = "22222222-2222-4222-8222-222222222222";
    let (hubu_endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"decision":"allow","operation_key":"private-root","auth_token_id":"private-continuation","approval":{"approval_request_id":"22222222-2222-4222-8222-222222222222","status":"approved","review":{"operation_key":"private-nested","reason":"approved private-root"}},"message":"private-nested private-continuation"}"#,
    );
    let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    gongbu_listener.set_nonblocking(true).unwrap();
    let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
    let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint));

    let response = tool_call(
        &server,
        "hubu_prompt_spend_approval",
        json!({"approval_request_id":approval_request_id}),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with("POST /spend/approval/prompt HTTP/1.1"));
    assert!(raw.contains("authorization: Bearer hubu-token-canary"));
    assert!(!raw.contains("x-hubu-approval-capability"));
    assert!(!raw.contains("x-hubu-reconciliation-capability"));
    assert!(!raw.contains("reconciliation-capability-canary"));
    assert!(!raw.contains("gongbu-token-canary"));
    let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"approval_request_id":approval_request_id}));
    assert_eq!(response["result"]["structuredContent"]["decision"], "allow");
    assert_eq!(
        response["result"]["structuredContent"]["requires_human_approval"],
        false
    );
    let serialized = response.to_string();
    assert!(!serialized.contains("operation_key"));
    assert!(!serialized.contains("private-root"));
    assert!(!serialized.contains("private-nested"));
    assert!(!serialized.contains("private-continuation"));
    assert!(!serialized.contains("auth_token_id"));
    assert!(serialized.contains("<private authorization redacted>"));
    assert!(
        matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
}

#[test]
fn spend_approval_arguments_are_strict_and_fail_before_network() {
    let cases = [
        (
            json!({"approval_request_id":"bad&approval"}),
            "approval_request_id to be a safe identifier",
        ),
        (
            json!({
                "approval_request_id":"44444444-4444-4444-8444-444444444444",
                "extra":true
            }),
            "accepts only its documented approval fields",
        ),
    ];
    for (arguments, expected) in cases {
        let error = route_tool_call_v1(
            json!({"name":"hubu_get_spend_approval","arguments":arguments}),
            None,
            |_| panic!("invalid approval read must not reach the backend"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected));
    }

    let error = route_tool_call_v1(
        json!({
            "name":"hubu_prompt_spend_approval",
            "arguments":{"approval_request_id":"approval-1","decision":"approve"}
        }),
        None,
        |_| panic!("invalid approval prompt must not reach the backend"),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("accepts only its documented approval fields"));
}

#[test]
fn public_spend_results_preserve_policy_semantics_and_remove_private_identity_recursively() {
    for (decision, requires_human_approval) in
        [("allow", false), ("deny", false), ("needs_approval", true)]
    {
        let response = public_spend_result(
            spend_response_with_approval_hint(json!({
                "decision": decision,
                "operation_key": "private-root",
                "task_id": "trusted-task",
                "reason": "backend mentioned private-root",
                "retry_guidance": {"operation_key":"private"},
                "approval": {"review":{"operation_key":"private-review"}}
            })),
            "hubu:public-operation:v1:test",
            Some("private-root"),
        );
        assert_eq!(response["decision"], decision);
        assert_eq!(response["requires_human_approval"], requires_human_approval);
        assert_eq!(
            response["operation_handle"],
            "hubu:public-operation:v1:test"
        );
        let serialized = response.to_string();
        assert!(!serialized.contains("operation_key"));
        assert!(!serialized.contains("private-"));
        assert_eq!(
            response["reason"],
            "backend mentioned <private operation redacted>"
        );
        assert_eq!(response["task_id"], "trusted-task");
    }
}

#[test]
fn denied_public_spend_result_requires_a_new_logical_operation() {
    let response = public_spend_result(
        spend_response_with_approval_hint(json!({
            "decision": "deny",
            "operation_key": "private-denied-operation",
            "retry_guidance": {
                "action": "reuse_operation_key",
                "operation_key": "private-denied-operation",
                "message": "this attempt was denied; reuse this operation key with corrected scope"
            }
        })),
        "hubu:public-operation:v1:test",
        Some("private-denied-operation"),
    );

    assert_eq!(response["retry_guidance"]["action"], "create_new_operation");
    assert_eq!(
        response["agent_guidance"]["on_denied_result"],
        "create_new_operation"
    );
    assert_eq!(
        response["agent_guidance"]["replacement_call"],
        "create_new_operation"
    );
    let serialized = response.to_string();
    assert!(!serialized.contains("private-denied-operation"));
    assert!(!serialized.contains("reuse_operation_key"));
    assert!(!serialized.contains("reuse this operation key"));
    assert!(serialized.contains("new logical operation"));
}

#[test]
fn denied_spend_redelivery_recovers_terminal_result_without_backend_reuse_guidance() {
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"decision":"deny","decision_id":"decision-denied","operation_key":"backend-private","retry_guidance":{"action":"reuse_operation_key","operation_key":"backend-private","message":"reuse this operation key with corrected scope"}}"#,
    );
    let server = server_with_backends(&endpoint, None);
    let arguments = json!({
        "account_id":"account-1",
        "amount": "0.25",
        "reason":"denied"
    });
    let meta = Some(json!({"callId":"denied-call-148"}));
    let first = tool_call(
        &server,
        "hubu_authorize_spend",
        arguments.clone(),
        meta.clone(),
    );
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    let replay = tool_call(
        &server,
        "hubu_authorize_spend",
        arguments.clone(),
        meta.clone(),
    );
    assert_eq!(replay["result"], first["result"]);
    assert_eq!(
        replay["result"]["structuredContent"]["retry_guidance"]["action"],
        "create_new_operation"
    );

    let operation_handle = replay["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap();
    let status = tool_call(
        &server,
        "hubu_operation_status",
        json!({"operation_handle":operation_handle}),
        None,
    );
    assert_eq!(status["result"]["structuredContent"]["terminal"], true);
    assert_eq!(
        status["result"]["structuredContent"]["retry_guidance"]["action"],
        "create_new_operation"
    );

    let collision = tool_call(
        &server,
        "hubu_authorize_spend",
        json!({"account_id":"account-1","amount": "0.20","reason":"corrected"}),
        meta,
    );
    assert!(collision["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refusing backend access"));
    let serialized = json!([first, replay, status, collision]).to_string();
    assert!(!serialized.contains("backend-private"));
    assert!(!serialized.contains("reuse_operation_key"));
}

#[test]
fn spend_identity_collision_fails_before_a_second_backend_request() {
    let (endpoint, request, handle) = one_shot_http_server(200, r#"{"decision":"allow"}"#);
    let server = server_with_backends(&endpoint, None);
    let meta = Some(json!({"callId": "codex-call-124"}));
    let first = tool_call(
        &server,
        "hubu_submit_spend",
        json!({"account_id":"account-1","amount": "0.25","reason":"first"}),
        meta.clone(),
    );
    assert!(first.get("error").is_none());
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    let collision = tool_call(
        &server,
        "hubu_submit_spend",
        json!({"account_id":"account-1","amount": "0.30","reason":"changed"}),
        meta,
    );
    assert_eq!(collision["error"]["code"], -32000);
    assert!(collision["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refusing backend access"));
}

#[test]
fn ambiguous_spend_result_returns_safe_handle_and_exact_redelivery_guidance() {
    let (endpoint, request, handle) = one_shot_http_server(200, "not-json");
    let server = server_with_backends(&endpoint, None);
    let response = tool_call(
        &server,
        "hubu_authorize_spend",
        json!({"account_id":"account-1","amount": "0.25","reason":"ambiguous"}),
        Some(json!({"callId":"ambiguous-call-125"})),
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    let private_key = serde_json::from_str::<Value>(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap()
        ["operation_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("hubu:public-operation:v1:"));
    assert!(message.contains("same call identity"));
    assert!(message.contains("do not submit a replacement"));
    assert!(!message.contains(&private_key));
}

#[test]
fn spend_application_error_cannot_echo_private_operation_key() {
    let operation = resolved_operation();
    let private_key = operation.operation_key.as_deref().unwrap();
    let message = super::operation_failure_message(
        &format!("backend rejected operation {private_key}"),
        &operation,
        false,
    );
    assert!(!message.contains(private_key));
    assert!(message.contains("<private operation redacted>"));
}

#[test]
fn unavailable_registry_hides_and_rejects_only_billable_hubu_tools() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut server = server_with_backends(&endpoint, Some(&endpoint));
    server.operation_registry = Arc::new(OperationRegistryCapability::Unavailable {
        reason_code: "configuration_missing",
    });

    let names = server
        .list_tools_for_snapshot()
        .into_iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(names.contains(&"hubu_registration_guidance".to_owned()));
    assert!(names.contains(&"gongbu_get_artifact".to_owned()));
    assert!(!names.contains(&"hubu_authorize_spend".to_owned()));
    assert!(!names.contains(&"hubu_submit_spend".to_owned()));
    let capability = server.capabilities();
    assert_eq!(
        capability["operation_registry"]["billable_operations_available"],
        false
    );

    let rejected = tool_call(
        &server,
        "hubu_submit_spend",
        json!({"account_id":"account-1","amount": "0.25","reason":"blocked"}),
        Some(json!({"callId":"call-without-registry"})),
    );
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("operation registry"));
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock
    ));
}

#[test]
fn unknown_hubu_tools_fail_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = server_with_backends(&endpoint, Some(&endpoint));

    let removed = tool_call(&server, "hubu_create_recurring_budget", json!({}), None);
    assert_eq!(removed["error"]["code"], -32602);
    let unknown = tool_call(&server, "hubu_not_a_tool", json!({}), None);
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[test]
fn hubu_outage_is_sanitized_retryable_and_has_no_fallback() {
    let unavailable = TcpListener::bind("127.0.0.1:0").unwrap();
    let hubu_endpoint = format!("http://{}", unavailable.local_addr().unwrap());
    drop(unavailable);
    let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    gongbu_listener.set_nonblocking(true).unwrap();
    let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
    let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint));

    let response = tool_call(&server, "hubu_registration_guidance", json!({}), None);
    assert_eq!(response["error"]["code"], -32010);
    assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
    assert_eq!(response["error"]["data"]["owner"], "hubu");
    assert_eq!(response["error"]["data"]["retryable"], true);
    let serialized = response.to_string();
    assert!(!serialized.contains("hubu-token-canary"));
    assert!(!serialized.contains(&hubu_endpoint));
    assert!(
        matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );

    let capability = tool_call(&server, "hubu_unified_capabilities", json!({}), None);
    assert!(capability.get("error").is_none());
}

#[test]
fn approval_prompt_preserves_refusal_status_and_redacts_agent_bearer() {
    let (endpoint, _request, handle) = one_shot_http_server(
        403,
        r#"{"error":"bearer hubu-token-canary","error_code":"owner_presence_denied","approval_status":"denied","retry_guidance":{"action":"ask_owner"}}"#,
    );
    let server = server_with_backends(&endpoint, None);
    let response = tool_call(
        &server,
        "hubu_prompt_spend_approval",
        json!({
            "approval_request_id":"99999999-9999-4999-8999-999999999999"
        }),
        None,
    );
    handle.join().unwrap();

    assert_eq!(response["result"]["isError"], true);
    let error = &response["result"]["structuredContent"];
    assert_eq!(error["http_status"], 403);
    assert_eq!(error["error"], "bearer <redacted>");
    assert_eq!(error["error_code"], "owner_presence_denied");
    assert_eq!(error["approval_status"], "denied");
    assert_eq!(error["retry_guidance"]["action"], "ask_owner");
    let serialized = response.to_string();
    assert!(!serialized.contains("hubu-token-canary"));
    assert!(!serialized.contains(&endpoint));
}

#[test]
fn malformed_mutation_response_is_sanitized_ambiguous_and_not_retried() {
    let (endpoint, request, handle) = one_shot_http_server(200, "backend-secret-not-json");
    let server = server_with_backends(&endpoint, None);
    let response = tool_call(
        &server,
        "hubu_register_agent",
        json!({"name":"test","version":"1"}),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with("POST /agents/register HTTP/1.1"));
    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "Hubu backend request failed after dispatch; mutation outcome may be ambiguous"
    );
    assert!(!response.to_string().contains("backend-secret-not-json"));
}

#[test]
fn connected_read_outage_is_retryable_and_never_reaches_gongbu() {
    let (hubu_endpoint, request, handle) = disconnect_after_request_server();
    let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    gongbu_listener.set_nonblocking(true).unwrap();
    let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
    let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint));

    let response = tool_call(&server, "hubu_registration_guidance", json!({}), None);
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert!(raw.starts_with("GET /registration/guidance HTTP/1.1"));
    assert_eq!(response["error"]["code"], -32010);
    assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
    assert_eq!(response["error"]["data"]["retryable"], true);
    assert!(
        matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
}

#[test]
fn production_dependencies_exclude_backend_implementation_crates() {
    let unified_manifest = include_str!("../../Cargo.toml");
    for forbidden in ["hubu-api", "hubu-core", "hubu-wallet", "gongbu-api"] {
        assert!(!unified_manifest.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn history_routes_filters_and_preserves_exact_values() {
    let result = route_tool_call_v1(
        json!({"name":"hubu_list_ledger","arguments":{"agent_id":"agent","budget_id":"budget","limit":2,"cursor":"a+b&%雪"}}), None,
        |request| {
            assert_eq!(request.method, "GET");
            assert_eq!(request.path, "/ledger/transactions?agent_id=agent&budget_id=budget&limit=2&cursor=a%2Bb%26%25%E9%9B%AA");
            Ok(json!({"schema_version":"hubu-history-v1","transactions":[{"effective_cost":{"amount":"1","scale":3},"entries":[{"amount":{"amount":"170141183460469231731687303715884105","scale":3}}]}],"coverage":{"unaccounted_consumption_cents":1},"next_cursor":"next"}))
        },
    ).unwrap();
    // Plain routing must not coerce exact decimal coefficients or lose coverage/cursors.
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("170141183460469231731687303715884105"));
    assert!(text.contains("unaccounted_consumption_cents"));
    assert!(text.contains("next_cursor"));
}

#[test]
fn history_rejects_private_keys_and_invalid_filters_before_http() {
    for (name, args) in [
        ("hubu_list_spend_workflows", json!({})),
        ("hubu_get_spend_workflow", json!({"workflow_id":"public"})),
        (
            "hubu_get_authorization_record",
            json!({"workflow_id":"public"}),
        ),
        (
            "hubu_get_authorization_record",
            json!({"operation_key":"secret","agent_id":"agent"}),
        ),
        (
            "hubu_get_authorization_record",
            json!({"authorization_id":"public","operation_key":"secret"}),
        ),
        ("hubu_list_ledger", json!({"budget_id":"budget"})),
        ("hubu_list_ledger", json!({"limit":101})),
        (
            "hubu_list_authorization_records",
            json!({"status":"invalid"}),
        ),
    ] {
        assert!(
            route_tool_call_v1(json!({"name":name,"arguments":args}), None, |_| panic!(
                "invalid query reached HTTP"
            ))
            .is_err()
        );
    }
}

#[test]
fn authorization_record_detail_preserves_public_ids_and_only_accepts_public_lookup() {
    let payload = json!({"schema_version":"hubu-history-v1","authorization_record":{"id":"authorization_record-public","claim_id":"claim-public"}});
    let response = route_tool_call_v1(
        json!({"name":"hubu_get_authorization_record","arguments":{"authorization_id":"authorization_record-public"}}),
        None,
        |request| {
            assert_eq!(
                request.path,
                "/spend/authorizations/show?authorization_id=authorization_record-public"
            );
            Ok(payload.clone())
        },
    )
    .unwrap();
    assert_eq!(response["structuredContent"], payload);
    assert!(route_tool_call_v1(json!({"name":"hubu_get_authorization_record","arguments":{"authorization_id":"authorization_record-public","agent_id":"agent"}}), None, |_| panic!("mixed selector reached HTTP")).is_err());
}

#[test]
fn history_routes_pass_transport_allowlist_and_support_unknown_legacy_status() {
    for (name, arguments) in [
        (
            "hubu_list_ledger",
            json!({"agent_id":"agent","budget_id":"budget"}),
        ),
        (
            "hubu_list_authorization_records",
            json!({"status":"unknown"}),
        ),
        (
            "hubu_get_authorization_record",
            json!({"authorization_id":"public-authorization_record"}),
        ),
    ] {
        route_tool_call_v1(
            json!({"name":name,"arguments":arguments}),
            None,
            |request| {
                assert!(
                    super::transport::is_approved_http_route(request.method, &request.path),
                    "{}",
                    request.path
                );
                assert!(!super::transport::is_approved_http_route(
                    "POST",
                    &request.path
                ));
                Ok(json!({}))
            },
        )
        .unwrap();
    }
}

#[test]
fn removed_tools_are_unknown_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    for available in [true, false] {
        let server = server_with_backends(&endpoint, Some(&endpoint));
        if !available {
            let mut snapshot = server.snapshot.lock().unwrap();
            snapshot.hubu.state = BackendState::Unavailable;
            snapshot.gongbu.state = BackendState::Unconfigured;
        }
        for name in [
            "hubu_add_policy",
            "hubu_export_policy",
            "hubu_health",
            "gongbu_get_provider_catalog",
            "gongbu_get_redaction_attestation",
            "hubu_client_approval_profile",
        ] {
            assert!(!server
                .list_tools_for_snapshot()
                .iter()
                .any(|tool| tool["name"] == name));
            assert!(!server.capabilities()["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name));
            assert_eq!(
                tool_call(&server, name, json!({}), None)["error"]["code"],
                -32602
            );
            let error = route_tool_call_v1(json!({"name":name,"arguments":{}}), None, |_| {
                panic!("removed tool reached backend")
            })
            .unwrap_err();
            assert!(error.to_string().contains("unknown Hubu MCP tool"));
        }
    }
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[test]
fn legacy_policy_yaml_export_fails_before_network() {
    let result = route_tool_call_v1(
        json!({"name":"hubu_show_policy","arguments":{"include_yaml":true}}),
        None,
        |_| panic!("policy export reached the backend"),
    )
    .unwrap();
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["structuredContent"]["error_code"],
        "owner_authority_required"
    );
}

#[test]
fn execution_lookup_survives_unavailable_operation_store() {
    let (endpoint, requests, worker) = one_shot_http_server(
        200,
        r#"{"schema_version":1,"execution_id":"exec-recovery","spend_auth_token_id":"recovery-token","status":"pending","outcome":"waiting","failure":null,"authorization":{"amount_minor":25,"currency":"USD"},"created_at":"now","updated_at":"now","started_at":null,"completed_at":null}"#,
    );
    let mut server = server_with_backends(&endpoint, Some(&endpoint));
    server.operation_registry = Arc::new(OperationRegistryCapability::Unavailable {
        reason_code: "configuration_missing",
    });
    assert!(server
        .list_tools_for_snapshot()
        .iter()
        .any(|tool| tool["name"] == "gongbu_get_execution"));
    let result = tool_call(
        &server,
        "gongbu_get_execution",
        json!({"execution_id":"exec-recovery"}),
        None,
    );
    assert_eq!(result["result"]["isError"], false, "{result}");
    let body: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["execution_id"], "exec-recovery");
    assert!(!body.to_string().contains("private-operation"));
    assert!(requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .starts_with("GET /v1/executions/exec-recovery "));
    worker.join().unwrap();
}

#[test]
fn agent_rename_and_identity_history_are_not_reachable_through_unified_mcp() {
    for (method, path) in [
        ("POST", "/agents/rename"),
        ("GET", "/agents/rename"),
        ("GET", "/agents/history"),
        ("GET", "/agents/history?agent_id=agt_0123456789ab"),
    ] {
        assert!(
            !super::transport::is_approved_http_route(method, path),
            "{method} {path}"
        );
    }
    assert!(crate::DOMAIN_TOOLS
        .iter()
        .all(|(name, _)| !name.contains("rename") && !name.contains("identity")));

    let error = route_tool_call_v1(
        json!({
            "name": "hubu_rename_agent",
            "arguments": {"agent_id": "agt_0123456789ab", "name": "renamed", "reason": "dodge review"}
        }),
        None,
        |_| panic!("agent rename must never reach the Hubu backend through MCP"),
    )
    .unwrap_err();
    assert!(!error.to_string().is_empty());
}

#[test]
fn save_artifact_tool_is_listed_only_for_human_allowlisted_folders() {
    let mut server = server_with_backends("http://127.0.0.1:1", Some("http://127.0.0.1:2"));
    let listed = |server: &Server| {
        server
            .list_tools_for_snapshot()
            .into_iter()
            .find(|tool| tool["name"] == artifact_save::TOOL_NAME)
    };
    let capability = |server: &Server| {
        server.capabilities()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == artifact_save::TOOL_NAME)
            .cloned()
            .unwrap()
    };
    assert!(listed(&server).is_none());
    assert_eq!(
        capability(&server)["reason_code"],
        "save_folders_unconfigured"
    );

    let root = tempfile::tempdir().unwrap();
    let folders = root.path().join("save-folders.json");
    server.save_folders_path = Some(folders.clone());
    assert!(
        listed(&server).is_none(),
        "missing file is an empty allowlist"
    );
    std::fs::write(
        &folders,
        serde_json::to_vec(&json!({"schema_version":1,"folders":[
            {"name":"brand","path":root.path().join("brand")}
        ]}))
        .unwrap(),
    )
    .unwrap();
    let definition = listed(&server).expect("allowlisted folder lists the tool");
    assert_eq!(
        definition["inputSchema"]["properties"]["folder"]["enum"],
        json!(["brand"])
    );
    assert!(!definition
        .to_string()
        .contains(root.path().to_str().unwrap()));
    assert_eq!(capability(&server)["available"], true);

    let handle = "hubu:public-operation:v1:0123456789abcdef0123456789abcdef";
    for (arguments, expected) in [
        (
            json!({"operation_handle":handle,"folder":"other"}),
            "allowlisted folders: brand",
        ),
        (
            json!({"operation_handle":handle,"folder":"/tmp"}),
            "not a path",
        ),
        (
            json!({"operation_handle":handle,"folder":"brand"}),
            "not a bound governed execution",
        ),
    ] {
        let response = tool_call(&server, artifact_save::TOOL_NAME, arguments, None);
        assert_eq!(response["result"]["isError"], true, "{response}");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(expected), "{text}");
    }
    let response = tool_call(
        &server,
        artifact_save::TOOL_NAME,
        json!({"operation_handle":handle,"folder":"brand","path":"/tmp"}),
        None,
    );
    assert_eq!(response["error"]["code"], -32602);
    assert!(!root.path().join("brand").exists());
}
