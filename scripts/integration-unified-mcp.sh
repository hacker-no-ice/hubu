#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
  cargo build --locked \
    -p hubu-unified-mcp --bin hubu-unified-mcp \
    -p hubu-mcp --bin hubu-mcp-server \
    -p gongbu-mcp --bin gongbu-mcp
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
HUBU_STANDALONE_MCP_CANARY_BIN="${ROOT_DIR}/target/debug/hubu-mcp-server" \
GONGBU_STANDALONE_MCP_CANARY_BIN="${ROOT_DIR}/target/debug/gongbu-mcp" \
  cargo test --locked -p hubu-unified-mcp --test golden_parity -- \
    --ignored --nocapture --test-threads=1
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
HUBU_STANDALONE_MCP_CANARY_BIN="${ROOT_DIR}/target/debug/hubu-mcp-server" \
GONGBU_STANDALONE_MCP_CANARY_BIN="${ROOT_DIR}/target/debug/gongbu-mcp" \
  cargo test --locked -p hubu-unified-mcp --test unified_mcp_e2e -- \
    --ignored --nocapture --test-threads=1
