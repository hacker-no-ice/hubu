#!/usr/bin/env bash
# Run the executor-neutral Hubu v4.5 conformance corpus against a real
# hubu-server, once with the built-in executor side and once through the
# external executor plugin protocol, then the legacy v4.4 corpus that cent
# executors still use during the transition.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

cargo build --locked --bin hubu-server

python3 scripts/hubu-executor-conformance.py --list >/dev/null
python3 scripts/hubu-executor-conformance.py \
  --server-bin "${ROOT_DIR}/target/debug/hubu-server"
python3 scripts/hubu-executor-conformance.py \
  --server-bin "${ROOT_DIR}/target/debug/hubu-server" \
  --executor-command "python3 ${ROOT_DIR}/scripts/hubu-executor-conformance-reference-plugin.py"
python3 scripts/hubu-executor-conformance.py \
  --server-bin "${ROOT_DIR}/target/debug/hubu-server" \
  --corpus "${ROOT_DIR}/fixtures/hubu-executor-conformance-v4.4.json"

printf 'integration-executor-conformance passed\n'
