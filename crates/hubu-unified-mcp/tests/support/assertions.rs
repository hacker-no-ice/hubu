use serde_json::Value;

use super::BackendStub;

pub fn tool_names(response: &Value) -> Vec<&str> {
    response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect()
}

pub fn assert_bearer_isolated(stub: &BackendStub, expected: &str, forbidden: &str) {
    let requests = stub.requests();
    assert!(!requests.is_empty());
    for request in requests {
        let raw = request.raw.to_ascii_lowercase();
        assert!(raw.contains(&format!(
            "authorization: bearer {}",
            expected.to_ascii_lowercase()
        )));
        assert!(!raw.contains(&forbidden.to_ascii_lowercase()));
    }
}
