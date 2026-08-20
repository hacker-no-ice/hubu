#![cfg(unix)]

use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
fn hubu() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hubu"))
}

fn run(args: &[&str]) -> Output {
    Command::new(hubu()).args(args).output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn quote(value: impl AsRef<str>) -> String {
    toml::Value::String(value.as_ref().to_owned()).to_string()
}

fn reserve_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

struct TestServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn gongbu(version: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => respond_gongbu(&mut stream, &version),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn respond_gongbu(stream: &mut TcpStream, version: &Value) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut request = [0_u8; 8192];
    let count = stream.read(&mut request).unwrap_or(0);
    let first_line = String::from_utf8_lossy(&request[..count])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match path {
        "/livez" => (200, serde_json::json!({"status": "live"})),
        "/readyz" => (200, serde_json::json!({"status": "ready"})),
        "/version" => (200, version.clone()),
        _ if path.starts_with("/v1/executions/") => {
            (404, serde_json::json!({"error": "not_found"}))
        }
        _ => (404, serde_json::json!({"error": "not_found"})),
    };
    let body = serde_json::to_vec(&body).unwrap();
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes());
    let _ = stream.write_all(&body);
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn fake_version_binary(path: &Path, version_json: &str) {
    write_executable(
        path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' '{}'\n  exit 0\nfi\nexit 2\n",
            version_json.replace('\'', "'\\''")
        ),
    );
}

fn fake_hubu_server(path: &Path, script: &Path, version_json: &str) {
    write_executable(
        path,
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' '{}';;\n  validate-config) exit 0;;\n  serve) exec /usr/bin/python3 '{}';;\n  *) exit 2;;\nesac\n",
            version_json.replace('\'', "'\\''"),
            script.display().to_string().replace('\'', "'\\''")
        ),
    );
}

fn wait_until_closed(address: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("managed test server remained reachable after stack stop");
}

#[test]
fn managed_hubu_lifecycle_is_idempotent_and_never_owns_external_gongbu() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile");
    fs::create_dir_all(&profile).unwrap();

    let version_output = run(&["--version"]);
    assert_success(&version_output);
    let version_json = String::from_utf8(version_output.stdout).unwrap();
    let version: Value = serde_json::from_str(&version_json).unwrap();
    let gongbu = TestServer::gongbu(version);
    let hubu_address = reserve_addr();

    let python = root.path().join("hubu_server.py");
    fs::write(
        &python,
        format!(
            r#"import http.server, json
VERSION = json.loads({})
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health": status, body = 200, {{"status":"ok"}}
        elif self.path == "/version": status, body = 200, VERSION
        elif self.path == "/agents": status, body = 200, []
        else: status, body = 404, {{"error":"not_found"}}
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
http.server.ThreadingHTTPServer(("127.0.0.1", {}), Handler).serve_forever()
"#,
            serde_json::to_string(version_json.trim()).unwrap(),
            hubu_address.port()
        ),
    )
    .unwrap();

    let hubu_server = root.path().join("hubu-server");
    let unified_mcp = root.path().join("hubu-unified-mcp");
    fake_hubu_server(&hubu_server, &python, version_json.trim());
    fake_version_binary(&unified_mcp, version_json.trim());

    let credentials = [
        ("hubu-auth", "hubu-test-token"),
        ("hubu-approval", "approval-test-token"),
        ("hubu-reconciliation", "reconciliation-test-token"),
        ("gongbu-caller", "gongbu-test-token"),
    ];
    for (name, value) in credentials {
        fs::write(root.path().join(name), value).unwrap();
    }
    let database = root.path().join("hubu.sqlite3");
    let log = root.path().join("hubu.log");
    fs::write(
        profile.join("stack.toml"),
        format!(
            r#"schema_version = 1
allow_development_builds = true

[binaries]
hubu = {}
hubu_server = {}
hubu_unified_mcp = {}

[hubu]
ownership = "managed"
endpoint = "http://{}"
listen = "{}"
database_path = {}
log_file = {}

[gongbu]
ownership = "external"
endpoint = "http://{}"

[runtime]
hubu_startup_timeout_ms = 5000
worker_drain_timeout_ms = 100
"#,
            quote(hubu().display().to_string()),
            quote(hubu_server.display().to_string()),
            quote(unified_mcp.display().to_string()),
            hubu_address,
            hubu_address,
            quote(database.display().to_string()),
            quote(log.display().to_string()),
            gongbu.address,
        ),
    )
    .unwrap();
    fs::write(
        profile.join("credentials.toml"),
        format!(
            r#"schema_version = 1
[files]
hubu_auth = {}
hubu_approval = {}
hubu_reconciliation = {}
gongbu_caller = {}
"#,
            quote(root.path().join("hubu-auth").display().to_string()),
            quote(root.path().join("hubu-approval").display().to_string()),
            quote(
                root.path()
                    .join("hubu-reconciliation")
                    .display()
                    .to_string()
            ),
            quote(root.path().join("gongbu-caller").display().to_string()),
        ),
    )
    .unwrap();
    fs::write(
        profile.join("providers.toml"),
        "schema_version = 1\nmode = \"disabled\"\n",
    )
    .unwrap();

    let profile_arg = profile.to_str().unwrap();
    let started = run(&["stack", "start", "--profile", profile_arg]);
    assert_success(&started);
    assert!(String::from_utf8_lossy(&started.stdout).contains("running_ready"));

    let state_path = profile.join("runtime/launcher-state.json");
    let first_state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    let first_pid = first_state["processes"]["hubu-server"]["pid"]
        .as_u64()
        .unwrap();
    assert!(first_state["processes"].get("gongbu-server").is_none());

    let repeated = run(&["stack", "start", "--profile", profile_arg]);
    assert_success(&repeated);
    let repeated_state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(
        repeated_state["processes"]["hubu-server"]["pid"].as_u64(),
        Some(first_pid)
    );

    fs::write(&database, b"durable-state-canary").unwrap();
    let replacement_log = root.path().join("hubu-restarted.log");
    let stack_path = profile.join("stack.toml");
    let changed_stack = fs::read_to_string(&stack_path).unwrap().replace(
        &format!("log_file = {}", quote(log.display().to_string())),
        &format!(
            "log_file = {}",
            quote(replacement_log.display().to_string())
        ),
    );
    fs::write(&stack_path, changed_stack).unwrap();
    let unconfirmed = run(&["stack", "start", "--profile", profile_arg]);
    assert!(!unconfirmed.status.success());
    assert!(String::from_utf8_lossy(&unconfirmed.stderr).contains("--confirm-restart"));
    let unchanged_state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(
        unchanged_state["processes"]["hubu-server"]["pid"].as_u64(),
        Some(first_pid)
    );

    let pending = run(&["stack", "status", "--json", "--profile", profile_arg]);
    assert_success(&pending);
    let pending: Value = serde_json::from_slice(&pending.stdout).unwrap();
    assert_eq!(
        pending["restart_impact"],
        serde_json::json!(["hubu-server"])
    );

    let confirmed = run(&[
        "stack",
        "start",
        "--confirm-restart",
        "--profile",
        profile_arg,
    ]);
    assert_success(&confirmed);
    let restarted_state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    let restarted_pid = restarted_state["processes"]["hubu-server"]["pid"]
        .as_u64()
        .unwrap();
    assert_ne!(restarted_pid, first_pid);
    assert_eq!(fs::read(&database).unwrap(), b"durable-state-canary");

    let status = run(&["stack", "status", "--json", "--profile", profile_arg]);
    assert_success(&status);
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["classification"], "running_ready");
    assert_eq!(status["source_or_render_drift"], false);
    assert_eq!(status["unified_mcp"]["lifecycle"], "client_owned");
    assert_eq!(status["unified_mcp"]["compatible"], true);
    assert_eq!(status["restart_impact"], serde_json::json!([]));
    assert_eq!(status["components"][0]["lifecycle"], "owned_running");
    assert_eq!(status["components"][1]["lifecycle"], "external_ready");

    let logs = run(&[
        "stack",
        "logs",
        "--component",
        "hubu",
        "--lines",
        "20",
        "--profile",
        profile_arg,
    ]);
    assert_success(&logs);
    assert!(String::from_utf8_lossy(&logs.stdout).contains("hubu"));

    let stopped = run(&["stack", "stop", "--profile", profile_arg]);
    assert_success(&stopped);
    wait_until_closed(hubu_address);
    assert!(!state_path.exists());
    assert!(TcpStream::connect_timeout(&gongbu.address, Duration::from_secs(1)).is_ok());
}

#[test]
fn status_reports_an_incomplete_profile_without_mutating_it() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("not-created");
    let status = run(&[
        "stack",
        "status",
        "--json",
        "--profile",
        profile.to_str().unwrap(),
    ]);
    assert_success(&status);
    let report: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["classification"], "invalid");
    assert_eq!(report["source_or_render_drift"], true);
    assert_eq!(report["components"][0]["ownership"], "unconfigured");
    assert!(!profile.exists());
}
