use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{json, Value};

use super::routing::{
    public_spend_result, route_tool_call_v1, spend_response_with_approval_hint,
    HubuRequestCapabilityV1,
};
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

fn server_with_backends(
    hubu_endpoint: &str,
    gongbu_endpoint: Option<&str>,
    trusted_client_approval: bool,
    reconciliation_capability: Option<&str>,
) -> Server {
    server_with_spend_approval(
        hubu_endpoint,
        gongbu_endpoint,
        trusted_client_approval,
        false,
        None,
        reconciliation_capability,
    )
}

fn server_with_spend_approval(
    hubu_endpoint: &str,
    gongbu_endpoint: Option<&str>,
    trusted_client_approval: bool,
    trusted_spend_approval: bool,
    approval_capability: Option<&str>,
    reconciliation_capability: Option<&str>,
) -> Server {
    let config = Config {
        hubu: Some(
            BackendConfig::new(BackendOwner::Hubu, hubu_endpoint, "hubu-token-canary").unwrap(),
        ),
        gongbu: gongbu_endpoint.map(|endpoint| {
            BackendConfig::new(BackendOwner::Gongbu, endpoint, "gongbu-token-canary").unwrap()
        }),
        hubu_routing: HubuRoutingConfig::new_with_spend_approval(
            trusted_client_approval,
            trusted_spend_approval,
            approval_capability.map(str::to_string),
            reconciliation_capability.map(str::to_string),
        ),
        ..Config::default()
    };
    let hubu_routing = config.hubu_routing.clone();
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
        hubu_routing,
        operation_registry: Arc::new(OperationRegistryCapability::Available(Mutex::new(
            crate::operation_registry::OperationRegistry::open_in_memory().unwrap(),
        ))),
        meta_shape_diagnostics: false,
        save_folders_path: None,
        active_task_budget: Arc::new(Mutex::new(None)),
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
    let server = server_with_backends("http://127.0.0.1:1", None, false, None);
    let actual = server.list_tools_for_snapshot();
    assert_eq!(actual[0], capability_tool());
    assert_eq!(&actual[1..3], hubu_feedback::tool_definitions().as_slice());
    assert_eq!(actual[3], gongbu::operation_status_definition());
    assert_eq!(actual[4], crate::resume_operation::tool_definition());

    let expected = super::catalog::tool_definitions();
    assert_eq!(&actual[5..], expected.as_slice());
    assert_eq!(expected.len(), 34);
    assert!(!actual
        .iter()
        .any(|tool| tool["name"] == "hubu_replace_budget"));
    let update = actual
        .iter()
        .find(|tool| tool["name"] == "hubu_update_budget")
        .unwrap();
    assert_eq!(
        update["inputSchema"]["required"],
        json!(["budget_id", "expected_revision"])
    );
    assert_eq!(
        update["inputSchema"]["properties"]["expected_revision"]["minimum"],
        1
    );
    assert_eq!(
        update["inputSchema"]["properties"]["amount_limit_cents"]["minimum"],
        1
    );
    assert_eq!(update["inputSchema"]["additionalProperties"], false);
    assert_eq!(update["annotations"]["idempotentHint"], true);
    assert_eq!(update["annotations"]["destructiveHint"], true);
    let history = actual
        .iter()
        .find(|tool| tool["name"] == "hubu_budget_history")
        .unwrap();
    assert_eq!(history["inputSchema"]["required"], json!(["budget_id"]));
    assert_eq!(history["inputSchema"]["additionalProperties"], false);
    assert_eq!(history["annotations"]["readOnlyHint"], true);
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
        .find(|tool| tool["name"] == "hubu_resolve_spend_approval")
        .unwrap();
    assert_eq!(
        resolve["inputSchema"]["required"],
        json!(["approval_request_id", "decision"])
    );
    assert_eq!(
        resolve["inputSchema"]["properties"]["decision"]["enum"],
        json!(["approve", "deny"])
    );
    assert_eq!(resolve["inputSchema"]["additionalProperties"], false);
    assert_eq!(resolve["annotations"]["destructiveHint"], true);
    assert_eq!(resolve["annotations"]["idempotentHint"], true);
    assert_eq!(
        resolve["annotations"]["x_hubu_client_approval_mode"],
        "prompt_before_call"
    );
    assert!(!actual.iter().any(|tool| tool["name"]
        .as_str()
        .is_some_and(|name| name.starts_with("gongbu_"))));
}

#[test]
fn spend_and_budget_tools_accept_exact_amounts_and_currency() {
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
            json!(["account_id", "reason"]),
            "{name}"
        );
        assert_eq!(schema["properties"]["amount"]["type"], "string", "{name}");
        assert_eq!(
            schema["properties"]["amount"]["pattern"], "^\\d+(\\.\\d{1,6})?$",
            "{name}"
        );
        assert!(schema["properties"]["amount_cents"].is_object(), "{name}");
        assert_eq!(
            schema["properties"]["currency"]["enum"],
            json!(["usd", "usdc"]),
            "{name}"
        );
    }
    assert_eq!(
        tool("hubu_create_budget")["inputSchema"]["properties"]["currency"]["enum"],
        json!(["usd", "usdc"])
    );
}

#[test]
fn billed_reconciliation_schema_exposes_exact_cost_and_legacy_cents() {
    let tool = super::catalog::tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "hubu_reconcile_vendor_billed_claim")
        .expect("billed reconciliation tool");
    let receipt = &tool["inputSchema"]["properties"]["receipt"];
    let exact = &receipt["properties"]["actual_vendor_cost"];

    assert_eq!(exact["properties"]["amount"]["type"], "integer");
    assert_eq!(exact["properties"]["scale"]["maximum"], 18);
    assert_eq!(
        exact["properties"]["currency"]["enum"],
        json!(["usd", "usdc"])
    );
    assert!(receipt["properties"]["actual_vendor_cost_cents"].is_object());
    assert_eq!(receipt["oneOf"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        receipt["properties"]["price_model_snapshot"]["type"],
        "object"
    );
}

#[test]
fn combined_catalog_exposes_both_approved_sets_under_readiness_gates() {
    let server = server_with_backends(
        "http://127.0.0.1:1",
        Some("http://127.0.0.1:2"),
        false,
        None,
    );
    let tools = server.list_tools_for_snapshot();
    assert_eq!(tools.len(), 45);
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
    assert_eq!(degraded.len(), 43);
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
    assert_eq!(hubu_down.len(), 8);
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
    let spend = json!({"account_id":"account-1","amount_cents":25,"reason":"test"});
    let approval_request_id = "11111111-1111-4111-8111-111111111111";
    let reconciliation = json!({
        "claim_id":"claim-1",
        "provider_reference":"provider-1",
        "evidence":"reviewed"
    });
    let cases = [
        (
            "hubu_registration_guidance",
            empty.clone(),
            "GET",
            "/registration/guidance",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_list_users",
            empty.clone(),
            "GET",
            "/users",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_register_human",
            empty.clone(),
            "POST",
            "/init",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_register_agent",
            empty.clone(),
            "POST",
            "/agents/register",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_apply_policy",
            json!({"policy_yaml":"version: 1"}),
            "POST",
            "/policies",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_show_policy",
            empty.clone(),
            "GET",
            "/policies/show",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_show_policy",
            json!({"include_yaml":true}),
            "GET",
            "/policies/export",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_policy_history",
            empty.clone(),
            "GET",
            "/policies/history",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_policy_diff",
            json!({"from_revision":1}),
            "GET",
            "/policies/diff?from_revision=1",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_create_budget",
            empty.clone(),
            "POST",
            "/budgets",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_revoke_budget",
            empty.clone(),
            "POST",
            "/budgets/revoke",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_update_budget",
            json!({
                "budget_id":"bgt_0123456789ab",
                "expected_revision":1,
                "amount_limit_cents":5_000,
                "reason":"raise total cap"
            }),
            "POST",
            "/budgets/bgt_0123456789ab/versions",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_budget_history",
            json!({"budget_id":"bgt_0123456789ab"}),
            "GET",
            "/budgets/bgt_0123456789ab/versions",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_set_spending_target",
            empty.clone(),
            "POST",
            "/user/spending-target",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_revoke_spending_target",
            empty.clone(),
            "POST",
            "/user/spending-target/revoke",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_show_spending_targets",
            empty.clone(),
            "GET",
            "/user/spending-target",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_submit_spend",
            spend.clone(),
            "POST",
            "/spend",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_authorize_spend",
            spend,
            "POST",
            "/spend/authorize",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_get_spend_approval",
            json!({"approval_request_id":approval_request_id}),
            "GET",
            "/spend/approval?approval_request_id=11111111-1111-4111-8111-111111111111",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_resolve_spend_approval",
            json!({"approval_request_id":approval_request_id,"decision":"approve"}),
            "POST",
            "/spend/approval/resolve",
            HubuRequestCapabilityV1::Approval,
        ),
        (
            "hubu_list_agents",
            empty.clone(),
            "GET",
            "/agents",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_list_budgets",
            empty.clone(),
            "GET",
            "/budgets",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_list_authorization_records",
            empty.clone(),
            "GET",
            "/spend/authorizations",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_get_authorization_record",
            json!({"authorization_id":"public-authorization_record"}),
            "GET",
            "/spend/authorizations/show?authorization_id=public-authorization_record",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_governance_summary",
            empty.clone(),
            "GET",
            "/governance/summary",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_list_governance_records",
            empty.clone(),
            "GET",
            "/governance/records",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_get_governance_record",
            json!({"record_id":"lev_0123456789abcdef0123456789abcdef"}),
            "GET",
            "/governance/records/lev_0123456789abcdef0123456789abcdef",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_list_ledger",
            empty.clone(),
            "GET",
            "/ledger/transactions",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_get_executor_claim",
            json!({"claim_id":"claim-1"}),
            "GET",
            "/spend/executor/claim?claim_id=claim-1",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_list_claims_requiring_reconciliation",
            empty.clone(),
            "GET",
            "/spend/executor/reconciliation",
            HubuRequestCapabilityV1::None,
        ),
        (
            "hubu_reconcile_vendor_billed_claim",
            reconciliation.clone(),
            "POST",
            "/spend/executor/settle",
            HubuRequestCapabilityV1::Reconciliation,
        ),
        (
            "hubu_reconcile_vendor_did_not_bill_claim",
            reconciliation,
            "POST",
            "/spend/executor/release",
            HubuRequestCapabilityV1::Reconciliation,
        ),
    ];
    assert_eq!(cases.len(), 32);
    for (name, arguments, method, path, capability) in cases {
        let params = json!({
            "name": name,
            "arguments": arguments,
        });
        let mut captured = None;
        let operation =
            matches!(name, "hubu_submit_spend" | "hubu_authorize_spend").then(resolved_operation);
        let result = route_tool_call_v1(params, true, true, operation, |request| {
            captured = Some(request);
            Ok(json!({"status":"ok"}))
        })
        .unwrap();
        let request = captured.expect("routed Hubu tool should make one request");
        assert_eq!(request.method, method, "{name}");
        assert_eq!(request.path, path, "{name}");
        assert_eq!(request.capability, capability, "{name}");
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
        if name == "hubu_resolve_spend_approval" {
            assert_eq!(
                request.body.as_ref().unwrap(),
                &json!({
                    "approval_request_id": approval_request_id,
                    "decision": "approve"
                })
            );
        }
        if name == "hubu_apply_policy" {
            assert_eq!(request.body.as_ref().unwrap()["source"], "mcp");
        }
        if name == "hubu_update_budget" {
            assert_eq!(
                request.body.as_ref().unwrap(),
                &json!({
                    "expected_revision": 1,
                    "amount_limit_cents": 5_000,
                    "reason": "raise total cap"
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
            json!({"agent_id":"agent-1","include_yaml":true}),
            "/policies/export?agent_id=agent-1",
        ),
        (
            "hubu_policy_diff",
            json!({"agent_id":"agent-1","from_revision":2,"to_revision":4}),
            "/policies/diff?agent_id=agent-1&from_revision=2&to_revision=4",
        ),
        (
            "hubu_show_spending_targets",
            json!({"include_all":true}),
            "/user/spending-target?all=true",
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
            false,
            false,
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
        false,
        false,
        None,
        |_| unreachable!(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("pass only one"));
}

#[test]
fn budget_version_arguments_and_transport_paths_fail_closed() {
    for (name, arguments, expected) in [
        (
            "hubu_update_budget",
            json!({"budget_id":"bgt_0123456789ab","expected_revision":0,"amount_limit_cents":1}),
            "expected_revision >= 1",
        ),
        (
            "hubu_update_budget",
            json!({"budget_id":"bgt_0123456789ab","expected_revision":1,"amount_limit_cents":0}),
            "amount_limit_cents >= 1",
        ),
        (
            "hubu_update_budget",
            json!({"budget_id":"bgt_0123456789ab/versions","expected_revision":1,"amount_limit_cents":1}),
            "safe public identifier",
        ),
        (
            "hubu_update_budget",
            json!({"budget_id":"bgt_0123456789ab","expected_revision":1,"amount_limit_cents":1,"extra":true}),
            "accepts only",
        ),
        (
            "hubu_update_budget",
            json!({"budget_id":"bgt_0123456789ab","expected_revision":1,"amount_limit":"1","amount_limit_cents":100}),
            "not both",
        ),
        (
            "hubu_update_budget",
            json!({"budget_id":"bgt_0123456789ab","expected_revision":1,"reason":"no amount"}),
            "requires amount_limit",
        ),
        (
            "hubu_budget_history",
            json!({"budget_id":"bgt_0123456789ai"}),
            "safe public identifier",
        ),
        (
            "hubu_budget_history",
            json!({"budget_id":"bgt_0123456789ab","extra":true}),
            "accepts only budget_id",
        ),
    ] {
        let error = route_tool_call_v1(
            json!({"name":name,"arguments":arguments}),
            true,
            true,
            None,
            |_| panic!("invalid budget route must not reach the backend"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }

    for method in ["GET", "POST"] {
        assert!(super::transport::is_approved_http_route(
            method,
            "/budgets/bgt_0123456789ab/versions"
        ));
    }
    for path in [
        "/budgets/replace",
        "/budgets/bgt_0123456789ab",
        "/budgets/bgt_0123456789ab/versions/extra",
        "/budgets/bgt_0123456789ab/versions?all=true",
        "/budgets/bgt_0123456789ab%2fescape/versions",
        "/budgets/../versions",
    ] {
        assert!(
            !super::transport::is_approved_http_route("POST", path),
            "{path}"
        );
    }
    assert!(!super::transport::is_approved_http_route(
        "DELETE",
        "/budgets/bgt_0123456789ab/versions"
    ));

    let old = route_tool_call_v1(
        json!({"name":"hubu_replace_budget","arguments":{}}),
        true,
        true,
        None,
        |_| panic!("retired budget tool must not reach the backend"),
    )
    .unwrap_err();
    assert!(old.to_string().contains("unknown Hubu MCP tool"));
}

#[test]
fn budget_application_errors_are_typed_tool_results_and_recursively_redacted() {
    let body = r#"{
        "error":"revision conflict hubu-token-canary",
        "error_code":"budget_revision_conflict",
        "details":{
            "expected_revision":1,
            "current_revision":2,
            "hubu-token-canary":"secret-bearing key",
            "nested":["approval-secret-canary",{"reconciliation-secret-canary":"nested secret-bearing key"}]
        },
        "retry_guidance":{
            "action":"refresh_budget_history",
            "message":"do not expose hubu-token-canary"
        }
    }"#;
    let (endpoint, request, handle) = one_shot_http_server(409, body);
    let server = server_with_spend_approval(
        &endpoint,
        None,
        true,
        false,
        Some("approval-secret-canary"),
        Some("reconciliation-secret-canary"),
    );

    let response = tool_call(
        &server,
        "hubu_update_budget",
        json!({
            "budget_id":"bgt_0123456789ab",
            "expected_revision":1,
            "amount_limit_cents":5_000,
            "reason":"raise cap"
        }),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with("POST /budgets/bgt_0123456789ab/versions HTTP/1.1"));
    let forwarded: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(forwarded.get("budget_id").is_none());
    assert_eq!(forwarded["expected_revision"], 1);
    assert_eq!(forwarded["amount_limit_cents"], 5_000);
    assert_eq!(response["result"]["isError"], true);
    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["http_status"], 409);
    assert_eq!(structured["error_code"], "budget_revision_conflict");
    assert_eq!(structured["details"]["expected_revision"], 1);
    assert_eq!(structured["details"]["current_revision"], 2);
    assert_eq!(
        structured["retry_guidance"]["action"],
        "refresh_budget_history"
    );
    assert_eq!(
        response["result"]["content"][0]["text"],
        serde_json::to_string_pretty(structured).unwrap()
    );
    let serialized = response.to_string();
    for secret in [
        "hubu-token-canary",
        "approval-secret-canary",
        "reconciliation-secret-canary",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert!(serialized.matches("<redacted>").count() >= 4);
}

#[test]
fn budget_history_not_found_is_a_typed_tool_result_but_other_hubu_tools_are_unchanged() {
    let (endpoint, request, handle) = one_shot_http_server(
        404,
        r#"{"error":"budget not found","error_code":"budget_not_found"}"#,
    );
    let server = server_with_backends(&endpoint, None, false, None);
    let response = tool_call(
        &server,
        "hubu_budget_history",
        json!({"budget_id":"bgt_0123456789ab"}),
        None,
    );
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(response["result"]["structuredContent"]["http_status"], 404);
    assert_eq!(
        response["result"]["structuredContent"]["error_code"],
        "budget_not_found"
    );
    assert!(response["result"]["structuredContent"]
        .get("details")
        .is_none());

    let (endpoint, request, handle) = one_shot_http_server(
        409,
        r#"{"error":"ordinary failure","error_code":"budget_revision_conflict"}"#,
    );
    let server = server_with_backends(&endpoint, None, false, None);
    let response = tool_call(&server, "hubu_list_users", json!({}), None);
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert_eq!(response["error"]["code"], -32000);
    assert!(response.get("result").is_none());
}

#[test]
fn routed_success_preserves_metadata_auth_and_spend_result_shape() {
    let (endpoint, request, handle) =
        one_shot_http_server(200, r#"{"decision":"needs_approval","payment":null}"#);
    let server = server_with_backends(&endpoint, None, false, None);
    let arguments = json!({"account_id":"account-1","amount_cents":25,"reason":"review"});
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
        false,
        false,
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
    let server = server_with_backends(&endpoint, None, false, None);
    let operation = resolved_operation();

    let response = super::dispatch_resolved_spend(
        &server,
        "hubu_authorize_spend",
        json!({"account_id":"account-1","amount_cents":25,"reason":"resume"}),
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
        let server = server_with_backends(&endpoint, None, false, None);
        let error = super::dispatch_resolved_spend(
            &server,
            "hubu_submit_spend",
            json!({"account_id":"account-1","amount_cents":25,"reason":"resume"}),
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
    let server = server_with_spend_approval(
        &endpoint,
        None,
        false,
        false,
        Some("unused-approval-capability"),
        None,
    );

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
    let serialized = response.to_string();
    assert!(!serialized.contains("operation_key"));
    assert!(!serialized.contains("private-root"));
    assert!(!serialized.contains("private-nested"));
    assert!(serialized.contains("<private operation redacted>"));
}

#[test]
fn spend_approval_response_identity_conflicts_fail_closed() {
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"approval_request_id":"99999999-9999-4999-8999-999999999999","status":"pending","review":{}}"#,
    );
    let server = server_with_spend_approval(
        &endpoint,
        None,
        false,
        false,
        Some("unused-approval-capability"),
        None,
    );

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
fn spend_approval_resolution_uses_only_its_narrow_gate_and_capability() {
    let approval_request_id = "22222222-2222-4222-8222-222222222222";
    let (hubu_endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"decision":"allow","operation_key":"private-root","auth_token_id":"private-continuation","approval":{"approval_request_id":"22222222-2222-4222-8222-222222222222","status":"approved","review":{"operation_key":"private-nested","reason":"approved private-root"}},"message":"private-nested private-continuation"}"#,
    );
    let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    gongbu_listener.set_nonblocking(true).unwrap();
    let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
    let server = server_with_spend_approval(
        &hubu_endpoint,
        Some(&gongbu_endpoint),
        false,
        true,
        Some("approval-capability-canary"),
        Some("reconciliation-capability-canary"),
    );

    let response = tool_call(
        &server,
        "hubu_resolve_spend_approval",
        json!({"approval_request_id":approval_request_id,"decision":"approve"}),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with("POST /spend/approval/resolve HTTP/1.1"));
    assert!(raw.contains("authorization: Bearer hubu-token-canary"));
    assert!(raw.contains("x-hubu-approval-capability: approval-capability-canary"));
    assert!(!raw.contains("x-hubu-reconciliation-capability"));
    assert!(!raw.contains("reconciliation-capability-canary"));
    assert!(!raw.contains("gongbu-token-canary"));
    let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"approval_request_id":approval_request_id,"decision":"approve"})
    );
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
fn broad_client_trust_remains_compatible_with_spend_approval_resolution() {
    let mut captured = None;
    let response = route_tool_call_v1(
        json!({
            "name":"hubu_resolve_spend_approval",
            "arguments":{
                "approval_request_id":"33333333-3333-4333-8333-333333333333",
                "decision":"deny"
            }
        }),
        true,
        false,
        None,
        |request| {
            captured = Some(request);
            Ok(json!({"decision":"deny"}))
        },
    )
    .unwrap();

    assert_eq!(
        captured.unwrap().capability,
        HubuRequestCapabilityV1::Approval
    );
    assert_eq!(response["structuredContent"]["decision"], "deny");
    assert_eq!(
        response["structuredContent"]["requires_human_approval"],
        false
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
            false,
            false,
            None,
            |_| panic!("invalid approval read must not reach the backend"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected));
    }

    for (arguments, expected) in [
        (
            json!({
                "approval_request_id":"bad?approval",
                "decision":"approve"
            }),
            "approval_request_id to be a safe identifier",
        ),
        (
            json!({
                "approval_request_id":"55555555-5555-4555-8555-555555555555",
                "decision":"maybe"
            }),
            "decision approve or deny",
        ),
        (
            json!({
                "approval_request_id":"55555555-5555-4555-8555-555555555555",
                "decision":"approve",
                "operation_key":"model-authored"
            }),
            "accepts only its documented approval fields",
        ),
    ] {
        let error = route_tool_call_v1(
            json!({"name":"hubu_resolve_spend_approval","arguments":arguments}),
            false,
            true,
            None,
            |_| panic!("invalid approval resolution must not reach the backend"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected));
    }
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
    let server = server_with_backends(&endpoint, None, false, None);
    let arguments = json!({
        "account_id":"account-1",
        "amount_cents":25,
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
        json!({"account_id":"account-1","amount_cents":20,"reason":"corrected"}),
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
    let server = server_with_backends(&endpoint, None, false, None);
    let meta = Some(json!({"callId": "codex-call-124"}));
    let first = tool_call(
        &server,
        "hubu_submit_spend",
        json!({"account_id":"account-1","amount_cents":25,"reason":"first"}),
        meta.clone(),
    );
    assert!(first.get("error").is_none());
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    let collision = tool_call(
        &server,
        "hubu_submit_spend",
        json!({"account_id":"account-1","amount_cents":30,"reason":"changed"}),
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
    let server = server_with_backends(&endpoint, None, false, None);
    let response = tool_call(
        &server,
        "hubu_authorize_spend",
        json!({"account_id":"account-1","amount_cents":25,"reason":"ambiguous"}),
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
    let mut server = server_with_backends(&endpoint, Some(&endpoint), false, None);
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
        json!({"account_id":"account-1","amount_cents":25,"reason":"blocked"}),
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
fn reconciliation_uses_distinct_hubu_capability_and_never_gongbu() {
    let (hubu_endpoint, request, handle) = one_shot_http_server(200, r#"{"status":"settled"}"#);
    let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    gongbu_listener.set_nonblocking(true).unwrap();
    let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
    let server = server_with_backends(
        &hubu_endpoint,
        Some(&gongbu_endpoint),
        true,
        Some("reconciliation-canary"),
    );
    let response = tool_call(
        &server,
        "hubu_reconcile_vendor_did_not_bill_claim",
        json!({"claim_id":"claim-1","provider_reference":"provider-1","evidence":"reviewed"}),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert!(response.get("error").is_none());
    assert!(raw.contains("authorization: Bearer hubu-token-canary"));
    assert!(raw.contains("x-hubu-reconciliation-capability: reconciliation-canary"));
    assert!(!raw.contains("gongbu-token-canary"));
    assert!(
        matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
}

#[test]
fn reconciliation_without_distinct_capability_fails_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut server = server_with_backends(&endpoint, None, true, None);
    server.hubu_routing.reconciliation_capability_file = format!(
        "/private/tmp/hubu-91-missing-reconciliation-token-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );

    let response = tool_call(
        &server,
        "hubu_reconcile_vendor_did_not_bill_claim",
        json!({"claim_id":"claim-1","provider_reference":"provider-1","evidence":"reviewed"}),
        None,
    );
    assert_eq!(response["error"]["code"], -32000);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("human reconciliation requires"));
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[test]
fn spend_approval_resolution_loads_its_capability_from_file() {
    let (endpoint, request, handle) = one_shot_http_server(
        200,
        r#"{"decision":"deny","approval":{"approval_request_id":"66666666-6666-4666-8666-666666666666","status":"denied"}}"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let capability_path = directory.path().join("approval-token");
    std::fs::write(&capability_path, "file-approval-capability\n").unwrap();
    let mut server = server_with_spend_approval(&endpoint, None, false, true, None, None);
    server.hubu_routing.approval_capability_file = capability_path.display().to_string();

    let response = tool_call(
        &server,
        "hubu_resolve_spend_approval",
        json!({
            "approval_request_id":"66666666-6666-4666-8666-666666666666",
            "decision":"deny"
        }),
        None,
    );
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(response.get("error").is_none());
    assert!(raw.contains("x-hubu-approval-capability: file-approval-capability"));
    assert!(!raw.contains("x-hubu-reconciliation-capability"));
}

#[test]
fn spend_approval_resolution_without_capability_fails_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut server = server_with_spend_approval(&endpoint, None, false, true, None, None);
    server.hubu_routing.approval_capability_file = format!(
        "/private/tmp/hubu-164-missing-approval-token-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );

    let response = tool_call(
        &server,
        "hubu_resolve_spend_approval",
        json!({
            "approval_request_id":"77777777-7777-4777-8777-777777777777",
            "decision":"approve"
        }),
        None,
    );

    assert_eq!(response["error"]["code"], -32000);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("human spend approval requires"));
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[test]
fn protected_and_unapproved_hubu_tools_fail_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = server_with_backends(&endpoint, Some(&endpoint), false, None);

    let protected = tool_call(&server, "hubu_create_budget", json!({}), None);
    assert_eq!(protected["error"]["code"], -32000);
    assert!(protected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("trusted MCP client approval gate"));
    let approval = tool_call(
        &server,
        "hubu_resolve_spend_approval",
        json!({
            "approval_request_id":"88888888-8888-4888-8888-888888888888",
            "decision":"approve"
        }),
        None,
    );
    assert_eq!(approval["error"]["code"], -32000);
    assert!(approval["error"]["message"]
        .as_str()
        .unwrap()
        .contains("trusted spend-approval client gate"));
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
    let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint), false, None);

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
fn forwarded_application_errors_preserve_hubu_contract_without_secrets() {
    let (endpoint, _request, handle) = one_shot_http_server(
        403,
        r#"{"error":"bearer hubu-token-canary reconciliation reconciliation-canary"}"#,
    );
    let server = server_with_backends(&endpoint, None, true, Some("reconciliation-canary"));
    let response = tool_call(
        &server,
        "hubu_reconcile_vendor_did_not_bill_claim",
        json!({"claim_id":"claim-1","provider_reference":"provider-1","evidence":"reviewed"}),
        None,
    );
    handle.join().unwrap();
    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "Hubu server returned HTTP 403: bearer <redacted> reconciliation <redacted>"
    );
    let serialized = response.to_string();
    assert!(!serialized.contains("hubu-token-canary"));
    assert!(!serialized.contains(&endpoint));
}

#[test]
fn approval_application_errors_cannot_echo_any_configured_secret() {
    let (endpoint, _request, handle) = one_shot_http_server(
        403,
        r#"{"error":"bearer hubu-token-canary approval approval-capability-canary"}"#,
    );
    let server = server_with_spend_approval(
        &endpoint,
        None,
        false,
        true,
        Some("approval-capability-canary"),
        None,
    );
    let response = tool_call(
        &server,
        "hubu_resolve_spend_approval",
        json!({
            "approval_request_id":"99999999-9999-4999-8999-999999999999",
            "decision":"deny"
        }),
        None,
    );
    handle.join().unwrap();

    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "Hubu server returned HTTP 403: bearer <redacted> approval <redacted>"
    );
    let serialized = response.to_string();
    assert!(!serialized.contains("hubu-token-canary"));
    assert!(!serialized.contains("approval-capability-canary"));
    assert!(!serialized.contains(&endpoint));
}

#[test]
fn malformed_mutation_response_is_sanitized_ambiguous_and_not_retried() {
    let (endpoint, request, handle) = one_shot_http_server(200, "backend-secret-not-json");
    let server = server_with_backends(&endpoint, None, true, None);
    let response = tool_call(&server, "hubu_create_budget", json!({}), None);
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();

    assert!(raw.starts_with("POST /budgets HTTP/1.1"));
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
    let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint), false, None);

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
        json!({"name":"hubu_list_ledger","arguments":{"agent_id":"agent","budget_id":"budget","limit":2,"cursor":"a+b&%雪"}}),
        false, false, None,
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
        assert!(route_tool_call_v1(
            json!({"name":name,"arguments":args}),
            false,
            false,
            None,
            |_| panic!("invalid query reached HTTP")
        )
        .is_err());
    }
}

#[test]
fn authorization_record_detail_preserves_public_ids_and_only_accepts_public_lookup() {
    let payload = json!({"schema_version":"hubu-history-v1","authorization_record":{"id":"authorization_record-public","claim_id":"claim-public"}});
    let response = route_tool_call_v1(
        json!({"name":"hubu_get_authorization_record","arguments":{"authorization_id":"authorization_record-public"}}),
        false,
        false,
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
    assert!(route_tool_call_v1(json!({"name":"hubu_get_authorization_record","arguments":{"authorization_id":"authorization_record-public","agent_id":"agent"}}), false, false, None, |_| panic!("mixed selector reached HTTP")).is_err());
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
            false,
            false,
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
    for (trusted, available) in [(false, true), (true, true), (true, false)] {
        let server = server_with_backends(&endpoint, Some(&endpoint), trusted, None);
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
            let error = route_tool_call_v1(
                json!({"name":name,"arguments":{}}),
                trusted,
                trusted,
                None,
                |_| panic!("removed tool reached backend"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("unknown Hubu MCP tool"));
        }
    }
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[test]
fn show_policy_optional_yaml_preserves_response_and_selectors() {
    let response = json!({"policy_id":"policy-1","revision":4,"payload_hash":"hash","policy":{"version":1},"assignments":[{"scope":"user_default"}],"policy_yaml":"version: 1\n"});
    for (arguments, expected) in [
        (json!({}), "/policies/show"),
        (json!({"include_yaml":false}), "/policies/show"),
        (json!({"include_yaml":true}), "/policies/export"),
        (
            json!({"include_yaml":true,"policy_id":"policy-1"}),
            "/policies/export?policy_id=policy-1",
        ),
        (
            json!({"include_yaml":true,"agent_id":"agent-1"}),
            "/policies/export?agent_id=agent-1",
        ),
    ] {
        let result = route_tool_call_v1(
            json!({"name":"hubu_show_policy","arguments":arguments}),
            false,
            false,
            None,
            |request| {
                assert_eq!(request.method, "GET");
                assert_eq!(request.path, expected);
                assert!(request.body.is_none());
                Ok(response.clone())
            },
        )
        .unwrap();
        assert_eq!(result["structuredContent"], response);
    }
    for arguments in [
        json!({"include_yaml":null}),
        json!({"include_yaml":"true"}),
        json!({"include_yaml":1}),
        json!({"unexpected":true}),
        json!({"include_yaml":true,"policy_id":"p","agent_id":"a"}),
    ] {
        assert!(route_tool_call_v1(
            json!({"name":"hubu_show_policy","arguments":arguments}),
            false,
            false,
            None,
            |_| panic!("invalid request reached backend")
        )
        .is_err());
    }
    let schema = super::catalog::tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "hubu_show_policy")
        .unwrap();
    assert_eq!(
        schema["inputSchema"]["properties"]["include_yaml"],
        json!({"type":"boolean","default":false})
    );
}

#[test]
fn canonical_apply_requires_yaml_and_preserves_assignment_and_cas() {
    let arguments = json!({"policy_yaml":"version: 1","declarative_key":"policy","display_name":"Policy","agent_id":"agent-1","expected_revision":3,"expected_hash":"hash"});
    let mut expected = arguments.clone();
    expected["source"] = json!("mcp");
    route_tool_call_v1(
        json!({"name":"hubu_apply_policy","arguments":arguments}),
        true,
        false,
        None,
        |request| {
            assert_eq!(request.path, "/policies");
            assert_eq!(request.body, Some(expected));
            Ok(json!({}))
        },
    )
    .unwrap();
    for arguments in [
        json!({}),
        json!({"policy_yaml":null}),
        json!({"policy_yaml":" "}),
        json!({"daily_limit_cents":100}),
        json!({"policy_yaml":"version: 1","daily_limit_cents":100}),
    ] {
        assert!(route_tool_call_v1(
            json!({"name":"hubu_apply_policy","arguments":arguments}),
            true,
            false,
            None,
            |_| panic!("shortcut reached backend")
        )
        .is_err());
    }
    assert!(route_tool_call_v1(
        json!({"name":"hubu_apply_policy","arguments":{"policy_yaml":"version: 1"}}),
        false,
        false,
        None,
        |_| panic!("ungated policy write")
    )
    .unwrap_err()
    .to_string()
    .contains("trusted MCP client approval gate"));
}

#[test]
fn execution_lookup_survives_unavailable_operation_store() {
    let (endpoint, requests, worker) = one_shot_http_server(
        200,
        r#"{"schema_version":1,"execution_id":"exec-recovery","spend_auth_token_id":"recovery-token","status":"pending","outcome":"waiting","failure":null,"authorization":{"amount_minor":25,"currency":"USD"},"created_at":"now","updated_at":"now","started_at":null,"completed_at":null}"#,
    );
    let mut server = server_with_backends(&endpoint, Some(&endpoint), false, None);
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
        true,
        true,
        None,
        |_| panic!("agent rename must never reach the Hubu backend through MCP"),
    )
    .unwrap_err();
    assert!(!error.to_string().is_empty());
}

fn governance_transport_fixture() -> Value {
    serde_json::from_str(include_str!("../../../../fixtures/governance-mcp-v1.json")).unwrap()
}

#[test]
fn governance_catalog_exposes_strict_ungated_read_tools() {
    let tools = super::catalog::tool_definitions();
    for name in [
        "hubu_governance_summary",
        "hubu_list_governance_records",
        "hubu_get_governance_record",
    ] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["annotations"]["x_hubu_human_approval"], "none");
        assert_eq!(tool["annotations"]["x_hubu_client_approval_mode"], "auto");
        assert_eq!(tool["annotations"]["x_hubu_runtime_approval"], "none");
        let properties = &tool["inputSchema"]["properties"];
        for private in [
            "owner_id",
            "operation_key",
            "claim_id",
            "spend_auth_token_id",
        ] {
            assert!(properties.get(private).is_none());
        }
        if name == "hubu_list_governance_records" {
            assert_eq!(properties["limit"]["minimum"], 1);
            assert_eq!(properties["limit"]["maximum"], 200);
            assert_eq!(properties["cursor"]["type"], "string");
        } else {
            assert!(properties.get("cursor").is_none());
            assert!(properties.get("limit").is_none());
        }
        if name == "hubu_get_governance_record" {
            assert_eq!(tool["inputSchema"]["required"], json!(["record_id"]));
        }
    }
}

#[test]
fn governance_routes_encode_opaque_tokens_once_and_preserve_http_projection() {
    let fixture = governance_transport_fixture();
    let snapshot = fixture["summary"]["snapshot"]["token"].as_str().unwrap();
    let cursor = fixture["cursor"].as_str().unwrap();
    let body = json!({"schema_version":"hubu-governance-v1","filters":fixture["filters"],"summary":fixture["summary"],"records":fixture["records"],"next_cursor":cursor});
    let args = json!({
        "agent_id":"agt_0123456789ab","account_id":"aga_0123456789ab",
        "categories":["automated_admission","human_resolution"],
        "outcomes":["automatic_deny","human_declined"],
        "occurred_from":"2026-10-08T00:00:00+00:00",
        "occurred_before":"2026-10-15T00:00:00Z",
        "snapshot":snapshot,"cursor":cursor,"limit":1
    });
    let result = route_tool_call_v1(
        json!({"name":"hubu_list_governance_records","arguments":args}),
        false,
        false,
        None,
        |request| {
            assert_eq!(request.method, "GET");
            assert_eq!(request.capability, HubuRequestCapabilityV1::None);
            assert!(request.body.is_none());
            assert!(super::transport::is_approved_http_route(
                "GET",
                &request.path
            ));
            assert!(!super::transport::is_approved_http_route(
                "POST",
                &request.path
            ));
            let parsed = reqwest::Url::parse(&format!("http://fixture{}", request.path)).unwrap();
            assert_eq!(parsed.path(), "/governance/records");
            let query = parsed
                .query_pairs()
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(query.get("snapshot").unwrap(), snapshot);
            assert_eq!(query.get("cursor").unwrap(), cursor);
            assert_eq!(
                query.get("categories").unwrap(),
                "automated_admission,human_resolution"
            );
            assert_eq!(
                query.get("outcomes").unwrap(),
                "automatic_deny,human_declined"
            );
            assert_eq!(
                query.get("occurred_from").unwrap(),
                "2026-10-08T00:00:00+00:00"
            );
            assert_eq!(query.get("limit").unwrap(), "1");
            assert_eq!(query.len(), 9);
            Ok(body.clone())
        },
    )
    .unwrap();
    assert_ne!(result.get("isError"), Some(&json!(true)));
    assert_eq!(result["structuredContent"], body);
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, body);
    assert_eq!(
        text["summary"]["requested"]["per_currency"][0]["units"],
        "9007199254740993"
    );
    assert_eq!(
        text["records"][0]["requested_amount"]["value"]["units"],
        "5000"
    );
    assert_eq!(text["summary"]["backlog"]["unresolved_reconciliations"], 1);
    assert_eq!(
        text["summary"]["record_coverage"][0]["complete_history"],
        false
    );
}

#[test]
fn governance_detail_preserves_snapshot_scope_and_cannot_select_private_records() {
    let fixture = governance_transport_fixture();
    let record_id = fixture["records"][0]["record_id"].as_str().unwrap();
    let snapshot = fixture["summary"]["snapshot"]["token"].as_str().unwrap();
    let body = json!({"schema_version":"hubu-governance-v1","filters":fixture["filters"],"summary":fixture["summary"],"record":fixture["records"][0]});
    let result = route_tool_call_v1(
        json!({"name":"hubu_get_governance_record","arguments":{"record_id":record_id,"agent_id":"agt_0123456789ab","snapshot":snapshot}}),
        false, false, None,
        |request| {
            let parsed = reqwest::Url::parse(&format!("http://fixture{}", request.path)).unwrap();
            assert_eq!(parsed.path(), format!("/governance/records/{record_id}"));
            let query = parsed.query_pairs().collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(query.get("snapshot").unwrap(), snapshot);
            assert_eq!(query.get("agent_id").unwrap(), "agt_0123456789ab");
            assert_eq!(query.len(), 2);
            Ok(body.clone())
        },
    ).unwrap();
    assert_eq!(result["structuredContent"], body);
}

#[test]
fn governance_invalid_inputs_fail_before_network_without_echoing_query_values() {
    let canary = "private-operation-canary";
    let valid_id = "lev_0123456789abcdef0123456789abcdef";
    let mut cases = vec![
        ("hubu_governance_summary", json!({"owner_id":canary})),
        ("hubu_governance_summary", json!({"operation_key":canary})),
        ("hubu_governance_summary", json!({"cursor":"opaque"})),
        ("hubu_governance_summary", json!({"limit":1})),
        ("hubu_governance_summary", json!({"agent_id":null})),
        ("hubu_governance_summary", json!({"agent_id":""})),
        (
            "hubu_governance_summary",
            json!({"agent_id":"owner-secret"}),
        ),
        (
            "hubu_governance_summary",
            json!({"account_id":"aga_unsafe/../"}),
        ),
        ("hubu_governance_summary", json!({"categories":[]})),
        (
            "hubu_governance_summary",
            json!({"categories":"automated_admission"}),
        ),
        (
            "hubu_governance_summary",
            json!({"categories":["automated_admission",null]}),
        ),
        ("hubu_governance_summary", json!({"outcomes":[canary]})),
        ("hubu_governance_summary", json!({"outcomes":[]})),
        ("hubu_governance_summary", json!({"snapshot":{}})),
        ("hubu_governance_summary", json!({"snapshot":""})),
        (
            "hubu_governance_summary",
            json!({"snapshot":"x".repeat(2049)}),
        ),
        (
            "hubu_governance_summary",
            json!({"occurred_from":"yesterday"}),
        ),
        ("hubu_governance_summary", json!({"occurred_before":42})),
        (
            "hubu_governance_summary",
            json!({"occurred_from":"2026-10-15T00:00:00Z","occurred_before":"2026-10-15T00:00:00Z"}),
        ),
        (
            "hubu_governance_summary",
            json!({"occurred_from":"2026-10-16T00:00:00Z","occurred_before":"2026-10-15T00:00:00Z"}),
        ),
        ("hubu_list_governance_records", json!({"limit":0})),
        ("hubu_list_governance_records", json!({"limit":201})),
        ("hubu_list_governance_records", json!({"limit":1.5})),
        ("hubu_list_governance_records", json!({"limit":"1"})),
        ("hubu_list_governance_records", json!({"cursor":null})),
        (
            "hubu_list_governance_records",
            json!({"cursor":"x".repeat(4097)}),
        ),
        (
            "hubu_list_governance_records",
            json!({"record_id":valid_id}),
        ),
        ("hubu_get_governance_record", json!({})),
        ("hubu_get_governance_record", json!({"record_id":null})),
        (
            "hubu_get_governance_record",
            json!({"record_id":"lev_unsafe?operation_key=secret"}),
        ),
        ("hubu_get_governance_record", json!({"record_id":canary})),
        (
            "hubu_get_governance_record",
            json!({"record_id":valid_id,"cursor":"opaque"}),
        ),
        (
            "hubu_get_governance_record",
            json!({"record_id":valid_id,"limit":1}),
        ),
    ];
    for name in [
        "hubu_governance_summary",
        "hubu_list_governance_records",
        "hubu_get_governance_record",
    ] {
        cases.push((name, json!([])));
    }
    for (name, args) in cases {
        let error = route_tool_call_v1(
            json!({"name":name,"arguments":args}),
            false,
            false,
            None,
            |_| panic!("invalid governance input reached HTTP"),
        )
        .unwrap_err();
        assert!(!error.to_string().contains(canary));
    }
}

#[test]
fn governance_errors_are_typed_safe_and_do_not_echo_backend_diagnostics() {
    for (status, body, code) in [
        (
            400,
            r#"{"error":"private-operation-canary hubu-token-canary","error_code":"governance_invalid_query"}"#,
            "governance_invalid_query",
        ),
        (
            404,
            r#"{"error":"private-operation-canary hubu-token-canary","error_code":"governance_record_not_found"}"#,
            "governance_record_not_found",
        ),
        (
            500,
            r#"{"error":"private-operation-canary hubu-token-canary","error_code":"governance_unavailable"}"#,
            "governance_unavailable",
        ),
    ] {
        let (endpoint, request, handle) = one_shot_http_server(status, body);
        let server = server_with_backends(&endpoint, None, false, None);
        let response = tool_call(&server, "hubu_governance_summary", json!({}), None);
        request.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert_eq!(response["result"]["isError"], true, "{response}");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["http_status"], status);
        assert_eq!(structured["error_code"], code);
        let text: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(&text, structured);
        assert!(!response.to_string().contains("private-operation-canary"));
        assert!(!response.to_string().contains("hubu-token-canary"));
    }
    let (endpoint, request, handle) = one_shot_http_server(
        403,
        r#"{"error":"unrecognized-private-canary","error_code":"private-code-canary"}"#,
    );
    let server = server_with_backends(&endpoint, None, false, None);
    let response = tool_call(&server, "hubu_governance_summary", json!({}), None);
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert!(!response.to_string().contains("unrecognized-private-canary"));
    assert!(!response.to_string().contains("private-code-canary"));
}

#[test]
fn governance_reads_survive_unavailable_operation_store_and_never_fallback_to_gongbu() {
    for name in [
        "hubu_governance_summary",
        "hubu_list_governance_records",
        "hubu_get_governance_record",
    ] {
        let (endpoint, request, handle) =
            one_shot_http_server(200, r#"{"schema_version":"hubu-governance-v1"}"#);
        let mut server = server_with_backends(&endpoint, None, false, None);
        server.operation_registry = Arc::new(OperationRegistryCapability::Unavailable {
            reason_code: "configuration_missing",
        });
        let args = if name == "hubu_get_governance_record" {
            json!({"record_id":"lev_0123456789abcdef0123456789abcdef"})
        } else {
            json!({})
        };
        assert!(server
            .list_tools_for_snapshot()
            .iter()
            .any(|tool| tool["name"] == name));
        let response = tool_call(&server, name, args, None);
        assert_ne!(
            response["result"].get("isError"),
            Some(&json!(true)),
            "{response}"
        );
        assert!(request
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .starts_with("GET /governance/"));
        handle.join().unwrap();
    }
    let (hubu_endpoint, request, handle) = disconnect_after_request_server();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = server_with_backends(
        &hubu_endpoint,
        Some(&format!("http://{}", listener.local_addr().unwrap())),
        false,
        None,
    );
    let response = tool_call(&server, "hubu_governance_summary", json!({}), None);
    request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
    assert_eq!(response["error"]["data"]["retryable"], true);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[test]
fn save_artifact_tool_is_listed_only_for_human_allowlisted_folders() {
    let mut server = server_with_backends(
        "http://127.0.0.1:1",
        Some("http://127.0.0.1:2"),
        false,
        None,
    );
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
            "not yet succeeded",
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

/// Serve `responses` to consecutive connections, capturing each raw request.
fn scripted_http_server(
    responses: Vec<(u16, String)>,
) -> (String, Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for (status, body) in responses {
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
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end + 4]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
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
        }
    });
    (endpoint, receiver, handle)
}

fn request_body(raw: &str) -> Value {
    serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
fn opened_task_budget_binds_session_spend_and_shows_in_operation_status() {
    let money =
        |units: &str| json!({"units": units, "scale": 6, "currency": "usd", "display": "x"});
    let task_budget = |state: &str, consumed: &str| {
        json!({
            "task_budget_id": "tbg_0123456789ab", "task_id": "landing-page", "state": state,
            "agent_id": "agt_1", "budget_id": "bgt_1", "currency": "usd",
            "amount_limit": money("3000000"), "consumed_amount": money(consumed),
            "frozen_amount": money("0"), "remaining_amount": money("1766000"),
        })
    };
    let responses = vec![
        (200, json!({"task_budget": task_budget("open", "0"), "idempotent_replay": false}).to_string()),
        (200, json!({"decision": "allow", "decision_id": "decision-1", "auth_token_id": "token-1",
            "authorization_expires_at": "2999-01-01T00:00:00Z", "payment": null}).to_string()),
        (200, json!({"authorization_record": {"authorization_id": "decision-1", "status": "authorized",
            "budget_hold": {"task_budget_id": "tbg_0123456789ab"}}}).to_string()),
        (200, json!({"task_budgets": [
            // Another agent's budget for the same task name must not be chosen.
            task_budget("open", "999000").as_object().cloned().map(|mut other| {
                other.insert("task_budget_id".into(), json!("tbg_other0000000"));
                Value::Object(other)
            }).unwrap(),
            task_budget("open", "1234000"),
        ]}).to_string()),
        (200, json!({"task_budget": task_budget("closed", "1234000"), "idempotent_replay": false}).to_string()),
        (200, json!({"decision": "deny", "reasons": ["policy"], "payment": null}).to_string()),
    ];
    let (endpoint, requests, handle) = scripted_http_server(responses);
    let server = server_with_backends(&endpoint, None, false, None);

    let opened = tool_call(
        &server,
        "hubu_open_task_budget",
        json!({"agent_id": "agt_1", "task_id": "landing-page", "amount": "3"}),
        None,
    );
    let raw = requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|_| panic!("open did not reach Hubu: {opened}"));
    assert!(raw.starts_with("POST /task-budgets HTTP/1.1"));
    assert_eq!(request_body(&raw)["source"], "hubu-unified-mcp");
    assert!(opened["result"]["structuredContent"]["session_binding"]
        .as_str()
        .unwrap()
        .contains("draw from this task budget"));

    let spend = |tool_use_id: &str| {
        tool_call(
            &server,
            "hubu_authorize_spend",
            json!({"account_id": "acct_1", "amount": "2.00", "reason": "draft", "merchant": "gongbu.image"}),
            Some(json!({"claudecode/toolUseId": tool_use_id})),
        )
    };
    let authorized = spend("toolu_1");
    let raw = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(raw.starts_with("POST /spend/authorize HTTP/1.1"));
    assert_eq!(request_body(&raw)["task_id"], "landing-page");
    let handle_id = authorized["result"]["structuredContent"]["operation_handle"]
        .as_str()
        .unwrap()
        .to_string();

    let status = server.call_tool_from_snapshot(
        json!(8),
        json!({"name": "hubu_operation_status", "arguments": {"operation_handle": handle_id}}),
    );
    let raw = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(raw.starts_with("GET /spend/authorizations/show?"), "{raw}");
    let raw = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(raw.starts_with("GET /task-budgets?all=true HTTP/1.1"));
    let projected = &status["result"]["structuredContent"]["task_budget"];
    assert_eq!(projected["task_budget_id"], "tbg_0123456789ab");
    assert_eq!(projected["spent_amount"]["units"], "1234000");
    assert!(status["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("spent_amount"));

    let closed = tool_call(
        &server,
        "hubu_close_task_budget",
        json!({"task_budget_id": "tbg_0123456789ab"}),
        None,
    );
    let raw = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(raw.starts_with("POST /task-budgets/close HTTP/1.1"));
    assert!(closed["result"]["structuredContent"]["session_binding"]
        .as_str()
        .unwrap()
        .contains("closed"));

    spend("toolu_2");
    let raw = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(request_body(&raw)["task_id"], Value::Null);
    handle.join().unwrap();

    // Redelivering the first call after the task closed keeps its recorded task.
    let replay = spend("toolu_1");
    assert_eq!(
        replay["result"]["structuredContent"]["operation_handle"],
        authorized["result"]["structuredContent"]["operation_handle"]
    );
}

#[test]
fn task_budget_tool_arguments_fail_before_network() {
    let server = server_with_backends("http://127.0.0.1:1", None, false, None);
    for (name, arguments) in [
        (
            "hubu_open_task_budget",
            json!({"agent_id": "agt_1", "task_id": "t"}),
        ),
        (
            "hubu_open_task_budget",
            json!({"agent_id": "agt_1", "task_id": "t", "amount": "1", "operation_key": "x"}),
        ),
        ("hubu_close_task_budget", json!({})),
        (
            "hubu_close_task_budget",
            json!({"task_budget_id": "tbg_1&all=true"}),
        ),
        ("hubu_list_task_budgets", json!({"agent_id": "agt 1"})),
        ("hubu_list_task_budgets", json!({"include_all": "yes"})),
    ] {
        let response = tool_call(&server, name, arguments.clone(), None);
        assert!(
            response.get("error").is_some(),
            "{name} {arguments} should fail before network: {response}"
        );
    }
}

#[test]
fn platform_envelopes_keep_their_own_task_over_the_session_task() {
    let envelope = crate::operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
        "hubu.dev/platform-invocation": {"platform": "codex-controlled", "invocation_id": "i-1", "task_id": "linear:HUB-1"}
    })))
    .unwrap();
    let claude = crate::operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
        "claudecode/toolUseId": "toolu_9"
    })))
    .unwrap();
    let mut registry = crate::operation_registry::OperationRegistry::open_in_memory().unwrap();
    let arguments = json!({"amount": "1"});
    let envelope = registry
        .resolve_or_allocate(
            &envelope.with_session_task(Some("session-task")),
            "hubu_authorize_spend",
            &arguments,
        )
        .unwrap();
    assert_eq!(envelope.task_id.as_deref(), Some("linear:HUB-1"));
    let claude = registry
        .resolve_or_allocate(
            &claude.with_session_task(Some("session-task")),
            "hubu_authorize_spend",
            &arguments,
        )
        .unwrap();
    assert_eq!(claude.task_id.as_deref(), Some("session-task"));
}
