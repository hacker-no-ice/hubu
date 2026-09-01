# Complete local stack configuration examples

These examples show the relationships among all three source files. Replace
every active `/absolute/...` path and public ID with values from your
installation. Managed service credential locations are not active fields in
these examples; the launcher selects them internally.

## Sandbox outcome

This is the recommended first profile. Initialization writes the complete
Hubu, Gongbu, and Temporal topology plus a deterministic fixture target and
synthetic pricing:

```sh
hubu stack init --mode sandbox --install-temporal --profile /absolute/path/to/profile
hubu stack doctor --profile /absolute/path/to/profile
```

Internal authentication, governance, authorization, execution submission,
receipts, and settlement use the real stack boundaries. Only the external
provider edge is replaced. Sandbox configuration contains no provider
credential, live-spend ceiling, or acknowledgement and cannot contact or
charge an external provider.

## Provider-disabled local-stack variation

This advanced variation manages Hubu, Gongbu, and local Temporal while
omitting all provider targets. It is useful for dependency diagnostics but,
unlike sandbox mode, cannot demonstrate governed provider execution.

### `stack.toml`

```toml
schema_version = 1
mode = "local-stack"
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

Managed Hubu and Gongbu need no service credential references. The final Hubu
process and the Gongbu-owned handoff provision them during `stack start`.

```toml
schema_version = 1
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

Doctor should move from `incomplete` to `ready_to_render` after every topology
path, port, version, and explicit reference is valid. Before first start it
reports the derived managed credentials as pending managed work. Provider
readiness is reported separately as disabled.

## Hubu-only outcome

Hubu-only mode deliberately removes the execution plane:

```sh
hubu stack init --mode hubu-only --profile /absolute/path/to/profile
```

Its `stack.toml` contains `mode = "hubu-only"`, `[binaries]`, `[hubu]`, and
`[runtime]`. It omits `[gongbu]`, `[temporal]`, and `binaries.gongbu_server`.
Its provider source is intentionally minimal:

```toml
schema_version = 1
mode = "disabled"
```

The generated unified-MCP handoff contains only Hubu endpoint and credential
references. Missing Gongbu and Temporal are reported as intentionally absent,
not as unhealthy services. Operator-owned workflows may use Hubu registration,
policy, authorization, and budget contracts without adopting Gongbu provider
execution.

## FLUX.2 provider contract

This is the ready-to-render provider shape for
`hubu.flux-2-pro.text-to-image/v1`. Use the complete managed `stack.toml` from
the disabled example. The operator supplies only the non-secret Keychain
coordinates and explicit spend choices; the target, policies, dimensions, and
pricing come from the frozen contract.

### `credentials.toml`

Create and store the key yourself with macOS Keychain Access. Never put the key
in this file, a terminal, environment variable, command, log, SQLite database,
or documentation.

```toml
schema_version = 1

[opaque.bfl_flux2_pro]
service = "operator-owned BFL Keychain service"
account = "operator-owned BFL Keychain account"
```

### `providers.toml`

```toml
schema_version = 1
mode = "live"

# Example only: replace with the positive USD-cent ceiling you reviewed.
maximum_spend_minor = 8
live_spend_acknowledgement = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"

[[contract_bindings]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux2_pro"
```

The contract-only catalog derives
`bfl-flux-2-pro-usd-2026-08-28-v1`; do not copy the target or its three pricing
rules into raw tables. Run `hubu stack catalog --json`, doctor, and render to
review exact target, resolutions, rational USD rates, and independent readiness
facts without contacting BFL. `live_qualified` remains false with
`not_performed` in this release. Apply the shared [live provider operations
guide](../../../operations/live-providers.md), then follow the
[FLUX.2 provider contract](../../../operations/flux-provider-contract.md).

## Representative generic live-provider configuration

This example is deliberately **not provider-ready**: `provider.example`, the model, version labels, Keychain coordinates, and illustrative one-cent price must be replaced with authoritative values. It demonstrates the complete schema without claiming current provider configuration or pricing.

Live execution can incur charges after a verified configuration is rendered, activated, started, and used.

### `stack.toml`

Use the same complete `stack.toml` from the
[provider-disabled local-stack variation](#provider-disabled-local-stack-variation).
Provider mode does not merge Hubu and Gongbu or change their topology. Register
and fund each agent in Hubu after startup; registration does not change the
stack generation or Gongbu configuration.

### `credentials.toml`

Add only the opaque provider reference needed by the live target:

```toml
schema_version = 1

# Replace with the Keychain coordinates created for the selected provider.
[opaque.provider_image]
service = "gongbu.provider.REPLACE"
account = "image-provider-credential"
```

### `providers.toml`

```toml
schema_version = 1
mode = "live"

# Required because this example combines two immutable contracts.
catalog_version = "gemini-flux-composite-REPLACE_WITH_DATE_AND_REVISION"

# Positive USD cents approved as this profile's explicit upper boundary.
maximum_spend_minor = 25
live_spend_acknowledgement = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"

[[contract_bindings]]
contract = "hubu.gemini-3.1-flash-lite-image.text-to-image/v1"
credential = "google_gemini_developer"

[[contract_bindings]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux2_pro"
```

The contracts supply immutable targets, adapter settings, capabilities, and
pricing. Runtime callers discover the resulting opaque `target_id` values and
never submit these provider details.

### Live-profile review checklist

- [ ] All four binaries share one verified Hubu release lineage.
- [ ] Each agent that will request spend is registered in Hubu and has the intended policy and budget.
- [ ] Managed service credentials are omitted; or every advanced/external file override is distinct, private, and owned by the selected service.
- [ ] Provider opaque coordinates resolve under the Gongbu process identity without exposing values.
- [ ] Provider, adapter, model, endpoint, API version, adapter settings, and artifact hosts are authoritative.
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

External Hubu requires all three Hubu file references in `credentials.toml`.
External Gongbu requires `files.gongbu_caller`. See the
[`credentials.toml` reference](credentials-toml.md) for explicit managed-Gongbu
override requirements.

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
