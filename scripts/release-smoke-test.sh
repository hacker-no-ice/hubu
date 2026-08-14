#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 4 ]]; then
  echo "usage: $0 RELEASE_TAG ASSET_NAME PRODUCT_VERSION SOURCE_COMMIT" >&2
  exit 2
fi

release_tag="$1"
asset_name="$2"
product_version="$3"
source_commit="$4"
repo="${GH_REPO:?GH_REPO must name the GitHub owner/repository}"
smoke_dir="$(mktemp -d)"
server_pid=""
port=18787
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

gh release download "${release_tag}" --repo "${repo}" --dir "${smoke_dir}" --pattern "${asset_name}"
gh release download "${release_tag}" --repo "${repo}" --dir "${smoke_dir}" --pattern SHA256SUMS

checksum_line="$(grep -F "  ${asset_name}" "${smoke_dir}/SHA256SUMS")"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "${smoke_dir}" && printf '%s\n' "${checksum_line}" | sha256sum -c -)
else
  expected_checksum="${checksum_line%% *}"
  actual_checksum="$(shasum -a 256 "${smoke_dir}/${asset_name}" | awk '{print $1}')"
  [[ "${actual_checksum}" == "${expected_checksum}" ]]
fi

tar -C "${smoke_dir}" -xzf "${smoke_dir}/${asset_name}"
package_dir="${smoke_dir}/${asset_name%.tar.gz}"
expected_files=(
  hubu
  hubu-server
  PROVENANCE.json
  LICENSE-MIT
  LICENSE-APACHE
  THIRD-PARTY-NOTICES.md
  Cargo.lock
)
for expected_file in "${expected_files[@]}"; do
  if [[ ! -s "${package_dir}/${expected_file}" ]]; then
    echo "release archive is missing non-empty ${expected_file}" >&2
    exit 1
  fi
done
chmod +x "${package_dir}/hubu" "${package_dir}/hubu-server"

version_output="$("${package_dir}/hubu-server" --version)"
grep -F "\"product_version\": \"${product_version}\"" <<<"${version_output}" >/dev/null
grep -F "\"source_commit\": \"${source_commit}\"" <<<"${version_output}" >/dev/null
grep -F '"executor_contract": "hubu-spend-executor-v4"' <<<"${version_output}" >/dev/null

HUBU_DB_PATH="${smoke_dir}/hubu.sqlite3" \
HUBU_AUTH_TOKEN_FILE="${smoke_dir}/hubu.auth-token" \
HUBU_RECONCILIATION_TOKEN_FILE="${smoke_dir}/hubu.reconciliation-token" \
  "${package_dir}/hubu-server" "127.0.0.1:${port}" >"${smoke_dir}/server.log" 2>&1 &
server_pid="$!"

for _ in {1..12}; do
  if curl --fail --silent \
    --connect-timeout "${curl_connect_timeout_seconds}" \
    --max-time "${curl_max_time_seconds}" \
    "http://127.0.0.1:${port}/health" >/dev/null; then
    break
  fi
  if ! kill -0 "${server_pid}" >/dev/null 2>&1; then
    cat "${smoke_dir}/server.log" >&2
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
grep -F "\"product_version\":\"${product_version}\"" <<<"${reported_version}" >/dev/null
grep -F "\"source_commit\":\"${source_commit}\"" <<<"${reported_version}" >/dev/null
grep -F '"executor_contract":"hubu-spend-executor-v4"' <<<"${reported_version}" >/dev/null

echo "Verified ${release_tag} (${asset_name}) at ${source_commit}"
