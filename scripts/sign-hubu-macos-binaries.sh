#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 || ( "$1" != "sign" && "$1" != "verify" ) ]]; then
  echo "usage: $0 {sign|verify} BINARY_DIRECTORY" >&2
  exit 2
fi

mode="$1"
binary_dir="$2"

for name in hubu hubu-server; do
  binary="${binary_dir%/}/${name}"
  if [[ ! -s "${binary}" || ! -x "${binary}" ]]; then
    echo "missing executable for macOS signing: ${binary}" >&2
    exit 1
  fi
  if [[ "${mode}" == "sign" ]]; then
    codesign --force --sign - --options runtime --timestamp=none "${binary}"
  fi
  codesign --verify --strict "${binary}"
  signature="$(LC_ALL=C codesign --display --verbose=2 "${binary}" 2>&1)"
  if ! grep -Eq '^CodeDirectory .*flags=0x[0-9a-f]+\(([^),]+,)*runtime[),]' <<<"${signature}" ||
    ! grep -Fxq 'Signature=adhoc' <<<"${signature}"; then
    echo "missing hardened ad hoc signature: ${binary}" >&2
    exit 1
  fi
done
