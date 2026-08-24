# Gongbu sandbox

`gongbu-sandbox` is a disposable manual-test environment for the Gongbu API,
Temporal worker, artifacts, and independently selectable Hubu and provider
boundaries. The persistent server remains the supported operator runtime.

## Install and start

Install the Temporal CLI, then install the sandbox from the workspace root:

```sh
cargo install --locked --force \
  --path crates/gongbu-api \
  --bin gongbu-sandbox
```

Start the deterministic configuration:

```sh
gongbu-sandbox start \
  --config examples/gongbu/sandbox.mock.json \
  --hubu-mode mock \
  --provider-mode mock
```

The command prints an isolated run directory, Gongbu URL, Temporal UI URL, and
a submit command, then remains attached until Ctrl+C. Each run receives its own
loopback ports, token, database, artifact root, Temporal state, and logs.

State is deleted after ordinary shutdown unless `--preserve` names a new,
absolute destination that does not already exist.

## Boundary matrix

| Hubu | Provider | Purpose |
| --- | --- | --- |
| Mock | Mock | Deterministic development and ordinary CI |
| Mock | Real | Live provider with deterministic Hubu accounting |
| Real | Mock | Real Hubu with deterministic provider output |
| Real | Real | Explicitly guarded release dogfood |

Real Hubu mode requires an exact immutable release and verifies published
checksums, provenance, product/source identity, and
`hubu-spend-executor-v4.3`. Mutable release aliases are rejected.

Real provider mode is potentially billable. It requires an operator-owned
credential reference, frozen pricing, a positive minor-unit ceiling, and the
literal live-spend acknowledgement. Never put the acknowledgement or provider
credentials in ordinary CI.

Configuration precedence is JSON profile, then environment, then CLI. Prefer
CLI mode overrides for manual work so potentially live selections remain
visible in shell history and do not persist into future invocations.

## Submit and inspect

For the mock/mock flow:

```sh
gongbu-sandbox submit \
  --run-dir "$RUN_DIR" \
  --operation-key mock-mock-1 \
  --prompt "Draw a blue circle"
```

Then inspect execution, artifacts, logs, and boundary summaries:

```sh
gongbu-sandbox status \
  --run-dir "$RUN_DIR" \
  --execution-id "$EXECUTION_ID"

gongbu-sandbox artifacts \
  --run-dir "$RUN_DIR" \
  --execution-id "$EXECUTION_ID" \
  --download-dir /tmp/gongbu-artifacts

gongbu-sandbox inspect --run-dir "$RUN_DIR"
```

The Temporal history should show preflight, claim, validation, provider work,
artifact persistence, and settlement in order. Proven non-billable failures
release authorization. Ambiguous outcomes enter reconciliation.

Replay the identical submit command with the same operation key and immutable
inputs. It must return the same execution ID without another provider or Hubu
financial mutation. If a real-provider request times out, do not submit a new
operation key because the provider may already have accepted a billable call.

Use `gongbu-sandbox --help` and subcommand help for the current option surface.
The implementation's mode checks remain authoritative over copied examples.
