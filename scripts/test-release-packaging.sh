#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_dir="$(mktemp -d)"
trap 'rm -rf "${test_dir}"' EXIT

mkdir -p "${test_dir}/bin" "${test_dir}/dist" "${test_dir}/unpacked"
printf '#!/usr/bin/env sh\nexit 0\n' > "${test_dir}/bin/hubu"
printf '#!/usr/bin/env sh\nexit 0\n' > "${test_dir}/bin/hubu-server"

"${root_dir}/scripts/package-release-archive.sh" \
  "0.0.0-test" \
  "0000000000000000000000000000000000000000" \
  "test-target" \
  "${test_dir}/bin" \
  "${test_dir}/dist" \
  "hacker-no-ice/hubu" \
  "https://github.com/hacker-no-ice/hubu/actions/runs/1/attempts/1"

archive="${test_dir}/dist/hubu-0.0.0-test-test-target.tar.gz"
tar -C "${test_dir}/unpacked" -xzf "${archive}"
package_dir="${test_dir}/unpacked/hubu-0.0.0-test-test-target"

for expected_file in \
  hubu \
  hubu-server \
  PROVENANCE.json \
  LICENSE-MIT \
  LICENSE-APACHE \
  THIRD-PARTY-NOTICES.md \
  Cargo.lock; do
  test -s "${package_dir}/${expected_file}"
done

jq -e \
  '.product_version == "0.0.0-test" and
   .source_commit == "0000000000000000000000000000000000000000" and
   .target == "test-target" and
   .repository == "hacker-no-ice/hubu"' \
  "${package_dir}/PROVENANCE.json" >/dev/null

echo "Release packaging test passed"
