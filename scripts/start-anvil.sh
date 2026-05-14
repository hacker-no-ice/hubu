#!/usr/bin/env bash
set -euo pipefail

ANVIL_PORT="${ANVIL_PORT:-8545}"

if ! command -v anvil >/dev/null 2>&1; then
  echo "anvil is not installed. Install Foundry from https://book.getfoundry.sh/getting-started/installation" >&2
  exit 1
fi

exec anvil --host 127.0.0.1 --port "${ANVIL_PORT}"
