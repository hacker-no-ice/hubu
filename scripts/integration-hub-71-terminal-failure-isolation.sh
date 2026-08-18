#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
cargo test --locked -p gongbu-api --test terminal_failure_isolation -- --ignored --nocapture
