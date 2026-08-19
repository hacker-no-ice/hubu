#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;

use serde_json::{json, Value};
use support::{BackendKind, BackendStub, McpProcess};

const HUBU_TOKEN: &str = "hub107-hubu-unified-credential";
const GONGBU_TOKEN: &str = "hub107-gongbu-unified-credential";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Hubu,
    Gongbu,
}

struct GoldenCase {
    name: &'static str,
    owner: Owner,
    method: &'static str,
    path: &'static str,
    arguments: Value,
    meta: Option<Value>,
}

fn platform_meta(operation_key: &str) -> Value {
    json!({"hubu.dev/platform-invocation": {
        "platform": "codex",
        "installation_id": "installation-hub-107",
        "invocation_id": format!("invocation-{operation_key}"),
        "operation_key": operation_key,
        "task_id": "linear:HUB-107"
    }})
}

fn execution_arguments() -> Value {
    json!({
        "schema_version": 2,
        "spend_auth_token_id": "fixture-no-spend-token",
        "input": {"prompt": "deterministic no-spend fixture", "image_count": 1},
        "input_schema_version": 1,
        "workload_type": "image_generation",
        "provider": "fixture",
        "adapter": "fixture",
        "model": "fixture-v1"
    })
}

fn cases() -> Vec<GoldenCase> {
    let hubu = Owner::Hubu;
    let gongbu = Owner::Gongbu;
    vec![
        GoldenCase {
            name: "gongbu_create_execution",
            owner: gongbu,
            method: "POST",
            path: "/v2/executions",
            arguments: execution_arguments(),
            meta: None,
        },
        GoldenCase {
            name: "gongbu_get_artifact",
            owner: gongbu,
            method: "GET",
            path: "/v1/artifacts/artifact-107",
            arguments: json!({"artifact_id":"artifact-107"}),
            meta: None,
        },
        GoldenCase {
            name: "gongbu_get_execution",
            owner: gongbu,
            method: "GET",
            path: "/v1/executions/exec-107",
            arguments: json!({"execution_id":"exec-107"}),
            meta: None,
        },
        GoldenCase {
            name: "gongbu_list_artifacts",
            owner: gongbu,
            method: "GET",
            path: "/v1/executions/exec-107/artifacts",
            arguments: json!({"execution_id":"exec-107"}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_add_policy",
            owner: hubu,
            method: "POST",
            path: "/policies",
            arguments: json!({"policy_yaml":"fixture"}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_apply_policy",
            owner: hubu,
            method: "POST",
            path: "/policies",
            arguments: json!({"policy_yaml":"fixture"}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_authorize_spend",
            owner: hubu,
            method: "POST",
            path: "/spend/authorize",
            arguments: json!({"account_id":"account-107","amount_cents":0,"reason":"no-spend parity fixture"}),
            meta: Some(platform_meta("authorize-107")),
        },
        GoldenCase {
            name: "hubu_client_approval_profile",
            owner: hubu,
            method: "LOCAL",
            path: "",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_create_budget",
            owner: hubu,
            method: "POST",
            path: "/budgets",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_create_recurring_budget",
            owner: hubu,
            method: "POST",
            path: "/budgets/series",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_export_policy",
            owner: hubu,
            method: "GET",
            path: "/policies/export",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_get_executor_claim",
            owner: hubu,
            method: "GET",
            path: "/spend/executor/claim?claim_id=claim-107",
            arguments: json!({"claim_id":"claim-107"}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_health",
            owner: hubu,
            method: "GET",
            path: "/health",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_list_agents",
            owner: hubu,
            method: "GET",
            path: "/agents",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_list_budgets",
            owner: hubu,
            method: "GET",
            path: "/budgets",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_list_claims_requiring_reconciliation",
            owner: hubu,
            method: "GET",
            path: "/spend/executor/reconciliation",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_list_ledger",
            owner: hubu,
            method: "GET",
            path: "/ledger",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_list_users",
            owner: hubu,
            method: "GET",
            path: "/users",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_policy_diff",
            owner: hubu,
            method: "GET",
            path: "/policies/diff?from_revision=1",
            arguments: json!({"from_revision":1}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_policy_history",
            owner: hubu,
            method: "GET",
            path: "/policies/history",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_reconcile_vendor_billed_claim",
            owner: hubu,
            method: "POST",
            path: "/spend/executor/settle",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_reconcile_vendor_did_not_bill_claim",
            owner: hubu,
            method: "POST",
            path: "/spend/executor/release",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_register_agent",
            owner: hubu,
            method: "POST",
            path: "/agents/register",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_register_human",
            owner: hubu,
            method: "POST",
            path: "/init",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_registration_guidance",
            owner: hubu,
            method: "GET",
            path: "/registration/guidance",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_replace_budget",
            owner: hubu,
            method: "POST",
            path: "/budgets/replace",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_revoke_budget",
            owner: hubu,
            method: "POST",
            path: "/budgets/revoke",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_revoke_spending_target",
            owner: hubu,
            method: "POST",
            path: "/user/spending-target/revoke",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_set_spending_target",
            owner: hubu,
            method: "POST",
            path: "/user/spending-target",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_show_policy",
            owner: hubu,
            method: "GET",
            path: "/policies/show",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_show_spending_targets",
            owner: hubu,
            method: "GET",
            path: "/user/spending-target",
            arguments: json!({}),
            meta: None,
        },
        GoldenCase {
            name: "hubu_submit_spend",
            owner: hubu,
            method: "POST",
            path: "/spend",
            arguments: json!({"account_id":"account-107","amount_cents":0,"reason":"no-spend parity fixture"}),
            meta: Some(platform_meta("submit-107")),
        },
    ]
}

fn routing_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/unified-mcp-routing-v1.json"
    ))
    .unwrap()
}

fn assert_complete_unique_matrix(cases: &[GoldenCase]) {
    let actual = cases.iter().map(|case| case.name).collect::<BTreeSet<_>>();
    assert_eq!(
        actual.len(),
        cases.len(),
        "golden case names contain duplicates"
    );
    assert_eq!(
        cases.len(),
        32,
        "golden matrix must contain exactly 32 cases"
    );
    let fixture = routing_fixture();
    let expected_names = fixture["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["owner"] != "router")
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    let expected = expected_names.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        expected_names.len(),
        32,
        "routing fixture must map 32 tools"
    );
    assert_eq!(
        expected.len(),
        expected_names.len(),
        "routing fixture contains duplicate mapped tool names"
    );
    assert_eq!(
        actual, expected,
        "golden matrix has an omission or extra tool"
    );
    assert_eq!(
        cases
            .iter()
            .filter(|case| case.owner == Owner::Hubu)
            .count(),
        28
    );
    assert_eq!(
        cases
            .iter()
            .filter(|case| case.owner == Owner::Gongbu)
            .count(),
        4
    );
}

fn execution_response() -> Value {
    json!({
        "schema_version":2,
        "execution_id":"exec-107",
        "operation_key":"operation-107",
        "status":"succeeded",
        "outcome":"deterministic-no-spend",
        "failure":null,
        "authorization":{"amount_minor":0,"currency":"USD"},
        "created_at":"2026-08-19T00:00:00Z",
        "updated_at":"2026-08-19T00:00:01Z",
        "started_at":"2026-08-19T00:00:00Z",
        "completed_at":"2026-08-19T00:00:01Z"
    })
}

fn artifact_list_response() -> Value {
    json!({
        "schema_version":2,
        "execution_id":"exec-107",
        "artifacts":[]
    })
}

fn success_body(case: &GoldenCase) -> Value {
    match case.name {
        "gongbu_create_execution" | "gongbu_get_execution" => execution_response(),
        "gongbu_list_artifacts" => artifact_list_response(),
        "gongbu_get_artifact" => unreachable!("artifact success uses image bytes"),
        _ => json!({"fixture":"HUB-107","tool":case.name,"status":"ok"}),
    }
}

fn call(process: &mut McpProcess, id: u64, case: &GoldenCase) -> Value {
    match &case.meta {
        Some(meta) => process.call_with_meta(id, case.name, case.arguments.clone(), meta.clone()),
        None => process.call(id, case.name, case.arguments.clone()),
    }
}

fn call_error(process: &mut McpProcess, id: u64, case: &GoldenCase) -> Value {
    if case.name == "hubu_client_approval_profile" {
        return process.call(id, case.name, json!({"unexpected":true}));
    }
    call(process, id, case)
}

fn configure_success(case: &GoldenCase, unified: &BackendStub, standalone: &BackendStub) {
    if case.name == "hubu_client_approval_profile" {
        return;
    }
    if case.name == "gongbu_get_artifact" {
        unified.respond_bytes(
            case.method,
            case.path,
            200,
            "image/png",
            b"\x89PNG\r\n\x1a\n",
        );
        standalone.respond_bytes(
            case.method,
            case.path,
            200,
            "image/png",
            b"\x89PNG\r\n\x1a\n",
        );
        return;
    }
    let body = success_body(case);
    if case.name == "hubu_health" {
        unified.respond_sequence_json(
            "GET",
            "/health",
            [(200, json!({"status":"ok"})), (200, body.clone())],
        );
    } else {
        unified.respond_json(case.method, case.path, 200, body.clone());
    }
    standalone.respond_json(case.method, case.path, 200, body);
}

fn configure_error(case: &GoldenCase, unified: &BackendStub, standalone: &BackendStub) {
    if case.name == "hubu_client_approval_profile" {
        return;
    }
    let body = match case.owner {
        Owner::Hubu => json!({"error":format!("HUB-107 application error for {}", case.name)}),
        Owner::Gongbu => json!({"error":{"code":"invalid_request"}}),
    };
    if case.name == "hubu_health" {
        unified.respond_sequence_json(
            "GET",
            "/health",
            [(200, json!({"status":"ok"})), (400, body.clone())],
        );
    } else {
        unified.respond_json(case.method, case.path, 400, body.clone());
    }
    standalone.respond_json(case.method, case.path, 400, body);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with standalone debug adapters"]
fn all_mapped_tools_have_golden_success_and_error_parity() {
    let cases = cases();
    assert_complete_unique_matrix(&cases);

    let unified_hubu = BackendStub::start(BackendKind::Hubu);
    let unified_gongbu = BackendStub::start(BackendKind::Gongbu);
    let standalone_hubu = BackendStub::start(BackendKind::Hubu);
    let standalone_gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut unified = McpProcess::start(
        Some((&unified_hubu, HUBU_TOKEN)),
        Some((&unified_gongbu, GONGBU_TOKEN)),
    );
    let mut hubu = McpProcess::start_standalone_hubu(&standalone_hubu);
    let mut gongbu = McpProcess::start_standalone_gongbu(&standalone_gongbu);
    unified.initialize();
    hubu.initialize();
    gongbu.initialize();

    let mut id = 10;
    for case in &cases {
        let (unified_backend, standalone_backend, standalone) = match case.owner {
            Owner::Hubu => (&unified_hubu, &standalone_hubu, &mut hubu),
            Owner::Gongbu => (&unified_gongbu, &standalone_gongbu, &mut gongbu),
        };
        configure_success(case, unified_backend, standalone_backend);
        let expected = call(standalone, id, case);
        let actual = call(&mut unified, id, case);
        assert_eq!(
            actual,
            expected,
            "{} success parity; unified requests: {:?}; standalone requests: {:?}",
            case.name,
            unified_backend.requests(),
            standalone_backend.requests()
        );
        id += 1;

        configure_error(case, unified_backend, standalone_backend);
        let expected = call_error(standalone, id, case);
        let actual = call_error(&mut unified, id, case);
        assert_eq!(actual, expected, "{} error parity", case.name);
        match case.owner {
            Owner::Hubu => assert!(
                expected.get("error").is_some(),
                "{} must exercise an error",
                case.name
            ),
            Owner::Gongbu => assert_eq!(
                expected["result"]["isError"], true,
                "{} must exercise an error",
                case.name
            ),
        }
        id += 1;
    }

    unified.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
    hubu.finish(&["hub107-hubu-standalone-credential"]);
    gongbu.finish(&["hub107-gongbu-standalone-credential"]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with standalone debug adapters"]
fn approval_metadata_is_injected_identically_without_forwarding_meta() {
    let unified_hubu = BackendStub::start(BackendKind::Hubu);
    let unified_gongbu = BackendStub::start(BackendKind::Gongbu);
    let standalone_hubu = BackendStub::start(BackendKind::Hubu);
    let mut unified = McpProcess::start(
        Some((&unified_hubu, HUBU_TOKEN)),
        Some((&unified_gongbu, GONGBU_TOKEN)),
    );
    let mut standalone = McpProcess::start_standalone_hubu(&standalone_hubu);
    unified.initialize();
    standalone.initialize();
    let case = cases()
        .into_iter()
        .find(|case| case.name == "hubu_authorize_spend")
        .unwrap();
    configure_success(&case, &unified_hubu, &standalone_hubu);

    let expected = call(&mut standalone, 10, &case);
    let actual = call(&mut unified, 10, &case);
    assert_eq!(actual, expected);
    for request in [
        unified_hubu
            .requests()
            .into_iter()
            .find(|request| request.path == case.path)
            .unwrap(),
        standalone_hubu
            .requests()
            .into_iter()
            .find(|request| request.path == case.path)
            .unwrap(),
    ] {
        assert!(request.raw.contains("\"operation_key\":\"authorize-107\""));
        assert!(request.raw.contains("\"task_id\":\"linear:HUB-107\""));
        assert!(!request.raw.contains("hubu.dev/platform-invocation"));
    }
    unified.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
    standalone.finish(&["hub107-hubu-standalone-credential"]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with standalone debug adapters"]
fn artifact_image_content_matches_at_the_json_value_level() {
    let unified_hubu = BackendStub::start(BackendKind::Hubu);
    let unified_gongbu = BackendStub::start(BackendKind::Gongbu);
    let standalone_gongbu = BackendStub::start(BackendKind::Gongbu);
    let mut unified = McpProcess::start(
        Some((&unified_hubu, HUBU_TOKEN)),
        Some((&unified_gongbu, GONGBU_TOKEN)),
    );
    let mut standalone = McpProcess::start_standalone_gongbu(&standalone_gongbu);
    unified.initialize();
    standalone.initialize();
    let case = cases()
        .into_iter()
        .find(|case| case.name == "gongbu_get_artifact")
        .unwrap();
    configure_success(&case, &unified_gongbu, &standalone_gongbu);

    let expected = call(&mut standalone, 10, &case);
    let actual = call(&mut unified, 10, &case);
    assert_eq!(actual, expected);
    assert_eq!(actual["result"]["content"][1]["type"], "image");
    assert_eq!(actual["result"]["content"][1]["mimeType"], "image/png");
    assert_eq!(actual["result"]["content"][1]["data"], "iVBORw0KGgo=");
    unified.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
    standalone.finish(&["hub107-gongbu-standalone-credential"]);
}
