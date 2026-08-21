#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v temporal >/dev/null 2>&1; then
  echo "local-stack acceptance requires the Temporal CLI on PATH" >&2
  exit 2
fi

cd "${root_dir}"

# The production Gongbu binary deliberately rejects fixture providers. Keep
# that boundary intact and compose the clean-environment acceptance proof from
# the real stack lifecycle, the real Hubu executor contract, and the real
# Gongbu HTTP/Temporal/artifact runtime with injected deterministic activities.
cargo test --locked -p hubu-cli -- --nocapture
./scripts/integration-hubu-gongbu-executor.sh
./scripts/integration-hub-71-terminal-failure-isolation.sh

echo "Local-stack acceptance passed: lifecycle, governed fixture execution, Temporal workflow discovery, artifact retrieval, restart persistence, and graceful shutdown"
