#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
cargo build --locked --bin hubu-server
HUBU_SERVER_BIN="${ROOT_DIR}/target/debug/hubu-server" \
  cargo test --locked -p gongbu-api --test hubu_executor_e2e -- --ignored --nocapture
