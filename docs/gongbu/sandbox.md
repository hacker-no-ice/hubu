# Gongbu sandbox

The sandbox is a long-running manual-test environment. It starts an isolated
Gongbu API, a Temporal worker, and (by default) a local Temporal development
server and UI. Hubu and provider boundaries are selected independently, so the
same operator workflow can exercise deterministic mocks or guarded real
traffic.

Install the Temporal and GitHub CLIs before the first managed run (`brew install
temporal gh` on macOS), and authenticate `gh` to the Hubu release repository.
Then install the sandbox CLI once from the repository root:

```sh
cargo install --locked --force \
  --path crates/gongbu-api \
  --bin gongbu-sandbox
```

This places `gongbu-sandbox` in Cargo's binary directory, normally
`$HOME/.cargo/bin`. Ensure that directory is on `PATH`. Re-run the install
command after updating or switching the sandbox branch so the command matches
the checked-out code.

In terminal 1, start the sandbox:

```sh
gongbu-sandbox start \
  --config examples/gongbu/sandbox.mock.json \
  --hubu-mode mock \
  --provider-mode mock
```

Wait for every readiness entry to report `ready: true`. The command prints the
run directory, Gongbu URL, Temporal UI URL, and a ready-to-copy submit command,
then stays running until Ctrl+C. Keep terminal 1 open.

Each run gets an operator token, SQLite database, artifact root, Temporal data,
logs, and loopback-only ports. A managed Hubu run adds its own isolated port and
database. State is deleted after shutdown unless
`--preserve /absolute/new/directory` is supplied. The destination must not
already exist; preserved manifest paths are rewritten to the destination so
they remain usable.

## Configuration precedence

Configuration is applied in this order, with later values winning:

1. JSON profile
2. environment variables
3. CLI parameters

For interactive use, prefer CLI parameters because the selected modes remain
visible in shell history and do not persist into later shell commands:

```sh
gongbu-sandbox start \
  --config examples/gongbu/sandbox.mock.json \
  --hubu-mode mock \
  --provider-mode mock
```

The available CLI overrides are `--hubu-mode`, `--hubu-version`, `--provider-mode`,
`--max-spend-minor`, and `--live-spend-ack`. The `submit` command accepts
`--image-size 1k|2k|4k`; this value selects the matching schema-v2 pricing rule
and is transmitted to adapters that support resolution selection. A managed
Hubu release provisions and supplies scoped authorization automatically; an
externally managed Hubu still requires `--hubu-token-reference`.

Environment variables remain useful for CI and automation:

- `GONGBU_SANDBOX_HUBU_MODE` (`mock` or `real`)
- `GONGBU_SANDBOX_PROVIDER_MODE` (`mock` or `real`)
- `GONGBU_SANDBOX_MAX_SPEND_MINOR`
- `GONGBU_SANDBOX_LIVE_SPEND_ACK`

Unknown JSON fields, unknown mode values, and mock modes combined with real
endpoint or credential fields fail at startup. A `production` profile rejects
either mock boundary.

## Mode matrix

| Hubu | Provider | Intended use |
| --- | --- | --- |
| Mock | Mock | deterministic local development and ordinary CI |
| Mock | Real | real provider traffic with deterministic Hubu accounting |
| Real | Mock | real Hubu traffic with deterministic provider output |
| Real | Real | guarded release dogfood |

The required real-Hubu compatibility profile uses an exact immutable release.
Mutable aliases such as `latest` and `main` are rejected. The sandbox selects
the current platform archive, verifies its published `SHA256SUMS` entry,
validates packaged provenance and `hubu-spend-executor-v4`, and checks the
binary's reported product/source/contract tuple before readiness.

The selected implementation is wired through the same `HubuActivities` and
`ProviderActivities` interfaces as the durable production workflow. Modes do
not change persistence, artifact, replay, or settlement invariants.

Managed Temporal is the default and requires no profile fields. To use an
already-running Temporal environment instead, add an explicit configuration:

```json
{
  "temporal": {
    "mode": "external",
    "address": "http://127.0.0.1:7233",
    "ui_url": "http://127.0.0.1:8233",
    "namespace": "default"
  }
}
```

## Common inspection and cleanup

Every `start` command prints a temporary run directory ending in
`gongbu-sandbox-...`. Copy that exact path into `RUN_DIR` in a second terminal.
Every `submit` waits for a terminal or reconciliation state and prints the
execution ID and a direct Temporal UI URL.

After submitting, use these commands in every mode:

```sh
gongbu-sandbox status \
  --run-dir "$RUN_DIR" \
  --execution-id "$EXECUTION_ID"

gongbu-sandbox artifacts \
  --run-dir "$RUN_DIR" \
  --execution-id "$EXECUTION_ID" \
  --download-dir /tmp/gongbu-artifacts

gongbu-sandbox inspect \
  --run-dir "$RUN_DIR"
```

Open the Temporal URL and inspect workflow status and activity attempts. New
executions use `GranularExecutionWorkflow`; a successful history shows these
operator-visible boundaries in order:

1. `preflight_execution`
2. `claim_authorization`
3. `validate_claim`
4. `execute_provider`
5. `persist_artifacts`
6. `settle_spend`

Proven provider failures use `release_authorization` instead of artifact and
settlement work. Ambiguous outcomes use `perform_reconciliation` after the
configured bounded recovery timer or an operator signal. Select an activity in
Temporal UI to inspect its duration and retry attempts. Inputs contain only the
execution ID; prompts, credentials, provider responses, account identifiers,
and raw authorization material are loaded from Gongbu persistence inside the
activity and never appear in Temporal payloads.

`GranularExecutionWorkflow` is the sole registered execution workflow. Gongbu
has not shipped a release with the earlier coarse workflow, so there is no
legacy workflow history or compatibility registration to retain.

Replay by running the identical `submit` command with the same operation key,
prompt, image size, and Hubu token reference. It must return the same execution
ID. For each mocked boundary, `inspect` must show no additional provider
invocation or Hubu financial mutation. If a real-provider request times out or
enters `reconciliation_required`, do not submit a new operation key: the
provider may already have accepted a billable request.

Press Ctrl+C in the first terminal to stop. Add
`--preserve /tmp/gongbu-sandbox-debug` to `start` to retain the database,
artifacts, Temporal data, logs, manifest, and safe mock summaries. The preserve
destination must not already exist.

## Mock Hubu + mock provider

Use this mode for a deterministic full-pipeline check with no external traffic
or spend.

1. Start the sandbox:

   ```sh
   gongbu-sandbox start \
     --config examples/gongbu/sandbox.mock.json \
     --hubu-mode mock \
     --provider-mode mock
   ```

2. Submit from a second terminal:

   ```sh
   gongbu-sandbox submit \
     --run-dir "$RUN_DIR" \
     --operation-key mock-mock-1 \
     --prompt "Draw a blue circle"
   ```

3. Run the common inspection commands. Expect `succeeded`, one normalized
   one-pixel PNG, one provider invocation, and two Hubu financial mutations
   (claim and settle). Replay and confirm those counts do not increase.

## Mock Hubu + real Gemini provider

This mode sends real, potentially billable provider traffic while keeping Hubu
accounting deterministic. The examples use the Gemini Developer API adapter.
The spend ceiling guards Gongbu's frozen estimate; it is not a Google account
billing cap. Verify current model availability and pricing before every run.

### 1. Store the API key

Store the key in macOS Keychain rather than JSON, environment variables, or
shell history:

```sh
security add-generic-password -U \
  -s gongbu.google-ai-studio \
  -a local-e2e \
  -w
```

### 2. Create a schema-v2 provider target

Create `/absolute/path/gemini-targets.json` with exactly one active execution
target. `gemini-3.1-flash-image` supports the 1K, 2K, and 4K selector example;
if you select a different model, include only sizes that model supports.

```json
{
  "schema_version": 2,
  "provider_configs": [{
    "provider_config_version": "google-gemini-developer-manual-v1",
    "workload_type": "image_generation",
    "provider": "google",
    "adapter": "gemini_developer_image",
    "model": "gemini-3.1-flash-image",
    "secret_service": "gongbu.google-ai-studio",
    "secret_account": "local-e2e",
    "active": true,
    "execution_enabled": true,
    "settings": {
      "type": "gemini_developer_image",
      "config": {
        "endpoint": "https://generativelanguage.googleapis.com",
        "api_version": "v1beta",
        "timeout_ms": 120000,
        "max_retries": 0,
        "headers": {}
      }
    }
  }]
}
```

### 3. Create a schema-v2 pricing catalog

Create `/absolute/path/gemini-pricing.json`. The rates below are an example of
exact rational USD-minor-unit rates; verify them against Google's current
pricing and your billing arrangement before use. Gongbu rounds the authorization
ceiling upward, so the corresponding maximum estimates are 7, 11, and 16 cents.

```json
{
  "schema_version": 2,
  "catalog_version": "gemini-image-manual-v1",
  "rules": [
    {
      "rule_id": "gemini-image-1k",
      "provider": "google",
      "model": "gemini-3.1-flash-image",
      "currency": "USD",
      "selector": {"image_size": "1k"},
      "components": [{
        "unit": "image",
        "rate_numerator_minor": 67,
        "rate_denominator": 10
      }]
    },
    {
      "rule_id": "gemini-image-2k",
      "provider": "google",
      "model": "gemini-3.1-flash-image",
      "currency": "USD",
      "selector": {"image_size": "2k"},
      "components": [{
        "unit": "image",
        "rate_numerator_minor": 101,
        "rate_denominator": 10
      }]
    },
    {
      "rule_id": "gemini-image-4k",
      "provider": "google",
      "model": "gemini-3.1-flash-image",
      "currency": "USD",
      "selector": {"image_size": "4k"},
      "components": [{
        "unit": "image",
        "rate_numerator_minor": 151,
        "rate_denominator": 10
      }]
    }
  ]
}
```

### 4. Create the sandbox profile

Create `/absolute/path/sandbox.mock-real.json`:

```json
{
  "profile": "development",
  "seed": 48,
  "hubu": {
    "mode": "mock",
    "currency": "USD",
    "maximum_authorization_minor": 100,
    "authorization_expires_at": "2099-01-01T00:00:00Z"
  },
  "provider": {
    "mode": "real",
    "target": {
      "workload_type": "image_generation",
      "provider": "google",
      "adapter": "gemini_developer_image",
      "model": "gemini-3.1-flash-image"
    },
    "target_config": "/absolute/path/gemini-targets.json",
    "pricing_catalog": "/absolute/path/gemini-pricing.json",
    "credential_reference": "gongbu.google-ai-studio:local-e2e"
  }
}
```

### 5. Start and submit

Choose a ceiling at least as large as the selected pricing tier. This 4K example
uses a 16-cent ceiling and the exact acknowledgement required by the sandbox:

```sh
gongbu-sandbox start \
  --config /absolute/path/sandbox.mock-real.json \
  --hubu-mode mock \
  --provider-mode real \
  --max-spend-minor 16 \
  --live-spend-ack I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND \
  --preserve /tmp/gongbu-gemini-manual
```

From a second terminal:

```sh
gongbu-sandbox submit \
  --run-dir "$RUN_DIR" \
  --operation-key mock-real-gemini-4k-1 \
  --prompt "Draw one small blue circle on a white background" \
  --image-size 4k
```

Run the common inspection commands. Expect a non-fixture image artifact and
mock Hubu claim and settle calls. Real-provider calls do not appear in the mock
provider invocation count; correlate the execution with the provider account's
usage/billing view. To test another size, choose a new operation key, pass the
new `--image-size`, and ensure the spend ceiling covers that tier.

## Real Hubu + mock provider

Use this mode to exercise real Hubu claim and settlement without provider
spend. One command resolves the pinned release, starts Hubu, Gongbu, Temporal,
and the deterministic mock provider, and provisions a fresh human, agent,
account, policy, and budget:

```sh
gongbu-sandbox start \
  --config examples/gongbu/sandbox.hubu-v0.1.0.json \
  --hubu-mode real \
  --hubu-version v0.1.0 \
  --provider-mode mock
```

In another terminal, submit without an externally supplied Hubu token:

```sh
gongbu-sandbox submit \
  --run-dir "$RUN_DIR" \
  --operation-key real-mock-1 \
  --prompt "Draw a blue circle"
```

The submit command authorizes the exact operation against the isolated fixture;
Gongbu then claims and settles it through the real executor protocol. The
provider output remains the deterministic one-pixel PNG. Replay the identical
submission and inspect `hubu/hubu.sqlite3`, `logs/hubu.jsonl`, and
`mock-side-effects.json` to verify there is one Hubu financial outcome and one
provider invocation.

Release archives are cached by exact version and platform under
`$XDG_CACHE_HOME/gongbu/hubu` or `$HOME/.cache/gongbu/hubu`. Cached provenance
and binary digests are revalidated before reuse, so an existing version is
never silently replaced. Once cached, the profile works offline. Upgrade by
selecting a different exact tag; cleanup removes per-run state but retains the
verified release cache. Use `--preserve /absolute/new/path` to retain a
self-consistent diagnostics run directory.

## Real Hubu + real provider

This is guarded release-level dogfood: both financial and provider traffic are
real. Use the pinned-release setup above together with the real-provider target,
pricing, Keychain, and acknowledgement setup from the Gemini section.

1. Create one combined profile with both boundaries set to `real`.
2. Confirm the Hubu authorization amount, mock-independent provider ceiling,
   pricing currency, provider account, model, and image size before startup.
3. Start with both explicit real modes, the spend ceiling, acknowledgement, and
   a new diagnostics destination.
4. Submit with `--image-size`; managed Hubu authorization is automatic.
5. Inspect Temporal, Gongbu status/artifacts, Hubu claim/settlement state, and
   provider billing. If any outcome is ambiguous, stop and reconcile the same
   execution rather than creating a new operation key.

Run focused automated sandbox tests separately; they never opt into live spend:

```sh
cargo test -p gongbu-api sandbox::
```

## Real-boundary gates

Real Hubu requires either a managed `release` with an exact version tag, or an
explicit external `http://` endpoint. External hosts must be loopback or appear
exactly in `allowlisted_hosts`, and external mode also requires an opaque scoped
credential reference and an isolated non-production account name. Managed and
external settings cannot be mixed.

Real provider mode requires:

- an explicit `target` object containing `workload_type`, `provider`, `adapter`,
  and `model`;
- an existing absolute provider target configuration path;
- an existing absolute frozen pricing catalog path;
- an opaque credential reference;
- a positive `maximum_spend_minor`; and
- the exact acknowledgement `I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND`.

The target configuration must expose exactly one active execution target, it
must match `target`, and fixture adapters are rejected. The credential reference
uses `service:account` and must match that target's Keychain reference. Both
provider files and the referenced Keychain credential are validated before
readiness succeeds. The spend ceiling is enforced again at provider preflight
and invocation against both authorization and the frozen pricing estimate. Mock
provider mode refuses all live-provider fields, cannot spend, and uses a
deterministic one-pixel PNG response.

## Fault injection and replay

Hubu supports `proven_before_mutation` and `commit_then_disconnect` independently
for claim, settle, and release. The mock is stateful: it checks account,
currency, expiry, authorization bounds, claim identity, and finalization state.
Repeated operation keys return the existing claim or settlement without another
financial mutation. Its call log stores only a SHA-256 operation-key digest.

Provider `scenario` supports `success`, `proven_rejection`,
`timeout_ambiguous`, and `malformed_response`. `execution_fault` additionally
supports the same proven-before-mutation and commit-then-disconnect distinction.
The provider caches outcomes by durable attempt ID, proving replay does not
perform a second invocation.

Artifact-persistence faults belong at the existing `ArtifactActivities`
boundary rather than masquerading as provider responses. The sandbox wraps that
activity with the configured `artifact_fault` while continuing to use the same
durable workflow.

## Diagnostics and secrets

The manifest contains modes, seed, redacted endpoint, file digests, process
version, optional build commit, isolated paths and ports, Gongbu and Temporal
URLs, selected provider target, spend ceiling, and readiness results. Managed
runs also record the exact Hubu version, source commit, artifact checksum,
executor contract, platform target, and public isolated fixture identifiers.
Query strings are removed from endpoints. Operator and Hubu tokens are stored
separately with owner-only permissions. Raw credentials, credential references,
prompts, and provider responses are not serialized to the manifest or safe mock
call log.
