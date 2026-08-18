#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DB_PATH="${HUBU_DB_PATH:-${ROOT_DIR}/hubu.sqlite3}"
AUTH_TOKEN_PATH="${HUBU_AUTH_TOKEN_FILE:-${ROOT_DIR}/hubu.auth-token}"
APPROVAL_TOKEN_PATH="${HUBU_APPROVAL_TOKEN_FILE:-${ROOT_DIR}/hubu.approval-token}"
RECONCILIATION_TOKEN_PATH="${HUBU_RECONCILIATION_TOKEN_FILE:-${ROOT_DIR}/hubu.reconciliation-token}"
CONFIRM=0
INCLUDE_AUTH_TOKEN=0
INCLUDE_APPROVAL_TOKEN=0
INCLUDE_RECONCILIATION_TOKEN=0

usage() {
  cat <<'USAGE'
Reset local Hubu demo state.

Usage:
  ./scripts/reset-local-state.sh [--yes] [--include-auth-token] [--include-approval-token] [--include-reconciliation-token]

By default this is a dry run. Pass --yes to delete the local SQLite DB.
Pass --include-auth-token with --yes to also delete the local auth token file.
Pass --include-approval-token with --yes to also delete the separate human
approval capability file.
Pass --include-reconciliation-token with --yes to also delete the separate human
reconciliation capability file. Credential files are never deleted unless their
individual include flag is provided.

Environment:
  HUBU_DB_PATH                        Override the database path
  HUBU_AUTH_TOKEN_FILE                Override the auth token file path
  HUBU_APPROVAL_TOKEN_FILE            Override the approval token file path
  HUBU_RECONCILIATION_TOKEN_FILE      Override the reconciliation token file path
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --yes)
      CONFIRM=1
      ;;
    --include-auth-token)
      INCLUDE_AUTH_TOKEN=1
      ;;
    --include-approval-token)
      INCLUDE_APPROVAL_TOKEN=1
      ;;
    --include-reconciliation-token)
      INCLUDE_RECONCILIATION_TOKEN=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

echo "Hubu local state reset"
echo "  database: ${DB_PATH}"
if [[ "${INCLUDE_AUTH_TOKEN}" -eq 1 ]]; then
  echo "  auth token: ${AUTH_TOKEN_PATH}"
fi
if [[ "${INCLUDE_APPROVAL_TOKEN}" -eq 1 ]]; then
  echo "  approval token: ${APPROVAL_TOKEN_PATH}"
fi
if [[ "${INCLUDE_RECONCILIATION_TOKEN}" -eq 1 ]]; then
  echo "  reconciliation token: ${RECONCILIATION_TOKEN_PATH}"
fi

if [[ "${CONFIRM}" -ne 1 ]]; then
  echo
  echo "Dry run only. Re-run with --yes to delete the listed file(s)."
  exit 0
fi

rm -f "${DB_PATH}"
if [[ "${INCLUDE_AUTH_TOKEN}" -eq 1 ]]; then
  rm -f "${AUTH_TOKEN_PATH}"
fi
if [[ "${INCLUDE_APPROVAL_TOKEN}" -eq 1 ]]; then
  rm -f "${APPROVAL_TOKEN_PATH}"
fi
if [[ "${INCLUDE_RECONCILIATION_TOKEN}" -eq 1 ]]; then
  rm -f "${RECONCILIATION_TOKEN_PATH}"
fi

echo "Local Hubu state reset complete."
