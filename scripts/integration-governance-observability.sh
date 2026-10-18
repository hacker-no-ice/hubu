#!/usr/bin/env bash
# Real Hubu + unified MCP, synthetic records and no provider/executor backend.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
qualification_source="$(git rev-parse HEAD)"
qualification_metadata="$(cargo metadata --locked --no-deps --format-version 1)"
qualification_target="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' <<<"${qualification_metadata}")"
qualification_version="$(python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "hubu-api"))' <<<"${qualification_metadata}")"
HUBU_PRODUCT_VERSION="${qualification_version}" HUBU_SOURCE_COMMIT="${qualification_source}" \
  cargo build --locked --bin hubu-server --bin hubu-unified-mcp
python3 scripts/integration-governance-observability.py \
  --hubu-server-bin "${qualification_target}/debug/hubu-server" \
  --unified-mcp-bin "${qualification_target}/debug/hubu-unified-mcp" "$@"
