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
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--approval-token-file is deprecated and ignored"));
    assert!(stderr.contains("--reconciliation-token-file is deprecated and ignored"));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("HUBU_APPROVAL_TOKEN"));
    assert!(!stdout.contains("HUBU_RECONCILIATION_TOKEN"));
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

#[test]
fn migrates_existing_managed_config_to_agent_credential_and_prompt() {
    let root = tempdir().unwrap();
    let mcp_server = root.path().join("hubu-unified-mcp");
    fs::write(&mcp_server, b"offline fixture\n").unwrap();
    let config_path = root.path().join("codex-home/config.toml");
    fs::create_dir(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        r#"model = "test-model"

# >>> hubu managed codex mcp
[mcp_servers.hubu]
command = "old-mcp"
[mcp_servers.hubu.env]
HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = "old-owner-token"
HUBU_MCP_TRUST_CLIENT_APPROVAL = "1"
HUBU_MCP_TRUST_SPEND_APPROVAL = "1"
HUBU_APPROVAL_TOKEN_FILE = "old-approval-token"
HUBU_RECONCILIATION_TOKEN_FILE = "old-reconciliation-token"
[mcp_servers.hubu.tools.hubu_resolve_spend_approval]
approval_mode = "prompt"
# <<< hubu managed codex mcp

[mcp_servers.other]
command = "other-mcp"
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .current_dir(root.path())
        .env("CODEX_HOME", root.path().join("codex-home"))
        .env("HUBU_HOME", root.path().join("hubu-home"))
        .env_remove("HUBU_URL")
        .env_remove("HUBU_AUTH_TOKEN")
        .env_remove("HUBU_AUTH_TOKEN_FILE")
        .env_remove("HUBU_AGENT_TOKEN")
        .env_remove("HUBU_AGENT_TOKEN_FILE")
        .env_remove("HUBU_APPROVAL_TOKEN")
        .env_remove("HUBU_APPROVAL_TOKEN_FILE")
        .env_remove("HUBU_RECONCILIATION_TOKEN")
        .env_remove("HUBU_RECONCILIATION_TOKEN_FILE")
        .args(["init", "codex", "--mcp-server"])
        .arg(&mcp_server)
        .args(["--token-file"])
        .arg(root.path().join("owner/hubu.auth-token"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let owner = root.path().join("owner/hubu.auth-token");
    let agent = root.path().join("owner/hubu.agent-token");
    let owner_token = fs::read_to_string(&owner).unwrap();
    let agent_token = fs::read_to_string(&agent).unwrap();
    assert!(agent_token.starts_with("hubu_agent_"));
    assert_ne!(owner_token.trim(), agent_token.trim());

    let config = fs::read_to_string(config_path).unwrap();
    for retired in [
        "old-owner-token",
        "HUBU_MCP_TRUST_CLIENT_APPROVAL",
        "HUBU_MCP_TRUST_SPEND_APPROVAL",
        "HUBU_APPROVAL_TOKEN_FILE",
        "HUBU_RECONCILIATION_TOKEN_FILE",
        "hubu_resolve_spend_approval",
    ] {
        assert!(!config.contains(retired), "{retired}");
    }
    assert!(config.contains(r#"model = "test-model""#));
    assert!(config.contains("[mcp_servers.other]"));
    assert!(config.contains("[mcp_servers.hubu.tools.hubu_prompt_spend_approval]"));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("HUBU_MCP_TRUST_CLIENT_APPROVAL is retired and ignored"));
    let canonical_agent = fs::canonicalize(&agent).unwrap();
    let canonical_owner = fs::canonicalize(&owner).unwrap();
    assert!(config.contains(&format!(
        "HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = \"{}\"",
        canonical_agent.display()
    )));
    assert!(!config.contains(&canonical_owner.display().to_string()));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!(
        "HUBU_AGENT_TOKEN_FILE={}",
        canonical_agent.display()
    )));
}

#[test]
fn init_help_omits_retired_token_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .args(["init", "codex", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(!help.contains("--approval-token-file"));
    assert!(!help.contains("--reconciliation-token-file"));
}
