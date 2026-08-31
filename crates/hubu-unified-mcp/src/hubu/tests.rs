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
    assert_eq!(actual[1], gongbu::operation_status_definition());
    assert_eq!(actual[2], crate::resume_operation::tool_definition());

    let expected = super::catalog::tool_definitions();
    assert_eq!(&actual[3..], expected.as_slice());
    assert_eq!(expected.len(), 31);
    assert!(!actual
        .iter()
        .any(|tool| tool["name"] == "hubu_replace_budget"));
    let update = actual
        .iter()
        .find(|tool| tool["name"] == "hubu_update_budget")
        .unwrap();
    assert_eq!(
        update["inputSchema"]["required"],
        json!(["budget_id", "expected_revision", "amount_limit_cents"])
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
fn billed_reconciliation_schema_exposes_exact_cost_and_legacy_cents() {
    let tool = super::catalog::tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "hubu_reconcile_vendor_billed_claim")
        .expect("billed reconciliation tool");
    let receipt = &tool["inputSchema"]["properties"]["receipt"];
    let exact = &receipt["properties"]["actual_vendor_cost"];

    assert_eq!(exact["properties"]["amount"]["type"], "integer");
    assert_eq!(exact["properties"]["scale"]["maximum"], 18);
    assert_eq!(exact["properties"]["currency"]["enum"], json!(["usd"]));
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
    assert_eq!(tools.len(), 39);
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
    assert_eq!(degraded.len(), 37);
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
    assert_eq!(hubu_down.len(), 5);
    assert!(!hubu_down.iter().any(|tool| tool["name"]
        .as_str()
        .is_some_and(|name| name.starts_with("hubu_")
            && !matches!(name, "hubu_unified_capabilities" | "hubu_operation_status"))));
    assert!(!hubu_down
        .iter()
        .any(|tool| tool["name"] == "gongbu_create_execution"));
}

#[test]
fn unified_approval_profile_contains_only_callable_continuations() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = server_with_backends(&endpoint, Some(&endpoint), false, None);

    let response = tool_call(&server, "hubu_client_approval_profile", json!({}), None);
    let profile = &response["result"]["structuredContent"];
    assert!(profile["client_policy"]["auto_approve_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("hubu_get_spend_approval")));
    assert!(profile["client_policy"]["prompt_before_call_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("hubu_resolve_spend_approval")));
    assert!(profile["client_policy"]["auto_approve_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("hubu_budget_history")));
    assert!(profile["client_policy"]["prompt_before_call_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("hubu_update_budget")));
    assert!(!profile.to_string().contains("hubu_replace_budget"));
    assert!(profile["client_policy"]["auto_approve_tools"]
        .as_array()
        .unwrap()
        .contains(&json!(crate::governed_execution::TOOL_NAME)));
    assert!(profile["client_policy"]["hubu_policy_conditional_tools"]
        .as_array()
        .unwrap()
        .contains(&json!(crate::governed_execution::TOOL_NAME)));
    assert_eq!(
        profile["response_contract"]["agent_action"],
        "Show approval.review to the human, wait for an explicit approve or deny answer in chat, then call hubu_resolve_spend_approval with approval_request_id and that decision. The native client prompt confirms or cancels the formed call; cancelling does not submit a denial."
    );
    assert_eq!(
        response["result"]["content"][0]["text"],
        serde_json::to_string_pretty(profile).unwrap()
    );
    let callable = server
        .list_tools_for_snapshot()
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    for names in [
        &profile["client_policy"]["auto_approve_tools"],
        &profile["client_policy"]["hubu_policy_conditional_tools"],
        &profile["client_policy"]["prompt_before_call_tools"],
        &profile["tools"][0]["names"],
        &profile["tools"][1]["names"],
        &profile["tools"][2]["names"],
    ] {
        for name in names.as_array().unwrap() {
            assert!(
                callable.contains(name.as_str().unwrap()),
                "approval profile advertises unavailable tool {name}"
            );
        }
    }
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
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
            "hubu_health",
            empty.clone(),
            "GET",
            "/health",
            HubuRequestCapabilityV1::None,
        ),
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
            "hubu_add_policy",
            empty.clone(),
            "POST",
            "/policies",
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
            "hubu_export_policy",
            empty.clone(),
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
            "hubu_create_recurring_budget",
            empty.clone(),
            "POST",
            "/budgets/series",
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
            "hubu_list_ledger",
            empty.clone(),
            "GET",
            "/ledger",
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
    assert_eq!(cases.len(), 30);
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

    let mut called = false;
    let local = route_tool_call_v1(
        json!({"name":"hubu_client_approval_profile","arguments":{}}),
        true,
        true,
        None,
        |_| {
            called = true;
            Ok(json!({}))
        },
    )
    .unwrap();
    assert!(!called);
    assert_eq!(
        local["structuredContent"],
        super::catalog::approval_profile()
    );
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
            "hubu_export_policy",
            json!({"agent_id":"agent-1"}),
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
    assert!(names.contains(&"hubu_health".to_owned()));
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

    let response = tool_call(&server, "hubu_health", json!({}), None);
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

    let response = tool_call(&server, "hubu_health", json!({}), None);
    let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert!(raw.starts_with("GET /health HTTP/1.1"));
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
