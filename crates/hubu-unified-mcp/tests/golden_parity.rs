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
        "invocation_id": format!("invocation-{operation_key}"),
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
            name: "gongbu_create_execution",
            owner: gongbu,
            method: "POST",
            path: "/v2/executions",
            arguments: execution_arguments(),
            meta: None,
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

fn execution_response_for(operation_key: &str) -> Value {
    json!({
        "schema_version":2,
        "execution_id":"exec-107",
        "operation_key":operation_key,
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

fn execution_response() -> Value {
    execution_response_for("operation-107")
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
        "hubu_authorize_spend" => json!({
            "fixture":"HUB-125",
            "tool":case.name,
            "status":"ok",
            "decision":"allow",
            "decision_id":format!("decision-{}", case.name),
            "auth_token_id":"fixture-no-spend-token"
        }),
        "hubu_submit_spend" => json!({
            "fixture":"HUB-125",
            "tool":case.name,
            "status":"ok",
            "decision":"allow",
            "decision_id":format!("decision-{}", case.name),
            "auth_token_id":format!("authorization-{}", case.name)
        }),
        _ => json!({"fixture":"HUB-107","tool":case.name,"status":"ok"}),
    }
}

fn call(process: &mut McpProcess, id: u64, case: &GoldenCase) -> Value {
    match &case.meta {
        Some(meta) => process.call_with_meta(id, case.name, case.arguments.clone(), meta.clone()),
        None => process.call(id, case.name, case.arguments.clone()),
    }
}

fn configure_success(case: &GoldenCase, backend: &BackendStub) {
    if case.name == "hubu_client_approval_profile" {
        return;
    }
    if case.name == "gongbu_get_artifact" {
        backend.respond_bytes(
            case.method,
            case.path,
            200,
            "image/png",
            b"\x89PNG\r\n\x1a\n",
        );
        return;
    }
    backend.respond_json(case.method, case.path, 200, success_body(case));
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with stamped unified metadata"]
fn all_mapped_tools_have_unified_owned_golden_routing_coverage() {
    let cases = cases();
    assert_complete_unique_matrix(&cases);

    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    for case in &cases {
        configure_success(
            case,
            match case.owner {
                Owner::Hubu => &hubu,
                Owner::Gongbu => &gongbu,
            },
        );
    }

    let mut unified = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    unified.initialize();

    for (offset, case) in cases.iter().enumerate() {
        if case.name == "gongbu_create_execution" {
            let authorization = hubu
                .requests()
                .into_iter()
                .rev()
                .find(|request| request.path == "/spend/authorize")
                .expect("authorization must precede execution creation");
            let body = authorization
                .raw
                .split_once("\r\n\r\n")
                .expect("captured HTTP request has a body")
                .1;
            let value: Value = serde_json::from_str(body).expect("authorization body is JSON");
            let operation_key = value["operation_key"]
                .as_str()
                .expect("unified MCP injects the private operation key");
            gongbu.respond_json(
                case.method,
                case.path,
                200,
                execution_response_for(operation_key),
            );
        }
        let response = call(&mut unified, 10 + offset as u64, case);
        assert!(
            response.get("error").is_none(),
            "{} unexpectedly failed: {response}",
            case.name
        );
        if case.name == "hubu_client_approval_profile" {
            assert_eq!(
                response["result"]["structuredContent"]["protocol_version"],
                "hubu-mcp-client-approval-v1"
            );
            continue;
        }
        if case.name == "gongbu_get_artifact" {
            assert_eq!(response["result"]["content"][1]["type"], "image");
            assert_eq!(response["result"]["content"][1]["mimeType"], "image/png");
            assert_eq!(response["result"]["content"][1]["data"], "iVBORw0KGgo=");
        }
        let backend = match case.owner {
            Owner::Hubu => &hubu,
            Owner::Gongbu => &gongbu,
        };
        let request = backend
            .requests()
            .into_iter()
            .rev()
            .find(|request| request.method == case.method && request.path == case.path)
            .unwrap_or_else(|| {
                panic!(
                    "{} did not route to {} {}",
                    case.name, case.method, case.path
                )
            });
        assert!(
            !request.raw.contains("hubu.dev/platform-invocation"),
            "{} forwarded trusted MCP metadata",
            case.name
        );
    }

    unified.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with stamped unified metadata"]
fn operation_identity_is_injected_without_forwarding_trusted_metadata() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    let case = cases()
        .into_iter()
        .find(|case| case.name == "hubu_authorize_spend")
        .unwrap();
    configure_success(&case, &hubu);
    let mut unified = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    unified.initialize();

    let response = call(&mut unified, 10, &case);
    assert!(response.get("error").is_none(), "{response}");
    let request = hubu
        .requests()
        .into_iter()
        .find(|request| request.path == case.path)
        .unwrap();
    assert!(request
        .raw
        .contains("\"operation_key\":\"hubu:operation:v1:codex:"));
    assert!(request.raw.contains("\"task_id\":\"linear:HUB-107\""));
    assert!(!request.raw.contains("hubu.dev/platform-invocation"));

    unified.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}

#[test]
#[ignore = "runs through scripts/integration-unified-mcp.sh with stamped unified metadata"]
fn backend_application_failures_are_owned_redacted_and_isolated() {
    let hubu = BackendStub::start(BackendKind::Hubu);
    let gongbu = BackendStub::start(BackendKind::Gongbu);
    hubu.respond_json(
        "GET",
        "/users",
        400,
        json!({"error":format!("rejected {HUBU_TOKEN}")}),
    );
    gongbu.respond_json(
        "GET",
        "/v1/executions/exec-107",
        400,
        json!({"error":{"code":"invalid_request","secret":GONGBU_TOKEN}}),
    );
    gongbu.respond_json("GET", "/v1/executions/exec-ok", 200, execution_response());
    let mut unified = McpProcess::start(Some((&hubu, HUBU_TOKEN)), Some((&gongbu, GONGBU_TOKEN)));
    unified.initialize();

    let hubu_error = unified.call(10, "hubu_list_users", json!({}));
    assert_eq!(hubu_error["error"]["code"], -32000);
    assert!(!hubu_error.to_string().contains(HUBU_TOKEN));

    let gongbu_error = unified.call(
        11,
        "gongbu_get_execution",
        json!({"execution_id":"exec-107"}),
    );
    assert_eq!(gongbu_error["result"]["isError"], true);
    assert!(!gongbu_error.to_string().contains(GONGBU_TOKEN));

    let healthy = unified.call(
        12,
        "gongbu_get_execution",
        json!({"execution_id":"exec-ok"}),
    );
    assert_eq!(healthy["result"]["isError"], false);

    unified.finish(&[HUBU_TOKEN, GONGBU_TOKEN]);
}
