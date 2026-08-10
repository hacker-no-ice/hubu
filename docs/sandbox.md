# Gongbu sandbox

The sandbox is a long-running manual-test environment. It starts an isolated
Gongbu API, a Temporal worker, and (by default) a local Temporal development
server and UI. Hubu and provider boundaries are selected independently, so the
same operator workflow can exercise deterministic mocks or guarded real
traffic.

Install the Temporal CLI before the first managed run (`brew install temporal`
on macOS). In terminal 1, start the sandbox:

```sh
cargo run -p gongbu-api --bin gongbu-sandbox -- start \
  --config examples/sandbox.mock.json \
  --hubu-mode mock \
  --provider-mode mock
```

Wait for every readiness entry to report `ready: true`. The command prints the
run directory, Gongbu URL, Temporal UI URL, and a ready-to-copy submit command,
then stays running until Ctrl+C. Keep terminal 1 open.

Each run gets an operator token, SQLite database, artifact root, Temporal data,
logs, and three loopback-only ports. State is deleted after shutdown unless
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
cargo run -p gongbu-api --bin gongbu-sandbox -- start \
  --config examples/sandbox.mock.json \
  --hubu-mode mock \
  --provider-mode mock
```

The available CLI overrides are `--hubu-mode`, `--provider-mode`,
`--max-spend-minor`, and `--live-spend-ack`.

Environment variables remain useful for CI and automation:

- `GONGBU_SANDBOX_HUBU_MODE` (`mock` or `real`)
- `GONGBU_SANDBOX_PROVIDER_MODE` (`mock` or `real`)
- `GONGBU_SANDBOX_MAX_SPEND_MINOR`
- `GONGBU_SANDBOX_LIVE_SPEND_ACK`

Unknown JSON fields, unknown mode values, and mock modes combined with real
endpoint or credential fields fail at startup. A `production` profile rejects
either mock boundary.

## Manual execution workflow

Copy the temporary run directory from terminal 1 (the path ending in
`gongbu-sandbox-...`). In terminal 2, submit a real API request. `submit` waits
for a terminal state and prints both the execution ID and its Temporal UI URL:

```sh
cargo run -p gongbu-api --bin gongbu-sandbox -- submit \
  --run-dir /tmp/gongbu-sandbox-EXAMPLE \
  --operation-key manual-1 \
  --prompt "Draw a blue circle"
```

Open the printed Temporal URL to inspect workflow and activity history. Query
the aggregate, list and download artifacts, and inspect safe mock-side-effect
summaries with:

```sh
cargo run -p gongbu-api --bin gongbu-sandbox -- status \
  --run-dir /tmp/gongbu-sandbox-EXAMPLE \
  --execution-id EXECUTION_ID

cargo run -p gongbu-api --bin gongbu-sandbox -- artifacts \
  --run-dir /tmp/gongbu-sandbox-EXAMPLE \
  --execution-id EXECUTION_ID \
  --download-dir /tmp/gongbu-artifacts

cargo run -p gongbu-api --bin gongbu-sandbox -- inspect \
  --run-dir /tmp/gongbu-sandbox-EXAMPLE
```

Run `submit` again with the same operation key and prompt to validate replay.
It must return the same execution ID; in mock mode, `inspect` must still report
one provider invocation and no duplicate financial mutation.

Press Ctrl+C in terminal 1 when finished. The sandbox supervises and stops its
managed Temporal child. Add `--preserve /tmp/gongbu-sandbox-debug` to `start`
when you want to retain the database, artifacts, Temporal data, logs, manifest,
and mock-side-effect summary after shutdown or startup failure.

Automated tests are complementary: they validate the scenario matrix and
failure invariants without requiring a manually running sandbox:

```sh
cargo test -p gongbu-api sandbox::
```


## Mode matrix

| Hubu | Provider | Intended use |
| --- | --- | --- |
| Mock | Mock | deterministic local development and ordinary CI |
| Mock | Real | real provider traffic with deterministic Hubu accounting |
| Real | Mock | real Hubu traffic with deterministic provider output |
| Real | Real | guarded release dogfood |

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

## Real-boundary gates

Real Hubu requires an explicit `http://` endpoint. The host must be loopback or
appear exactly in `allowlisted_hosts`; it also requires an opaque scoped
credential reference and an isolated non-production account name. Credentials
are references only and never appear in the run manifest.

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
URLs, selected provider target, spend ceiling, and readiness results. Query
strings are removed from endpoints. The operator token is stored separately
with owner-only permissions. Raw credentials, credential references, prompts,
account identifiers, and provider responses are not serialized to the manifest
or safe mock call log.
