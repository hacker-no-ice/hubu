#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Owner actions need owner presence (v0.3.0). These temp-database servers use
# the scripted fixture provider, which confirms every prompt; hubu-server
# refuses it for a database inside a managed profile.
export HUBU_OWNER_PRESENCE_PROVIDER="${HUBU_OWNER_PRESENCE_PROVIDER:-fixture}"

cd "${ROOT_DIR}"
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
  cargo build --locked \
    -p hubu-unified-mcp --bin hubu-unified-mcp
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
  cargo test --locked -p hubu-unified-mcp --test golden_parity -- \
    --ignored --nocapture --test-threads=1
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
  cargo test --locked -p hubu-unified-mcp --test unified_mcp_e2e -- \
    --ignored --nocapture --test-threads=1
