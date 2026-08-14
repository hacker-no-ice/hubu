#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_PID=""
CHECK_DIR=""

stop_server() {
  if [[ -n "${SERVER_PID}" ]]; then
    if kill -0 "${SERVER_PID}" 2>/dev/null; then
      kill "${SERVER_PID}" 2>/dev/null || true
    fi
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  SERVER_PID=""
}

cleanup() {
  stop_server
  if [[ -n "${CHECK_DIR}" ]]; then
    rm -rf "${CHECK_DIR}"
  fi
}

CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hubu-credential-ignore.XXXXXX")"
trap cleanup EXIT

cargo build --locked --bin hubu-server

CHECKOUT_DIR="${CHECK_DIR}/checkout"
git clone --quiet --no-hardlinks "${ROOT_DIR}" "${CHECKOUT_DIR}"
# Include the ignore rules under test when the helper runs before they are
# committed locally. In CI this is identical to the cloned file.
cp "${ROOT_DIR}/.gitignore" "${CHECKOUT_DIR}/.gitignore"
NESTED_WORK_DIR="${CHECKOUT_DIR}/crates/hubu-api"

run_default_server_check() {
  local work_dir="$1"
  local label="$2"
  local auth_token_path="${work_dir}/hubu.auth-token"
  local reconciliation_token_path="${work_dir}/hubu.reconciliation-token"

  (
    cd "${work_dir}"
    env \
      -u HUBU_AUTH_TOKEN \
      -u HUBU_AUTH_TOKEN_FILE \
      -u HUBU_RECONCILIATION_TOKEN \
      -u HUBU_RECONCILIATION_TOKEN_FILE \
      HUBU_DB_PATH="${CHECK_DIR}/${label}.sqlite3" \
      "${ROOT_DIR}/target/debug/hubu-server" "127.0.0.1:0" \
      >"${CHECK_DIR}/${label}-server.log" 2>&1
  ) &
  SERVER_PID=$!

  for _ in $(seq 1 100); do
    if [[ -f "${auth_token_path}" && -f "${reconciliation_token_path}" ]]; then
      break
    fi
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
      echo "hubu-server exited before creating ${label} default credential files" >&2
      exit 1
    fi
    sleep 0.05
  done

  for credential in "${auth_token_path}" "${reconciliation_token_path}"; do
    if [[ ! -s "${credential}" ]]; then
      echo "hubu-server did not create a non-empty ${label} $(basename "${credential}")" >&2
      exit 1
    fi
    if ! git -C "${CHECKOUT_DIR}" check-ignore --quiet -- "${credential}"; then
      echo "${label} $(basename "${credential}") is not ignored" >&2
      exit 1
    fi
  done

  if [[ -n "$(git -C "${CHECKOUT_DIR}" status --porcelain --untracked-files=all -- \
    "${auth_token_path}" "${reconciliation_token_path}")" ]]; then
    echo "a generated ${label} credential file is eligible for commit" >&2
    exit 1
  fi

  stop_server
}

run_default_server_check "${CHECKOUT_DIR}" "root"
run_default_server_check "${NESTED_WORK_DIR}" "nested"

for ignored_path in \
  "${CHECKOUT_DIR}/.hubu/hubu.auth-token" \
  "${CHECKOUT_DIR}/.hubu/hubu.reconciliation-token"; do
  if ! git -C "${CHECKOUT_DIR}" check-ignore --quiet -- "${ignored_path}"; then
    echo "$(basename "${ignored_path}") in .hubu is not ignored" >&2
    exit 1
  fi
done

echo "root and nested default auth and reconciliation credential files are ignored"
