#!/usr/bin/env bash
# Qualify Hubu-only MCP authorization with an external executor (HUB-206):
# real hubu-server + hubu-unified-mcp with Gongbu unconfigured. Both binaries
# carry the same deterministic build stamps so the router accepts the backend.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="2062062062062062062062062062062062062062" \
  cargo build --locked --bin hubu-server --bin hubu-unified-mcp

python3 scripts/integration-standalone-mcp-executor.py \
  --hubu-server-bin "${ROOT_DIR}/target/debug/hubu-server" \
  --unified-mcp-bin "${ROOT_DIR}/target/debug/hubu-unified-mcp"
