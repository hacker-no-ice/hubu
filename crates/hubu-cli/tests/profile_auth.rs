use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread::JoinHandle,
};
use tempfile::TempDir;

const STALE_TOKEN_FILE: &str = "/path/that/must/not/be/read/hubu-token";

fn initialized_profile(path: &Path) {
    fs::create_dir_all(path).unwrap();
    for name in ["stack.toml", "credentials.toml", "providers.toml"] {
        fs::write(path.join(name), "schema_version = 1\n").unwrap();
    }
}

fn write_active_handoff(profile: &Path, endpoint: &str, credentials: &Path) {
    initialized_profile(profile);
    fs::create_dir_all(credentials).unwrap();
    let auth = credentials.join("auth.token");
    let approval = credentials.join("approval.token");
    let reconciliation = credentials.join("reconciliation.token");
    fs::write(&auth, "profile-auth\n").unwrap();
    fs::write(&approval, "profile-approval\n").unwrap();
    fs::write(&reconciliation, "profile-reconciliation\n").unwrap();

    let generation_id = "a".repeat(64);
    let generation = profile.join("generated/generations").join(&generation_id);
    fs::create_dir_all(&generation).unwrap();
    let handoff = serde_json::to_vec_pretty(&json!({
        "schema_version": 2,
        "mcp_server": profile.join("bin/hubu-unified-mcp"),
        "hubu_endpoint": endpoint,
        "hubu_token_file": auth,
        "approval_token_file": approval,
        "reconciliation_token_file": reconciliation,
        "operation_state_path": profile.join("state/operations.json"),
        "gongbu_endpoint": "http://127.0.0.1:8789",
        "gongbu_token_file": credentials.join("gongbu.token"),
    }))
    .unwrap();
    fs::write(generation.join("client-handoff.json"), &handoff).unwrap();
    let digest = format!("sha256:{:x}", Sha256::digest(&handoff));
    let manifest = json!({
        "schema_version": 1,
        "generation_id": generation_id,
        "generation": format!("generations/{generation_id}"),
        "source_schema_versions": {},
        "source_digests": {},
        "generated_file_digests": {"client-handoff.json": digest},
        "binary_provenance": [],
        "process_log_files": {},
        "restart_impact": [],
    });
    fs::write(
        profile.join("generated/active-manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn select_profile(hubu_home: &Path, profile: &Path) {
    let output = clean_command(hubu_home)
        .args(["stack", "select", "--profile"])
        .arg(profile)
        .output()
        .unwrap();
    assert_success(&output);
}

fn clean_command(hubu_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hubu"));
    command
        .env("HUBU_HOME", hubu_home)
        .env_remove("HUBU_URL")
        .env_remove("HUBU_AUTH_TOKEN")
        .env_remove("HUBU_AUTH_TOKEN_FILE")
        .env_remove("HUBU_APPROVAL_TOKEN")
        .env_remove("HUBU_APPROVAL_TOKEN_FILE")
        .env_remove("HUBU_RECONCILIATION_TOKEN")
        .env_remove("HUBU_RECONCILIATION_TOKEN_FILE");
    command
}

fn stale_environment(command: &mut Command) -> &mut Command {
    command
        .env("HUBU_URL", "http://127.0.0.1:1")
        .env("HUBU_AUTH_TOKEN", "stale-auth")
        .env("HUBU_AUTH_TOKEN_FILE", STALE_TOKEN_FILE)
        .env("HUBU_APPROVAL_TOKEN", "stale-approval")
        .env("HUBU_APPROVAL_TOKEN_FILE", STALE_TOKEN_FILE)
        .env("HUBU_RECONCILIATION_TOKEN", "stale-reconciliation")
        .env("HUBU_RECONCILIATION_TOKEN_FILE", STALE_TOKEN_FILE)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Server {
    endpoint: String,
    requests: JoinHandle<Vec<String>>,
}

impl Server {
    fn start(expected_requests: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::thread::spawn(move || {
            (0..expected_requests)
                .map(|_| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = String::new();
                    stream.read_to_string(&mut request).unwrap();
                    let first_line = request.lines().next().unwrap_or_default();
                    let health = first_line == "GET /health HTTP/1.1";
                    let (status, body) = if health {
                        ("200 OK", r#"{"status":"ok"}"#)
                    } else {
                        ("400 Bad Request", r#"{"error":"captured"}"#)
                    };
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                    request
                })
                .collect()
        });
        Self {
            endpoint: format!("http://{address}"),
            requests,
        }
    }

    fn finish(self) -> Vec<String> {
        self.requests.join().unwrap()
    }
}

fn fixture_profile(root: &TempDir, endpoint: &str) -> (PathBuf, PathBuf) {
    let profile = root.path().join("external-profile");
    let credentials = root.path().join("credentials");
    write_active_handoff(&profile, endpoint, &credentials);
    (profile, credentials)
}

#[test]
fn selected_profile_replaces_stale_endpoint_and_all_credential_environment() {
    let root = tempfile::tempdir().unwrap();
    let hubu_home = root.path().join("hubu-home");
    let server = Server::start(3);
    let (profile, _) = fixture_profile(&root, &server.endpoint);
    select_profile(&hubu_home, &profile);

    let health = stale_environment(clean_command(&hubu_home).args(["health"]))
        .output()
        .unwrap();
    assert_success(&health);

    let approval = stale_environment(clean_command(&hubu_home).args([
        "spend",
        "approval",
        "approve",
        "--approval-request-id",
        "approval-1",
    ]))
    .output()
    .unwrap();
    assert!(!approval.status.success());
    assert!(String::from_utf8_lossy(&approval.stderr).contains("captured"));

    let reconciliation = stale_environment(clean_command(&hubu_home).args([
        "spend",
        "reconcile",
        "not-billed",
        "--claim-id",
        "claim-1",
        "--provider-reference",
        "provider-1",
        "--evidence",
        "not-billed",
    ]))
    .output()
    .unwrap();
    assert!(!reconciliation.status.success());
    assert!(String::from_utf8_lossy(&reconciliation.stderr).contains("captured"));

    let requests = server.finish();
    assert!(requests[0].contains("Authorization: Bearer profile-auth\r\n"));
    assert!(requests[1].contains("Authorization: Bearer profile-auth\r\n"));
    assert!(requests[1].contains("X-Hubu-Approval-Capability: profile-approval\r\n"));
    assert!(requests[2].contains("Authorization: Bearer profile-auth\r\n"));
    assert!(requests[2].contains("X-Hubu-Reconciliation-Capability: profile-reconciliation\r\n"));
    assert!(requests.iter().all(|request| !request.contains("stale-")));
}

#[test]
fn explicit_url_uses_legacy_environment_even_with_a_selected_profile() {
    let root = tempfile::tempdir().unwrap();
    let hubu_home = root.path().join("hubu-home");
    let unused_server = Server::start(0);
    let (profile, _) = fixture_profile(&root, &unused_server.endpoint);
    select_profile(&hubu_home, &profile);
    let server = Server::start(1);

    let output = clean_command(&hubu_home)
        .env("HUBU_AUTH_TOKEN", "manual-auth")
        .args(["--url", &server.endpoint, "health"])
        .output()
        .unwrap();
    assert_success(&output);
    let requests = server.finish();
    assert!(requests[0].contains("Authorization: Bearer manual-auth\r\n"));
}

#[test]
fn active_default_profile_is_used_without_an_explicit_selection() {
    let root = tempfile::tempdir().unwrap();
    let hubu_home = root.path().join("hubu-home");
    let server = Server::start(1);
    write_active_handoff(
        &hubu_home.join("stacks/default"),
        &server.endpoint,
        &root.path().join("credentials"),
    );

    let output = stale_environment(clean_command(&hubu_home).args(["health"]))
        .output()
        .unwrap();
    assert_success(&output);
    let requests = server.finish();
    assert!(requests[0].contains("Authorization: Bearer profile-auth\r\n"));
}

#[test]
fn missing_selected_handoff_fails_closed_but_local_commands_still_work() {
    let root = tempfile::tempdir().unwrap();
    let hubu_home = root.path().join("hubu-home");
    let profile = root.path().join("incomplete-profile");
    initialized_profile(&profile);
    select_profile(&hubu_home, &profile);

    let remote = stale_environment(clean_command(&hubu_home).args(["health"]))
        .output()
        .unwrap();
    assert!(!remote.status.success());
    let error = String::from_utf8_lossy(&remote.stderr);
    assert!(error.contains("selected stack profile"), "{error}");
    assert!(error.contains("explicit `--url`"), "{error}");

    let policy = root.path().join("policy.yaml");
    let local = stale_environment(
        clean_command(&hubu_home)
            .args(["policy", "new-template", "--path"])
            .arg(&policy),
    )
    .output()
    .unwrap();
    assert_success(&local);
    assert!(policy.is_file());
}

#[test]
fn legacy_environment_is_preserved_when_no_active_profile_exists() {
    let root = tempfile::tempdir().unwrap();
    let server = Server::start(1);
    let output = clean_command(root.path())
        .env("HUBU_URL", &server.endpoint)
        .env("HUBU_AUTH_TOKEN", "legacy-auth")
        .arg("health")
        .output()
        .unwrap();
    assert_success(&output);
    let requests = server.finish();
    assert!(requests[0].contains("Authorization: Bearer legacy-auth\r\n"));
}
