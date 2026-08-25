#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(mktemp -d)"
profile="${workspace}/profile"
stack_started=0

cleanup() {
  local status=$?
  if [[ "${stack_started}" == "1" && -x "${root_dir}/target/debug/hubu" ]]; then
    "${root_dir}/target/debug/hubu" stack stop --profile "${profile}" >/dev/null 2>&1 || true
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
cargo build --locked -p hubu-cli --bin hubu --features local-fixture-canary
cargo build --locked --bin hubu-server --bin hubu-unified-mcp
cargo build --locked -p gongbu-api --bin gongbu-server --features local-fixture-canary

hubu_bin="${root_dir}/target/debug/hubu"
hubu_server_bin="${root_dir}/target/debug/hubu-server"
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
hubu_auth="${workspace}/hubu-auth"
hubu_approval="${workspace}/hubu-approval"
hubu_reconciliation="${workspace}/hubu-reconciliation"
gongbu_caller="${workspace}/gongbu-caller"
provider_secret="${workspace}/provider"
printf '%s\n' 'hub-105-local-broad-bearer' >"${hubu_auth}"
printf '%s\n' 'hub-105-local-approval' >"${hubu_approval}"
printf '%s\n' 'hub-105-human-reconciliation-never-given-to-gongbu' >"${hubu_reconciliation}"
printf '%s\n' 'hub-105-gongbu-caller' >"${gongbu_caller}"
printf '%s\n' 'hub-105-fixture-provider' >"${provider_secret}"
chmod 600 "${hubu_auth}" "${hubu_approval}" "${hubu_reconciliation}" "${gongbu_caller}" "${provider_secret}"

init_output="$("${hubu_bin}" stack init --profile "${profile}")"
grep -F 'input needed:' <<<"${init_output}" >/dev/null
for name in README.md stack.toml credentials.toml providers.toml; do
  [[ -f "${profile}/${name}" ]] || fail "init omitted ${name}"
done
[[ ! -e "${profile}/runtime/launcher-state.json" ]] || fail "init started or recorded a service"

doctor_output="$("${hubu_bin}" stack doctor --profile "${profile}" 2>&1 || true)"
grep -F 'stack.toml:hubu.ownership' <<<"${doctor_output}" >/dev/null
grep -F 'credentials.toml:files.hubu_auth' <<<"${doctor_output}" >/dev/null
grep -F 'providers.toml:mode' <<<"${doctor_output}" >/dev/null

cat >"${profile}/credentials.toml" <<EOF
schema_version = 1

[files]
hubu_auth = $(quote "${hubu_auth}")
hubu_approval = $(quote "${hubu_approval}")
hubu_reconciliation = $(quote "${hubu_reconciliation}")
gongbu_caller = $(quote "${gongbu_caller}")
EOF

cat >"${profile}/providers.toml" <<'EOF'
schema_version = 1
mode = "disabled"
EOF

cat >"${profile}/stack.toml" <<EOF
schema_version = 1
allow_development_builds = true

[binaries]
hubu = $(quote "${hubu_bin}")
hubu_server = $(quote "${hubu_server_bin}")
hubu_unified_mcp = $(quote "${mcp_bin}")

[hubu]
ownership = "managed"
endpoint = $(quote "${hubu_endpoint}")
listen = "127.0.0.1:${hubu_port}"
database_path = $(quote "${workspace}/hubu.sqlite3")
log_file = $(quote "${workspace}/hubu.log")

[gongbu]
ownership = "external"
endpoint = $(quote "${gongbu_endpoint}")
EOF

"${hubu_bin}" stack doctor --profile "${profile}" >/dev/null
if first_start="$("${hubu_bin}" stack start --profile "${profile}" 2>&1)"; then
  fail "external Gongbu unexpectedly existed before bootstrap"
fi
grep -F 'external component is unavailable' <<<"${first_start}" >/dev/null
stack_started=1

hubu() {
  env -u HUBU_AUTH_TOKEN -u HUBU_APPROVAL_TOKEN -u HUBU_RECONCILIATION_TOKEN \
    HUBU_URL="${hubu_endpoint}" \
    HUBU_AUTH_TOKEN_FILE="${hubu_auth}" \
    "${hubu_bin}" "$@"
}

human_output="$(hubu register human --username hub-105-owner --display-name 'HUB-105 Owner')"
[[ -n "$(field "${human_output}" user_id)" ]] || fail "could not register the clean-profile owner"
agent_output="$(hubu register agent --name hub-105-canary --version v1)"
agent_id="$(field "${agent_output}" agent_id)"
account_id="$(field "${agent_output}" account_id)"
[[ -n "${agent_id}" && -n "${account_id}" ]] || fail "could not register the clean-profile agent"
hubu budget create --agent-id "${agent_id}" --amount 1 >/dev/null

policy="${workspace}/fixture-policy.yaml"
cat >"${policy}" <<'EOF'
id: hub_105_fixture
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
EOF
hubu policy add --path "${policy}" >/dev/null

authorization="$(hubu spend authorize \
  --operation-key hub-105-fixture \
  --account-id "${account_id}" \
  --amount 0.01 \
  --currency USD \
  --reason 'local stack acceptance fixture' \
  --provider provider:local:fixture \
  --executor executor:gongbu:image \
  --capability capability:image:generate \
  --billing-merchant merchant:local \
  --lease-profile default)"
spend_auth_token_id="$(field "${authorization}" auth_token_id)"
[[ -n "${spend_auth_token_id}" ]] || fail "Hubu did not issue fixture authorization"

"${hubu_bin}" stack stop --profile "${profile}" >/dev/null
stack_started=0

cat >>"${profile}/credentials.toml" <<'EOF'

[opaque.gongbu_hubu]
service = "hubu.local-fixture"
account = "executor"

[opaque.gongbu_caller]
service = "hubu.local-fixture"
account = "caller"

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
maximum_spend_minor = 1
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

export GONGBU_LOCAL_FIXTURE_CANARY=1
export GONGBU_LOCAL_FIXTURE_SECRET_DIR="${workspace}"
export HUBU_LOCAL_FIXTURE_CANARY=1
render_output="$("${hubu_bin}" stack render --profile "${profile}")"
generation="$(awk '/validated staged generation:/ { print $4; exit }' <<<"${render_output}")"
[[ -n "${generation}" ]] || fail "managed fixture generation was not staged"
"${hubu_bin}" stack activate --generation "${generation}" --profile "${profile}" >/dev/null
if ! start_output="$("${hubu_bin}" stack start --profile "${profile}" 2>&1)"; then
  echo "${start_output}" >&2
  fail "managed Hubu/Gongbu stack did not start"
fi
stack_started=1
jq -e '.classification == "running_ready"' < <("${hubu_bin}" stack status --json --profile "${profile}") >/dev/null

submission_response="$(curl --silent --show-error --write-out '\n%{http_code}' \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg token "${spend_auth_token_id}" '{schema_version:2,spend_auth_token_id:$token,input:{prompt:"acceptance canary",image_count:1,image_size:"1k"},input_schema_version:1,workload_type:"default",provider:"example",adapter:"fixture",model:"image-v1"}')" \
  "${gongbu_endpoint}/v2/executions")"
submission_status="$(tail -n 1 <<<"${submission_response}")"
submission="$(sed '$d' <<<"${submission_response}")"
if [[ ! "${submission_status}" =~ ^2 ]]; then
  echo "${submission}" >&2
  fail "Gongbu rejected the governed fixture submission with HTTP ${submission_status}"
fi
execution_id="$(jq -r '.execution_id' <<<"${submission}")"
[[ "${execution_id}" != "null" && -n "${execution_id}" ]] || fail "Gongbu did not create the fixture execution"

terminal=''
for _ in {1..150}; do
  terminal="$(curl --fail --silent \
    -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
    "${gongbu_endpoint}/v1/executions/${execution_id}")"
  if [[ "$(jq -r '.status' <<<"${terminal}")" == "succeeded" ]]; then
    break
  fi
  sleep 0.1
done
[[ "$(jq -r '.status' <<<"${terminal}")" == "succeeded" ]] || fail "fixture execution did not succeed"
pricing_record="$(sqlite3 "${workspace}/gongbu.sqlite3" "SELECT pricing_schema_version || '|' || json_extract(pricing_snapshot_json, '$.schema_version') || '|' || json_extract(pricing_snapshot_json, '$.pricing_rule_id') || '|' || json_extract(pricing_snapshot_json, '$.selector.image_size') FROM executions WHERE execution_id = '${execution_id}';")"
[[ "${pricing_record}" == "2|2|fixture-image-1k|1k" ]] || fail "fixture execution did not persist the selected schema-v2 1k price"

workflow_id="gongbu-execution-${execution_id}"
temporal workflow describe \
  --address "127.0.0.1:${temporal_port}" \
  --namespace default \
  --workflow-id "${workflow_id}" >/dev/null

artifacts="$(curl --fail --silent \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
  "${gongbu_endpoint}/v1/executions/${execution_id}/artifacts")"
artifact_id="$(jq -r '.artifacts[0].artifact_id' <<<"${artifacts}")"
artifact_sha="$(jq -r '.artifacts[0].sha256' <<<"${artifacts}")"
[[ "${artifact_id}" != "null" && -n "${artifact_id}" ]] || fail "fixture execution has no artifact"
curl --fail --silent \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
  "${gongbu_endpoint}/v1/artifacts/${artifact_id}" \
  -o "${workspace}/artifact.png"
[[ "$(shasum -a 256 "${workspace}/artifact.png" | awk '{ print $1 }')" == "${artifact_sha}" ]] || fail "retrieved artifact digest changed"

if grep -R -F 'hub-105-human-reconciliation-never-given-to-gongbu' "${profile}/generated" >/dev/null; then
  fail "generated stack artifacts contain the human reconciliation capability"
fi

"${hubu_bin}" stack stop --profile "${profile}" >/dev/null
stack_started=0
"${hubu_bin}" stack start --profile "${profile}" >/dev/null
stack_started=1
temporal workflow describe \
  --address "127.0.0.1:${temporal_port}" \
  --namespace default \
  --workflow-id "${workflow_id}" >/dev/null
persisted="$(curl --fail --silent \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
  "${gongbu_endpoint}/v1/executions/${execution_id}")"
[[ "$(jq -r '.status' <<<"${persisted}")" == "succeeded" ]] || fail "execution state did not survive whole-stack restart"
curl --fail --silent \
  -H "Authorization: Bearer $(tr -d '\r\n' <"${gongbu_caller}")" \
  "${gongbu_endpoint}/v1/artifacts/${artifact_id}" \
  -o "${workspace}/artifact-after-restart.png"
cmp "${workspace}/artifact.png" "${workspace}/artifact-after-restart.png" >/dev/null || fail "artifact did not survive whole-stack restart"

"${hubu_bin}" stack stop --profile "${profile}" >/dev/null
stack_started=0
[[ ! -e "${profile}/runtime/launcher-state.json" ]] || fail "graceful stop left launcher ownership state"

echo "Local-stack acceptance passed: real init/doctor/render/start, governed fixture execution, Temporal workflow and artifact discovery, whole-stack restart persistence, and graceful shutdown"
