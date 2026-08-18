#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
  echo "usage: $0 PRODUCT_VERSION SOURCE_COMMIT" >&2
  exit 2
fi

product_version="$1"
source_commit="$2"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_dir="$(mktemp -d)"
trap 'rm -rf "${test_dir}"' EXIT
target="$(rustc -vV | sed -n 's/^host: //p')"

"${root_dir}/scripts/package-release-archive.sh" \
  "${product_version}" \
  "${source_commit}" \
  "${target}" \
  "${root_dir}/target/release" \
  "${test_dir}" \
  "hacker-no-ice/hubu" \
  "local-release-check"
"${root_dir}/scripts/verify-release-archive.sh" \
  "${test_dir}/hubu-${product_version}-${target}.tar.gz" \
  "${product_version}" \
  "${source_commit}"
