#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 7 ]]; then
  echo "usage: $0 RELEASE_VERSION SOURCE_COMMIT TARGET BINARY_DIR DIST_DIR REPOSITORY WORKFLOW_RUN" >&2
  exit 2
fi

release_version="$1"
source_commit="$2"
target="$3"
binary_dir="$4"
dist_dir="$5"
repository="$6"
workflow_run="$7"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
package="hubu-${release_version}-${target}"
package_dir="${dist_dir}/${package}"
production_binaries=(
  hubu
  hubu-server
  hubu-unified-mcp
  gongbu-server
)

if [[ ! "${source_commit}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "source commit must be an exact 40-character lowercase SHA" >&2
  exit 1
fi

for binary in "${production_binaries[@]}"; do
  if [[ ! -f "${binary_dir}/${binary}" || ! -s "${binary_dir}/${binary}" ]]; then
    echo "release binary is missing or empty: ${binary}" >&2
    exit 1
  fi
done

if [[ -e "${package_dir}" || -e "${dist_dir}/${package}.tar.gz" ]]; then
  echo "refusing to overwrite existing release package ${package}" >&2
  exit 1
fi

mkdir -p "${package_dir}"
for binary in "${production_binaries[@]}"; do
  cp "${binary_dir}/${binary}" "${package_dir}/${binary}"
done
cp "${root_dir}/LICENSE-MIT" "${package_dir}/LICENSE-MIT"
cp "${root_dir}/LICENSE-APACHE" "${package_dir}/LICENSE-APACHE"
cp "${root_dir}/THIRD-PARTY-NOTICES.md" "${package_dir}/THIRD-PARTY-NOTICES.md"
cp "${root_dir}/Cargo.lock" "${package_dir}/Cargo.lock"
"${root_dir}/scripts/generate-third-party-licenses.sh" \
  "${target}" \
  "${package_dir}/THIRD-PARTY-LICENSES.txt"
jq -n \
  --arg product_version "${release_version}" \
  --arg source_commit "${source_commit}" \
  --arg executor_contract "hubu-spend-executor-v4.2" \
  --arg target "${target}" \
  '{schema_version: 2, product: "hubu", product_version: $product_version, source_commit: $source_commit, executor_contract: $executor_contract, target: $target, binaries: ["hubu", "hubu-server", "hubu-unified-mcp", "gongbu-server"], supported_agent_surfaces: ["hubu-unified-mcp"], development_tools_excluded: ["hubu-bench", "gongbu-sandbox"], files: ["Cargo.lock", "LICENSE-APACHE", "LICENSE-MIT", "PROVENANCE.json", "SHA256SUMS", "THIRD-PARTY-LICENSES.txt", "THIRD-PARTY-NOTICES.md", "gongbu-server", "hubu", "hubu-server", "hubu-unified-mcp"]}' \
  > "${package_dir}/MANIFEST.json"
jq -n \
  --arg product_version "${release_version}" \
  --arg source_commit "${source_commit}" \
  --arg executor_contract "hubu-spend-executor-v4.2" \
  --arg target "${target}" \
  --arg repository "${repository}" \
  --arg workflow_run "${workflow_run}" \
  '{schema_version: 2, product: "hubu", product_version: $product_version, source_commit: $source_commit, executor_contract: $executor_contract, target: $target, repository: $repository, workflow_run: $workflow_run, binaries: ["hubu", "hubu-server", "hubu-unified-mcp", "gongbu-server"], supported_agent_surfaces: ["hubu-unified-mcp"], manifest: "MANIFEST.json", dependencies: "Cargo.lock", third_party_licenses: "THIRD-PARTY-LICENSES.txt"}' \
  > "${package_dir}/PROVENANCE.json"

checksum_files=(
  Cargo.lock
  LICENSE-APACHE
  LICENSE-MIT
  MANIFEST.json
  PROVENANCE.json
  THIRD-PARTY-LICENSES.txt
  THIRD-PARTY-NOTICES.md
  gongbu-server
  hubu
  hubu-server
  hubu-unified-mcp
)
(
  cd "${package_dir}"
  for file in "${checksum_files[@]}"; do
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "${file}"
    else
      shasum -a 256 "${file}"
    fi
  done
) > "${package_dir}/SHA256SUMS"
tar -C "${dist_dir}" -czf "${dist_dir}/${package}.tar.gz" "${package}"
