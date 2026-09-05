use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn offline_feedback_ignores_broken_profile_and_never_echoes_extra_diagnostics() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.json"), "broken").unwrap();
    let help = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("feedback"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .args(["feedback", "prepare"])
        .env("HUBU_HOME", home.path())
        .env("HUBU_URL", "not-a-url")
        .env("HUBU_AUTH_TOKEN", "credential-canary")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"trying":"View an image", "happened":"No inline image", "diagnostics":{"raw_logs":"log-canary"}}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("credential-canary"));
    assert!(!text.contains("log-canary"));
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["status"], "prepared_not_sent");
    assert!(report["body"].as_str().unwrap().contains("No inline image"));
    assert!(output.stderr.is_empty());
}

#[test]
fn malformed_feedback_fails_without_leaking_input() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .args(["feedback", "prepare"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{ secret-canary invalid json")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret-canary"));
}
