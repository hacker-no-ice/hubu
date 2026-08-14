#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NESTED_WORK_DIR="${ROOT_DIR}/crates/hubu-api"
SERVER_PID=""
CHECK_DIR=""
CREDENTIAL_PATHS=(
  "${ROOT_DIR}/hubu.auth-token"
  "${ROOT_DIR}/hubu.reconciliation-token"
  "${NESTED_WORK_DIR}/hubu.auth-token"
  "${NESTED_WORK_DIR}/hubu.reconciliation-token"
)

stop_server() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  SERVER_PID=""
}

cleanup() {
  stop_server
  for credential in "${CREDENTIAL_PATHS[@]}"; do
    rm -f "${credential}"
  done
  if [[ -n "${CHECK_DIR}" ]]; then
    rm -rf "${CHECK_DIR}"
  fi
}

# Check every cleanup target before installing the trap. From this point on, the
# helper removes only paths that were absent when this invocation began.
for credential in "${CREDENTIAL_PATHS[@]}"; do
  if [[ -e "${credential}" ]]; then
    echo "refusing to replace existing default credential files" >&2
    exit 1
  fi
done

CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hubu-credential-ignore.XXXXXX")"
trap cleanup EXIT

cargo build --locked --bin hubu-server

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
    if ! git -C "${ROOT_DIR}" check-ignore --quiet -- "${credential}"; then
      echo "${label} $(basename "${credential}") is not ignored" >&2
      exit 1
    fi
  done

  if [[ -n "$(git -C "${ROOT_DIR}" status --porcelain --untracked-files=all -- \
    "${auth_token_path}" "${reconciliation_token_path}")" ]]; then
    echo "a generated ${label} credential file is eligible for commit" >&2
    exit 1
  fi

  stop_server
  rm -f "${auth_token_path}" "${reconciliation_token_path}"
}

run_default_server_check "${ROOT_DIR}" "root"
run_default_server_check "${NESTED_WORK_DIR}" "nested"

for ignored_path in \
  "${ROOT_DIR}/.hubu/hubu.auth-token" \
  "${ROOT_DIR}/.hubu/hubu.reconciliation-token"; do
  if ! git -C "${ROOT_DIR}" check-ignore --quiet -- "${ignored_path}"; then
    echo "$(basename "${ignored_path}") in .hubu is not ignored" >&2
    exit 1
  fi
done

echo "root and nested default auth and reconciliation credential files are ignored"
