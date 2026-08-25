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
