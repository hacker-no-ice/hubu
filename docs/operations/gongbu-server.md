# Gongbu server operations

`gongbu-server` is the supported persistent local execution-plane runtime. It
owns the Gongbu API, SQLite database, artifact store, Temporal worker, and—in
`managed_local` mode—the local Temporal child. Hubu remains a separately
provisioned service.

## Build and configure

```sh
cargo build --release --locked -p gongbu-api --bin gongbu-server
```

Copy [`examples/gongbu/gongbu.server.json`](../../examples/gongbu/gongbu.server.json)
to an operator-owned absolute path. The file passed to `--config` is the only
runtime configuration source; unknown fields are rejected and changes require
a graceful restart.

All state, catalog, artifact, and managed Temporal paths must be absolute. Raw
tokens and provider keys never belong in JSON. Configuration references
operator-owned Keychain items, and the authenticated caller account must match
the configured Hubu account exactly.

Use `providers: {"mode":"disabled"}` for dependency or configuration work that
must not permit provider traffic. Live mode requires an exact target, complete
pricing, a credential reference, a positive spend ceiling, and the literal
live-spend acknowledgement.

## Temporal mode

In `managed_local` mode, configure an absolute, version-pinned Temporal CLI.
Gongbu runs the local service on loopback, retains its data and log, supervises
the child, and stops only that child during shutdown.

For an external service:

```json
{
  "mode": "external",
  "address": "http://127.0.0.1:7233",
  "namespace": "default",
  "task_queue": "gongbu-local-executions",
  "ui_url": "http://127.0.0.1:8233"
}
```

Gongbu never starts or stops external Temporal. In both modes, readiness
requires an active worker polling the configured task queue.

After startup, a single failed Temporal or Hubu dependency probe does not stop
Gongbu. The supervisor allows a fixed 30-second recovery grace so routine gRPC
connection rotation and other transient transport failures can reconnect. A
healthy sample resets the grace window; continuously unhealthy probes still
remove readiness and shut down the process. The `gongbu_dependency_probe` log
event records only the dependency, outcome, consecutive-failure count, and a
redacted gRPC status code when available.

## Validate and start

Validation creates no state, reads no Keychain secret, and connects to no
dependency:

```sh
target/release/gongbu-server validate-config --config /absolute/path/gongbu.json
```

Start the independently configured Hubu service first, then Gongbu:

```sh
target/release/gongbu-server serve --config /absolute/path/gongbu.json
curl -fsS http://127.0.0.1:8788/livez
curl -fsS http://127.0.0.1:8788/readyz
curl -fsS http://127.0.0.1:8788/version
```

These GET endpoints expose safe status and compatibility metadata only. All
execution and artifact endpoints require the caller capability. A 503 from
`/readyz` means new admission is closed.

## Submit and inspect

The version-2 execution request supplies only the authorization token and
execution intent selected from operator configuration:

```json
{
  "schema_version": 2,
  "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
  "input": {"prompt": "A small blue circle", "image_count": 1},
  "input_schema_version": 1,
  "workload_type": "image_generation",
  "provider": "google",
  "adapter": "gemini_image",
  "model": "OPERATOR_APPROVED_MODEL"
}
```

```sh
curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  -H 'Content-Type: application/json' \
  -d @execution.json \
  http://127.0.0.1:8788/v2/executions

curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  http://127.0.0.1:8788/v1/executions/EXECUTION_ID

curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  http://127.0.0.1:8788/v1/executions/EXECUTION_ID/artifacts
```

Submitting an identical body replays the same execution. A changed immutable
field conflicts. On restart, Gongbu resubmits nonterminal executions to their
stable workflow IDs and never creates a second provider or financial side
effect merely to recover scheduling.

Agents normally use these routes through [Unified MCP](../unified-mcp.md).

## Shutdown and backup

Send SIGINT or SIGTERM. Gongbu removes readiness, stops accepting requests,
drains its worker within a bound, and stops only a managed Temporal child. Hubu
and external Temporal remain untouched.

For a consistent cold backup, stop Gongbu and copy its SQLite database,
artifact root, and managed Temporal data directory together. Do not restore
only one part of that set.

When billing is ambiguous, preserve the execution and provider evidence and use
the authenticated reconciliation path. Do not create a new operation key,
blindly repeat provider work, or release the Hubu claim.
