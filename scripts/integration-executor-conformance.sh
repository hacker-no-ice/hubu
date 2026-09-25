#!/usr/bin/env bash
# Run the executor-neutral Hubu v4.3 conformance corpus against a real
# hubu-server, once with the built-in executor side and once through the
# external executor plugin protocol.
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

printf 'integration-executor-conformance passed\n'
