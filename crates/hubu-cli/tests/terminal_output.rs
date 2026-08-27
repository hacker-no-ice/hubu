use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    thread::JoinHandle,
};
use tempfile::tempdir;

fn run(hubu_home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hubu"))
        .env("HUBU_HOME", hubu_home)
        .env_remove("NO_COLOR")
        .args(args)
        .output()
        .unwrap()
}

fn write_incomplete_profile(path: &Path) {
    fs::create_dir_all(path).unwrap();
    for name in ["stack.toml", "credentials.toml", "providers.toml"] {
        fs::write(path.join(name), "schema_version = 1\n").unwrap();
    }
}

fn serve_json_once(body: &str) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = body.to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        request.lines().next().unwrap_or_default().to_string()
    });
    (format!("http://{address}"), server)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_no_ansi(bytes: &[u8]) {
    assert!(
        !bytes.windows(2).any(|window| window == b"\x1b["),
        "unexpected ANSI output: {}",
        String::from_utf8_lossy(bytes)
    );
}

#[test]
fn captured_auto_output_is_plain_and_forced_human_output_is_colored() {
    let home = tempdir().unwrap();

    let automatic = run(home.path(), &["stack", "profiles"]);
    assert_success(&automatic);
    assert_no_ansi(&automatic.stdout);

    let colored = run(home.path(), &["stack", "profiles", "--color", "always"]);
    assert_success(&colored);
    assert!(colored.stdout.windows(2).any(|window| window == b"\x1b["));

    let plain = run(home.path(), &["--color", "never", "stack", "profiles"]);
    assert_success(&plain);
    assert_no_ansi(&plain.stdout);
}

#[test]
fn explicit_color_choice_overrides_no_color() {
    let home = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .env("HUBU_HOME", home.path())
        .env("NO_COLOR", "1")
        .args(["--color", "always", "stack", "profiles"])
        .output()
        .unwrap();

    assert_success(&output);
    assert!(output.stdout.windows(2).any(|window| window == b"\x1b["));
}

#[test]
fn forced_color_never_contaminates_machine_output() {
    let home = tempdir().unwrap();
    let profile = home.path().join("stacks/incomplete");
    write_incomplete_profile(&profile);
    let profile = profile.to_str().unwrap();

    for args in [
        vec!["--color", "always", "stack", "profiles", "--json"],
        vec![
            "--color",
            "always",
            "stack",
            "doctor",
            "--profile",
            profile,
            "--json",
        ],
        vec![
            "--color",
            "always",
            "stack",
            "status",
            "--profile",
            profile,
            "--json",
        ],
        vec!["--color", "always", "--version"],
    ] {
        let output = run(home.path(), &args);
        assert_success(&output);
        assert_no_ansi(&output.stdout);
        serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
}

#[test]
fn forced_color_keeps_protocol_and_policy_exports_machine_readable() {
    let home = tempdir().unwrap();

    let (url, server) = serve_json_once(r#"{"protocol_version":"1"}"#);
    let protocol = run(
        home.path(),
        &[
            "--url",
            &url,
            "--color",
            "always",
            "protocol",
            "agent-registration",
        ],
    );
    assert_success(&protocol);
    assert_no_ansi(&protocol.stdout);
    serde_json::from_slice::<Value>(&protocol.stdout).unwrap();
    assert_eq!(
        server.join().unwrap(),
        "GET /registration/guidance HTTP/1.1"
    );

    let (url, server) = serve_json_once(r#"{"policy_id":"demo","revision":2}"#);
    let policy = run(
        home.path(),
        &["--color", "always", "policy", "show", "--url", &url],
    );
    assert_success(&policy);
    assert_no_ansi(&policy.stdout);
    serde_json::from_slice::<Value>(&policy.stdout).unwrap();
    assert_eq!(server.join().unwrap(), "GET /policies/show HTTP/1.1");

    let (url, server) = serve_json_once(r#"{"policy_yaml":"id: demo\nversion: 1\n"}"#);
    let exported = run(
        home.path(),
        &["--url", &url, "policy", "export", "--color", "always"],
    );
    assert_success(&exported);
    assert_no_ansi(&exported.stdout);
    assert_eq!(exported.stdout, b"id: demo\nversion: 1\n");
    assert_eq!(server.join().unwrap(), "GET /policies/export HTTP/1.1");
}

#[test]
fn captured_errors_are_plain_unless_color_is_forced() {
    let home = tempdir().unwrap();

    let automatic = run(home.path(), &["unknown-command"]);
    assert!(!automatic.status.success());
    assert_no_ansi(&automatic.stderr);

    let colored = run(home.path(), &["unknown-command", "--color", "always"]);
    assert!(!colored.status.success());
    assert!(colored.stderr.windows(2).any(|window| window == b"\x1b["));

    let global_parse_error = run(home.path(), &["--color", "always", "--url"]);
    assert!(!global_parse_error.status.success());
    assert!(global_parse_error
        .stderr
        .windows(2)
        .any(|window| window == b"\x1b["));
}
