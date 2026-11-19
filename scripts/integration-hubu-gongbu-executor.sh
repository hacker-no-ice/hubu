#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Owner actions need owner presence (v0.3.0). These temp-database servers use
# the scripted fixture provider, which confirms every prompt; hubu-server
# refuses it for a database inside a managed profile.
export HUBU_OWNER_PRESENCE_PROVIDER="${HUBU_OWNER_PRESENCE_PROVIDER:-fixture}"

cd "${ROOT_DIR}"
cargo build --locked --bin hubu-server
HUBU_SERVER_BIN="${ROOT_DIR}/target/debug/hubu-server" \
  cargo test --locked -p gongbu-api --test hubu_executor_e2e -- --ignored --nocapture
