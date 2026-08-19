# Unified MCP migration

`hubu-unified-mcp` is the default agent-facing surface. It gives an MCP client
one stdio server while preserving two independently operated HTTP backends:
`hubu-server` owns governance and `gongbu-server` owns provider execution and
artifacts. The router has separate endpoints, bearer credentials, health
probes, and failure handling for each backend.

The standalone `hubu-mcp-server` and `gongbu-mcp` binaries remain operational
during the compatibility window. They are opt-in rollback surfaces, not the
recommended setup, and this migration does not stop, combine, or migrate either
backend process or database.

## Compatibility requirements

Use all runtime binaries from one release archive. The unified router requires
each configured backend to report the same product version and exact
40-character source commit as the router, plus
`hubu-spend-executor-v4.2`. Gongbu must also report API schema 2, MCP schema 2,
and MCP protocol `2024-11-05`. An unstamped source build reports an unknown
commit and intentionally fails the compatibility check; use a release archive
for an operator migration.

Keep the runtime boundaries unchanged:

- run `hubu-server` and `gongbu-server` as separate supervised processes;
- retain their separate SQLite files, configuration, credentials, and backup
  procedures;
- leave provider credentials, calls, retries, Temporal state, and artifact bytes
  in Gongbu;
- give the unified router a Hubu bearer credential and a distinct Gongbu caller
  capability. Never reuse or swap them.

## 1. Prepare the new surface

Download one target-specific release archive, verify both checksum layers, and
install `hubu`, `hubu-server`, `hubu-unified-mcp`, and `gongbu-server` from that
archive as described in [the release runbook](releases.md). Keep
`hubu-mcp-server` and `gongbu-mcp` from the same archive available until rollback
has been tested and the compatibility window closes.

Start Hubu with its existing database and capability files. Start Gongbu with
its existing configuration, database, artifact root, provider credentials, and
Temporal ownership. Confirm their public status endpoints before changing the
MCP client:

```sh
curl -fsS http://127.0.0.1:8787/health
curl -fsS http://127.0.0.1:8787/version
curl -fsS http://127.0.0.1:8788/livez
curl -fsS http://127.0.0.1:8788/readyz
curl -fsS http://127.0.0.1:8788/version
```

Place the Gongbu caller capability in an operator-owned file readable only by
the account that launches the MCP client. The value must be the capability
accepted by the already configured Gongbu server; it is not a Hubu token or a
provider credential.

```sh
chmod 600 ~/.hubu/gongbu.mcp-token
```

## 2. Preview and migrate Codex

Back up `~/.codex/config.toml`, then preview the managed unified block without
writing files:

```sh
cp -p ~/.codex/config.toml ~/.codex/config.toml.pre-unified-mcp
```

```sh
hubu init codex \
  --dry-run \
  --token-file ~/.hubu/hubu.auth-token \
  --reconciliation-token-file ~/.hubu/hubu.reconciliation-token \
  --gongbu-endpoint http://127.0.0.1:8788 \
  --gongbu-token-file ~/.hubu/gongbu.mcp-token
```

If the current config contains both `[mcp_servers.hubu]` and
`[mcp_servers.gongbu]`, replace them atomically with the one managed
`[mcp_servers.hubu]` entry:

```sh
hubu init codex \
  --migrate-standalone \
  --token-file ~/.hubu/hubu.auth-token \
  --reconciliation-token-file ~/.hubu/hubu.reconciliation-token \
  --gongbu-endpoint http://127.0.0.1:8788 \
  --gongbu-token-file ~/.hubu/gongbu.mcp-token
```

Migration refuses to modify the config when it finds a standalone Gongbu entry
without both replacement Gongbu options. It removes only the `hubu` and
`gongbu` MCP table families and preserves unrelated MCP servers and client
settings. Do not use `--force` as a substitute for
`--migrate-standalone`; `--force` only replaces an unmanaged Hubu table.

For a fresh Codex install with no standalone Gongbu entry, omit
`--migrate-standalone`. The same command writes the unified entry directly:

```sh
hubu init codex \
  --token-file ~/.hubu/hubu.auth-token \
  --reconciliation-token-file ~/.hubu/hubu.reconciliation-token \
  --gongbu-endpoint http://127.0.0.1:8788 \
  --gongbu-token-file ~/.hubu/gongbu.mcp-token
```

Add `--trust-client-approval` only when the MCP client is trusted to show a
human approval prompt for protected Hubu setup and administration tools. It
does not weaken Hubu policy or replace durable spend approval.

Restart Codex after the configuration changes. Codex launches
`hubu-unified-mcp`; operators continue to launch the two backend servers.

## 3. Validate discovery and health

Ask the client to list MCP tools and call `hubu_unified_capabilities`. A fully
ready installation reports:

- `contract_version: "hubu-gongbu-mcp-v1"` and `routing_revision: 1`;
- `backends.hubu.state: "available"`;
- `backends.gongbu.state: "available"`;
- 33 capability entries: one router tool, 28 Hubu tools, and four Gongbu tools.

Interpret backend states independently:

| State | Meaning and operator action |
| --- | --- |
| `available` | Health and every required compatibility field match; the backend's eligible tools are callable. |
| `degraded` | Gongbu is live and compatible but not ready. Read and artifact tools remain available; new execution admission is blocked. Repair Gongbu readiness. |
| `unavailable` | A required health, liveness, or version probe failed. Check only the named backend process and its local route. |
| `incompatible` | Version, source commit, executor contract, MCP protocol, or schema metadata mismatched. Install all binaries from the same verified archive; do not bypass the check. |
| `unconfigured` | The endpoint/credential pair is absent or incomplete. Supply both values for that backend and restart the MCP client. |

The fixed `reason_code` identifies the failed check without exposing endpoints,
credentials, paths, or raw backend errors. `hubu_health` remains the Hubu-only
health tool; use `hubu_unified_capabilities` for the cross-backend view.

After the MCP initialize handshake completes, the client sends
`notifications/initialized`. The router then records the current callable
catalog without emitting an initial notification and begins background probes.
Each later catalog change produces exactly one payload-free
`notifications/tools/list_changed` notification. The client should respond by
requesting `tools/list`; it should not infer backend details from the
notification itself. An out-of-order lifecycle notification is ignored.

The normal probe interval is 250 ms. Deterministic canaries may set
`HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS` to an integer from 10 through 60000;
production operators should keep the default unless diagnosing lifecycle
timing. Polling never retries or replays a routed mutation.

Partial availability is intentional. An unhealthy or unconfigured backend does
not hide the router-owned capability tool or compatible tools owned by the
other backend. `gongbu_create_execution` additionally requires Hubu to be
available because execution consumes Hubu authorization. The router never
falls back to another backend, queues a call, or retries an ambiguous mutation.

Complete a non-billable smoke before normal use: list tools, read Hubu health,
and read an existing Gongbu execution or artifact if one is available. Do not
use provider execution as a migration probe.

For a restart/recovery smoke, stop one backend, wait for one list-changed
notification, and confirm `tools/list` removes only the affected catalog. Call
a read on the healthy backend, restart the stopped backend, then require one
more notification and the restored catalog. Repeated notifications without an
effective catalog change are a failure. Never put endpoint, credential, path,
or raw error data into captured notification fixtures.

## Roll back

Rollback changes only the MCP client configuration. It does not roll back or
copy backend state.

1. Stop new agent work and preserve the exact error and capability snapshot.
2. Restore the backed-up two-entry client configuration, or generate the Hubu
   compatibility entry with:

   ```sh
   hubu init codex \
     --compatibility-standalone \
     --token-file ~/.hubu/hubu.auth-token \
     --reconciliation-token-file ~/.hubu/hubu.reconciliation-token
   ```

3. Retain or restore the separately configured `[mcp_servers.gongbu]` entry
   using `gongbu-mcp`, `GONGBU_MCP_ENDPOINT`, and
   `GONGBU_MCP_BEARER_TOKEN` from the pre-migration configuration.
4. Restart Codex and verify both standalone servers initialize and list their
   existing tool catalogs.

Use the standalone binaries from the same pinned release as the running
backends. If the regression is in the release rather than only the unified
router, roll every consumer binary back to one older validated release tag and
checksum according to [the release rollback procedure](releases.md); never mix
product versions or source commits.

## Owner-approved zero-user cutover

On 2026-08-18 (America/Los_Angeles), the product owner confirmed that there are
no active or supported consumers of the standalone `hubu-mcp-server` or
`gongbu-mcp` agent surfaces. This dated decision replaces the former
general-purpose two-canary/14-day pre-deprecation window and 90-day/two-stable-
release post-deprecation window for this cutover only. The former windows remain
historical context for the unchanged
[HUB-96 NO-GO record](canaries/HUB-96-unified-mcp-migration-canary.md).

The unified entry remains the documented default, but standalone configuration
is not yet deprecated. The sequence is:

1. Merge HUB-106, HUB-107, and the HUB-108 policy amendment to `main`.
2. From that exact final `main` commit, publish one fresh immutable packaged
   canary. Do not reuse the HUB-96 local package or an earlier release.
3. Record the complete catalog, golden behavior parity,
   `notifications/tools/list_changed`, failure-isolation, redaction,
   migration, rollback, workspace/release-policy, documentation, and
   stale-reference evidence required by
   [the contract matrix](unified-mcp-contract.md#fresh-packaged-canary-evidence-matrix).
4. Confirm zero unresolved P0/P1 findings and record an explicit GO tied to the
   immutable release URL and exact source SHA. Any failed row is a NO-GO and
   keeps HUB-97 blocked.
5. After GO, execute HUB-97. Then, and only then, HUB-98 may execute without an
   elapsed-time or stable-release-count wait. Retain one immutable packaged
   rollback artifact through HUB-98 removal verification.

HUB-98 still requires zero unresolved P0/P1 findings plus successful workspace,
document-link, installer/example, architecture-responsibility, and retired-
repository stale-reference verification. Do not remove standalone binaries,
change runtime routing, or delete saved configuration before the applicable
HUB-97/HUB-98 step. Deprecation and removal affect only the standalone
agent-facing adapters: Hubu and Gongbu retain separate processes, credentials,
storage, provider execution, artifacts, and failure domains.
