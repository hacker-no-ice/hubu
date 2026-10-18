use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn binary_initializes_lists_tools_and_exits_on_stdin_close() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hubu-unified-mcp"))
        .env_remove("HUBU_UNIFIED_OPERATION_STATE_PATH")
        .env_remove("HUBU_UNIFIED_HUBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_HUBU_BEARER_TOKEN")
        .env_remove("HUBU_UNIFIED_GONGBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_GONGBU_BEARER_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("unified MCP binary should start");

    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize"}}"#).unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    drop(stdin);

    let output = child
        .wait_with_output()
        .expect("unified MCP binary should shut down");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 2);
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        "hubu-unified-mcp"
    );
    assert_eq!(
        responses[0]["result"]["capabilities"]["experimental"]["hubu.dev/unified-mcp"]
            ["operation_registry"]["state"],
        "unavailable"
    );
    let names = responses[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "hubu_unified_capabilities",
            "hubu_feedback_guidance",
            "hubu_prepare_feedback"
        ]
    );
}

#[test]
fn initialized_monitor_stops_cleanly_on_eof() {
    let state = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hubu-unified-mcp"))
        .env(
            "HUBU_UNIFIED_OPERATION_STATE_PATH",
            state.path().join("operations.sqlite3"),
        )
        .env_remove("HUBU_UNIFIED_HUBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_HUBU_BEARER_TOKEN")
        .env_remove("HUBU_UNIFIED_GONGBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_GONGBU_BEARER_TOKEN")
        .env("HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS", "10")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize"}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    drop(stdin);

    let started = Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn binary_rejects_invalid_configuration_without_printing_credentials() {
    let state = tempfile::tempdir().unwrap();
    let secret = "credential-that-must-not-appear";
    let output = Command::new(env!("CARGO_BIN_EXE_hubu-unified-mcp"))
        .env(
            "HUBU_UNIFIED_OPERATION_STATE_PATH",
            state.path().join("operations.sqlite3"),
        )
        .env("HUBU_UNIFIED_HUBU_ENDPOINT", "https://url-secret@hubu.test")
        .env("HUBU_UNIFIED_HUBU_BEARER_TOKEN", secret)
        .env_remove("HUBU_UNIFIED_GONGBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_GONGBU_BEARER_TOKEN")
        .stdin(Stdio::null())
        .output()
        .expect("unified MCP binary should report invalid configuration");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("hubu backend endpoint"));
    assert!(!stderr.contains("url-secret"));
    assert!(!stderr.contains(secret));
}

fn run_spend_call_with_unrecognized_meta(diagnostics: Option<&str>) -> (Value, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hubu-unified-mcp"));
    command
        .env_remove("HUBU_UNIFIED_OPERATION_STATE_PATH")
        .env_remove("HUBU_UNIFIED_HUBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_HUBU_BEARER_TOKEN")
        .env_remove("HUBU_UNIFIED_GONGBU_ENDPOINT")
        .env_remove("HUBU_UNIFIED_GONGBU_BEARER_TOKEN")
        .env_remove("HUBU_UNIFIED_META_SHAPE_DIAGNOSTICS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(value) = diagnostics {
        command.env("HUBU_UNIFIED_META_SHAPE_DIAGNOSTICS", value);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "rpc-raw-id",
            "method": "tools/call",
            "params": {
                "name": "hubu_submit_spend",
                "arguments": {"reason": "prompt-raw-value"},
                "_meta": {"openai/unrecognized": "meta-raw-value"}
            }
        })
    )
    .unwrap();
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    (response, String::from_utf8(output.stderr).unwrap())
}

#[test]
fn meta_shape_diagnostics_are_opt_in_sanitized_and_do_not_change_responses() {
    let (default_response, default_stderr) = run_spend_call_with_unrecognized_meta(None);
    assert!(!default_stderr.contains("meta-shape"), "{default_stderr}");

    let (diagnostic_response, diagnostic_stderr) = run_spend_call_with_unrecognized_meta(Some("1"));
    assert_eq!(diagnostic_response, default_response);
    let lines = diagnostic_stderr
        .lines()
        .filter_map(|line| line.strip_prefix("hubu-unified-mcp meta-shape: "))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{diagnostic_stderr}");
    let record = serde_json::from_str::<Value>(lines[0]).unwrap();
    assert_eq!(record["tool"], "hubu_submit_spend");
    assert_eq!(
        record["meta"]["keys"]["openai/unrecognized"]["type"],
        "string"
    );
    assert_eq!(record["trusted_identity"]["outcome"], "rejected");
    for raw in ["rpc-raw-id", "prompt-raw-value", "meta-raw-value", "reason"] {
        assert!(!diagnostic_stderr.contains(raw), "{raw} leaked");
    }
}
