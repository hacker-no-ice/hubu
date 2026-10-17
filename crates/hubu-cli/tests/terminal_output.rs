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

fn money(units: &str, display: &str) -> Value {
    serde_json::json!({"units": units, "scale": 6, "currency": "usd", "display": display})
}

fn budget_list_body() -> String {
    let budget = |id: &str,
                  limit: (&str, &str),
                  consumed: (&str, &str),
                  frozen: (&str, &str),
                  remaining: (&str, &str),
                  cents: [i64; 4]| {
        serde_json::json!({
            "budget_id": id, "agent_id": "agt_0123456789ab", "status": "active",
            "currency": "usd",
            "amount_limit": money(limit.0, limit.1),
            "consumed_amount": money(consumed.0, consumed.1),
            "frozen_amount": money(frozen.0, frozen.1),
            "remaining_amount": money(remaining.0, remaining.1),
            "amount_limit_cents": cents[0], "consumed_amount_cents": cents[1],
            "frozen_amount_cents": cents[2], "remaining_amount_cents": cents[3],
            "current_version_id": "bgv_000000000001", "current_revision": 1,
            "starting_at": "2026-10-01T00:00:00+00:00", "ending_before": null
        })
    };
    serde_json::json!({"budgets": [
        budget(
            "bgt_000000000001",
            ("10000000", "10.00"),
            ("35000", "0.035"),
            ("1", "0.000001"),
            ("9964999", "9.964999"),
            [1000, 4, 1, 996],
        ),
        // A reconciled overrun leaves a negative remainder.
        budget(
            "bgt_000000000002",
            ("10000", "0.01"),
            ("13100", "0.0131"),
            ("0", "0.00"),
            ("-3100", "-0.0031"),
            [1, 2, 0, -1],
        ),
    ]})
    .to_string()
}

#[test]
fn budget_amounts_show_exact_adaptive_money() {
    let home = tempdir().unwrap();
    let (url, server) = serve_json_once(&budget_list_body());
    let output = run(home.path(), &["--url", &url, "budget", "list"]);
    assert_success(&output);
    assert_eq!(server.join().unwrap(), "GET /budgets HTTP/1.1");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let amounts: Vec<&str> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("limit:"))
        .collect();
    assert_eq!(
        amounts,
        [
            "    limit: $10.00  consumed: $0.035  frozen: $0.000001  remaining: $9.964999",
            "    limit: $0.01  consumed: $0.0131  frozen: $0.00  remaining: -$0.0031",
        ]
    );
}

#[test]
fn round_cents_marks_and_conservatively_rounds_amounts() {
    let home = tempdir().unwrap();
    let (url, server) = serve_json_once(&budget_list_body());
    let output = run(
        home.path(),
        &["--url", &url, "--round", "cents", "budget", "list"],
    );
    assert_success(&output);
    server.join().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let amounts: Vec<&str> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("limit:"))
        .collect();
    // Usage rounds up; limits and remaining round down.
    assert_eq!(
        amounts,
        [
            "    limit: $10.00  consumed: ≈$0.04  frozen: ≈$0.01  remaining: ≈$9.96",
            "    limit: $0.01  consumed: ≈$0.02  frozen: $0.00  remaining: ≈-$0.01",
        ]
    );
}

#[test]
fn older_servers_without_money_objects_fall_back_to_cents() {
    let home = tempdir().unwrap();
    let body = serde_json::json!({"budgets": [{
        "budget_id": "bgt_000000000001", "agent_id": "agt_0123456789ab", "status": "active",
        "amount_limit_cents": 1000, "consumed_amount_cents": 4,
        "frozen_amount_cents": 1, "remaining_amount_cents": 995,
        "current_version_id": "bgv_000000000001", "current_revision": 1,
        "starting_at": "2026-10-01T00:00:00+00:00", "ending_before": null
    }]})
    .to_string();
    let (url, server) = serve_json_once(&body);
    let output = run(home.path(), &["--url", &url, "budget", "list"]);
    assert_success(&output);
    server.join().unwrap();
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("limit: $10.00  consumed: $0.04  frozen: $0.01  remaining: $9.95"));
}
