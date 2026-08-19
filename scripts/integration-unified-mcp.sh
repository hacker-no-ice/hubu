#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
HUBU_PRODUCT_VERSION="0.1.0" \
HUBU_SOURCE_COMMIT="9393939393939393939393939393939393939393" \
  cargo test --locked -p hubu-unified-mcp --test unified_mcp_e2e -- \
    --ignored --nocapture --test-threads=1
