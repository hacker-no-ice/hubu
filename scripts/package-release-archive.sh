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

mkdir -p "${package_dir}"
cp "${binary_dir}/hubu" "${package_dir}/hubu"
cp "${binary_dir}/hubu-server" "${package_dir}/hubu-server"
cp "${root_dir}/LICENSE-MIT" "${package_dir}/LICENSE-MIT"
cp "${root_dir}/LICENSE-APACHE" "${package_dir}/LICENSE-APACHE"
cp "${root_dir}/THIRD-PARTY-NOTICES.md" "${package_dir}/THIRD-PARTY-NOTICES.md"
cp "${root_dir}/Cargo.lock" "${package_dir}/Cargo.lock"
jq -n \
  --arg product_version "${release_version}" \
  --arg source_commit "${source_commit}" \
  --arg executor_contract "hubu-spend-executor-v4" \
  --arg target "${target}" \
  --arg repository "${repository}" \
  --arg workflow_run "${workflow_run}" \
  '{product_version: $product_version, source_commit: $source_commit, executor_contract: $executor_contract, target: $target, repository: $repository, workflow_run: $workflow_run, dependencies: "Cargo.lock"}' \
  > "${package_dir}/PROVENANCE.json"
tar -C "${dist_dir}" -czf "${dist_dir}/${package}.tar.gz" "${package}"
