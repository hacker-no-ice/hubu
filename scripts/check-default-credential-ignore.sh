#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUTH_TOKEN_PATH="${ROOT_DIR}/hubu.auth-token"
RECONCILIATION_TOKEN_PATH="${ROOT_DIR}/hubu.reconciliation-token"
SERVER_PID=""
CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hubu-credential-ignore.XXXXXX")"

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -f "${AUTH_TOKEN_PATH}" "${RECONCILIATION_TOKEN_PATH}"
  rm -rf "${CHECK_DIR}"
}
trap cleanup EXIT

if [[ -e "${AUTH_TOKEN_PATH}" || -e "${RECONCILIATION_TOKEN_PATH}" ]]; then
  echo "refusing to replace existing default credential files" >&2
  exit 1
fi

cargo build --locked --bin hubu-server

(
  cd "${ROOT_DIR}"
  env \
    -u HUBU_AUTH_TOKEN \
    -u HUBU_AUTH_TOKEN_FILE \
    -u HUBU_RECONCILIATION_TOKEN \
    -u HUBU_RECONCILIATION_TOKEN_FILE \
    HUBU_DB_PATH="${CHECK_DIR}/hubu.sqlite3" \
    "${ROOT_DIR}/target/debug/hubu-server" "127.0.0.1:0" \
    >"${CHECK_DIR}/server.log" 2>&1
) &
SERVER_PID=$!

for _ in $(seq 1 100); do
  if [[ -f "${AUTH_TOKEN_PATH}" && -f "${RECONCILIATION_TOKEN_PATH}" ]]; then
    break
  fi
  if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
    echo "hubu-server exited before creating default credential files" >&2
    exit 1
  fi
  sleep 0.05
done

for credential in "${AUTH_TOKEN_PATH}" "${RECONCILIATION_TOKEN_PATH}"; do
  if [[ ! -s "${credential}" ]]; then
    echo "hubu-server did not create a non-empty $(basename "${credential}")" >&2
    exit 1
  fi
done

for ignored_path in \
  "${AUTH_TOKEN_PATH}" \
  "${RECONCILIATION_TOKEN_PATH}" \
  "${ROOT_DIR}/.hubu/hubu.auth-token" \
  "${ROOT_DIR}/.hubu/hubu.reconciliation-token"; do
  if ! git -C "${ROOT_DIR}" check-ignore --quiet -- "${ignored_path}"; then
    echo "$(basename "${ignored_path}") at a documented location is not ignored" >&2
    exit 1
  fi
done

if [[ -n "$(git -C "${ROOT_DIR}" status --porcelain --untracked-files=all -- \
  "${AUTH_TOKEN_PATH}" "${RECONCILIATION_TOKEN_PATH}")" ]]; then
  echo "a generated default credential file is eligible for commit" >&2
  exit 1
fi

echo "default auth and reconciliation credential files are ignored"
