#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUTH_TOKEN_PATH="${ROOT_DIR}/hubu.auth-token"
RECONCILIATION_TOKEN_PATH="${ROOT_DIR}/hubu.reconciliation-token"
CHECK_DIR=""

# Do not install a cleanup trap until every path it could remove is known to be
# absent. This regression must never disturb real local credentials either.
if [[ -e "${AUTH_TOKEN_PATH}" || -e "${RECONCILIATION_TOKEN_PATH}" ]]; then
  echo "refusing to replace existing default credential files" >&2
  exit 1
fi

CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hubu-credential-refusal.XXXXXX")"
cleanup() {
  rm -f "${AUTH_TOKEN_PATH}" "${RECONCILIATION_TOKEN_PATH}"
  rm -rf "${CHECK_DIR}"
}
trap cleanup EXIT

printf '%s\n' 'auth-sentinel' >"${CHECK_DIR}/expected-auth"
printf '%s\n' 'reconciliation-sentinel' >"${CHECK_DIR}/expected-reconciliation"
cp "${CHECK_DIR}/expected-auth" "${AUTH_TOKEN_PATH}"
cp "${CHECK_DIR}/expected-reconciliation" "${RECONCILIATION_TOKEN_PATH}"

if "${ROOT_DIR}/scripts/check-default-credential-ignore.sh" \
  >"${CHECK_DIR}/helper.stdout" 2>"${CHECK_DIR}/helper.stderr"; then
  echo "credential helper did not refuse pre-existing credentials" >&2
  exit 1
fi

if ! grep -q 'refusing to replace existing default credential files' \
  "${CHECK_DIR}/helper.stderr"; then
  echo "credential helper failed without the expected refusal" >&2
  exit 1
fi

if [[ ! -f "${AUTH_TOKEN_PATH}" || ! -f "${RECONCILIATION_TOKEN_PATH}" ]]; then
  echo "credential helper deleted a pre-existing credential" >&2
  exit 1
fi

if ! cmp -s "${CHECK_DIR}/expected-auth" "${AUTH_TOKEN_PATH}" || \
  ! cmp -s "${CHECK_DIR}/expected-reconciliation" "${RECONCILIATION_TOKEN_PATH}"; then
  echo "credential helper altered a pre-existing credential" >&2
  exit 1
fi

echo "credential helper preserves pre-existing credential contents"
