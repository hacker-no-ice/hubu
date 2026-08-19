use serde_json::Value;
use std::{
    env,
    ffi::OsString,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn canary_binary() -> OsString {
    env::var_os("HUBU_UNIFIED_MCP_CANARY_BIN")
        .unwrap_or_else(|| OsString::from(env!("CARGO_BIN_EXE_hubu-unified-mcp")))
}

#[test]
fn binary_initializes_lists_tools_and_exits_on_stdin_close() {
    let mut child = Command::new(canary_binary())
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
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 1);
}

#[test]
fn initialized_monitor_stops_cleanly_on_eof() {
    let mut child = Command::new(canary_binary())
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
    let secret = "credential-that-must-not-appear";
    let output = Command::new(canary_binary())
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
