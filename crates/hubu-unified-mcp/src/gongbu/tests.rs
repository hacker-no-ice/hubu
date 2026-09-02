use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::{
    operation_registry::GongbuContinuation, BackendClient, BackendClients, BackendConfig,
    BackendOwner, Config,
};

use super::{
    catalog::tool_definitions,
    response::{api_error, ApiErrorContext, ProviderCatalogResponse},
    transport::{call_tool, fetch_durable_execution_observation},
};

const EXECUTION: &str = r#"{"schema_version":2,"execution_id":"exec-1","operation_key":"op-1","status":"pending","outcome":"backend echoed op-1","failure":null,"authorization":{"amount_minor":25,"currency":"USD"},"created_at":"now","updated_at":"now","started_at":null,"completed_at":null}"#;
const PROVIDER_CATALOG: &str = r#"{"schema_version":1,"contracts":[{"contract":"hubu.flux-2-pro.text-to-image/v1","pricing_version":"bfl-flux-2-pro-usd-2026-08-28-v1","pricing_reviewed_on":"2026-08-28","target":{"workload_type":"image_generation","provider":"flux","adapter":"flux2_api","model":"flux-2-pro"},"capability":{"image_count":1,"output_formats":["png","jpeg"],"presets":[{"name":"1k","width":1024,"height":1024,"currency":"USD","rate_numerator_minor":3,"rate_denominator":1},{"name":"2k","width":1920,"height":1088,"currency":"USD","rate_numerator_minor":45,"rate_denominator":10},{"name":"4k","width":2048,"height":2048,"currency":"USD","rate_numerator_minor":75,"rate_denominator":10}]},"policies":{"generation_retries":0,"fallback":false,"poll":"bfl-async-status-poll-500ms-v1","artifact_delivery":"bfl-delivery-single-region-label-v1","recovery":"hubu-durable-async-resume-v1"},"readiness":{"configured":true,"credential_reference_present":true,"production_validated":true,"live_qualified":false,"live_qualification":"not_performed"}}]}"#;
const TARGET_ID: &str =
    "gongbu:target:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn mock_server(
    responses: Vec<(&'static str, &'static str, &'static str)>,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    thread::spawn(move || {
        for (status, content_type, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            captured.lock().unwrap().push(request);
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (address, requests)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap();
        }
        request.push_str(&line);
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    request.push_str(&String::from_utf8(body).unwrap());
    request
}

fn client(endpoint: &str, token: &str) -> BackendClient {
    BackendClients::new(Config {
        hubu: None,
        gongbu: Some(BackendConfig::new(BackendOwner::Gongbu, endpoint, token).unwrap()),
        ..Config::default()
    })
    .unwrap()
    .gongbu
    .unwrap()
}

fn create_arguments() -> Value {
    json!({"schema_version":2,"spend_auth_token_id":"hubu-token-1","input":{"prompt":"circle","image_count":1},"input_schema_version":1,"target_id":TARGET_ID})
}

fn continuation() -> GongbuContinuation {
    GongbuContinuation {
        operation_key: "op-1".into(),
        operation_handle: "hubu:public-operation:v1:test".into(),
        execution_id: None,
    }
}

#[test]
fn catalog_matches_owned_gongbu_v2_contract() {
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/fixtures/gongbu-tool-definitions-v2.json"
    ))
    .unwrap();
    assert_eq!(tool_definitions(), expected);
}

#[test]
fn provider_catalog_routes_read_only_and_returns_only_the_validated_contract() {
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", PROVIDER_CATALOG)]);
    let result = call_tool(
        &client(&endpoint, "gongbu-catalog-secret"),
        "gongbu_get_provider_catalog",
        json!({}),
        None,
    )
    .result;
    assert_eq!(result["isError"], false);
    let projected: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(projected["schema_version"], 1);
    assert_eq!(projected["contracts"][0]["target"]["provider"], "flux");
    assert_eq!(
        projected["contracts"][0]["capability"]["presets"][1],
        json!({
            "name": "2k",
            "width": 1920,
            "height": 1088,
            "currency": "USD",
            "rate_numerator_minor": 45,
            "rate_denominator": 10
        })
    );
    assert_eq!(
        projected["contracts"][0]["readiness"]["live_qualified"],
        false
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("get /v1/provider-catalog "));
    assert!(request.contains("authorization: bearer gongbu-catalog-secret"));
    assert!(!request.contains("api.bfl.ai"));
}

#[test]
fn create_schema_requires_only_an_opaque_target_id() {
    let create = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "gongbu_create_execution")
        .unwrap();
    assert!(create["inputSchema"].get("oneOf").is_none());
    assert!(create["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .contains(&json!("target_id")));
    for field in ["workload_type", "provider", "adapter", "model"] {
        assert!(create["inputSchema"]["properties"].get(field).is_none());
    }
}

#[test]
fn selectable_target_catalog_is_read_only_structured_and_sanitized() {
    let response = format!(
        r#"{{"schema_version":2,"targets":[{{"target_id":"{TARGET_ID}","workload_type":"image_generation","provider":"google","model":"gemini-image","execution_scope":{{"schema_version":1,"provider":"provider:google:gemini-developer","executor":"executor:gongbu:image","capability":"capability:image:generate","billing_merchant":"merchant:google"}},"image_sizes":["1k","2k"],"pricing":[{{"rule_id":"gemini-1k","selector":{{"image_size":"1k"}},"currency":"USD","components":[{{"unit":"image","rate_numerator_minor":4,"rate_denominator":1}}]}},{{"rule_id":"gemini-2k","selector":{{"image_size":"2k"}},"currency":"USD","components":[{{"unit":"image","rate_numerator_minor":8,"rate_denominator":1}}]}}]}}]}}"#
    );
    let response: &'static str = Box::leak(response.into_boxed_str());
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", response)]);
    let result = call_tool(
        &client(&endpoint, "gongbu-execution-secret"),
        "gongbu_list_execution_targets",
        json!({}),
        None,
    )
    .result;
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["structuredContent"]["targets"][0]["target_id"],
        TARGET_ID
    );
    assert_eq!(
        result["structuredContent"]["targets"][0]["image_sizes"],
        json!(["1k", "2k"])
    );
    let serialized = result.to_string();
    for private in ["credential", "endpoint", "headers", "config_version"] {
        assert!(!serialized.contains(private));
    }
    assert!(requests.lock().unwrap()[0].starts_with("GET /v2/execution-targets "));
}

#[test]
fn provider_catalog_rejects_arguments_and_unsanitized_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let result = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_get_provider_catalog",
        json!({"credential_reference":"private"}),
        None,
    )
    .result;
    assert_eq!(result["isError"], true);
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));

    let unsanitized = r#"{"schema_version":1,"contracts":[],"credential":"secret-canary"}"#;
    let (endpoint, _) = mock_server(vec![("200 OK", "application/json", unsanitized)]);
    let result = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_get_provider_catalog",
        json!({}),
        None,
    )
    .result;
    assert_eq!(result["isError"], true);
    assert!(result.to_string().contains("invalid_response"));
    assert!(!result.to_string().contains("secret-canary"));
}

#[test]
fn selectable_target_catalog_rejects_unapproved_backend_fields() {
    let response = r#"{"schema_version":2,"targets":[],"credential":"secret-canary"}"#;
    let (endpoint, _) = mock_server(vec![("200 OK", "application/json", response)]);
    let result = call_tool(
        &client(&endpoint, "gongbu-execution-secret"),
        "gongbu_list_execution_targets",
        json!({}),
        None,
    )
    .result;
    assert_eq!(result["isError"], true);
    assert!(result.to_string().contains("invalid_response"));
    assert!(!result.to_string().contains("secret-canary"));
}

#[test]
fn provider_catalog_rejects_contract_pricing_policy_and_readiness_drift() {
    let exact: Value = serde_json::from_str(PROVIDER_CATALOG).unwrap();
    for (pointer, changed) in [
        ("/contracts/0/target/model", json!("flux-2-pro-preview")),
        (
            "/contracts/0/capability/presets/1/rate_numerator_minor",
            json!(46),
        ),
        (
            "/contracts/0/policies/poll",
            json!("operator-selected-poll-policy"),
        ),
        (
            "/contracts/0/readiness/credential_reference_present",
            json!(false),
        ),
        ("/contracts/0/readiness/live_qualified", json!(true)),
    ] {
        let mut mutated = exact.clone();
        *mutated.pointer_mut(pointer).unwrap() = changed;
        let response: ProviderCatalogResponse = serde_json::from_value(mutated).unwrap();
        assert_eq!(response.validate().unwrap_err().code(), "invalid_response");
    }
}

#[test]
fn provider_catalog_accepts_order_independent_exact_subsets_and_rejects_bad_sets() {
    let flux = serde_json::from_str::<Value>(PROVIDER_CATALOG).unwrap()["contracts"][0].clone();
    let gemini = json!({
        "contract":"hubu.gemini-3.1-flash-lite-image.text-to-image/v1",
        "pricing_version":"google-gemini-3.1-flash-lite-image-usd-2026-09-01-v1",
        "pricing_reviewed_on":"2026-09-01",
        "target":{"workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"gemini-3.1-flash-lite-image"},
        "capability":{"image_count":1,"output_formats":["png","jpeg"],"presets":[{"name":"1k","width":1024,"height":1024,"currency":"USD","rate_numerator_minor":336,"rate_denominator":100}]},
        "policies":{"generation_retries":0,"fallback":false,"poll":"synchronous-response-v1","artifact_delivery":"google-inline-image-v1","recovery":"hubu-durable-synchronous-replay-v1"},
        "readiness":{"configured":true,"credential_reference_present":true,"production_validated":true,"live_qualified":false,"live_qualification":"not_performed"}
    });
    let gemini_full = json!({
        "contract":"hubu.gemini-3.1-flash-image.text-to-image/v1",
        "pricing_version":"google-gemini-3.1-flash-image-usd-2026-09-01-v1",
        "pricing_reviewed_on":"2026-09-01",
        "target":{"workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"gemini-3.1-flash-image"},
        "capability":{"image_count":1,"output_formats":["png","jpeg"],"presets":[
            {"name":"1k","width":1024,"height":1024,"currency":"USD","rate_numerator_minor":67,"rate_denominator":10},
            {"name":"2k","width":2048,"height":2048,"currency":"USD","rate_numerator_minor":101,"rate_denominator":10},
            {"name":"4k","width":4096,"height":4096,"currency":"USD","rate_numerator_minor":151,"rate_denominator":10}
        ]},
        "policies":{"generation_retries":0,"fallback":false,"poll":"synchronous-response-v1","artifact_delivery":"google-inline-image-v1","recovery":"hubu-durable-synchronous-replay-v1"},
        "readiness":{"configured":true,"credential_reference_present":true,"production_validated":true,"live_qualified":false,"live_qualification":"not_performed"}
    });

    for contracts in [
        vec![gemini.clone()],
        vec![gemini_full.clone()],
        vec![flux.clone()],
        vec![gemini.clone(), gemini_full.clone()],
        vec![gemini.clone(), gemini_full.clone(), flux.clone()],
        vec![flux.clone(), gemini_full.clone(), gemini.clone()],
        vec![flux.clone(), gemini.clone()],
    ] {
        let response: ProviderCatalogResponse =
            serde_json::from_value(json!({"schema_version":1,"contracts":contracts})).unwrap();
        response.validate().unwrap();
    }

    for contracts in [
        vec![gemini.clone(), gemini],
        vec![json!({"contract":"operator.unknown/v1"})],
    ] {
        let parsed = serde_json::from_value::<ProviderCatalogResponse>(
            json!({"schema_version":1,"contracts":contracts}),
        );
        assert!(parsed.is_err() || parsed.unwrap().validate().is_err());
    }
}

#[test]
fn target_id_execution_is_forwarded_without_a_raw_target_tuple() {
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", EXECUTION)]);
    let arguments = json!({
        "schema_version":2,
        "spend_auth_token_id":"hubu-token-1",
        "input":{"prompt":"circle","image_count":1,"image_size":"2k"},
        "input_schema_version":1,
        "target_id":TARGET_ID
    });
    let result = call_tool(
        &client(&endpoint, "gongbu-execution-secret"),
        "gongbu_create_execution",
        arguments,
        Some(&continuation()),
    )
    .result;
    assert_eq!(result["isError"], false);
    let request = &requests.lock().unwrap()[0];
    assert!(request.contains(&format!(r#""target_id":"{TARGET_ID}""#)));
    for field in ["workload_type", "provider", "adapter", "model"] {
        assert!(!request.contains(&format!(r#""{field}":"#)));
    }
}

#[test]
fn redaction_attestation_routes_read_only_and_preserves_only_the_strict_projection() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let response = json!({
        "schema_version":1,
        "attestation_contract":"gongbu.flux-redaction-attestation/v1",
        "allowlist_projection":true,
        "terminal_execution":true,
        "registered_provider_secret_resolved":true,
        "registered_provider_secret_absent_from_scanned_projections":true,
        "scan":{
            "logical_database_record_count":4,
            "artifact_metadata_record_count":1,
            "public_projection_count":3,
            "bytes_scanned":4096
        },
        "facts":{
            "authorization_snapshot_count":1,
            "claim_reference_count":1,
            "provider_attempt_count":1,
            "provider_submission_count":1,
            "durable_checkpoint_count":1,
            "provider_poll_count":2,
            "artifact_fetch_count":1,
            "artifact_count":1,
            "receipt_count":1,
            "settlement_delivery_count":1,
            "authorized_minor":3,
            "authorization_currency":"USD",
            "provider_cost_minor":3,
            "provider_cost_currency":"USD",
            "settled_minor":3,
            "settled_currency":"USD",
            "artifact_content_sha256":digest
        },
        "execution_sha256":digest,
        "artifact_sha256":digest,
        "settlement_sha256":digest,
        "combined_projection_sha256":digest
    })
    .to_string();
    let response = Box::leak(response.into_boxed_str());
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", response)]);
    let result = call_tool(
        &client(&endpoint, "gongbu-attestation-caller"),
        "gongbu_get_redaction_attestation",
        json!({"execution_id":"exec-1"}),
        None,
    )
    .result;
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("gongbu.flux-redaction-attestation/v1"));
    assert!(!text.contains("exec-1"));
    assert!(!text.contains("gongbu-attestation-caller"));
    assert!(!text.contains("http"));
    assert!(!text.contains("path"));
    assert!(!text.contains("operation_key"));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /v1/executions/exec-1/redaction-attestation "));
}

#[test]
fn redaction_attestation_rejects_probe_arguments_and_unsanitized_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let result = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_get_redaction_attestation",
        json!({"execution_id":"exec-1","candidate":"secret-canary"}),
        None,
    )
    .result;
    assert_eq!(result["isError"], true);
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));

    let digest = format!("sha256:{}", "a".repeat(64));
    let mut response = json!({
        "schema_version":1,
        "attestation_contract":"gongbu.flux-redaction-attestation/v1",
        "allowlist_projection":true,
        "terminal_execution":true,
        "registered_provider_secret_resolved":true,
        "registered_provider_secret_absent_from_scanned_projections":true,
        "scan":{"logical_database_record_count":4,"artifact_metadata_record_count":1,"public_projection_count":3,"bytes_scanned":4096},
        "facts":{"authorization_snapshot_count":1,"claim_reference_count":1,"provider_attempt_count":1,"provider_submission_count":1,"durable_checkpoint_count":1,"provider_poll_count":2,"artifact_fetch_count":1,"artifact_count":1,"receipt_count":1,"settlement_delivery_count":1,"authorized_minor":3,"authorization_currency":"USD","provider_cost_minor":3,"provider_cost_currency":"USD","settled_minor":3,"settled_currency":"USD","artifact_content_sha256":digest},
        "execution_sha256":digest,
        "artifact_sha256":digest,
        "settlement_sha256":digest,
        "combined_projection_sha256":digest
    });
    let mut detected = response.clone();
    detected["registered_provider_secret_absent_from_scanned_projections"] = json!(false);
    let detected = Box::leak(detected.to_string().into_boxed_str());
    let (endpoint, _) = mock_server(vec![("200 OK", "application/json", detected)]);
    let result = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_get_redaction_attestation",
        json!({"execution_id":"exec-1"}),
        None,
    )
    .result;
    assert_eq!(result["isError"], true);
    assert!(result.to_string().contains("invalid_response"));

    response["execution_id"] = json!("exec-secret");
    response["storage_path"] = json!("/private/secret");
    let response = Box::leak(response.to_string().into_boxed_str());
    let (endpoint, _) = mock_server(vec![("200 OK", "application/json", response)]);
    let result = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_get_redaction_attestation",
        json!({"execution_id":"exec-1"}),
        None,
    )
    .result;
    assert_eq!(result["isError"], true);
    assert!(result.to_string().contains("invalid_response"));
    assert!(!result.to_string().contains("exec-secret"));
    assert!(!result.to_string().contains("/private/secret"));
}

#[test]
fn create_keeps_private_operation_identity_internal() {
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", EXECUTION)]);
    let result = call_tool(
        &client(&endpoint, "gongbu-execution-secret"),
        "gongbu_create_execution",
        create_arguments(),
        Some(&continuation()),
    )
    .result;
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("operation_key") && !text.contains("op-1"));
    assert!(text.contains("hubu:public-operation:v1:test"));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("post /v2/executions "));
    assert!(request.contains("authorization: bearer gongbu-execution-secret"));
    assert!(request.contains("\"spend_auth_token_id\":\"hubu-token-1\""));
    assert!(!request.contains("account_id"));
}

#[test]
fn durable_observation_returns_only_validated_gongbu_timing() {
    let response = r#"{"schema_version":1,"execution_id":"exec-1","operation_key":"op-1","status":"succeeded","outcome":"succeeded","failure":null,"authorization":{"amount_minor":25,"currency":"USD"},"created_at":"2026-08-05T00:00:00Z","updated_at":"2026-08-05T00:00:04Z","started_at":"2026-08-05T00:00:00.100Z","completed_at":"2026-08-05T00:00:04Z","timing":{"schema_version":1,"scope":"gongbu_execution","execution_total_ms":4000,"provider_interaction_ms":3500,"non_provider_ms":500},"provider_transport":{"schema_version":1,"poll_count":2,"artifact_fetch_count":1}}"#;
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", response)]);
    let mut expected = continuation();
    expected.execution_id = Some("exec-1".into());
    let observed = fetch_durable_execution_observation(
        &client(&endpoint, "gongbu-execution-secret"),
        "exec-1",
        &expected,
    )
    .unwrap();

    assert_eq!(observed.lifecycle.status, "succeeded");
    assert_eq!(observed.execution_total_ms, Some(4_000));
    assert_eq!(observed.provider_interaction_ms, Some(3_500));
    assert_eq!(observed.non_provider_ms, Some(500));
    let provider_transport = observed.provider_transport.unwrap();
    assert_eq!(provider_transport.poll_count, 2);
    assert_eq!(provider_transport.artifact_fetch_count, 1);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /v1/executions/exec-1 "));
}

#[test]
fn unsafe_owner_overrides_and_missing_authorization_do_not_send() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let client = client(&endpoint, "secret");
    let mut cases = Vec::new();
    let mut missing = create_arguments();
    missing
        .as_object_mut()
        .unwrap()
        .remove("spend_auth_token_id");
    cases.push(missing);
    for field in [
        "account_id",
        "endpoint",
        "credentials",
        "headers",
        "artifact_root",
        "retry",
    ] {
        let mut arguments = create_arguments();
        arguments
            .as_object_mut()
            .unwrap()
            .insert(field.into(), json!("override"));
        cases.push(arguments);
    }
    for arguments in cases {
        let result = call_tool(&client, "gongbu_create_execution", arguments, None).result;
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid_request"));
    }
    assert!(listener.accept().is_err());
}

#[test]
fn artifacts_preserve_safe_metadata_and_image_content() {
    let list = r#"{"schema_version":1,"execution_id":"exec-1","artifacts":[{"artifact_id":"a-1","execution_id":"exec-1","kind":"image","media_type":"image/png","size_bytes":9,"sha256":"sha256:x","metadata":{"storage_key":"private/file","nested":{"token":"canary","width":1}},"metadata_schema_version":1,"created_at":"now"}]}"#;
    let (endpoint, _) = mock_server(vec![
        ("200 OK", "application/json", list),
        ("200 OK", "image/png", "png-bytes"),
    ]);
    let client = client(&endpoint, "secret");
    let listed = call_tool(
        &client,
        "gongbu_list_artifacts",
        json!({"execution_id":"exec-1"}),
        None,
    )
    .result;
    let text = listed["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("private/file") && !text.contains("canary"));
    assert!(text.contains("[REDACTED]") && text.contains("width"));
    let artifact = call_tool(
        &client,
        "gongbu_get_artifact",
        json!({"artifact_id":"a-1"}),
        None,
    )
    .result;
    assert_eq!(artifact["isError"], false);
    assert_eq!(artifact["content"][1]["type"], "image");
    assert_eq!(artifact["content"][1]["mimeType"], "image/png");
}

#[test]
fn application_and_transport_failures_preserve_contract_without_retry() {
    let conflict = r#"{"error":{"code":"immutable_scope_conflict","message":"provider-secret /private/path"}}"#;
    let (endpoint, _) = mock_server(vec![("409 Conflict", "application/json", conflict)]);
    let rejected = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_create_execution",
        create_arguments(),
        Some(&continuation()),
    )
    .result;
    assert_eq!(rejected["isError"], true);
    assert!(rejected["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("immutable_scope_conflict"));
    assert!(!rejected.to_string().contains("provider-secret"));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let accepts = Arc::new(Mutex::new(0_u32));
    let captured = accepts.clone();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        *captured.lock().unwrap() += 1;
        drop(stream);
    });
    let unavailable = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_create_execution",
        create_arguments(),
        Some(&continuation()),
    )
    .result;
    assert_eq!(unavailable["isError"], true);
    assert!(unavailable["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("gongbu_unavailable"));
    thread::sleep(Duration::from_millis(10));
    assert_eq!(*accepts.lock().unwrap(), 1);
}

fn projected_api_error(body: &str) -> Value {
    projected_api_error_with_context(body, ApiErrorContext::CreateExecutionV2)
}

fn projected_api_error_with_context(body: &str, context: ApiErrorContext) -> Value {
    let result = api_error(StatusCode::BAD_REQUEST, Some(body.as_bytes()), context).into_result();
    let result = serde_json::to_value(result).unwrap();
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn validation_diagnostics_project_only_allowlisted_reason_and_fields() {
    let target = projected_api_error(
        r#"{"schema_version":2,"error":{"code":"invalid_request","message":"target secret-canary","reason_code":"target_not_selectable","fields":["target_id"],"private_detail":"secret-canary"}}"#,
    );
    assert_eq!(
        target,
        json!({
            "schema_version": 2,
            "error": {
                "code": "invalid_request",
                "message": "request validation failed",
                "reason_code": "target_not_selectable",
                "fields": ["target_id"]
            }
        })
    );
    assert!(!target.to_string().contains("secret-canary"));

    let pricing = projected_api_error(
        r#"{"schema_version":2,"error":{"code":"invalid_request","message":"pricing secret-canary","reason_code":"pricing_selector_not_matched","fields":["input.image_size"],"private_detail":"secret-canary"}}"#,
    );
    assert_eq!(
        pricing,
        json!({
            "schema_version": 2,
            "error": {
                "code": "invalid_request",
                "message": "request validation failed",
                "reason_code": "pricing_selector_not_matched",
                "fields": ["input.image_size"]
            }
        })
    );
    assert!(!pricing.to_string().contains("secret-canary"));
}

#[test]
fn validation_diagnostics_drop_unknown_malformed_or_spoofed_values() {
    let generic = json!({
        "schema_version": 2,
        "error": {
            "code": "invalid_request",
            "message": "request validation failed"
        }
    });
    for body in [
        r#"{"schema_version":2,"error":{"code":"invalid_request","message":"secret-canary","reason_code":"secret-canary","fields":["input.image_size"]}}"#,
        r#"{"schema_version":2,"error":{"code":"invalid_request","message":"secret-canary","reason_code":"target_not_selectable","fields":["workload_type","provider","adapter","model","secret-canary"]}}"#,
        r#"{"schema_version":2,"error":{"code":"invalid_request","message":"secret-canary","reason_code":"pricing_selector_not_matched","fields":["secret-canary"]}}"#,
        r#"{"schema_version":2,"error":{"code":"invalid_request","message":"secret-canary","reason_code":{"value":"target_not_selectable","canary":"secret-canary"},"fields":"secret-canary"}}"#,
    ] {
        let projected = projected_api_error(body);
        assert_eq!(projected, generic, "body: {body}");
        assert!(!projected.to_string().contains("secret-canary"));
    }
}

#[test]
fn validation_diagnostics_require_v2_create_context() {
    let generic = json!({
        "schema_version": 2,
        "error": {
            "code": "invalid_request",
            "message": "request validation failed"
        }
    });
    let diagnostic = |schema| {
        format!(
            r#"{{"schema_version":{schema},"error":{{"code":"invalid_request","message":"secret-canary","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}}}"#
        )
    };

    assert_eq!(
        projected_api_error_with_context(&diagnostic(2), ApiErrorContext::General),
        generic
    );
    assert_eq!(projected_api_error(&diagnostic(1)), generic);
    assert_eq!(
        projected_api_error(
            r#"{"error":{"code":"invalid_request","message":"secret-canary","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#,
        ),
        generic
    );
}

#[test]
fn transport_projects_admission_diagnostics_only_for_v2_create() {
    const DIAGNOSTIC: &str = r#"{"schema_version":2,"error":{"code":"invalid_request","message":"secret-canary","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}}"#;
    let (endpoint, requests) = mock_server(vec![
        ("400 Bad Request", "application/json", DIAGNOSTIC),
        ("400 Bad Request", "application/json", DIAGNOSTIC),
    ]);
    let client = client(&endpoint, "secret");

    let created = call_tool(
        &client,
        "gongbu_create_execution",
        create_arguments(),
        Some(&continuation()),
    )
    .result;
    assert!(created.to_string().contains("pricing_selector_not_matched"));
    assert!(!created.to_string().contains("secret-canary"));

    let fetched = call_tool(
        &client,
        "gongbu_get_execution",
        json!({"execution_id":"exec-1"}),
        None,
    )
    .result;
    assert!(!fetched.to_string().contains("pricing_selector_not_matched"));
    assert!(!fetched.to_string().contains("secret-canary"));

    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("POST /v2/executions "));
    assert!(requests[1].starts_with("GET /v1/executions/exec-1 "));
}
