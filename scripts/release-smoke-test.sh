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

cleanup() {
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

"$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/verify-release-archive.sh" \
  "${smoke_dir}/${asset_name}" \
  "${product_version}" \
  "${source_commit}"

echo "Verified ${release_tag} (${asset_name}) at ${source_commit}"
