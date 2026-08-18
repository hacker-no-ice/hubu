use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

use hubu_unified_mcp::{BackendConfig, BackendOwner, Config, Server};

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

fn server(endpoint: &str) -> Server {
    Server::new(Config {
        hubu: None,
        gongbu: Some(
            BackendConfig::new(BackendOwner::Gongbu, endpoint, "gongbu-operator-secret").unwrap(),
        ),
    })
    .unwrap()
}

fn exchange(server: Server, messages: &[Value]) -> Vec<Value> {
    let input = messages
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mut output = Vec::new();
    server.run(input.as_bytes(), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn call(id: u32, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

fn create_arguments() -> Value {
    json!({
        "schema_version": 2,
        "spend_auth_token_id": "hubu-token-1",
        "input": {"prompt":"circle","image_count":1},
        "input_schema_version": 1,
        "workload_type": "image_generation",
        "provider": "example",
        "adapter": "fixture",
        "model": "v1"
    })
}

#[test]
fn unified_catalog_matches_standalone_gongbu_v2_contract() {
    let (endpoint, _) = mock_server(vec![]);
    let response = exchange(
        server(&endpoint),
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let actual = response[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["name"].as_str().unwrap().starts_with("gongbu_"))
        .cloned()
        .collect::<Vec<_>>();
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../gongbu-mcp/tests/fixtures/tool-definitions-v2.json"
    ))
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn create_preserves_operation_key_and_is_forwarded_once_without_overrides() {
    let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", EXECUTION)]);
    let response = exchange(
        server(&endpoint),
        &[call(1, "gongbu_create_execution", create_arguments())],
    );

    assert_eq!(response[0]["result"]["isError"], false);
    assert_eq!(response[0]["result"]["content"][0]["text"], EXECUTION);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("post /v2/executions "));
    assert!(request.contains("authorization: bearer gongbu-operator-secret"));
    for forbidden in [
        "account_id",
        "artifact_root",
        "retry",
        "endpoint",
        "credentials",
    ] {
        assert!(!request.contains(forbidden));
    }
}

#[test]
fn artifact_routes_preserve_safe_metadata_and_image_content() {
    let list = r#"{"schema_version":1,"execution_id":"exec-1","artifacts":[{"artifact_id":"a-1","execution_id":"exec-1","kind":"image","media_type":"image/png","size_bytes":9,"sha256":"sha256:x","metadata":{"storage_key":"private/file","nested":{"token":"canary","width":1}},"metadata_schema_version":1,"created_at":"now"}]}"#;
    let (endpoint, requests) = mock_server(vec![
        ("200 OK", "application/json", list),
        ("200 OK", "image/png", "png-bytes"),
    ]);
    let response = exchange(
        server(&endpoint),
        &[
            call(1, "gongbu_list_artifacts", json!({"execution_id":"exec-1"})),
            call(2, "gongbu_get_artifact", json!({"artifact_id":"a-1"})),
        ],
    );

    let listed = response[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(!listed.contains("private/file") && !listed.contains("canary"));
    assert!(listed.contains("[REDACTED]") && listed.contains("width"));
    assert_eq!(response[1]["result"]["isError"], false);
    assert_eq!(response[1]["result"]["content"][1]["type"], "image");
    assert_eq!(response[1]["result"]["content"][1]["mimeType"], "image/png");
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /v1/executions/exec-1/artifacts "));
    assert!(requests[1].starts_with("GET /v1/artifacts/a-1 "));
}

#[test]
fn backend_application_errors_match_standalone_gongbu_contract() {
    let body = r#"{"error":{"code":"immutable_scope_conflict","message":"provider-secret /private/path"}}"#;
    let (endpoint, _) = mock_server(vec![("409 Conflict", "application/json", body)]);
    let response = exchange(
        server(&endpoint),
        &[call(1, "gongbu_create_execution", create_arguments())],
    );
    let result = &response[0]["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["content"][0]["text"],
        r#"{"error":{"code":"immutable_scope_conflict","message":"operation key was already used with different immutable input"},"schema_version":2}"#
    );
    assert!(!result.to_string().contains("provider-secret"));
    assert!(!result.to_string().contains("private/path"));
}

#[test]
fn invalid_owner_overrides_never_reach_gongbu() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut missing_authorization = create_arguments();
    missing_authorization
        .as_object_mut()
        .unwrap()
        .remove("spend_auth_token_id");
    let mut messages = vec![call(99, "gongbu_create_execution", missing_authorization)];
    for (index, field) in [
        "account_id",
        "endpoint",
        "credentials",
        "headers",
        "pricing",
        "artifact_root",
        "deadline_ms",
        "retry",
    ]
    .iter()
    .enumerate()
    {
        let mut arguments = create_arguments();
        arguments
            .as_object_mut()
            .unwrap()
            .insert((*field).into(), json!("override"));
        messages.push(call(index as u32, "gongbu_create_execution", arguments));
    }
    let responses = exchange(server(&endpoint), &messages);
    assert!(responses.iter().all(|response| {
        response["result"]["isError"] == true
            && response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("invalid_request")
    }));
    assert!(listener.accept().is_err());
}

#[test]
fn gongbu_route_uses_only_gongbu_backend_and_credential() {
    let hubu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    hubu_listener.set_nonblocking(true).unwrap();
    let hubu_endpoint = format!("http://{}", hubu_listener.local_addr().unwrap());
    let (gongbu_endpoint, requests) = mock_server(vec![("200 OK", "application/json", EXECUTION)]);
    let server = Server::new(Config {
        hubu: Some(
            BackendConfig::new(BackendOwner::Hubu, hubu_endpoint, "hubu-control-secret").unwrap(),
        ),
        gongbu: Some(
            BackendConfig::new(
                BackendOwner::Gongbu,
                gongbu_endpoint,
                "gongbu-execution-secret",
            )
            .unwrap(),
        ),
    })
    .unwrap();

    let response = exchange(
        server,
        &[call(1, "gongbu_create_execution", create_arguments())],
    );
    assert_eq!(response[0]["result"]["isError"], false);
    assert!(hubu_listener.accept().is_err());
    let request = requests.lock().unwrap()[0].to_ascii_lowercase();
    assert!(request.contains("authorization: bearer gongbu-execution-secret"));
    assert!(request.contains("\"spend_auth_token_id\":\"hubu-token-1\""));
    assert!(!request.contains("hubu-control-secret"));
}

#[test]
fn outage_returns_standalone_transport_error_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let accepts = Arc::new(Mutex::new(0_u32));
    let captured = accepts.clone();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        *captured.lock().unwrap() += 1;
        drop(stream);
    });
    let response = exchange(
        server(&endpoint),
        &[call(1, "gongbu_create_execution", create_arguments())],
    );
    assert_eq!(response[0]["result"]["isError"], true);
    assert!(response[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("gongbu_unavailable"));
    assert_eq!(*accepts.lock().unwrap(), 1);
}

#[test]
fn unconfigured_gongbu_fails_before_forwarding() {
    let response = exchange(
        Server::new(Config::default()).unwrap(),
        &[call(
            1,
            "gongbu_get_execution",
            json!({"execution_id":"exec-1"}),
        )],
    );
    assert_eq!(response[0]["error"]["code"], -32010);
    assert_eq!(response[0]["error"]["data"]["code"], "backend_unconfigured");
    assert_eq!(response[0]["error"]["data"]["owner"], "gongbu");
    assert_eq!(response[0]["error"]["data"]["retryable"], false);
}
