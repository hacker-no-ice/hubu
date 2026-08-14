#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hubu-credential-refusal.XXXXXX")"
CHECKOUT_DIR="${CHECK_DIR}/checkout"
HELPER_PID=""

cleanup() {
  if [[ -n "${HELPER_PID}" ]]; then
    # Release the fake cargo child, terminate the helper if it is still alive,
    # and reap the helper before deleting any path the child may reference.
    touch "${CHECK_DIR}/build-release"
    if kill -0 "${HELPER_PID}" 2>/dev/null; then
      kill "${HELPER_PID}" 2>/dev/null || true
    fi
    wait "${HELPER_PID}" 2>/dev/null || true
  fi
  if [[ -n "${CHECK_DIR}" ]]; then
    rm -rf "${CHECK_DIR}"
  fi
}
trap cleanup EXIT

git clone --quiet --no-hardlinks "${ROOT_DIR}" "${CHECKOUT_DIR}"
# Exercise the current working-tree helper and ignore rules even before a local
# fix is committed. All fixtures and cleanup remain inside this private clone.
cp "${ROOT_DIR}/.gitignore" "${CHECKOUT_DIR}/.gitignore"
cp "${ROOT_DIR}/scripts/check-default-credential-ignore.sh" \
  "${CHECKOUT_DIR}/scripts/check-default-credential-ignore.sh"

AUTH_TOKEN_PATH="${CHECKOUT_DIR}/hubu.auth-token"
RECONCILIATION_TOKEN_PATH="${CHECKOUT_DIR}/hubu.reconciliation-token"
printf '%s\n' 'auth-sentinel' >"${CHECK_DIR}/expected-auth"
printf '%s\n' 'reconciliation-sentinel' >"${CHECK_DIR}/expected-reconciliation"
cp "${CHECK_DIR}/expected-auth" "${AUTH_TOKEN_PATH}"
cp "${CHECK_DIR}/expected-reconciliation" "${RECONCILIATION_TOKEN_PATH}"

if "${CHECKOUT_DIR}/scripts/check-default-credential-ignore.sh" \
  >"${CHECK_DIR}/helper.stdout" 2>"${CHECK_DIR}/helper.stderr"; then
  echo "credential helper did not refuse pre-existing credentials" >&2
  exit 1
fi

if ! grep -q 'refusing to replace existing default credential files' \
  "${CHECK_DIR}/helper.stderr"; then
  echo "credential helper failed without the expected refusal" >&2
  exit 1
fi

if [[ ! -f "${AUTH_TOKEN_PATH}" || ! -f "${RECONCILIATION_TOKEN_PATH}" ]] || \
  ! cmp -s "${CHECK_DIR}/expected-auth" "${AUTH_TOKEN_PATH}" || \
  ! cmp -s "${CHECK_DIR}/expected-reconciliation" "${RECONCILIATION_TOKEN_PATH}"; then
  echo "credential helper altered a pre-existing credential" >&2
  exit 1
fi

rm -f "${AUTH_TOKEN_PATH}" "${RECONCILIATION_TOKEN_PATH}"

# Exercise the review-reported race inside the private clone: let preflight
# finish and the helper's trap install, then create credentials while a fake
# cargo build is blocked.
mkdir -p "${CHECK_DIR}/bin"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'touch "${HUBU_REFUSAL_TEST_READY}"' \
  'while [[ ! -e "${HUBU_REFUSAL_TEST_RELEASE}" ]]; do sleep 0.01; done' \
  'exit 1' \
  >"${CHECK_DIR}/bin/cargo"
chmod +x "${CHECK_DIR}/bin/cargo"

PATH="${CHECK_DIR}/bin:${PATH}" \
  HUBU_REFUSAL_TEST_READY="${CHECK_DIR}/build-ready" \
  HUBU_REFUSAL_TEST_RELEASE="${CHECK_DIR}/build-release" \
  "${CHECKOUT_DIR}/scripts/check-default-credential-ignore.sh" \
  >"${CHECK_DIR}/race.stdout" 2>"${CHECK_DIR}/race.stderr" &
HELPER_PID=$!

for _ in $(seq 1 100); do
  if [[ -e "${CHECK_DIR}/build-ready" ]]; then
    break
  fi
  if ! kill -0 "${HELPER_PID}" 2>/dev/null; then
    echo "credential helper exited before the delayed build" >&2
    exit 1
  fi
  sleep 0.01
done

if [[ ! -e "${CHECK_DIR}/build-ready" ]]; then
  echo "credential helper did not reach the delayed build" >&2
  exit 1
fi

cp "${CHECK_DIR}/expected-auth" "${AUTH_TOKEN_PATH}"
cp "${CHECK_DIR}/expected-reconciliation" "${RECONCILIATION_TOKEN_PATH}"
touch "${CHECK_DIR}/build-release"

if wait "${HELPER_PID}"; then
  HELPER_PID=""
  echo "credential helper unexpectedly passed the forced build failure" >&2
  exit 1
fi
HELPER_PID=""

if [[ ! -f "${AUTH_TOKEN_PATH}" || ! -f "${RECONCILIATION_TOKEN_PATH}" ]] || \
  ! cmp -s "${CHECK_DIR}/expected-auth" "${AUTH_TOKEN_PATH}" || \
  ! cmp -s "${CHECK_DIR}/expected-reconciliation" "${RECONCILIATION_TOKEN_PATH}"; then
  echo "credential helper altered a credential created after preflight" >&2
  exit 1
fi

# Exercise the interruption cleanup path directly: cleanup must release,
# terminate, and reap a blocked helper before removing its temporary directory.
rm -f "${CHECK_DIR}/build-release"
(
  while [[ ! -e "${CHECK_DIR}/build-release" ]]; do
    sleep 0.01
  done
) &
HELPER_PID=$!
INTERRUPTION_HELPER_PID="${HELPER_PID}"
cleanup
HELPER_PID=""
CHECK_DIR=""

if kill -0 "${INTERRUPTION_HELPER_PID}" 2>/dev/null; then
  echo "credential helper cleanup left a delayed helper running" >&2
  exit 1
fi

echo "credential helper preserves isolated credentials and reaps delayed helpers"
