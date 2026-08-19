#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
  echo "usage: $0 TARGET OUTPUT_PATH" >&2
  exit 2
fi

target="$1"
output_path="$2"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
active_packages="$(
  cargo tree \
    --manifest-path "${root_dir}/Cargo.toml" \
    --locked \
    --target "${target}" \
    -p hubu-cli \
    -p hubu-api \
    -p hubu-unified-mcp \
    -p gongbu-api \
    --edges normal \
    --prefix none \
    --format '{p}' \
    | sed -E 's/ \(\*\)$//; s/ \(proc-macro\)$//' \
    | sort -u
)"
metadata="$(cargo metadata --manifest-path "${root_dir}/Cargo.toml" --locked --format-version 1)"

shopt -s nullglob
{
  printf '%s\n' 'Hubu Third-Party Dependency Licenses'
  printf '%s\n' '===================================='
  printf '\nTarget: %s\n' "${target}"
  printf '%s\n' 'Graph: locked normal dependencies of the four packaged Hubu and Gongbu binaries'

  while IFS=$'\t' read -r package_name package_version declared_license source manifest_path; do
    package_key="${package_name} v${package_version}"
    if ! grep -Fqx "${package_key}" <<< "${active_packages}"; then
      continue
    fi

    package_dir="${manifest_path%/Cargo.toml}"
    license_files=(
      "${package_dir}"/LICENSE*
      "${package_dir}"/COPYING*
      "${package_dir}"/COPYRIGHT*
      "${package_dir}"/NOTICE*
    )
    if [[ "${#license_files[@]}" -eq 0 ]]; then
      case "${package_key}" in
        "objc2-core-foundation v0.3.2"|"objc2-io-kit v0.3.2")
          license_files=("${root_dir}/third-party-license-material/objc2-0.3.2-LICENSE.md")
          ;;
        "pbjson v0.9.0")
          license_files=("${root_dir}/third-party-license-material/pbjson-0.9.0-LICENSE")
          ;;
        "prost-wkt v0.7.1"|"prost-wkt-types v0.7.1")
          license_files=("${root_dir}/LICENSE-APACHE")
          ;;
        "tonic-prost v0.14.6")
          license_files=("${root_dir}/third-party-license-material/tonic-0.14.6-LICENSE")
          ;;
      esac
    fi
    if [[ "${#license_files[@]}" -eq 0 ]]; then
      echo "no license material found for ${package_key}" >&2
      exit 1
    fi

    printf '\n%s\n' '-------------------------------------------------------------------------------'
    printf 'Package: %s\n' "${package_key}"
    printf 'Declared license: %s\n' "${declared_license}"
    printf 'Source: %s\n' "${source}"
    for license_file in "${license_files[@]}"; do
      printf '\nLicense file: %s\n\n' "${license_file##*/}"
      sed -e '$a\' "${license_file}"
    done
  done < <(
    jq -r '
      .packages
      | map(select(.source != null))
      | sort_by(.name, .version, .source)
      | .[]
      | [.name, .version, (.license // "NOASSERTION"), .source, .manifest_path]
      | @tsv
    ' <<< "${metadata}"
  )
} > "${output_path}"
