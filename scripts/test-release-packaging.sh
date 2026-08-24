#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_dir="$(mktemp -d)"
trap 'rm -rf "${test_dir}"' EXIT

production_binaries=(
  hubu
  hubu-server
  hubu-unified-mcp
  gongbu-server
)
packaging_test_targets=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
)

mkdir -p "${test_dir}/bin"
for binary in "${production_binaries[@]}" hubu-bench gongbu-sandbox; do
  printf '#!/usr/bin/env sh\nexit 0\n' > "${test_dir}/bin/${binary}"
  chmod +x "${test_dir}/bin/${binary}"
done

verify_package() {
  local target="$1"
  local archive="${test_dir}/dist-${target}/hubu-0.0.0-test-${target}.tar.gz"
  local unpacked="${test_dir}/unpacked-${target}"
  local package_dir="${unpacked}/hubu-0.0.0-test-${target}"
  mkdir -p "${unpacked}"
  tar -C "${unpacked}" -xzf "${archive}"

  expected_files=(
    Cargo.lock
    LICENSE-APACHE
    LICENSE-MIT
    LOCAL-STACK.md
    MANIFEST.json
    PROVENANCE.json
    SHA256SUMS
    THIRD-PARTY-LICENSES.txt
    THIRD-PARTY-NOTICES.md
    gongbu-server
    hubu
    hubu-server
    hubu-unified-mcp
    operations/gongbu-server.md
    unified-mcp.md
  )
  actual_files="$(cd "${package_dir}" && find . -type f -print | sed 's#^\./##' | LC_ALL=C sort)"
  expected_listing="$(printf '%s\n' "${expected_files[@]}" | LC_ALL=C sort)"
  [[ "${actual_files}" == "${expected_listing}" ]]

  for expected_file in "${expected_files[@]}"; do
    test -s "${package_dir}/${expected_file}"
  done
  test ! -e "${package_dir}/hubu-bench"
  test ! -e "${package_dir}/gongbu-sandbox"
  jq -e \
    --arg target "${target}" \
    '.schema_version == 2 and
     .product == "hubu" and
     .product_version == "0.0.0-test" and
     .source_commit == "0000000000000000000000000000000000000000" and
     .executor_contract == "hubu-spend-executor-v4.3" and
     .target == $target and
     .binaries == ["hubu", "hubu-server", "hubu-unified-mcp", "gongbu-server"] and
     .supported_agent_surfaces == ["hubu-unified-mcp"] and
     (.files | contains(["LOCAL-STACK.md", "operations/gongbu-server.md", "unified-mcp.md"])) and
     .development_tools_excluded == ["hubu-bench", "gongbu-sandbox"]' \
    "${package_dir}/MANIFEST.json" >/dev/null
  jq -e \
    --arg target "${target}" \
    '.schema_version == 2 and
     .product == "hubu" and
     .product_version == "0.0.0-test" and
     .source_commit == "0000000000000000000000000000000000000000" and
     .executor_contract == "hubu-spend-executor-v4.3" and
     .target == $target and
     .repository == "hacker-no-ice/hubu" and
     .binaries == ["hubu", "hubu-server", "hubu-unified-mcp", "gongbu-server"] and
     .supported_agent_surfaces == ["hubu-unified-mcp"] and
     .manifest == "MANIFEST.json" and
     .dependencies == "Cargo.lock"' \
    "${package_dir}/PROVENANCE.json" >/dev/null

  if command -v sha256sum >/dev/null 2>&1; then
    (cd "${package_dir}" && sha256sum -c SHA256SUMS >/dev/null)
  else
    (cd "${package_dir}" && shasum -a 256 -c SHA256SUMS >/dev/null)
  fi

  grep -F 'Package: libsqlite3-sys v0.30.1' \
    "${package_dir}/THIRD-PARTY-LICENSES.txt" >/dev/null
  grep -F 'Package: rusqlite v0.32.1' \
    "${package_dir}/THIRD-PARTY-LICENSES.txt" >/dev/null
  grep -F 'Permission is hereby granted, free of charge' \
    "${package_dir}/THIRD-PARTY-LICENSES.txt" >/dev/null
}

for target in "${packaging_test_targets[@]}"; do
  mkdir -p "${test_dir}/dist-${target}"
  "${root_dir}/scripts/package-release-archive.sh" \
    "0.0.0-test" \
    "0000000000000000000000000000000000000000" \
    "${target}" \
    "${test_dir}/bin" \
    "${test_dir}/dist-${target}" \
    "hacker-no-ice/hubu" \
    "https://github.com/hacker-no-ice/hubu/actions/runs/1/attempts/1"
  verify_package "${target}"
done

missing_dir="${test_dir}/missing-bin"
mkdir -p "${missing_dir}"
cp "${test_dir}/bin/"* "${missing_dir}/"
rm "${missing_dir}/gongbu-server"
if "${root_dir}/scripts/package-release-archive.sh" \
  "0.0.0-missing" \
  "0000000000000000000000000000000000000000" \
  "x86_64-unknown-linux-gnu" \
  "${missing_dir}" \
  "${test_dir}/missing-dist" \
  "hacker-no-ice/hubu" \
  "local-test" >/dev/null 2>&1; then
  echo "packaging accepted a missing production binary" >&2
  exit 1
fi

empty_dir="${test_dir}/empty-bin"
mkdir -p "${empty_dir}"
cp "${test_dir}/bin/"* "${empty_dir}/"
: > "${empty_dir}/hubu-unified-mcp"
if "${root_dir}/scripts/package-release-archive.sh" \
  "0.0.0-empty" \
  "0000000000000000000000000000000000000000" \
  "x86_64-unknown-linux-gnu" \
  "${empty_dir}" \
  "${test_dir}/empty-dist" \
  "hacker-no-ice/hubu" \
  "local-test" >/dev/null 2>&1; then
  echo "packaging accepted an empty production binary" >&2
  exit 1
fi

echo "Release packaging format tests passed for four target strings; release matrix policy is validated separately"
