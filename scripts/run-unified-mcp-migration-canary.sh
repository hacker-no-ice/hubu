#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
  echo "usage: $0 ARCHIVE_PATH PRODUCT_VERSION SOURCE_COMMIT" >&2
  exit 2
fi

archive_path="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
product_version="$2"
source_commit="$3"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
canary_dir="$(mktemp -d)"
trap 'rm -rf "${canary_dir}"' EXIT

if [[ ! "${source_commit}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "source commit must be an exact 40-character lowercase SHA" >&2
  exit 1
fi

"${root_dir}/scripts/verify-release-archive.sh" \
  "${archive_path}" \
  "${product_version}" \
  "${source_commit}"

tar -C "${canary_dir}" -xzf "${archive_path}"
package_name="$(tar -tzf "${archive_path}" | sed -n '1{s#/$##;p;}')"
package_dir="${canary_dir}/${package_name}"
unified_binary="${package_dir}/hubu-unified-mcp"

test "$(${unified_binary} --version | jq -r .source_commit)" = "${source_commit}"

cd "${root_dir}"
HUBU_PRODUCT_VERSION="${product_version}" \
HUBU_SOURCE_COMMIT="${source_commit}" \
HUBU_UNIFIED_MCP_CANARY_BIN="${unified_binary}" \
  cargo test --locked -p hubu-unified-mcp --test unified_mcp_e2e -- \
    --ignored --nocapture --test-threads=1

archive_sha256="$(if command -v sha256sum >/dev/null 2>&1; then sha256sum "${archive_path}" | awk '{print $1}'; else shasum -a 256 "${archive_path}" | awk '{print $1}'; fi)"
printf 'HUB-96 packaged canary PASS\narchive=%s\narchive_sha256=%s\nproduct_version=%s\nsource_commit=%s\npackage=%s\nprovider_spend=disabled\n' \
  "${archive_path}" \
  "${archive_sha256}" \
  "${product_version}" \
  "${source_commit}" \
  "${package_name}"
