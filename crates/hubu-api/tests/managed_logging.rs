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
fn managed_server_keeps_successful_probes_quiet_and_writes_events_once() {
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
        let accepting = try_get(listen, "/health", None)
            .is_some_and(|response| response.starts_with("HTTP/1.1 200 OK"));
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

    let idle_log = fs::read(&log_path).unwrap();
    for _ in 0..3 {
        for target in [
            "/health",
            "/version",
            "/agents?operational_probe=gongbu_credential_check",
        ] {
            let response = get(listen, target, Some("auth-token"));
            assert!(response.starts_with("HTTP/1.1 200 OK"));
        }
    }
    assert_eq!(
        fs::read(&log_path).unwrap(),
        idle_log,
        "healthy Gongbu compatibility cycles must not grow the managed log"
    );

    let response = get(listen, "/agents", Some("auth-token"));
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    let response = get(
        listen,
        "/agents?operational_probe=gongbu_credential_check",
        Some("wrong-token"),
    );
    assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));

    child.kill().unwrap();
    child.wait().unwrap();

    let events = fs::read_to_string(&log_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    for event in ["log_file_configured", "server_starting", "server_listening"] {
        assert_eq!(
            events
                .iter()
                .filter(|candidate| candidate["event"].as_str() == Some(event))
                .count(),
            1,
            "managed structured event `{event}` was duplicated: {events:?}"
        );
    }
    for event in ["http_request_started", "http_request_finished"] {
        assert_eq!(
            events
                .iter()
                .filter(|candidate| candidate["event"].as_str() == Some(event))
                .count(),
            1,
            "only the ordinary agent-list read should emit `{event}`: {events:?}"
        );
    }
    let started = event(&events, "http_request_started");
    assert_eq!(started["fields"]["method"], "GET");
    assert_eq!(started["fields"]["path"], "/agents");
    let finished = event(&events, "http_request_finished");
    assert_eq!(finished["fields"]["method"], "GET");
    assert_eq!(finished["fields"]["path"], "/agents");
    assert_eq!(finished["fields"]["status"], 200);
    assert_eq!(
        events
            .iter()
            .filter(|candidate| candidate["event"] == "http_request_unauthorized")
            .count(),
        1
    );
    let unauthorized = event(&events, "http_request_unauthorized");
    assert_eq!(unauthorized["fields"]["method"], "GET");
    assert_eq!(unauthorized["fields"]["path"], "/agents");
    assert_eq!(
        events
            .iter()
            .filter(|candidate| candidate["event"] == "http_probe_failed")
            .count(),
        1
    );
    let failed_probe = event(&events, "http_probe_failed");
    assert_eq!(failed_probe["fields"]["method"], "GET");
    assert_eq!(failed_probe["fields"]["path"], "/agents");
    assert_eq!(failed_probe["fields"]["status"], 401);
    assert!(fs::read_to_string(stderr_path).unwrap().is_empty());
}

fn get(listen: std::net::SocketAddr, target: &str, bearer: Option<&str>) -> String {
    try_get(listen, target, bearer).expect("server should answer request")
}

fn try_get(listen: std::net::SocketAddr, target: &str, bearer: Option<&str>) -> Option<String> {
    let mut stream = TcpStream::connect(listen).ok()?;
    let authorization = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\n{authorization}Connection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

fn event<'a>(events: &'a [Value], name: &str) -> &'a Value {
    events
        .iter()
        .find(|candidate| candidate["event"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("missing `{name}` event: {events:?}"))
}
