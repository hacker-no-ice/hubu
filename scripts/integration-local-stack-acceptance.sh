#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(mktemp -d)"
profile="${workspace}/profile"
export HUBU_HOME="${workspace}/hubu-home"
stack_started=0

cleanup() {
  local status=$?
  if [[ "${stack_started}" == "1" && -x "${root_dir}/target/debug/hubu" ]]; then
    "${root_dir}/target/debug/hubu" stack stop --profile "${profile}" >/dev/null 2>&1 || true
  fi
  if [[ "${HUBU_ACCEPTANCE_PRESERVE:-0}" == "1" ]]; then
    echo "local-stack acceptance preserved workspace: ${workspace}" >&2
    return "${status}"
  fi
  rm -rf "${workspace}"
  return "${status}"
}
trap cleanup EXIT
trap 'echo "local-stack acceptance failed at line ${LINENO}" >&2' ERR

fail() {
  echo "local-stack acceptance: $*" >&2
  exit 1
}

quote() {
  jq -Rn --arg value "$1" '$value'
}

field() {
  awk -v name="$2" '$1 == name ":" { print $2; exit }' <<<"$1"
}

file_mode() {
  stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"
}

if ! command -v temporal >/dev/null 2>&1; then
  echo "local-stack acceptance requires the Temporal CLI on PATH" >&2
  exit 2
fi

for tool in jq curl python3 sqlite3; do
  command -v "${tool}" >/dev/null 2>&1 || {
    echo "local-stack acceptance requires ${tool} on PATH" >&2
    exit 2
  }
done

cd "${root_dir}"
acceptance_version="0.2.0"
acceptance_commit="9393939393939393939393939393939393939393"
HUBU_PRODUCT_VERSION="${acceptance_version}" \
HUBU_SOURCE_COMMIT="${acceptance_commit}" \
  cargo build --locked -p hubu-cli --bin hubu --features local-fixture-canary
HUBU_PRODUCT_VERSION="${acceptance_version}" \
HUBU_SOURCE_COMMIT="${acceptance_commit}" \
  cargo build --locked --bin hubu-server --bin hubu-unified-mcp
HUBU_PRODUCT_VERSION="${acceptance_version}" \
HUBU_SOURCE_COMMIT="${acceptance_commit}" \
GONGBU_PRODUCT_VERSION="${acceptance_version}" \
GONGBU_SOURCE_COMMIT="${acceptance_commit}" \
  cargo build --locked -p gongbu-api --bin gongbu-server --features local-fixture-canary

hubu_bin="${root_dir}/target/debug/hubu"
real_hubu_server_bin="${root_dir}/target/debug/hubu-server"
gongbu_server_bin="${root_dir}/target/debug/gongbu-server"
mcp_bin="${root_dir}/target/debug/hubu-unified-mcp"
temporal_bin="$(command -v temporal)"
temporal_version="$(temporal --version | awk 'NR == 1 { print $3 }')"
[[ -n "${temporal_version}" ]] || fail "could not parse Temporal CLI version"

read -r hubu_port gongbu_port temporal_port temporal_ui_port < <(
  python3 -c 'import socket; sockets=[socket.socket() for _ in range(4)]; [s.bind(("127.0.0.1",0)) for s in sockets]; print(*(s.getsockname()[1] for s in sockets)); [s.close() for s in sockets]'
)

hubu_endpoint="http://127.0.0.1:${hubu_port}"
gongbu_endpoint="http://127.0.0.1:${gongbu_port}"
provider_secret="${workspace}/provider"
printf '%s\n' 'hub-105-fixture-provider' >"${provider_secret}"
chmod 600 "${provider_secret}"

hubu_serve_trace="${workspace}/hubu-serve.trace"
hubu_server_bin="${workspace}/hubu-server"
cat >"${hubu_server_bin}" <<EOF
#!/bin/sh
if [ "\$1" = "serve" ] && [ "\$2" = "--config" ]; then
  printf '%s|%s\n' "\$\$" "\$3" >>$(quote "${hubu_serve_trace}")
fi
exec $(quote "${real_hubu_server_bin}") "\$@"
EOF
chmod 700 "${hubu_server_bin}"

init_output="$("${hubu_bin}" stack init --mode local-stack --profile "${profile}")"
"${hubu_bin}" stack select --profile "${profile}" >/dev/null
profile_canonical="$(cd "${profile}" && pwd -P)"
managed_credential_root="${profile_canonical}/state/credentials"
grep -F 'input needed:' <<<"${init_output}" >/dev/null
for name in README.md stack.toml credentials.toml providers.toml; do
  [[ -f "${profile}/${name}" ]] || fail "init omitted ${name}"
done
[[ ! -e "${profile}/runtime/launcher-state.json" ]] || fail "init started or recorded a service"
[[ -f "${managed_credential_root}/.gitignore" ]] || fail "init omitted the managed credential ignore guard"
if grep -Eq '^\[files\]|^\[opaque\.gongbu_(hubu|caller)\]|state/credentials' "${profile}/credentials.toml"; then
  fail "init exposed managed credential implementation details in operator source"
fi
[[ -z "$(find "${managed_credential_root}" -type f ! -name .gitignore -print -quit)" ]] || fail "init created credential material"

doctor_output="$("${hubu_bin}" stack doctor --profile "${profile}" 2>&1 || true)"
if grep -F 'stack.toml:hubu.ownership' <<<"${doctor_output}" >/dev/null; then
  fail "outcome-oriented init left Hubu ownership unresolved"
fi
if grep -F 'credentials.toml:files.hubu_auth' <<<"${doctor_output}" >/dev/null; then
  fail "managed Hubu capability paths were presented as user input"
fi
grep -F 'providers.toml:targets' <<<"${doctor_output}" >/dev/null

cat >"${profile}/credentials.toml" <<EOF
schema_version = 1

[opaque.fixture_provider]
service = "hubu.local-fixture"
account = "provider"
EOF

cat >"${profile}/providers.toml" <<'EOF'
schema_version = 1
mode = "live"
EOF
incomplete_live="$("${hubu_bin}" stack doctor --profile "${profile}" 2>&1 || true)"
grep -F 'providers.toml:maximum_spend_minor' <<<"${incomplete_live}" >/dev/null
grep -F 'providers.toml:live_spend_acknowledgement' <<<"${incomplete_live}" >/dev/null
grep -F 'providers.toml:targets' <<<"${incomplete_live}" >/dev/null

cat >"${profile}/providers.toml" <<'EOF'
schema_version = 1
mode = "live"
catalog_version = "hub-114-fixture-v2"
maximum_spend_minor = 2
live_spend_acknowledgement = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"

[[targets]]
provider_config_version = "fixture-v1"
workload_type = "default"
provider = "example"
adapter = "fixture"
model = "image-v1"
credential = "fixture_provider"
active = true
execution_enabled = true
settings = { type = "fixture" }

[[pricing_rules]]
rule_id = "fixture-image-1k"
provider = "example"
model = "image-v1"
currency = "USD"
selector = { image_size = "1k" }
components = [{ unit = "image", rate_numerator_minor = 1, rate_denominator = 1 }]

[[pricing_rules]]
rule_id = "fixture-image-2k"
provider = "example"
model = "image-v1"
currency = "USD"
selector = { image_size = "2k" }
components = [{ unit = "image", rate_numerator_minor = 2, rate_denominator = 1 }]

[[pricing_rules]]
rule_id = "fixture-image-4k"
provider = "example"
model = "image-v1"
currency = "USD"
selector = { image_size = "4k" }
components = [{ unit = "image", rate_numerator_minor = 3, rate_denominator = 1 }]
EOF

cat >"${profile}/stack.toml" <<EOF
schema_version = 1
allow_development_builds = true

[binaries]
hubu = $(quote "${hubu_bin}")
hubu_server = $(quote "${hubu_server_bin}")
gongbu_server = $(quote "${gongbu_server_bin}")
hubu_unified_mcp = $(quote "${mcp_bin}")

[hubu]
ownership = "managed"
endpoint = $(quote "${hubu_endpoint}")
listen = "127.0.0.1:${hubu_port}"
database_path = $(quote "${workspace}/hubu.sqlite3")
log_file = $(quote "${workspace}/hubu.log")

[gongbu]
ownership = "managed"
endpoint = $(quote "${gongbu_endpoint}")
listen = "127.0.0.1:${gongbu_port}"
database_path = $(quote "${workspace}/gongbu.sqlite3")
artifact_root = $(quote "${workspace}/artifacts")
log_file = $(quote "${workspace}/gongbu.log")

[temporal]
mode = "managed_local"
binary_path = $(quote "${temporal_bin}")
expected_cli_version = $(quote "${temporal_version}")
data_path = $(quote "${workspace}/temporal")
rpc_port = ${temporal_port}
ui_port = ${temporal_ui_port}
namespace = "default"
task_queue = "gongbu-executions"
ui_url = "http://127.0.0.1:${temporal_ui_port}"

[runtime]
hubu_startup_timeout_ms = 15000
temporal_startup_timeout_ms = 30000
dependency_check_interval_ms = 250
worker_drain_timeout_ms = 15000
EOF

if grep -Eq '^\[identity(\.|])' "${profile}/stack.toml" "${profile}/providers.toml"; then
  fail "principal-neutral infrastructure/provider configuration contains stack identity"
fi

export GONGBU_LOCAL_FIXTURE_CANARY=1
export GONGBU_LOCAL_FIXTURE_SECRET_DIR="${workspace}"
export HUBU_LOCAL_FIXTURE_CANARY=1
lifecycle_log="${workspace}/lifecycle.log"
stack_lifecycle() {
  printf '%s\n' "$1" >>"${lifecycle_log}"
  "${hubu_bin}" stack "$@" --profile "${profile}"
}

managed_doctor="$("${hubu_bin}" stack doctor --profile "${profile}")"
if ! grep -F 'managed_credential_pending' <<<"${managed_doctor}" >/dev/null; then
  printf '%s\n' "${managed_doctor}" >&2
  fail "doctor did not classify unprovisioned managed credentials"
fi
[[ -z "$(find "${managed_credential_root}" -type f ! -name .gitignore -print -quit)" ]] || fail "doctor provisioned managed credentials"
if grep -Eq '^\[files\]|^\[opaque\.gongbu_(hubu|caller)\]|state/credentials' "${profile}/credentials.toml"; then
  fail "managed credential implementation details leaked into operator source"
fi
render_output="$(stack_lifecycle render)"
[[ -z "$(find "${managed_credential_root}" -type f ! -name .gitignore -print -quit)" ]] || fail "render provisioned managed credentials"
generation="$(awk '/rendered generation:/ { print $3; exit } /validated staged generation:/ { print $4; exit }' <<<"${render_output}")"
if [[ -z "${generation}" ]]; then
  echo "${render_output}" >&2
  fail "managed fixture generation was not rendered"
fi
grep -F 'active manifest:' <<<"${render_output}" >/dev/null || fail "first managed generation did not activate automatically"
if ! start_output="$(stack_lifecycle start 2>&1)"; then
  echo "${start_output}" >&2
  fail "managed Hubu/Gongbu stack did not start"
fi
stack_started=1
jq -e '.classification == "running_ready"' < <("${hubu_bin}" stack status --json --profile "${profile}") >/dev/null

active_manifest="${profile}/generated/active-manifest.json"
active_generation="$(jq -r '.generation' "${active_manifest}")"
client_handoff="${profile}/generated/${active_generation}/client-handoff.json"
hubu_auth="$(jq -r '.hubu_token_file' "${client_handoff}")"
hubu_approval="$(jq -r '.approval_token_file' "${client_handoff}")"
hubu_reconciliation="$(jq -r '.reconciliation_token_file' "${client_handoff}")"
gongbu_caller="$(jq -r '.gongbu_token_file' "${client_handoff}")"
gongbu_hubu="${managed_credential_root}/gongbu/hubu-executor"
codex_config="${workspace}/codex-config.toml"
"${hubu_bin}" init codex \
  --stack-profile "${profile}" \
  --config "${codex_config}" >/dev/null
grep -F "HUBU_APPROVAL_TOKEN_FILE = \"${hubu_approval}\"" "${codex_config}" >/dev/null || fail "Codex config omitted the approval capability file"
grep -F 'HUBU_MCP_TRUST_SPEND_APPROVAL = "1"' "${codex_config}" >/dev/null || fail "Codex config omitted the narrow spend-approval gate"
grep -A1 -F '[mcp_servers.hubu.tools.hubu_resolve_spend_approval]' "${codex_config}" | grep -F 'approval_mode = "prompt"' >/dev/null || fail "Codex config did not prompt for spend approval resolution"
for directory in "${managed_credential_root}" "${managed_credential_root}/hubu" "${managed_credential_root}/gongbu"; do
  [[ "$(file_mode "${directory}")" == "700" ]] || fail "managed credential directory permissions are not private"
done
for credential in "${hubu_auth}" "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}" "${gongbu_hubu}"; do
  [[ "${credential}" == "${managed_credential_root}/"* ]] || fail "managed credential escaped the private profile state"
  [[ -s "${credential}" ]] || fail "managed credential was not provisioned"
  [[ "$(file_mode "${credential}")" == "600" ]] || fail "managed credential permissions are not private"
done
cmp "${hubu_auth}" "${gongbu_hubu}" >/dev/null || fail "Gongbu's internal Hubu handoff does not match the verified Hubu capability"
credential_digests_before="$(shasum -a 256 "${hubu_auth}" "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}")"
[[ "$(cut -d' ' -f1 <<<"${credential_digests_before}" | sort -u | wc -l | tr -d ' ')" == "4" ]] || fail "managed credential classes reused material"
serve_record="$(cat "${hubu_serve_trace}")"
[[ "$(wc -l <<<"${serve_record}" | tr -d ' ')" == "1" ]] || fail "first startup launched more than one Hubu server"
serve_pid="${serve_record%%|*}"
serve_config="${serve_record#*|}"
launcher_pid="$(jq -r '.processes["hubu-server"].pid' "${profile}/runtime/launcher-state.json")"
[[ "${serve_pid}" == "${launcher_pid}" ]] || fail "first Hubu process was not launcher-owned"
[[ "${serve_config}" == "${profile}/generated/${active_generation}/hubu-launch.json" ]] || fail "first Hubu process did not use the active managed config"
jq -e '
  .auth_token_file_generated == true and
  .approval_token_file_generated == true and
  .reconciliation_token_file_generated == true
' "${serve_config}" >/dev/null || fail "derived Hubu credential ownership was not preserved in the active config"
running_doctor="$("${hubu_bin}" stack doctor --json --profile "${profile}")"
jq -e '[.checks[] | select(.code == "credential_file_available")] | length == 4 and all(has("field") | not)' <<<"${running_doctor}" >/dev/null || fail "running doctor exposed derived credential paths as source fields"
if jq -e '.checks[] | select(.code == "managed_credential_pending")' <<<"${running_doctor}" >/dev/null; then
  fail "running doctor still classified a managed credential as pending"
fi

curl --fail --silent "${hubu_endpoint}/health" >/dev/null
curl --fail --silent "${gongbu_endpoint}/readyz" >/dev/null
lifecycle_count_before_registration="$(wc -l <"${lifecycle_log}" | tr -d ' ')"

hubu() {
  env HUBU_URL="http://127.0.0.1:1" \
    HUBU_AUTH_TOKEN="stale-auth-token" \
    HUBU_AUTH_TOKEN_FILE="${workspace}/stale-auth-token" \
    HUBU_APPROVAL_TOKEN="stale-approval-token" \
    HUBU_APPROVAL_TOKEN_FILE="${workspace}/stale-approval-token" \
    HUBU_RECONCILIATION_TOKEN="stale-reconciliation-token" \
    HUBU_RECONCILIATION_TOKEN_FILE="${workspace}/stale-reconciliation-token" \
    "${hubu_bin}" "$@"
}

human_output="$(hubu register human --username hub-140-owner --display-name 'HUB-140 Owner')"
[[ -n "$(field "${human_output}" user_id)" ]] || fail "could not register the post-start owner"
agent_a_output="$(hubu register agent --name hub-140-agent-a --version v1)"
agent_b_output="$(hubu register agent --name hub-140-agent-b --version v1)"
approval_agent_output="$(hubu register agent --name hub-164-approval-agent --version v1)"
agent_a_id="$(field "${agent_a_output}" agent_id)"
agent_a_account_id="$(field "${agent_a_output}" account_id)"
agent_b_id="$(field "${agent_b_output}" agent_id)"
agent_b_account_id="$(field "${agent_b_output}" account_id)"
approval_agent_id="$(field "${approval_agent_output}" agent_id)"
approval_agent_account_id="$(field "${approval_agent_output}" account_id)"
[[ -n "${agent_a_id}" && -n "${agent_a_account_id}" ]] || fail "could not register Agent A"
[[ -n "${agent_b_id}" && -n "${agent_b_account_id}" ]] || fail "could not register Agent B"
[[ -n "${approval_agent_id}" && -n "${approval_agent_account_id}" ]] || fail "could not register the approval agent"
[[ "${agent_a_id}" != "${agent_b_id}" ]] || fail "two registrations resolved to one agent"
hubu budget create --agent-id "${agent_a_id}" --amount 1 >/dev/null
hubu budget create --agent-id "${agent_b_id}" --amount 1 >/dev/null
hubu budget create --agent-id "${approval_agent_id}" --amount 1 >/dev/null

policy="${workspace}/fixture-policy.yaml"
cat >"${policy}" <<'EOF'
id: hub_140_fixture
version: v1
default_effect: deny
rules:
  - id: allow_local_fixture
    effect: allow
    reason: deterministic local-stack acceptance fixture
    when:
      op: eq
      field: provider
      value:
        string: provider:local:fixture
  - id: review_local_fixture
    effect: needs_approval
    reason: deterministic local-stack human approval fixture
    when:
      op: eq
      field: amount
      value:
        money_cents: 2
EOF
hubu policy add --path "${policy}" >/dev/null

python3 - \
  "${mcp_bin}" \
  "${hubu_bin}" \
  "${hubu_endpoint}" \
  "${gongbu_endpoint}" \
  "${hubu_auth}" \
  "${hubu_approval}" \
  "${hubu_reconciliation}" \
  "${gongbu_caller}" \
  "${workspace}/approval-operations.sqlite3" \
  "${workspace}/gongbu.sqlite3" \
  "${approval_agent_account_id}" \
  "${workspace}/approval-mcp.stderr" <<'PY'
import json
import os
import sqlite3
import subprocess
import sys
import time

(
    mcp_bin,
    hubu_bin,
    hubu_endpoint,
    gongbu_endpoint,
    hubu_auth,
    hubu_approval,
    hubu_reconciliation,
    gongbu_caller,
    operation_state_path,
    gongbu_database,
    account_id,
    stderr_path,
) = sys.argv[1:]


def fail(message):
    raise AssertionError(message)


def execution_count():
    with sqlite3.connect(gongbu_database) as connection:
        return connection.execute("SELECT COUNT(*) FROM executions").fetchone()[0]


environment = os.environ.copy()
for name in (
    "HUBU_UNIFIED_HUBU_BEARER_TOKEN",
    "HUBU_UNIFIED_GONGBU_BEARER_TOKEN",
    "HUBU_APPROVAL_TOKEN",
    "HUBU_RECONCILIATION_TOKEN",
):
    environment.pop(name, None)
environment.update(
    {
        "HUBU_UNIFIED_HUBU_ENDPOINT": hubu_endpoint,
        "HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE": hubu_auth,
        "HUBU_UNIFIED_GONGBU_ENDPOINT": gongbu_endpoint,
        "HUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE": gongbu_caller,
        "HUBU_APPROVAL_TOKEN_FILE": hubu_approval,
        "HUBU_RECONCILIATION_TOKEN_FILE": hubu_reconciliation,
        "HUBU_UNIFIED_OPERATION_STATE_PATH": operation_state_path,
        "HUBU_MCP_TRUST_SPEND_APPROVAL": "1",
        "HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS": "50",
        "HUBU_UNIFIED_OPERATION_TICK_MS": "10",
        "HUBU_UNIFIED_GOVERNED_EXECUTION_WAIT_MS": "2000",
    }
)

stderr_file = open(stderr_path, "w", encoding="utf-8")
process = None
next_id = 1


def request(method, params=None):
    global next_id
    request_id = next_id
    next_id += 1
    message = {"jsonrpc": "2.0", "id": request_id, "method": method}
    if params is not None:
        message["params"] = params
    process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    process.stdin.flush()
    while True:
        line = process.stdout.readline()
        if not line:
            fail(f"unified MCP exited before replying to {method}")
        response = json.loads(line)
        if response.get("id") != request_id:
            if response.get("method") == "notifications/tools/list_changed":
                continue
            fail(f"unexpected MCP message while waiting for {method}: {response}")
        if "error" in response:
            fail(f"{method} failed: {response['error']}")
        return response["result"]


def notify(method):
    process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
    process.stdin.flush()


def start_process():
    global process, next_id
    if process is not None:
        fail("unified MCP was already running")
    process = subprocess.Popen(
        [mcp_bin],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=stderr_file,
        text=True,
        encoding="utf-8",
        env=environment,
    )
    next_id = 1
    request("initialize")
    notify("notifications/initialized")
    request("ping")


def stop_process():
    global process
    if process is None:
        return
    running = process
    if running.stdin and not running.stdin.closed:
        running.stdin.close()
    try:
        running.wait(timeout=10)
    except subprocess.TimeoutExpired:
        running.terminate()
        running.wait(timeout=10)
    process = None
    if running.returncode != 0:
        stderr_file.flush()
        with open(stderr_path, "r", encoding="utf-8") as captured:
            fail(f"unified MCP exited with {running.returncode}: {captured.read()}")


def call(name, arguments, call_id=None):
    params = {"name": name, "arguments": arguments}
    if call_id is not None:
        params["_meta"] = {"callId": call_id}
    result = request("tools/call", params)
    if result.get("isError"):
        fail(f"{name} returned an application error: {result}")
    structured = result.get("structuredContent")
    if not isinstance(structured, dict):
        fail(f"{name} omitted structuredContent: {result}")
    return structured


def governed_arguments(label):
    return {
        "authorization": {
            "account_id": account_id,
            "amount_cents": 2,
            "reason": f"local stack approval acceptance {label}",
            "execution_scope": {
                "schema_version": 1,
                "provider": "provider:local:fixture",
                "executor": "executor:gongbu:image",
                "capability": "capability:image:generate",
                "billing_merchant": "merchant:local",
            },
            "lease_profile": "default",
        },
        "execution": {
            "schema_version": 2,
            "input": {
                "prompt": f"approval acceptance canary {label}",
                "image_count": 1,
                "image_size": "2k",
            },
            "input_schema_version": 1,
            "target_id": "gongbu:target:v1:05934e1fe9c59160d3c148fdc465ea37fb3ec3110ccd8456c10ed467cb56c9d9",
        },
    }


def pending_operation(label):
    before = execution_count()
    pending = call(
        "hubu_submit_governed_execution",
        governed_arguments(label),
        f"local-stack-approval-{label}",
    )
    if pending.get("outcome") != "approval_required":
        fail(f"{label} did not require approval: {pending}")
    if pending.get("state") != "approval_required":
        fail(f"{label} did not persist approval_required: {pending}")
    approval = pending.get("authorization", {}).get("approval", {})
    approval_request_id = approval.get("approval_request_id")
    operation_handle = pending.get("operation_handle")
    if not approval_request_id or not operation_handle:
        fail(f"{label} omitted approval identity or public handle: {pending}")
    if execution_count() != before:
        fail(f"{label} contacted Gongbu before human approval")
    review = call(
        "hubu_get_spend_approval",
        {"approval_request_id": approval_request_id},
    )
    if review.get("status") != "pending":
        fail(f"{label} approval review was not pending: {review}")
    if "operation_key" in json.dumps(review):
        fail(f"{label} approval review exposed a private operation key")
    return before, approval_request_id, operation_handle


try:
    start_process()

    expected_tools = {
        "hubu_budget_history",
        "gongbu_get_provider_catalog",
        "gongbu_list_execution_targets",
        "gongbu_get_redaction_attestation",
        "hubu_get_spend_approval",
        "hubu_resolve_spend_approval",
        "hubu_resume_operation",
        "hubu_update_budget",
    }
    tools = request("tools/list").get("tools", [])
    tool_names = {tool.get("name") for tool in tools}
    if (
        len(tool_names) != 42
        or not expected_tools.issubset(tool_names)
        or "hubu_replace_budget" in tool_names
    ):
        fail(f"unexpected unified MCP tool catalog: {len(tool_names)} tools")

    denied_before, denied_approval_id, denied_handle = pending_operation("deny")
    denied = call(
        "hubu_resolve_spend_approval",
        {"approval_request_id": denied_approval_id, "decision": "deny"},
    )
    if denied.get("decision") != "deny":
        fail(f"unified MCP denial was not recorded: {denied}")
    denied_status = call(
        "hubu_operation_status", {"operation_handle": denied_handle}
    )
    if denied_status.get("state") != "failed" or not denied_status.get("terminal"):
        fail(f"denied operation was not terminal: {denied_status}")
    if denied_status.get("result", {}).get("code") != "approval_denied":
        fail(f"denied operation had the wrong result code: {denied_status}")
    if execution_count() != denied_before:
        fail("denial started Gongbu or provider work")

    approved_before, approved_approval_id, approved_handle = pending_operation("approve")
    cli_environment = os.environ.copy()
    for name in ("HUBU_AUTH_TOKEN", "HUBU_APPROVAL_TOKEN"):
        cli_environment.pop(name, None)
    cli_environment.update(
        {
            "HUBU_URL": hubu_endpoint,
            "HUBU_AUTH_TOKEN_FILE": hubu_auth,
            "HUBU_APPROVAL_TOKEN_FILE": hubu_approval,
        }
    )
    approved = subprocess.run(
        [
            hubu_bin,
            "spend",
            "approval",
            "approve",
            "--approval-request-id",
            approved_approval_id,
        ],
        env=cli_environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if approved.returncode != 0:
        fail(f"CLI approval failed: {approved.stderr}{approved.stdout}")

    stop_process()
    start_process()
    synchronized = call(
        "hubu_operation_status", {"operation_handle": approved_handle}
    )
    if synchronized.get("state") != "resume_required":
        fail(f"external approval did not synchronize: {synchronized}")
    if synchronized.get("result", {}).get("code") != "approval_resolved_resume_required":
        fail(f"external approval had the wrong resume result code: {synchronized}")
    if execution_count() != approved_before:
        fail("approval or status synchronization started provider work")

    original_redelivery = call(
        "hubu_submit_governed_execution",
        governed_arguments("approve"),
        "local-stack-approval-approve",
    )
    if original_redelivery.get("operation_handle") != approved_handle:
        fail(f"original redelivery changed the public handle: {original_redelivery}")
    if original_redelivery.get("state") != "resume_required":
        fail(f"original redelivery bypassed handle resume: {original_redelivery}")
    if original_redelivery.get("outcome") != "resume_required":
        fail(f"original redelivery returned stale approval outcome: {original_redelivery}")
    if (
        original_redelivery.get("authorization", {})
        .get("retry_guidance", {})
        .get("action")
        != "resume_operation"
    ):
        fail(f"original redelivery returned stale retry guidance: {original_redelivery}")
    if "hubu_resume_operation" not in original_redelivery.get("guidance", ""):
        fail(f"original redelivery omitted handle-resume guidance: {original_redelivery}")
    if execution_count() != approved_before:
        fail("original approved-call redelivery started Gongbu or provider work")

    resumed = call("hubu_resume_operation", {"operation_handle": approved_handle})
    if resumed.get("state") not in {
        "accepted",
        "queued",
        "dispatching",
        "reconciling",
        "succeeded",
    }:
        fail(f"approved operation did not resume: {resumed}")
    terminal = resumed
    for _ in range(300):
        if terminal.get("state") == "succeeded":
            break
        time.sleep(0.05)
        terminal = call(
            "hubu_operation_status", {"operation_handle": approved_handle}
        )
    if terminal.get("state") != "succeeded":
        fail(f"resumed operation did not succeed: {terminal}")
    if execution_count() != approved_before + 1:
        fail("public-handle resume did not create exactly one Gongbu execution")

    replay = call("hubu_resume_operation", {"operation_handle": approved_handle})
    if replay.get("state") != "succeeded":
        fail(f"idempotent resume did not return terminal state: {replay}")
    if execution_count() != approved_before + 1:
        fail("repeated public-handle resume created another Gongbu execution")
finally:
    stop_process()
    stderr_file.close()
PY

authorize_agent() {
  local label="$1"
  local account_id="$2"
  local authorization
  authorization="$(hubu spend authorize \
    --operation-key "hub-140-${label}" \
    --account-id "${account_id}" \
    --amount 0.01 \
    --currency USD \
    --reason "local stack acceptance ${label}" \
    --provider provider:local:fixture \
    --executor executor:gongbu:image \
    --capability capability:image:generate \
    --billing-merchant merchant:local \
    --lease-profile default)"
  field "${authorization}" auth_token_id
}

execution_body() {
  jq -n --arg token "$1" --arg label "$2" \
    '{schema_version:2,spend_auth_token_id:$token,input:{prompt:("acceptance canary " + $label),image_count:1,image_size:"1k"},input_schema_version:1,target_id:"gongbu:target:v1:05934e1fe9c59160d3c148fdc465ea37fb3ec3110ccd8456c10ed467cb56c9d9"}'
}

submit_execution() {
  local token="$1"
  local label="$2"
  local response status body
  response="$(curl --silent --show-error --write-out '\n%{http_code}' \
    -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
    -H 'Content-Type: application/json' \
    -d "$(execution_body "${token}" "${label}")" \
    "${gongbu_endpoint}/v2/executions")"
  status="$(tail -n 1 <<<"${response}")"
  body="$(sed '$d' <<<"${response}")"
  if [[ ! "${status}" =~ ^2 ]]; then
    echo "${body}" >&2
    fail "Gongbu rejected ${label} with HTTP ${status}"
  fi
  jq -r '.execution_id' <<<"${body}"
}

wait_for_execution() {
  local execution_id="$1"
  local terminal=''
  for _ in {1..150}; do
    terminal="$(curl --fail --silent \
      -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
      "${gongbu_endpoint}/v1/executions/${execution_id}")"
    if [[ "$(jq -r '.status' <<<"${terminal}")" == "succeeded" ]]; then
      return 0
    fi
    sleep 0.1
  done
  echo "${terminal}" >&2
  tail -n 40 "${workspace}/gongbu.log" >&2 || true
  tail -n 40 "${workspace}/hubu.log" >&2 || true
  fail "execution ${execution_id} did not succeed"
}

retrieve_artifact() {
  local execution_id="$1"
  local destination="$2"
  local artifacts artifact_id artifact_sha
  artifacts="$(curl --fail --silent \
    -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
    "${gongbu_endpoint}/v1/executions/${execution_id}/artifacts")"
  artifact_id="$(jq -r '.artifacts[0].artifact_id' <<<"${artifacts}")"
  artifact_sha="$(jq -r '.artifacts[0].sha256' <<<"${artifacts}")"
  [[ "${artifact_id}" != "null" && -n "${artifact_id}" ]] || fail "execution ${execution_id} has no artifact"
  curl --fail --silent \
    -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
    "${gongbu_endpoint}/v1/artifacts/${artifact_id}" \
    -o "${destination}"
  [[ "$(shasum -a 256 "${destination}" | awk '{ print $1 }')" == "${artifact_sha}" ]] || fail "retrieved artifact digest changed"
  printf '%s|%s\n' "${artifact_id}" "${artifact_sha}"
}

agent_a_token="$(authorize_agent agent-a "${agent_a_account_id}")"
agent_b_token="$(authorize_agent agent-b "${agent_b_account_id}")"
[[ -n "${agent_a_token}" && -n "${agent_b_token}" ]] || fail "Hubu did not issue both authorizations"
[[ "${agent_a_token}" != "${agent_b_token}" ]] || fail "two agents received one authorization token"

invalid_caller_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  -H 'Authorization: Bearer invalid-installation-caller' \
  "${gongbu_endpoint}/v1/executions/known-only")"
[[ "${invalid_caller_status}" == "401" ]] || fail "Gongbu accepted an invalid caller capability"
caller_identity_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
  -H 'Content-Type: application/json' \
  -d "$(execution_body "${agent_a_token}" agent-a | jq --arg account "${agent_a_account_id}" '. + {account_id:$account}')" \
  "${gongbu_endpoint}/v2/executions")"
[[ "${caller_identity_status}" == "400" ]] || fail "Gongbu accepted caller-supplied execution identity"

agent_a_execution_id="$(submit_execution "${agent_a_token}" agent-a)"
agent_b_execution_id="$(submit_execution "${agent_b_token}" agent-b)"
[[ -n "${agent_a_execution_id}" && -n "${agent_b_execution_id}" ]] || fail "Gongbu omitted an execution ID"
[[ "${agent_a_execution_id}" != "${agent_b_execution_id}" ]] || fail "two agents converged on one execution"
wait_for_execution "${agent_a_execution_id}"
wait_for_execution "${agent_b_execution_id}"

for execution_id in "${agent_a_execution_id}" "${agent_b_execution_id}"; do
  pricing_record="$(sqlite3 "${workspace}/gongbu.sqlite3" "SELECT pricing_schema_version || '|' || json_extract(pricing_snapshot_json, '$.schema_version') || '|' || json_extract(pricing_snapshot_json, '$.pricing_rule_id') || '|' || json_extract(pricing_snapshot_json, '$.selector.image_size') FROM executions WHERE execution_id = '${execution_id}';")"
  [[ "${pricing_record}" == "2|2|fixture-image-1k|1k" ]] || fail "execution ${execution_id} did not persist the selected schema-v2 1k price"
  temporal workflow describe \
    --address "127.0.0.1:${temporal_port}" \
    --namespace default \
    --workflow-id "gongbu-execution-${execution_id}" >/dev/null
done

agent_a_attribution="$(sqlite3 "${workspace}/gongbu.sqlite3" "SELECT agent_id || '|' || account_id || '|' || operation_key FROM hubu_authorization_snapshots WHERE execution_id = '${agent_a_execution_id}';")"
agent_b_attribution="$(sqlite3 "${workspace}/gongbu.sqlite3" "SELECT agent_id || '|' || account_id || '|' || operation_key FROM hubu_authorization_snapshots WHERE execution_id = '${agent_b_execution_id}';")"
[[ "${agent_a_attribution}" == "${agent_a_id}|${agent_a_account_id}|hub-140-agent-a" ]] || fail "Agent A attribution changed"
[[ "${agent_b_attribution}" == "${agent_b_id}|${agent_b_account_id}|hub-140-agent-b" ]] || fail "Agent B attribution changed"

budgets="$(curl --fail --silent \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${hubu_auth}")" \
  "${hubu_endpoint}/budgets")"
jq -e --arg agent "${agent_a_id}" '.budgets[] | select(.agent_id == $agent) | .consumed_amount_cents == 1 and .frozen_amount_cents == 0 and .remaining_amount_cents == 99' <<<"${budgets}" >/dev/null || fail "Agent A budget did not settle independently"
jq -e --arg agent "${agent_b_id}" '.budgets[] | select(.agent_id == $agent) | .consumed_amount_cents == 1 and .frozen_amount_cents == 0 and .remaining_amount_cents == 99' <<<"${budgets}" >/dev/null || fail "Agent B budget did not settle independently"
jq -e --arg agent "${approval_agent_id}" '.budgets[] | select(.agent_id == $agent) | .consumed_amount_cents == 2 and .frozen_amount_cents == 0 and .remaining_amount_cents == 98' <<<"${budgets}" >/dev/null || fail "approval resume did not settle the fixture's authoritative two-cent 2k provider cost"

agent_a_artifact_record="$(retrieve_artifact "${agent_a_execution_id}" "${workspace}/agent-a.png")"
agent_b_artifact_record="$(retrieve_artifact "${agent_b_execution_id}" "${workspace}/agent-b.png")"
agent_a_artifact_id="${agent_a_artifact_record%%|*}"
agent_b_artifact_id="${agent_b_artifact_record%%|*}"
[[ "${agent_a_artifact_id}" != "${agent_b_artifact_id}" ]] || fail "two executions shared one artifact identity"

[[ "$(submit_execution "${agent_a_token}" agent-a)" == "${agent_a_execution_id}" ]] || fail "Agent A settled-token replay changed execution"
[[ "$(submit_execution "${agent_b_token}" agent-b)" == "${agent_b_execution_id}" ]] || fail "Agent B settled-token replay changed execution"

[[ "$(wc -l <"${lifecycle_log}" | tr -d ' ')" == "${lifecycle_count_before_registration}" ]] || fail "stack lifecycle changed between registration and execution"
[[ "$(grep -c '^render$' "${lifecycle_log}")" == "1" ]] || fail "stack rendered again after registration"
if grep -q '^activate$' "${lifecycle_log}"; then
  fail "stack activated explicitly after its automatic first-generation activation"
fi

for credential in "${hubu_auth}" "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}" "${provider_secret}"; do
  secret="$(tr -d '\r\n' <"${credential}")"
  if grep -R -F "${secret}" "${profile}/generated" "${workspace}/hubu.log" "${workspace}/gongbu.log" >/dev/null; then
    fail "generated artifacts or service logs exposed credential material"
  fi
done

stack_lifecycle stop >/dev/null
stack_started=0
stack_lifecycle start >/dev/null
stack_started=1
[[ "$(grep -c '^start$' "${lifecycle_log}")" == "2" && "$(grep -c '^stop$' "${lifecycle_log}")" == "1" ]] || fail "managed stack restart count changed"
[[ "$(wc -l <"${hubu_serve_trace}" | tr -d ' ')" == "2" ]] || fail "restart did not preserve one managed Hubu process per start"
[[ "$(shasum -a 256 "${hubu_auth}" "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}")" == "${credential_digests_before}" ]] || fail "managed credentials changed during restart"

for label in agent-a agent-b; do
  if [[ "${label}" == "agent-a" ]]; then
    token="${agent_a_token}"
    execution_id="${agent_a_execution_id}"
    artifact_before="${workspace}/agent-a.png"
  else
    token="${agent_b_token}"
    execution_id="${agent_b_execution_id}"
    artifact_before="${workspace}/agent-b.png"
  fi
  temporal workflow describe \
    --address "127.0.0.1:${temporal_port}" \
    --namespace default \
    --workflow-id "gongbu-execution-${execution_id}" >/dev/null
  [[ "$(submit_execution "${token}" "${label}")" == "${execution_id}" ]] || fail "${label} replay changed after restart"
  artifact_after="${workspace}/${label}-after-restart.png"
  retrieve_artifact "${execution_id}" "${artifact_after}" >/dev/null
  cmp "${artifact_before}" "${artifact_after}" >/dev/null || fail "${label} artifact did not survive whole-stack restart"
done

budgets_after_restart="$(curl --fail --silent \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${hubu_auth}")" \
  "${hubu_endpoint}/budgets")"
[[ "${budgets_after_restart}" == "${budgets}" ]] || fail "budget settlement changed during restart/replay"

stable_credential_digests="$(shasum -a 256 "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}")"
hubu_auth_digest_before="$(shasum -a 256 "${hubu_auth}" | awk '{ print $1 }')"
stack_lifecycle stop >/dev/null
stack_started=0
rm -f "${hubu_auth}"
stack_lifecycle start >/dev/null
stack_started=1
[[ "$(wc -l <"${hubu_serve_trace}" | tr -d ' ')" == "3" ]] || fail "Hubu credential recovery did not use one final managed process"
hubu_auth_digest_after="$(shasum -a 256 "${hubu_auth}" | awk '{ print $1 }')"
[[ "${hubu_auth_digest_after}" != "${hubu_auth_digest_before}" ]] || fail "Hubu did not regenerate the removed managed capability"
cmp "${hubu_auth}" "${gongbu_hubu}" >/dev/null || fail "Gongbu did not refresh its verified Hubu credential handoff"
[[ "$(shasum -a 256 "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}")" == "${stable_credential_digests}" ]] || fail "Hubu credential recovery changed unrelated managed capabilities"

stack_lifecycle stop >/dev/null
stack_started=0
[[ ! -e "${profile}/runtime/launcher-state.json" ]] || fail "graceful stop left launcher ownership state"

echo "Local-stack acceptance passed: managed credential bootstrap and recovery, cross-surface approval synchronization, provider-free denial, idempotent public-handle resume, principal-neutral execution, restart persistence, and graceful shutdown"
