use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

fn dry_run(root: &Path, operation_key_db: &Path) -> std::process::Output {
    let mcp_server = root.join("hubu-unified-mcp");
    fs::write(&mcp_server, b"offline fixture\n").unwrap();
    Command::new(env!("CARGO_BIN_EXE_hubu"))
        .current_dir(root)
        .env("CODEX_HOME", root.join("codex-home"))
        .env("HUBU_HOME", root.join("hubu-home"))
        .env_remove("HUBU_URL")
        .env_remove("HUBU_AUTH_TOKEN")
        .env_remove("HUBU_AUTH_TOKEN_FILE")
        .env_remove("HUBU_APPROVAL_TOKEN")
        .env_remove("HUBU_APPROVAL_TOKEN_FILE")
        .env_remove("HUBU_RECONCILIATION_TOKEN")
        .env_remove("HUBU_RECONCILIATION_TOKEN_FILE")
        .args(["init", "codex", "--dry-run", "--mcp-server"])
        .arg(&mcp_server)
        .args(["--token-file"])
        .arg(root.join("hubu-token"))
        .args(["--approval-token-file"])
        .arg(root.join("approval-token"))
        .args(["--reconciliation-token-file"])
        .arg(root.join("reconciliation-token"))
        .args(["--mcp-state-file"])
        .arg(root.join("unified-operations.sqlite3"))
        .args(["--operation-key-db"])
        .arg(operation_key_db)
        .output()
        .unwrap()
}

#[test]
fn operation_key_db_is_explicit_absolute_and_dry_run_is_non_mutating() {
    let root = tempdir().unwrap();
    let private_db = root.path().join("private/operation-keys.sqlite3");
    let output = dry_run(root.path(), &private_db);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!(
        "HUBU_UNIFIED_OPERATION_KEY_DB = \"{}\"",
        private_db.display()
    )));
    assert!(!private_db.exists());
    assert!(!root.path().join("unified-operations.sqlite3").exists());
    assert!(!root.path().join("hubu-token").exists());
    assert!(!root.path().join("approval-token").exists());
    assert!(!root.path().join("reconciliation-token").exists());
    assert!(!root.path().join("codex-home/config.toml").exists());

    let relative = Path::new("private/operation-keys.sqlite3");
    let rejected = dry_run(root.path(), relative);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("--operation-key-db requires an absolute private path"));
    assert!(!root.path().join(relative).exists());
}
