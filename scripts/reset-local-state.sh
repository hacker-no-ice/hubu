#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DB_PATH="${HUBU_DB_PATH:-${ROOT_DIR}/hubu.sqlite3}"
AUTH_TOKEN_PATH="${HUBU_AUTH_TOKEN_FILE:-${ROOT_DIR}/hubu.auth-token}"
CONFIRM=0
INCLUDE_AUTH_TOKEN=0

usage() {
  cat <<'USAGE'
Reset local Hubu demo state.

Usage:
  ./scripts/reset-local-state.sh [--yes] [--include-auth-token]

By default this is a dry run. Pass --yes to delete the local SQLite DB.
Pass --include-auth-token with --yes to also delete the local auth token file.

Environment:
  HUBU_DB_PATH              Override the database path
  HUBU_AUTH_TOKEN_FILE      Override the auth token file path
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

if [[ "${CONFIRM}" -ne 1 ]]; then
  echo
  echo "Dry run only. Re-run with --yes to delete the listed file(s)."
  exit 0
fi

rm -f "${DB_PATH}"
if [[ "${INCLUDE_AUTH_TOKEN}" -eq 1 ]]; then
  rm -f "${AUTH_TOKEN_PATH}"
fi

echo "Local Hubu state reset complete."
