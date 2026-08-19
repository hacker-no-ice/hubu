use std::process::Command;

#[test]
fn help_warns_and_points_to_unified_migration_without_echoing_secrets() {
    let output = Command::new(env!("CARGO_BIN_EXE_hubu-mcp-server"))
        .arg("--help")
        .env("HUBU_AUTH_TOKEN", "must-not-appear")
        .output()
        .expect("run hubu-mcp-server help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help stdout is utf-8");
    let stderr = String::from_utf8(output.stderr).expect("help stderr is utf-8");
    assert!(stdout.contains("hubu-unified-mcp"));
    assert!(stdout.contains("--migrate-standalone"));
    assert!(stderr.contains("deprecated and unsupported"));
    assert!(!stdout.contains("must-not-appear"));
    assert!(!stderr.contains("must-not-appear"));
}
