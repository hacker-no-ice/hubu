use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use tempfile::tempdir;

#[test]
fn managed_server_writes_each_structured_event_once() {
    let root = tempdir().unwrap();
    let state = root.path().join("state");
    let logs = root.path().join("logs");
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&logs).unwrap();
    for name in ["auth", "approval", "reconciliation"] {
        fs::write(root.path().join(name), format!("{name}-token")).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["auth", "approval", "reconciliation"] {
            fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = listener.local_addr().unwrap();
    drop(listener);

    let log_path = logs.join("hubu.jsonl");
    let config_path = root.path().join("hubu-launch.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "listen": listen,
            "database_path": state.join("hubu.sqlite3"),
            "log_file": log_path,
            "auth_token_file": root.path().join("auth"),
            "approval_token_file": root.path().join("approval"),
            "reconciliation_token_file": root.path().join("reconciliation")
        }))
        .unwrap(),
    )
    .unwrap();
    let stderr_path = logs.join("hubu-server.stderr.log");
    let launcher_stderr = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&stderr_path)
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hubu-server"))
        .args(["serve", "--config"])
        .arg(&config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::from(launcher_stderr))
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let accepting = TcpStream::connect(listen).is_ok();
        let listening_logged = fs::read_to_string(&log_path)
            .is_ok_and(|contents| contents.contains("\"event\":\"server_listening\""));
        if accepting && listening_logged {
            break;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "managed Hubu exited early"
        );
        assert!(Instant::now() < deadline, "managed Hubu did not start");
        thread::sleep(Duration::from_millis(20));
    }
    let mut health = TcpStream::connect(listen).unwrap();
    health
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    health.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    child.kill().unwrap();
    child.wait().unwrap();

    let events = fs::read_to_string(&log_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|line| line["event"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    for event in ["log_file_configured", "server_starting", "server_listening"] {
        assert_eq!(
            events
                .iter()
                .filter(|candidate| candidate.as_str() == event)
                .count(),
            1,
            "managed structured event `{event}` was duplicated: {events:?}"
        );
    }
    assert!(!events.iter().any(|event| matches!(
        event.as_str(),
        "http_request_started" | "http_request_finished"
    )));
    assert!(fs::read_to_string(stderr_path).unwrap().is_empty());
}
