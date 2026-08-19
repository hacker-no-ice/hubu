#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
  echo "usage: $0 ARCHIVE_PATH PRODUCT_VERSION SOURCE_COMMIT" >&2
  exit 2
fi

archive_path="$1"
product_version="$2"
source_commit="$3"
smoke_dir="$(mktemp -d)"
server_pid=""
curl_connect_timeout_seconds=2
curl_max_time_seconds=5

cleanup() {
  if [[ -n "${server_pid}" ]]; then
    kill "${server_pid}" >/dev/null 2>&1 || true
    wait "${server_pid}" >/dev/null 2>&1 || true
  fi
  rm -rf "${smoke_dir}"
}
trap cleanup EXIT

tar -C "${smoke_dir}" -xzf "${archive_path}"
package_name="$(tar -tzf "${archive_path}" | sed -n '1{s#/$##;p;}')"
package_dir="${smoke_dir}/${package_name}"
expected_binaries=(
  hubu
  hubu-server
  hubu-unified-mcp
  hubu-mcp-server
  gongbu-server
  gongbu-mcp
)
expected_files=(
  Cargo.lock
  LICENSE-APACHE
  LICENSE-MIT
  MANIFEST.json
  PROVENANCE.json
  SHA256SUMS
  THIRD-PARTY-LICENSES.txt
  THIRD-PARTY-NOTICES.md
  "${expected_binaries[@]}"
)

for expected_file in "${expected_files[@]}"; do
  if [[ ! -s "${package_dir}/${expected_file}" ]]; then
    echo "release archive is missing non-empty ${expected_file}" >&2
    exit 1
  fi
done
if [[ -e "${package_dir}/hubu-bench" || -e "${package_dir}/gongbu-sandbox" ]]; then
  echo "release archive contains a development-only binary" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  (cd "${package_dir}" && sha256sum -c SHA256SUMS >/dev/null)
else
  (cd "${package_dir}" && shasum -a 256 -c SHA256SUMS >/dev/null)
fi

target="$(jq -r '.target' "${package_dir}/PROVENANCE.json")"
jq -e \
  --arg product_version "${product_version}" \
  --arg source_commit "${source_commit}" \
  --arg target "${target}" \
  '.schema_version == 1 and
   .product == "hubu" and
   .product_version == $product_version and
   .source_commit == $source_commit and
   .executor_contract == "hubu-spend-executor-v4.2" and
   .target == $target and
   .binaries == ["hubu", "hubu-server", "hubu-unified-mcp", "hubu-mcp-server", "gongbu-server", "gongbu-mcp"] and
   .default_agent_surface == "hubu-unified-mcp" and
   .compatibility_agent_surfaces == ["hubu-mcp-server", "gongbu-mcp"] and
   .manifest == "MANIFEST.json" and
   .dependencies == "Cargo.lock" and
   .third_party_licenses == "THIRD-PARTY-LICENSES.txt"' \
  "${package_dir}/PROVENANCE.json" >/dev/null
jq -e \
  --arg product_version "${product_version}" \
  --arg source_commit "${source_commit}" \
  --arg target "${target}" \
  '.schema_version == 1 and
   .product == "hubu" and
   .product_version == $product_version and
   .source_commit == $source_commit and
   .executor_contract == "hubu-spend-executor-v4.2" and
   .target == $target and
   .binaries == ["hubu", "hubu-server", "hubu-unified-mcp", "hubu-mcp-server", "gongbu-server", "gongbu-mcp"] and
   .default_agent_surface == "hubu-unified-mcp" and
   .compatibility_agent_surfaces == ["hubu-mcp-server", "gongbu-mcp"] and
   .development_tools_excluded == ["hubu-bench", "gongbu-sandbox"]' \
  "${package_dir}/MANIFEST.json" >/dev/null

for binary in "${expected_binaries[@]}"; do
  chmod +x "${package_dir}/${binary}"
  version_output="$("${package_dir}/${binary}" --version)"
  jq -e \
    --arg product_version "${product_version}" \
    --arg source_commit "${source_commit}" \
    '.product_version == $product_version and
     .source_commit == $source_commit and
     ((.executor_contract // .hubu_executor_contract) == "hubu-spend-executor-v4.2")' \
    <<<"${version_output}" >/dev/null
done

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
HUBU_DB_PATH="${smoke_dir}/hubu.sqlite3" \
HUBU_AUTH_TOKEN_FILE="${smoke_dir}/hubu.auth-token" \
HUBU_RECONCILIATION_TOKEN_FILE="${smoke_dir}/hubu.reconciliation-token" \
  "${package_dir}/hubu-server" "127.0.0.1:${port}" >"${smoke_dir}/hubu-server.log" 2>&1 &
server_pid="$!"

for _ in {1..20}; do
  if curl --fail --silent \
    --connect-timeout "${curl_connect_timeout_seconds}" \
    --max-time "${curl_max_time_seconds}" \
    "http://127.0.0.1:${port}/health" >/dev/null; then
    break
  fi
  if ! kill -0 "${server_pid}" >/dev/null 2>&1; then
    cat "${smoke_dir}/hubu-server.log" >&2
    exit 1
  fi
  sleep 1
done

curl --fail --silent \
  --connect-timeout "${curl_connect_timeout_seconds}" \
  --max-time "${curl_max_time_seconds}" \
  "http://127.0.0.1:${port}/health" | grep -F '"status":"ok"' >/dev/null
reported_version="$(curl --fail --silent \
  --connect-timeout "${curl_connect_timeout_seconds}" \
  --max-time "${curl_max_time_seconds}" \
  "http://127.0.0.1:${port}/version")"
jq -e \
  --arg product_version "${product_version}" \
  --arg source_commit "${source_commit}" \
  '.product_version == $product_version and
   .source_commit == $source_commit and
   .executor_contract == "hubu-spend-executor-v4.2"' \
  <<<"${reported_version}" >/dev/null

initialize='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
tools_list='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
unified_mcp_response="$(printf '%s\n%s\n' "${initialize}" "${tools_list}" | \
  HUBU_UNIFIED_HUBU_ENDPOINT="http://127.0.0.1:${port}" \
  HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE="${smoke_dir}/hubu.auth-token" \
  HUBU_RECONCILIATION_TOKEN_FILE="${smoke_dir}/hubu.reconciliation-token" \
  "${package_dir}/hubu-unified-mcp")"
jq -s -e \
  --arg product_version "${product_version}" \
  '.[0].result.serverInfo.name == "hubu-unified-mcp" and
   .[0].result.serverInfo.version == $product_version and
   (.[1].result.tools | map(.name) | contains(["hubu_health", "hubu_unified_capabilities"])) and
   (.[1].result.tools | map(.name) | any(startswith("gongbu_")) | not)' \
  <<<"${unified_mcp_response}" >/dev/null

generated_config="$("${package_dir}/hubu" init codex \
  --dry-run \
  --mcp-server "${package_dir}/hubu-unified-mcp" \
  --token-file "${smoke_dir}/hubu.auth-token" \
  --reconciliation-token-file "${smoke_dir}/hubu.reconciliation-token" \
  --approval-token-file "${smoke_dir}/hubu.approval-token")"
grep -F '[mcp_servers.hubu]' <<<"${generated_config}" >/dev/null
grep -E 'command = ".*/hubu-unified-mcp"' <<<"${generated_config}" >/dev/null
grep -F 'HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE' <<<"${generated_config}" >/dev/null
if grep -F '[mcp_servers.gongbu]' <<<"${generated_config}" >/dev/null; then
  echo "default agent config emitted a second MCP server entry" >&2
  exit 1
fi

migration_config="${smoke_dir}/codex-config.toml"
printf '%s\n' \
  '[mcp_servers.hubu]' \
  'command = "hubu-mcp-server"' \
  '[mcp_servers.hubu.env]' \
  'HUBU_URL = "http://127.0.0.1:8787"' \
  '[mcp_servers.gongbu]' \
  'command = "gongbu-mcp"' \
  '[mcp_servers.gongbu.env]' \
  'GONGBU_MCP_ENDPOINT = "http://127.0.0.1:8788"' \
  '[mcp_servers.other]' \
  'command = "keep"' >"${migration_config}"
"${package_dir}/hubu" init codex \
  --config "${migration_config}" \
  --mcp-server "${package_dir}/hubu-unified-mcp" \
  --token-file "${smoke_dir}/hubu.auth-token" \
  --reconciliation-token-file "${smoke_dir}/hubu.reconciliation-token" \
  --approval-token-file "${smoke_dir}/hubu.approval-token" \
  --migrate-standalone >/dev/null
grep -E 'command = ".*/hubu-unified-mcp"' "${migration_config}" >/dev/null
grep -F '[mcp_servers.other]' "${migration_config}" >/dev/null
if grep -F 'hubu-mcp-server' "${migration_config}" >/dev/null || \
   grep -F 'gongbu-mcp' "${migration_config}" >/dev/null; then
  echo "standalone MCP entries remained after explicit migration" >&2
  exit 1
fi

compatibility_config="$("${package_dir}/hubu" init codex \
  --dry-run \
  --compatibility-standalone \
  --mcp-server "${package_dir}/hubu-mcp-server" \
  --token-file "${smoke_dir}/hubu.auth-token" \
  --reconciliation-token-file "${smoke_dir}/hubu.reconciliation-token" \
  --approval-token-file "${smoke_dir}/hubu.approval-token")"
grep -E 'command = ".*/hubu-mcp-server"' <<<"${compatibility_config}" >/dev/null
grep -F 'HUBU_URL' <<<"${compatibility_config}" >/dev/null

# Both standalone adapters remain packaged, startable compatibility surfaces.
hubu_mcp_response="$(printf '%s\n' "${initialize}" | \
  HUBU_URL="http://127.0.0.1:${port}" "${package_dir}/hubu-mcp-server")"
jq -e \
  --arg product_version "${product_version}" \
  '.result.serverInfo.name == "hubu-mcp-server" and
   .result.serverInfo.version == $product_version' \
  <<<"${hubu_mcp_response}" >/dev/null
gongbu_mcp_response="$(printf '%s\n' "${initialize}" | \
  GONGBU_MCP_ENDPOINT="http://127.0.0.1:9" \
  GONGBU_MCP_BEARER_TOKEN="archive-smoke-no-provider" \
  "${package_dir}/gongbu-mcp")"
jq -e \
  --arg product_version "${product_version}" \
  '.result.serverInfo.name == "gongbu-mcp" and
   .result.serverInfo.version == $product_version' \
  <<<"${gongbu_mcp_response}" >/dev/null

echo "Verified unified archive ${package_name}: six binaries, unified default discovery, and opt-in standalone compatibility; no provider call or spend was attempted"
