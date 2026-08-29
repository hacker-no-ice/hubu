#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLOW_ADDR="${HUBU_CORE_FLOW_ADDR:-127.0.0.1:8788}"
FLOW_URL="http://${FLOW_ADDR}"
SERVER_LOG="$(mktemp -t hubu-core-flow-server.XXXXXX.log)"
DB_PATH="$(mktemp -t hubu-core-flow.XXXXXX.sqlite3)"
POLICY_FILE="$(mktemp -t hubu-core-flow-policy.XXXXXX.yaml)"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  rm -f "${SERVER_LOG}" "${DB_PATH}" "${POLICY_FILE}"
}
trap cleanup EXIT

fail() {
  printf 'integration-core-flow failed: %s\n' "$*" >&2
  if [[ -s "${SERVER_LOG}" ]]; then
    printf '\nserver log:\n' >&2
    sed 's/^/  /' "${SERVER_LOG}" >&2 || true
  fi
  exit 1
}

assert_contains() {
  local label="$1"
  local haystack="$2"
  local needle="$3"

  if [[ "${haystack}" != *"${needle}"* ]]; then
    printf 'expected %s to contain: %s\n' "${label}" "${needle}" >&2
    printf '\n%s output:\n%s\n' "${label}" "${haystack}" >&2
    fail "missing expected output"
  fi
}

assert_not_contains() {
  local label="$1"
  local haystack="$2"
  local needle="$3"

  if [[ "${haystack}" == *"${needle}"* ]]; then
    printf 'expected %s not to contain: %s\n' "${label}" "${needle}" >&2
    printf '\n%s output:\n%s\n' "${label}" "${haystack}" >&2
    fail "unexpected output"
  fi
}

extract_field() {
  local output="$1"
  local field="$2"

  awk -F': ' -v field="${field}" '$1 == "  " field { print $2; exit }' <<< "${output}"
}

hubu() {
  "${ROOT_DIR}/target/debug/hubu" --url "${FLOW_URL}" "$@"
}

wait_for_server() {
  for _ in $(seq 1 100); do
    if [[ -n "${SERVER_PID}" ]] && ! kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
      fail "server exited before becoming ready"
    fi
    if hubu health >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done

  fail "server did not become ready"
}

cd "${ROOT_DIR}"

cargo build --locked --bin hubu-server --bin hubu

HUBU_DB_PATH="${DB_PATH}" "${ROOT_DIR}/target/debug/hubu-server" "${FLOW_ADDR}" \
  >"${SERVER_LOG}" 2>&1 &
SERVER_PID="$!"
wait_for_server

HUMAN_OUTPUT="$(hubu register human --username alice-example --display-name "Alice Example" --email alice@example.com)"
assert_contains "human registration" "${HUMAN_OUTPUT}" "Human registered"
assert_contains "human registration" "${HUMAN_OUTPUT}" "username: alice-example"
assert_contains "human registration" "${HUMAN_OUTPUT}" "display_name: Alice Example"
USER_ID="$(extract_field "${HUMAN_OUTPUT}" "user_id")"
[[ -n "${USER_ID}" ]] || fail "could not parse user_id"

USER_LIST_OUTPUT="$(hubu user list)"
assert_contains "user list" "${USER_LIST_OUTPUT}" "CURRENT"
assert_contains "user list" "${USER_LIST_OUTPUT}" "${USER_ID}"
assert_contains "user list" "${USER_LIST_OUTPUT}" "alice-example"
assert_contains "user list" "${USER_LIST_OUTPUT}" "*"

GUIDANCE_OUTPUT="$(hubu protocol agent-registration)"
assert_contains "registration guidance" "${GUIDANCE_OUTPUT}" '"protocol_version": "hubu-agent-registration-v1"'
assert_contains "registration guidance" "${GUIDANCE_OUTPUT}" '"canonicalization": "canonical_json_v1"'
assert_contains "registration guidance" "${GUIDANCE_OUTPUT}" '"agent_name"'
assert_contains "registration guidance" "${GUIDANCE_OUTPUT}" '"identity_fingerprint"'

DRY_RUN_OUTPUT="$(hubu register agent --name core-flow-agent --version ci --dry-run)"
assert_contains "agent dry-run envelope" "${DRY_RUN_OUTPUT}" '"agent_name": "core-flow-agent"'
assert_contains "agent dry-run envelope" "${DRY_RUN_OUTPUT}" '"owner"'
assert_contains "agent dry-run envelope" "${DRY_RUN_OUTPUT}" '"pub_id": "'"${USER_ID}"'"'
assert_contains "agent dry-run envelope" "${DRY_RUN_OUTPUT}" '"fingerprint": "sha256:'

AGENT_OUTPUT="$(hubu register agent --name core-flow-agent --version ci)"
assert_contains "agent registration" "${AGENT_OUTPUT}" "Registration review"
assert_contains "agent registration" "${AGENT_OUTPUT}" "Agent registered"
AGENT_ID="$(extract_field "${AGENT_OUTPUT}" "agent_id")"
[[ -n "${AGENT_ID}" ]] || fail "could not parse agent_id"
ACCOUNT_ID="$(extract_field "${AGENT_OUTPUT}" "account_id")"
[[ -n "${ACCOUNT_ID}" ]] || fail "could not parse account_id"

set +e
DUPLICATE_AGENT_OUTPUT="$(hubu register agent --name core-flow-agent --version ci 2>&1)"
DUPLICATE_AGENT_STATUS=$?
set -e
[[ "${DUPLICATE_AGENT_STATUS}" -ne 0 ]] || fail "duplicate agent registration unexpectedly succeeded"
assert_contains "duplicate agent registration" "${DUPLICATE_AGENT_OUTPUT}" "agent is already registered for this owner"
assert_not_contains "duplicate agent registration" "${DUPLICATE_AGENT_OUTPUT}" "Registration review"
assert_not_contains "duplicate agent registration" "${DUPLICATE_AGENT_OUTPUT}" "identity_fingerprint"

rm -f "${POLICY_FILE}"
hubu policy new-template --path "${POLICY_FILE}" >/dev/null
POLICY_VALIDATE_OUTPUT="$(hubu policy validate --path "${POLICY_FILE}")"
assert_contains "policy validate" "${POLICY_VALIDATE_OUTPUT}" "Policy valid"
POLICY_OUTPUT="$(hubu policy add --path "${POLICY_FILE}")"
assert_contains "policy add" "${POLICY_OUTPUT}" "Policy added"
assert_contains "policy add" "${POLICY_OUTPUT}" "scope: user_default"
assert_contains "policy add" "${POLICY_OUTPUT}" "default_decision: needs_approval"
POLICY_LIST_OUTPUT="$(hubu policy list)"
assert_contains "policy list" "${POLICY_LIST_OUTPUT}" "SCOPE"
assert_contains "policy list" "${POLICY_LIST_OUTPUT}" "user_default"
assert_contains "policy list" "${POLICY_LIST_OUTPUT}" "starter_spending_policy"

AGENTS_OUTPUT="$(hubu agent list)"
assert_contains "agent list" "${AGENTS_OUTPUT}" "${AGENT_ID}"
assert_contains "agent list" "${AGENTS_OUTPUT}" "core-flow-agent"
assert_contains "agent list" "${AGENTS_OUTPUT}" "${USER_ID}"
assert_contains "agent list" "${AGENTS_OUTPUT}" "alice-example"
assert_contains "agent list" "${AGENTS_OUTPUT}" "active"
ALL_AGENTS_OUTPUT="$(hubu agent list --all)"
assert_contains "agent list all" "${ALL_AGENTS_OUTPUT}" "${AGENT_ID}"
assert_contains "agent list all" "${ALL_AGENTS_OUTPUT}" "core-flow-agent"

TARGET_OUTPUT="$(hubu user spending-target set --amount 50)"
assert_contains "spending target set" "${TARGET_OUTPUT}" "Spending target set (advisory)"
assert_contains "spending target set" "${TARGET_OUTPUT}" 'target: $50.00'
assert_contains "spending target set" "${TARGET_OUTPUT}" 'allocated: $0.00'
TARGET_ID="$(awk '/target_id:/ { print $2; exit }' <<< "${TARGET_OUTPUT}")"
[[ "${TARGET_ID}" == tgt_* ]] || fail "could not parse public spending target id"

BUDGET_OUTPUT="$(hubu budget create --agent-id "${AGENT_ID}" --amount 75 --ending-before 2999-01-01T00:00:00Z)"
assert_contains "budget create" "${BUDGET_OUTPUT}" "Budget created"
assert_contains "budget create" "${BUDGET_OUTPUT}" 'limit: $75.00'
assert_contains "budget create" "${BUDGET_OUTPUT}" 'remaining: $75.00'
assert_contains "budget create" "${BUDGET_OUTPUT}" "Spending target warning (advisory)"
assert_contains "budget create" "${BUDGET_OUTPUT}" "target_id: ${TARGET_ID}"
assert_contains "budget create" "${BUDGET_OUTPUT}" 'exceeded by: $25.00'
ACTIVE_BUDGET_ID="$(awk '/budget_id:/ { print $2; exit }' <<< "${BUDGET_OUTPUT}")"
[[ "${ACTIVE_BUDGET_ID}" == bgt_* ]] || fail "could not parse active public budget id"

ALLOW_OUTPUT="$(hubu spend --operation-key integration-allow --account-id "${ACCOUNT_ID}" --amount 20 --reason "Purchase API credits")"
assert_contains "allowed spend" "${ALLOW_OUTPUT}" "decision: allow"
assert_contains "allowed spend" "${ALLOW_OUTPUT}" "status: succeeded"
assert_contains "allowed spend" "${ALLOW_OUTPUT}" "owner_user: Alice Example (${USER_ID})"
assert_contains "allowed spend" "${ALLOW_OUTPUT}" "status: settled"
assert_contains "allowed spend" "${ALLOW_OUTPUT}" 'consumed: $20.00'
assert_contains "allowed spend" "${ALLOW_OUTPUT}" 'frozen: $0.00'
assert_contains "allowed spend" "${ALLOW_OUTPUT}" 'remaining: $55.00'
assert_not_contains "allowed spend" "${ALLOW_OUTPUT}" "Cap hold"

FAILED_OUTPUT="$(hubu spend --operation-key integration-failed --account-id "${ACCOUNT_ID}" --amount 15 --reason "Failed merchant payout" --merchant fail)"
assert_contains "failed payment spend" "${FAILED_OUTPUT}" "decision: allow"
assert_contains "failed payment spend" "${FAILED_OUTPUT}" "status: failed"
assert_contains "failed payment spend" "${FAILED_OUTPUT}" "status: released"
assert_contains "failed payment spend" "${FAILED_OUTPUT}" 'consumed: $20.00'
assert_contains "failed payment spend" "${FAILED_OUTPUT}" 'frozen: $0.00'
assert_contains "failed payment spend" "${FAILED_OUTPUT}" 'remaining: $55.00'

OVER_BUDGET_OUTPUT="$(hubu spend --operation-key integration-over-budget --account-id "${ACCOUNT_ID}" --amount 60 --reason "Over budget purchase")"
assert_contains "over budget spend" "${OVER_BUDGET_OUTPUT}" "decision: deny"
assert_contains "over budget spend" "${OVER_BUDGET_OUTPUT}" "budget does not have enough remaining balance"
if [[ "${OVER_BUDGET_OUTPUT}" == *"Payment"* ]]; then
  printf '%s\n' "${OVER_BUDGET_OUTPUT}" >&2
  fail "over budget spend should not create a payment"
fi
if [[ "${OVER_BUDGET_OUTPUT}" == *"Budget hold"* ]]; then
  printf '%s\n' "${OVER_BUDGET_OUTPUT}" >&2
  fail "over budget spend should not create a budget hold"
fi

NEEDS_APPROVAL_OUTPUT="$(hubu spend --operation-key integration-needs-approval --account-id "${ACCOUNT_ID}" --amount 120 --reason "Large API credit purchase")"
assert_contains "approval spend" "${NEEDS_APPROVAL_OUTPUT}" "decision: needs_approval"
if [[ "${NEEDS_APPROVAL_OUTPUT}" == *"Payment"* ]]; then
  printf '%s\n' "${NEEDS_APPROVAL_OUTPUT}" >&2
  fail "needs_approval spend should not create a payment"
fi

DENY_OUTPUT="$(hubu spend --operation-key integration-deny --account-id "${ACCOUNT_ID}" --amount 20 --reason "Blocked merchant purchase" --merchant blocked-merchant)"
assert_contains "denied spend" "${DENY_OUTPUT}" "decision: deny"
assert_contains "denied spend" "${DENY_OUTPUT}" "merchant is blocked by the starter policy"
if [[ "${DENY_OUTPUT}" == *"Payment"* ]]; then
  printf '%s\n' "${DENY_OUTPUT}" >&2
  fail "denied spend should not create a payment"
fi

BALANCE_OUTPUT="$(hubu budget list)"
assert_contains "budget list" "${BALANCE_OUTPUT}" 'limit: $75.00'
assert_contains "budget list" "${BALANCE_OUTPUT}" 'consumed: $20.00'
assert_contains "budget list" "${BALANCE_OUTPUT}" 'frozen: $0.00'
assert_contains "budget list" "${BALANCE_OUTPUT}" 'remaining: $55.00'
TARGET_STATUS_OUTPUT="$(hubu user spending-target show)"
assert_contains "spending target show" "${TARGET_STATUS_OUTPUT}" "${TARGET_ID}"
assert_contains "spending target show" "${TARGET_STATUS_OUTPUT}" 'target: $50.00'
assert_contains "spending target show" "${TARGET_STATUS_OUTPUT}" 'allocated: $75.00'
assert_contains "spending target show" "${TARGET_STATUS_OUTPUT}" 'exceeded by: $25.00'
assert_contains "spending target show" "${TARGET_STATUS_OUTPUT}" 'enforcement: advisory only'

AGENT_BUDGET_OUTPUT="$(hubu budget create --agent-id "${AGENT_ID}" --amount 5 --starting-at 2999-01-01T00:00:00Z --ending-before 2999-01-02T00:00:00Z)"
assert_contains "agent budget create" "${AGENT_BUDGET_OUTPUT}" "Budget created"
assert_contains "agent budget create" "${AGENT_BUDGET_OUTPUT}" "agent_id: ${AGENT_ID}"
assert_contains "agent budget create" "${AGENT_BUDGET_OUTPUT}" 'limit: $5.00'
AGENT_BUDGET_LIST_OUTPUT="$(hubu budget list)"
assert_contains "agent budget list" "${AGENT_BUDGET_LIST_OUTPUT}" "agent_id: ${AGENT_ID}"
assert_contains "agent budget list" "${AGENT_BUDGET_LIST_OUTPUT}" "status: scheduled"
SCHEDULED_BUDGET_ID="$(awk '/budget_id:/ { print $2; exit }' <<< "${AGENT_BUDGET_OUTPUT}")"
[[ "${SCHEDULED_BUDGET_ID}" == bgt_* ]] || fail "could not parse scheduled public budget id"
REPLACED_BUDGET_OUTPUT="$(hubu budget replace --budget-id "${ACTIVE_BUDGET_ID}" --amount 80)"
assert_contains "budget replace" "${REPLACED_BUDGET_OUTPUT}" "Budget replaced"
assert_contains "budget replace" "${REPLACED_BUDGET_OUTPUT}" "Revoked budget"
assert_contains "budget replace" "${REPLACED_BUDGET_OUTPUT}" "Replacement budget"
assert_contains "budget replace" "${REPLACED_BUDGET_OUTPUT}" 'limit: $80.00'
REPLACEMENT_BUDGET_ID="$(awk '/budget_id:/ && seen { print $2; exit } /Replacement budget/ { seen=1 }' <<< "${REPLACED_BUDGET_OUTPUT}")"
[[ "${REPLACEMENT_BUDGET_ID}" == bgt_* ]] || fail "could not parse replacement budget id"
REVOKED_BUDGET_OUTPUT="$(hubu budget revoke --budget-id "${REPLACEMENT_BUDGET_ID}")"
assert_contains "budget revoke" "${REVOKED_BUDGET_OUTPUT}" "Budget revoked"
assert_contains "budget revoke" "${REVOKED_BUDGET_OUTPUT}" "status: revoked"
ACTIVE_BUDGET_LIST_OUTPUT="$(hubu budget list)"
assert_not_contains "active budget list" "${ACTIVE_BUDGET_LIST_OUTPUT}" "status: revoked"
ALL_BUDGET_LIST_OUTPUT="$(hubu budget list --all)"
assert_contains "all budget list" "${ALL_BUDGET_LIST_OUTPUT}" "status: revoked"

LEDGER_OUTPUT="$(hubu ledger list)"
assert_contains "ledger list" "${LEDGER_OUTPUT}" "via fiat_mock"
assert_contains "ledger list" "${LEDGER_OUTPUT}" "owner: Alice Example (${USER_ID})"
assert_contains "ledger list" "${LEDGER_OUTPUT}" 'debit      $20.00'
assert_contains "ledger list" "${LEDGER_OUTPUT}" 'credit     $20.00'

printf 'integration-core-flow passed\n'
