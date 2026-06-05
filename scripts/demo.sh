#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_ADDR="${HUBU_DEMO_ADDR:-127.0.0.1:8787}"
DEMO_URL="http://${DEMO_ADDR}"
DEMO_STEP_DELAY="${HUBU_DEMO_STEP_DELAY:-1.6}"
DEMO_READ_DELAY="${HUBU_DEMO_READ_DELAY:-2.4}"
SERVER_LOG="$(mktemp -t hubu-demo-server.XXXXXX.log)"
POLICY_FILE="$(mktemp -t hubu-demo-policy.XXXXXX.yaml)"
SERVER_PID=""

if [[ ! -t 1 || "${NO_COLOR:-}" != "" ]]; then
  BOLD=""
  DIM=""
  RESET=""
  RED=""
  GREEN=""
  YELLOW=""
  BLUE=""
  MAGENTA=""
  CYAN=""
else
  BOLD="$(printf '\033[1m')"
  DIM="$(printf '\033[2m')"
  RESET="$(printf '\033[0m')"
  RED="$(printf '\033[31m')"
  GREEN="$(printf '\033[32m')"
  YELLOW="$(printf '\033[33m')"
  BLUE="$(printf '\033[34m')"
  MAGENTA="$(printf '\033[35m')"
  CYAN="$(printf '\033[36m')"
fi

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  rm -f "${SERVER_LOG}" "${POLICY_FILE}"
}
trap cleanup EXIT

say() {
  printf '%b\n' "$*"
}

banner() {
  say ""
  say "${BOLD}${CYAN}██╗  ██╗${MAGENTA}██╗   ██╗${BLUE}██████╗ ${GREEN}██╗   ██╗${RESET}"
  say "${BOLD}${CYAN}██║  ██║${MAGENTA}██║   ██║${BLUE}██╔══██╗${GREEN}██║   ██║${RESET}"
  say "${BOLD}${CYAN}███████║${MAGENTA}██║   ██║${BLUE}██████╔╝${GREEN}██║   ██║${RESET}"
  say "${BOLD}${CYAN}██╔══██║${MAGENTA}██║   ██║${BLUE}██╔══██╗${GREEN}██║   ██║${RESET}"
  say "${BOLD}${CYAN}██║  ██║${MAGENTA}╚██████╔╝${BLUE}██████╔╝${GREEN}╚██████╔╝${RESET}"
  say "${BOLD}${CYAN}╚═╝  ╚═╝${MAGENTA} ╚═════╝ ${BLUE}╚═════╝ ${GREEN} ╚═════╝ ${RESET}"
  say "${DIM}local agent spend control plane demo${RESET}"
  say ""
}

step() {
  sleep "${DEMO_STEP_DELAY}"
  say "${BOLD}${BLUE}==>${RESET} ${BOLD}$*${RESET}"
}

note() {
  say "${DIM}    $*${RESET}"
}

run() {
  say "${YELLOW}\$ $*${RESET}"
  sleep 0.4
  "$@"
}

pause_for_reading() {
  sleep "${DEMO_READ_DELAY}"
}

show_cli_output() {
  local output="$1"
  local line

  while IFS= read -r line; do
    case "${line}" in
      "  decision: allow")
        say "  decision: ${BOLD}${GREEN}allow${RESET}"
        ;;
      "  decision: needs_approval")
        say "  decision: ${BOLD}${YELLOW}needs_approval${RESET}"
        ;;
      "  decision: deny")
        say "  decision: ${BOLD}${RED}deny${RESET}"
        ;;
      "  status: succeeded")
        say "  status: ${BOLD}${GREEN}succeeded${RESET}"
        ;;
      "  status: failed")
        say "  status: ${BOLD}${RED}failed${RESET}"
        ;;
      "  status: settled")
        say "  status: ${BOLD}${GREEN}settled${RESET}"
        ;;
      "  status: frozen")
        say "  status: ${BOLD}${CYAN}frozen${RESET}"
        ;;
      "  status: released")
        say "  status: ${BOLD}${YELLOW}released${RESET}"
        ;;
      *)
        say "${line}"
        ;;
    esac
  done <<< "${output}"
}

hubu() {
  "${ROOT_DIR}/target/debug/hubu" --url "${DEMO_URL}" "$@"
}

wait_for_server() {
  for _ in $(seq 1 50); do
    if [[ -n "${SERVER_PID}" ]] && ! kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
      say "${RED}Hubu server exited before becoming ready.${RESET}" >&2
      say "${DIM}Server log:${RESET}" >&2
      sed 's/^/  /' "${SERVER_LOG}" >&2 || true
      return 1
    fi
    if hubu health >/dev/null 2>&1 && kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done

  say "${RED}Hubu server did not become ready.${RESET}" >&2
  say "${DIM}Server log:${RESET}" >&2
  sed 's/^/  /' "${SERVER_LOG}" >&2 || true
  return 1
}

cd "${ROOT_DIR}"

banner

step "Build the demo binaries"
run cargo build --bin hubu-server --bin hubu
pause_for_reading

step "Start Hubu locally"
note "server: ${DEMO_URL}"
HUBU_LOG_FILE="${SERVER_LOG}" HUBU_LOG_STDERR=0 "${ROOT_DIR}/target/debug/hubu-server" "${DEMO_ADDR}" >"${SERVER_LOG}" 2>&1 &
SERVER_PID="$!"
wait_for_server
say "${GREEN}Hubu server is ready.${RESET}"
pause_for_reading

step "Register the human user"
INIT_OUTPUT="$(hubu register human --display-name "Alice Example" --email alice@example.com)"
say "${INIT_OUTPUT}"
USER_ID="$(printf '%s\n' "${INIT_OUTPUT}" | awk -F': ' '/^  user_id:/ { print $2; exit }')"
if [[ -z "${USER_ID}" ]]; then
  say "${RED}Could not parse user_id from init output.${RESET}" >&2
  exit 1
fi
note "captured public user_id=${USER_ID}"
pause_for_reading

step "Read agent registration guidance"
hubu registration guidance
pause_for_reading

step "Register an agent"
REGISTER_OUTPUT="$(hubu register agent)"
say "${REGISTER_OUTPUT}"
AGENT_ID="$(printf '%s\n' "${REGISTER_OUTPUT}" | awk -F': ' '/^  agent_id:/ { print $2; exit }')"
ACCOUNT_ID="$(printf '%s\n' "${REGISTER_OUTPUT}" | awk -F': ' '/^  account_id:/ { print $2; exit }')"
if [[ -z "${AGENT_ID}" ]]; then
  say "${RED}Could not parse agent_id from registration output.${RESET}" >&2
  exit 1
fi
if [[ -z "${ACCOUNT_ID}" ]]; then
  say "${RED}Could not parse account_id from registration output.${RESET}" >&2
  exit 1
fi
note "captured public agent_id=${AGENT_ID}"
note "captured public account_id=${ACCOUNT_ID}"
pause_for_reading

step "Generate and attach a spending policy"
rm -f "${POLICY_FILE}"
hubu init --policy "${POLICY_FILE}"
hubu policy add --agent-id "${AGENT_ID}" --path "${POLICY_FILE}"
pause_for_reading

step "List registered agents"
hubu agent list
pause_for_reading

step "Create a recurring human budget"
BUDGET_OUTPUT="$(hubu budget create-recurring \
  --amount 75 \
  --recurrence monthly \
  --period-count 2)"
say "${BUDGET_OUTPUT}"
pause_for_reading

step "Submit an allowed spend request"
ALLOW_OUTPUT="$(hubu spend \
  --account-id "${ACCOUNT_ID}" \
  --amount 20 \
  --reason "Purchase API credits")"
show_cli_output "${ALLOW_OUTPUT}"
pause_for_reading

step "Authorize a \$5 logo-generation budget"
LOGO_BUDGET_OUTPUT="$(hubu budget create \
  --agent-id "${AGENT_ID}" \
  --amount 5)"
show_cli_output "${LOGO_BUDGET_OUTPUT}"
LOGO_BUDGET_ID="$(printf '%s\n' "${LOGO_BUDGET_OUTPUT}" | awk -F': ' '/^  budget_id:/ { split($2, parts, "  "); print parts[1]; exit }')"
if [[ -z "${LOGO_BUDGET_ID}" ]]; then
  say "${RED}Could not parse budget_id from logo budget output.${RESET}" >&2
  exit 1
fi
note "captured logo budget_id=${LOGO_BUDGET_ID}"
LOGO_AUTH_OUTPUT="$(hubu spend authorize \
  --account-id "${ACCOUNT_ID}" \
  --amount 5 \
  --reason "Generate Project Hubu logo" \
  --merchant hubu-model-proxy \
  --budget-id "${LOGO_BUDGET_ID}")"
show_cli_output "${LOGO_AUTH_OUTPUT}"
LOGO_AUTH_TOKEN_ID="$(printf '%s\n' "${LOGO_AUTH_OUTPUT}" | awk -F': ' '/^  auth_token_id:/ { print $2; exit }')"
if [[ -z "${LOGO_AUTH_TOKEN_ID}" ]]; then
  say "${RED}Could not parse auth_token_id from logo spend authorization output.${RESET}" >&2
  exit 1
fi
note "captured spend_auth_token_id=${LOGO_AUTH_TOKEN_ID}"
pause_for_reading

step "Generate a Project Hubu logo through the Hubu model proxy"
LOGO_OUTPUT="$(hubu model-call image \
  --spend-auth-token-id "${LOGO_AUTH_TOKEN_ID}" \
  --prompt "Create a crisp logo for Project Hubu")"
show_cli_output "${LOGO_OUTPUT}"
LOGO_OUTPUT_REF="$(printf '%s\n' "${LOGO_OUTPUT}" | awk -F': ' '/^  output_ref:/ { print $2; exit }')"
LOGO_OUTPUT_PATH="${LOGO_OUTPUT_REF#file://}"
if [[ -z "${LOGO_OUTPUT_REF}" || "${LOGO_OUTPUT_REF}" == "${LOGO_OUTPUT_PATH}" || ! -f "${LOGO_OUTPUT_PATH}" ]]; then
  say "${RED}Could not verify logo artifact from output_ref=${LOGO_OUTPUT_REF}.${RESET}" >&2
  exit 1
fi
note "verified logo artifact=${LOGO_OUTPUT_PATH}"
pause_for_reading

step "Submit an allowed spend whose mock payment fails"
FAILED_PAYMENT_OUTPUT="$(hubu spend \
  --account-id "${ACCOUNT_ID}" \
  --amount 15 \
  --reason "Test failed merchant payout" \
  --merchant fail)"
show_cli_output "${FAILED_PAYMENT_OUTPUT}"
pause_for_reading

step "Submit an over-limit spend request"
NEEDS_APPROVAL_OUTPUT="$(hubu spend \
  --account-id "${ACCOUNT_ID}" \
  --amount 120 \
  --reason "Large API credit purchase")"
show_cli_output "${NEEDS_APPROVAL_OUTPUT}"
pause_for_reading

step "Submit a denied spend request"
DENY_OUTPUT="$(hubu spend \
  --account-id "${ACCOUNT_ID}" \
  --amount 20 \
  --reason "Attempt blocked merchant purchase" \
  --merchant blocked-merchant)"
show_cli_output "${DENY_OUTPUT}"
pause_for_reading

step "Inspect the budget balance"
hubu budget list
pause_for_reading

step "Inspect the ledger"
hubu ledger list
pause_for_reading

say ""
say "${BOLD}${GREEN}Demo complete.${RESET} ${DIM}Allowed spend settled budget into consumed balance; logo generation used a scoped Hubu auth token; failed payment released frozen budget; over-limit and denied spends did not execute payment.${RESET}"
