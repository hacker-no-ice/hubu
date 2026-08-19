use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde_json::{json, Value};

use crate::{BackendClient, BackendClients, BackendConfig, BackendOwner, Config};

use super::{catalog::tool_definitions, transport::call_tool};

const EXECUTION: &str = r#"{"schema_version":2,"execution_id":"exec-1","operation_key":"op-1","status":"pending","outcome":null,"failure":null,"authorization":{"amount_minor":25,"currency":"USD"},"created_at":"now","updated_at":"now","started_at":null,"completed_at":null}"#;

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
    })
    .unwrap()
    .gongbu
    .unwrap()
}

fn create_arguments() -> Value {
    json!({"schema_version":2,"spend_auth_token_id":"hubu-token-1","input":{"prompt":"circle","image_count":1},"input_schema_version":1,"workload_type":"image_generation","provider":"example","adapter":"fixture","model":"v1"})
}

#[test]
fn catalog_matches_standalone_gongbu_v2_contract() {
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../../gongbu-mcp/tests/fixtures/tool-definitions-v2.json"
    ))
    .unwrap();
    assert_eq!(tool_definitions(), expected);
}

#[test]
fn create_preserves_operation_key_authorization_and_no_retry_semantics() {
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", EXECUTION)]);
    let result = call_tool(
        &client(&endpoint, "gongbu-execution-secret"),
        "gongbu_create_execution",
        create_arguments(),
    );
    assert_eq!(result["isError"], false);
    assert_eq!(result["content"][0]["text"], EXECUTION);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("post /v2/executions "));
    assert!(request.contains("authorization: bearer gongbu-execution-secret"));
    assert!(request.contains("\"spend_auth_token_id\":\"hubu-token-1\""));
    assert!(!request.contains("account_id"));
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
        let result = call_tool(&client, "gongbu_create_execution", arguments);
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
    );
    let text = listed["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("private/file") && !text.contains("canary"));
    assert!(text.contains("[REDACTED]") && text.contains("width"));
    let artifact = call_tool(&client, "gongbu_get_artifact", json!({"artifact_id":"a-1"}));
    assert_eq!(artifact["isError"], false);
    assert_eq!(artifact["content"][1]["type"], "image");
    assert_eq!(artifact["content"][1]["mimeType"], "image/png");
}

#[test]
fn application_and_transport_failures_match_standalone_without_retry() {
    let conflict = r#"{"error":{"code":"immutable_scope_conflict","message":"provider-secret /private/path"}}"#;
    let (endpoint, _) = mock_server(vec![("409 Conflict", "application/json", conflict)]);
    let rejected = call_tool(
        &client(&endpoint, "secret"),
        "gongbu_create_execution",
        create_arguments(),
    );
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
    );
    assert_eq!(unavailable["isError"], true);
    assert!(unavailable["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("gongbu_unavailable"));
    thread::sleep(Duration::from_millis(10));
    assert_eq!(*accepts.lock().unwrap(), 1);
}
