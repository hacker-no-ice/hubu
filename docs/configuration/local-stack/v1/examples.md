# Complete local stack configuration examples

These examples show the relationships among all three source files. Replace every `/absolute/...` path and public ID with values from your installation. A path example does not become valid until the referenced binary, directory ancestor, or credential file exists as required.

## Provider-disabled local profile

This is the recommended first profile. It manages Hubu, Gongbu, and local Temporal while omitting all provider targets, prices, spend ceilings, and live-spend acknowledgement.

### `stack.toml`

```toml
schema_version = 1
allow_development_builds = false

[binaries]
hubu = "/absolute/path/to/hubu"
hubu_server = "/absolute/path/to/hubu-server"
gongbu_server = "/absolute/path/to/gongbu-server"
hubu_unified_mcp = "/absolute/path/to/hubu-unified-mcp"

[hubu]
ownership = "managed"
endpoint = "http://127.0.0.1:8787"
listen = "127.0.0.1:8787"
database_path = "/absolute/path/to/profile/state/hubu/hubu.sqlite3"
log_file = "/absolute/path/to/profile/state/hubu/hubu.jsonl"

[gongbu]
ownership = "managed"
endpoint = "http://127.0.0.1:8788"
listen = "127.0.0.1:8788"
database_path = "/absolute/path/to/profile/state/gongbu/gongbu.sqlite3"
artifact_root = "/absolute/path/to/profile/state/gongbu/artifacts"
log_file = "/absolute/path/to/profile/state/gongbu/gongbu.jsonl"

[temporal]
mode = "managed_local"
binary_path = "/absolute/path/to/temporal"
# Copy the exact version printed by the selected Temporal CLI.
expected_cli_version = "REPLACE_WITH_EXACT_INSTALLED_VERSION"
data_path = "/absolute/path/to/profile/state/temporal"
rpc_port = 7233
ui_port = 8233
namespace = "default"
task_queue = "gongbu-local-executions"
ui_url = "http://127.0.0.1:8233"

[runtime]
hubu_startup_policy = "wait"
hubu_startup_timeout_ms = 30000
recovery_delays_seconds = [30, 120, 600]
temporal_startup_timeout_ms = 30000
dependency_check_interval_ms = 5000
worker_drain_timeout_ms = 30000
max_artifacts_per_execution = 4
max_encoded_bytes = 20971520
max_decoded_bytes = 104857600
max_width = 16384
max_height = 16384
log_level = "info"
log_format = "text"
```

### `credentials.toml`

The four file paths refer to existing capability files. The two opaque pairs are Gongbu-owned Keychain coordinates. None of these strings is a credential value.

```toml
schema_version = 1

[files]
hubu_auth = "/absolute/path/to/profile/state/hubu/hubu.auth-token"
hubu_approval = "/absolute/path/to/profile/state/hubu/hubu.approval-token"
hubu_reconciliation = "/absolute/path/to/profile/state/hubu/hubu.reconciliation-token"
gongbu_caller = "/absolute/path/to/profile/state/gongbu/gongbu.caller-token"

[opaque.gongbu_hubu]
service = "gongbu.hubu"
account = "local-stack"

[opaque.gongbu_caller]
service = "gongbu.caller"
account = "local-stack"
```

### `providers.toml`

Disabled means every live-only field and table is absent.

```toml
schema_version = 1
mode = "disabled"
```

### Validate the disabled profile

```sh
hubu stack doctor --profile /absolute/path/to/profile
```

Doctor should move from `incomplete` to `ready_to_render` only after every path, coordinate, port, version, and credential reference is valid. Provider readiness is reported separately as disabled.

## Representative live-provider profile

This example is deliberately **not provider-ready**: `provider.example`, the model, version labels, Keychain coordinates, and illustrative one-cent price must be replaced with authoritative values. It demonstrates the complete schema without claiming current provider configuration or pricing.

Live execution can incur charges after a verified configuration is rendered, activated, started, and used.

### `stack.toml`

Use the same complete `stack.toml` from the provider-disabled example. Provider mode does not merge Hubu and Gongbu or change their topology. Register and fund each agent in Hubu after startup; registration does not change the stack generation or Gongbu configuration.

### `credentials.toml`

Add one opaque provider reference to the complete credential file:

```toml
schema_version = 1

[files]
hubu_auth = "/absolute/path/to/profile/state/hubu/hubu.auth-token"
hubu_approval = "/absolute/path/to/profile/state/hubu/hubu.approval-token"
hubu_reconciliation = "/absolute/path/to/profile/state/hubu/hubu.reconciliation-token"
gongbu_caller = "/absolute/path/to/profile/state/gongbu/gongbu.caller-token"

[opaque.gongbu_hubu]
service = "gongbu.hubu"
account = "local-stack"

[opaque.gongbu_caller]
service = "gongbu.caller"
account = "local-stack"

# Replace with the Keychain coordinates created for the selected provider.
[opaque.provider_image]
service = "gongbu.provider.REPLACE"
account = "image-provider-credential"
```

### `providers.toml`

```toml
schema_version = 1
mode = "live"

# Create new immutable labels whenever target or pricing content changes.
catalog_version = "operator-verified-REPLACE_WITH_DATE_AND_REVISION"

# Positive USD cents approved as this profile's explicit upper boundary.
maximum_spend_minor = 25
live_spend_acknowledgement = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"

[[targets]]
provider_config_version = "provider-image-REPLACE_WITH_IMMUTABLE_REVISION"
workload_type = "image_generation"
provider = "REPLACE_WITH_GONGBU_PROVIDER_ID"
adapter = "gemini_developer_image"
model = "REPLACE_WITH_EXACT_PROVIDER_MODEL_ID"
credential = "provider_image"
active = true
execution_enabled = true

[targets.settings]
type = "gemini_developer_image"

[targets.settings.config]
endpoint = "https://provider.example"
api_version = "REPLACE_WITH_SUPPORTED_API_VERSION"
timeout_ms = 30000
max_retries = 0
headers = {}

[[pricing_rules]]
rule_id = "provider-model-1k-REPLACE"
provider = "REPLACE_WITH_GONGBU_PROVIDER_ID"
model = "REPLACE_WITH_EXACT_PROVIDER_MODEL_ID"
currency = "USD"
selector = { image_size = "1k" }

# Illustrative one-cent price only. Replace with the exact verified provider
# rate in cents expressed as an integer numerator and denominator.
components = [
  { unit = "image", rate_numerator_minor = 1, rate_denominator = 1 },
]
```

If the selected model enables `2k` or `4k`, add a separate selector-qualified rule for every enabled size. If billing also includes input or output tokens, use an unqualified multi-component rule when supported by the request/pricing contract; never omit billable components.

### Live-profile review checklist

- [ ] All four binaries share one verified Hubu release lineage.
- [ ] Each agent that will request spend is registered in Hubu and has the intended policy and budget.
- [ ] File paths refer to distinct existing capability files.
- [ ] Opaque coordinates resolve under the Gongbu process identity without exposing values.
- [ ] Provider, adapter, model, endpoint, API version, region/project fields, and artifact hosts are authoritative.
- [ ] `provider_config_version` and `catalog_version` have never represented different content.
- [ ] Every enabled request selector has exactly one matching pricing rule.
- [ ] Every billable component is represented using exact minor-unit rational rates.
- [ ] `maximum_spend_minor` is conservative and independently approved.
- [ ] The exact live-spend acknowledgement was entered intentionally.
- [ ] Doctor and both service-owned production validators pass.
- [ ] The rendered generation ID, changed files, and affected components were reviewed before activation.

## External-service variations

For external Hubu, retain `hubu.ownership` and `hubu.endpoint` but omit managed-only `hubu.listen`, `hubu.database_path`, and `binaries.hubu_server`. The endpoint remains an explicit loopback origin in schema version 1.

For external Gongbu, retain `gongbu.ownership` and `gongbu.endpoint` but omit managed-only Gongbu binary, state, artifact, and local Temporal configuration. Provider readiness is owned by external Gongbu and reported as unknown by the local profile rather than certified from unused local provider inputs.

For external Temporal with managed Gongbu:

```toml
[temporal]
mode = "external"
address = "http://127.0.0.1:7233"
namespace = "default"
task_queue = "gongbu-local-executions"
ui_url = "http://127.0.0.1:8233"
```

Omit managed-local Temporal binary, version, data path, and port fields. Gongbu still owns its worker; the external operator owns the Temporal service lifecycle.
