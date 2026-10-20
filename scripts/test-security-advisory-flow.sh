#!/usr/bin/env bash
# Real RustSec regression, isolated from the workspace and all provider code.
set -euo pipefail
repo_root=$(cd "$(dirname "$0")/.." && pwd)
proof_dir=$(mktemp -d "${TMPDIR:-/tmp}/hubu-security-proof.XXXXXX")
trap 'rm -rf "$proof_dir"' EXIT
mkdir -p "$proof_dir/src"
cat > "$proof_dir/Cargo.toml" <<'EOF'
[package]
name = "hubu-security-proof"
version = "0.0.0"
edition = "2021"
rust-version = "1.88"
publish = false
license = "MIT"

[dependencies]
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
EOF
cat > "$proof_dir/src/lib.rs" <<'EOF'
#[test]
fn patched_tls_provider_is_available() {
    assert!(!rustls::crypto::ring::default_provider().cipher_suites.is_empty());
}
EOF
cp "$repo_root/deny.toml" "$proof_dir/deny.toml"
cargo generate-lockfile --manifest-path "$proof_dir/Cargo.toml"
cargo update --manifest-path "$proof_dir/Cargo.toml" --package rustls --precise 0.23.45
cp "$proof_dir/Cargo.toml" "$proof_dir/manifest.before"
cp "$proof_dir/deny.toml" "$proof_dir/policy.before"
python3 "$repo_root/scripts/security-advisories.py" scan --run --cwd "$proof_dir" \
  --output "$proof_dir/scan.json" --markdown "$proof_dir/scan.md"
python3 - "$proof_dir/scan.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
assert report["status"] == "advisories", report
assert any(item["id"] == "RUSTSEC-2026-0346" and item["package"] == "rustls"
           and item["version"] == "0.23.45" for item in report["advisories"]), report
PY
python3 "$repo_root/scripts/security-advisories.py" remediate --cwd "$proof_dir" \
  --report "$proof_dir/scan.json" --output "$proof_dir/fix.json" --markdown "$proof_dir/fix.md"
python3 - "$proof_dir/fix.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
assert report["status"] == "fix_available", report
updates = [item for item in report["updates"] if item["package"] == "rustls"]
assert len(updates) == 1 and updates[0]["from"] == "0.23.45", report
assert tuple(map(int, updates[0]["to"].split("."))) >= (0, 23, 46), report
PY
cmp "$proof_dir/manifest.before" "$proof_dir/Cargo.toml"
cmp "$proof_dir/policy.before" "$proof_dir/deny.toml"
cargo deny --manifest-path "$proof_dir/Cargo.toml" --all-features --locked check advisories licenses sources
cargo +1.88.0 check --manifest-path "$proof_dir/Cargo.toml" --all-targets --all-features --locked
cargo test --manifest-path "$proof_dir/Cargo.toml" --all-targets --locked
cat "$proof_dir/fix.md"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  cat "$proof_dir/fix.md" >> "$GITHUB_STEP_SUMMARY"
fi
echo 'Verified real RustSec detection, minimal rustls repair, policy preservation, MSRV, and tests.'
