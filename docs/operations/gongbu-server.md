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
a graceful restart. Replace `REPLACE_WITH_EXACT_HUBU_PRODUCT_VERSION` with the
exact `product_version` reported by the selected `hubu-server --version`,
including any leading `v` and prerelease suffix. Hubu and Gongbu binaries must
come from one compatible, release-stamped lineage. The exact-tag source
installer is the recommended initial-user macOS path.

All state, catalog, artifact, and managed Temporal paths must be absolute. Raw
tokens and provider keys never belong in JSON. Provider and explicit service
overrides use Gongbu-owned secret coordinates. A launcher-managed stack instead
uses fixed internal service references backed by private profile state; those
locations are derived and never become operator JSON or TOML input. Schema
version 3 selects no execution account or agent: the authenticated caller
capability identifies the installation/service and contains no execution
identity claim.

`gongbu-server credentials bootstrap-managed` is a launcher-internal interface,
not a human setup command. After the final Hubu process is ready, it reads the
Hubu capability from a file descriptor-safe private path, proves that capability
against a protected Hubu route, and creates or reuses Gongbu's caller and Hubu
handoff state with private permissions. Repeated calls are idempotent, conflicts
fail closed, and output is categorical; secret values never enter arguments,
configuration, output, or logs.

Use `providers: {"mode":"disabled"}` for dependency or configuration work that
must not permit provider traffic. Live mode requires an exact target, complete
pricing, a credential reference, a positive spend ceiling, and the literal
live-spend acknowledgement.

## Precise-cost database upgrade

The HUB-33 upgrade runs when Gongbu opens its SQLite repository. It converts
legacy v4.3 provider-attempt and receipt minor-unit amounts to exact integer
amounts with decimal scale 2 and the already stored currency. Existing frozen
execution pricing JSON, execution IDs, provider-attempt IDs, receipt IDs, and
Hubu settlement IDs remain unchanged. A migrated legacy receipt records the
same retry payload the old binary sent: `provider_request_id` is its receipt ID,
and `price_model_snapshot` is the old reduced provider/model/estimated-price
projection reconstructed from the frozen execution snapshot. New receipts keep
the provider-reported reference and complete frozen snapshot. Reopening the
database is idempotent and must not rewrite an already precise receipt.

Hubu performs a separate migration in the Hubu database. Do not copy either
database over the other, point one process at the other process's state, or
expect one migration to repair both stores. Before upgrading, stop the stack
and take independent cold backups of Hubu state and Gongbu state. An older
binary that knows only the legacy columns must use its matching pre-upgrade
backup; do not use an upgraded database as a rollback mechanism.

The executor contract remains v4.3 and accepts the legacy cents receipt shape.
New precise receipts carry exact amount, scale, currency, and the complete
frozen pricing snapshot. If an older Hubu rejects that additive shape after a
provider charge, Gongbu retains the receipt and enters reconciliation. Upgrade
the installation before resolving the charge; never retry the provider or
discard precision to force settlement.

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
Gongbu. Hubu probes include protected access with the credential Gongbu loaded,
not just public health and version. The supervisor allows a fixed 30-second
recovery grace so routine gRPC connection rotation and other transient
transport failures can reconnect.
Readiness and new execution admission are withdrawn on the first failed sample
and restored only after every dependency is healthy. A healthy sample resets
its grace window; continuously unhealthy probes still shut down the process.
Runtime probe intervals are capped at the grace duration so that shutdown is
re-evaluated within the documented bound. The `gongbu_dependency_probe` log
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

The same installation caller can access a known execution and its artifacts
regardless of which of the owner's agents Hubu attributed it to. This is not an
owner-wide browsing API—clients must already know the execution or artifact
ID—and it is not strong multi-user or per-agent isolation.

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

For a new token, Hubu's resolved spend authorization is authoritative for the
persisted execution account and agent. Submitting an identical body first
replays the persisted token locally and does not ask Hubu to resolve a token
that may already be claimed or settled. A changed immutable field conflicts.
Settlement and release use the persisted execution agent. On restart, Gongbu
resubmits nonterminal executions to their
stable workflow IDs and never creates a second provider or financial side
effect merely to recover scheduling.

Agents normally use these routes through [Unified MCP](../unified-mcp.md).

### Diagnose rejected admission

An HTTP 400 response may add one safe diagnostic to the existing
`invalid_request` error: `target_not_selectable` names the four target fields,
while `pricing_selector_not_matched` names `input.image_size`. Gongbu reports
field paths only and never echoes their submitted values.

The first occurrence of each allowlisted route-version/reason pair in a Gongbu
process writes one structured line to stderr (and thus to a launcher-managed
Gongbu process log when stderr is captured):

```json
{"event":"gongbu_admission_rejected","route":"create_execution","route_version":2,"status":400,"code":"invalid_request","reason_code":"pricing_selector_not_matched","fields":["input.image_size"]}
```

No event is written for generic, malformed, or unknown diagnostics. The event
never contains request bodies, values, identifiers, target values, or raw
errors, so use the returned field paths to compare the request with the active
operator configuration rather than expecting the log to reproduce either.
Managed Gongbu logs append across process starts, so an event may predate the
current process. Admission rejections have no execution ID; inspect them with
`hubu stack logs --component gongbu --lines 100`, without `--execution-id`.

## Shutdown and backup

Send SIGINT or SIGTERM. Gongbu removes readiness, stops accepting requests,
drains its worker within a bound, and stops only a managed Temporal child. Hubu
and external Temporal remain untouched.

For a consistent cold backup, stop Gongbu and copy its SQLite database,
artifact root, managed bootstrap credential state, and managed Temporal data
directory together. Do not restore only one part of that set. Treat the backup
as secret material and preserve private access controls.

When billing is ambiguous, preserve the execution and provider evidence and use
the authenticated reconciliation path. Do not create a new operation key,
blindly repeat provider work, or release the Hubu claim.
