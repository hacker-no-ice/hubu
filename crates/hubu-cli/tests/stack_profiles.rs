use serde_json::Value;
use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

fn write_initialized_profile(path: &Path) {
    fs::create_dir_all(path).unwrap();
    for name in ["stack.toml", "credentials.toml", "providers.toml"] {
        fs::write(path.join(name), "schema_version = 1\n").unwrap();
    }
}

#[test]
fn deprecated_live_alias_generates_canonical_full_stack_and_reports_mode() {
    let root = tempdir().unwrap();
    let profile = root.path().join("live");
    let initialized = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .env("HUBU_HOME", root.path())
        .args(["stack", "init", "--mode", "local-stack", "--profile"])
        .arg(&profile)
        .output()
        .unwrap();
    assert!(initialized.status.success());
    assert!(String::from_utf8_lossy(&initialized.stderr).contains("deprecated"));
    let source = fs::read_to_string(profile.join("stack.toml")).unwrap();
    assert!(source.contains("mode = \"live\""));
    assert!(source.contains("[gongbu]"));
    assert!(source.contains("[temporal]"));

    // Existing profiles remain readable, but diagnostics teach the canonical name.
    fs::write(
        profile.join("stack.toml"),
        source.replace("mode = \"live\"", "mode = \"local-stack\""),
    )
    .unwrap();
    for command in ["doctor", "status"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hubu"))
            .env("HUBU_HOME", root.path())
            .args(["stack", command, "--json", "--profile"])
            .arg(&profile)
            .output()
            .unwrap();
        assert!(output.status.success());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["mode"], "live");
        assert_eq!(report["executor"], "gongbu");
        if command == "doctor" {
            assert!(report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["code"] == "legacy_mode_alias"));
        }
    }
}

#[test]
fn decisions_only_is_rejected_before_initialization_or_startup() {
    let root = tempdir().unwrap();
    for mode in ["hubu-only", "decisions-only"] {
        let profile = root.path().join(mode);
        let output = Command::new(env!("CARGO_BIN_EXE_hubu"))
            .env("HUBU_HOME", root.path())
            .args(["stack", "init", "--mode", mode, "--profile"])
            .arg(&profile)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not supported"));
        assert!(!profile.exists());
    }

    let profile = root.path().join("legacy");
    write_initialized_profile(&profile);
    let source = "schema_version = 1\nmode = \"hubu-only\"\n";
    fs::write(profile.join("stack.toml"), source).unwrap();
    let doctor = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .env("HUBU_HOME", root.path())
        .args(["stack", "doctor", "--json", "--profile"])
        .arg(&profile)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(report["classification"], "invalid");
    let migration = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["code"] == "unsupported_stack_mode")
        .unwrap();
    assert!(migration["message"]
        .as_str()
        .unwrap()
        .contains("not migrated automatically"));
    for command in ["render", "start"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hubu"))
            .env("HUBU_HOME", root.path())
            .args(["stack", command, "--profile"])
            .arg(&profile)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    assert_eq!(
        fs::read_to_string(profile.join("stack.toml")).unwrap(),
        source
    );
    assert!(!profile.join("generated/active-manifest.json").exists());
    assert!(!profile.join("state/launcher-state.json").exists());
}

#[test]
fn selects_and_lists_registered_and_conventional_profiles() {
    let hubu_home = tempdir().unwrap();
    let external_root = tempdir().unwrap();
    let external = external_root.path().join("external");
    let default = hubu_home.path().join("stacks/default");
    let fixture = hubu_home.path().join("stacks/fixture");
    write_initialized_profile(&external);
    write_initialized_profile(&default);
    write_initialized_profile(&fixture);

    let selected = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .env("HUBU_HOME", hubu_home.path())
        .args(["stack", "select", "--profile"])
        .arg(&external)
        .output()
        .unwrap();
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );

    let listed = Command::new(env!("CARGO_BIN_EXE_hubu"))
        .env("HUBU_HOME", hubu_home.path())
        .args(["stack", "profiles", "--json"])
        .output()
        .unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let report: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    let profiles = report["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 3);
    assert_eq!(
        profiles
            .iter()
            .filter(|profile| profile["selected"] == true)
            .count(),
        1
    );
    assert_eq!(
        profiles
            .iter()
            .filter(|profile| profile["default"] == true)
            .count(),
        1
    );
    assert!(profiles
        .windows(2)
        .all(|pair| { pair[0]["path"].as_str().unwrap() < pair[1]["path"].as_str().unwrap() }));

    let registry: Value =
        serde_json::from_slice(&fs::read(hubu_home.path().join("stack-profiles.json")).unwrap())
            .unwrap();
    assert_eq!(registry["schema_version"], 1);
    assert_eq!(registry["profiles"].as_array().unwrap().len(), 1);
}
