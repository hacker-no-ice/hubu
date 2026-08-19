# Persistent local Gongbu server

`gongbu-server` is Gongbu's supported persistent local runtime. It owns the
Gongbu HTTP API, SQLite database, artifact store, Temporal execution worker,
and—only in `managed_local` mode—the configured local Temporal child. Hubu is a
separate service. Start and provision Hubu before Gongbu; stopping Gongbu never
stops, upgrades, provisions, migrates, or otherwise changes Hubu's lifecycle.

## Install and configure

Build the execution-plane operator binary:

```sh
cargo build --release --locked --bin gongbu-server
```

The default agent-facing `hubu-unified-mcp` binary is distributed with the
shared release. Build `gongbu-mcp` only for the explicit standalone
compatibility path.

Copy [the complete configuration example][server-example] to an operator-owned
absolute path. Every field is required unless marked optional by the example.
The schema rejects unknown fields. All state and catalog paths and the managed
Temporal binary must be absolute; state survives ordinary shutdown and restart.
HTTP and external dependencies are loopback-only unless a Hubu host is explicitly
allowlisted.

[server-example]: ../../examples/gongbu/gongbu.server.json

The configuration has one source and one precedence rule: the file passed to
`--config` is authoritative. `gongbu-server` does not apply environment or CLI
field overrides and does not reload the file. Any change requires a graceful
restart. Build provenance environment variables are compile-time metadata only,
and MCP client variables configure only `gongbu-mcp`.

Raw tokens never belong in the JSON. Store three kinds of credentials in macOS
Keychain: the scoped Hubu bearer, the caller capability, and every provider
credential referenced by the provider target catalog. For example:

```sh
security add-generic-password -U -s gongbu.hubu -a local -w 'HUBU_SCOPED_BEARER'
security add-generic-password -U -s gongbu.caller -a local-mcp -w 'LONG_RANDOM_CALLER_CAPABILITY'
security add-generic-password -U -s gongbu.google -a local -w 'PROVIDER_CREDENTIAL'
```

The authenticated caller account must exactly equal the configured Hubu account.
Requests contain no account override. Provider endpoints, credentials, target
revisions, prices, spend ceiling, Hubu endpoint, and Hubu agent identity are all
operator-owned and cannot be changed by HTTP or MCP callers.

## Start Hubu independently

Start the already installed Hubu server using its own persistent database and
authentication configuration. Register or select the account and agent, create
the relevant budget/policy, and issue the authorization used by the execution.
Verify Hubu reports the exact `product_version` configured in Gongbu and the
`hubu-spend-executor-v4.2` executor contract. The Gongbu startup policy may exit
immediately or wait for the configured bounded interval; it never repairs Hubu.

## Choose Temporal ownership

For `managed_local`, pin an existing Temporal CLI by absolute path and exact
version. Gongbu runs `temporal server start-dev` on the configured loopback ports,
stores its SQLite state and log below `data_path`, supervises that child, and
stops only that child during Gongbu shutdown. It never deletes the Temporal data
directory.

For an independently operated Temporal service, replace the `temporal` object:

```json
{
  "mode": "external",
  "address": "http://127.0.0.1:7233",
  "namespace": "default",
  "task_queue": "gongbu-local-executions",
  "ui_url": "http://127.0.0.1:8233"
}
```

Gongbu connects without lifecycle ownership. It never stops external Temporal.
In either mode, the execution workflow and activities are registered and an
active workflow poller must be visible on the configured task queue before HTTP
acceptance begins. Losing the poller, Temporal, or Hubu compatibility removes
readiness, stops new admission, and gracefully closes the server.

## Start and inspect Gongbu

```sh
target/release/gongbu-server serve --config /absolute/path/gongbu.json
curl -fsS http://127.0.0.1:8788/livez
curl -fsS http://127.0.0.1:8788/readyz
curl -fsS http://127.0.0.1:8788/version
```

These three GET surfaces are unauthenticated and deliberately safe. They expose
only status plus product/build/schema/contract identifiers—never endpoints,
credential references, paths, provider configuration, or account identity. All
Execution and Artifact endpoints require the caller capability. A 503 from
`/readyz` or execution creation means admission is closed.

## Point the unified MCP surface at the live server

Give `hubu-unified-mcp` the same configured caller capability. The router keeps
it separate from the Hubu bearer credential and cannot choose an account:

```sh
export HUBU_UNIFIED_GONGBU_ENDPOINT=http://127.0.0.1:8788
export HUBU_UNIFIED_GONGBU_BEARER_TOKEN="$(security find-generic-password -s gongbu.caller -a local-mcp -w)"
hubu-unified-mcp
```

The server maps that capability to the operator-configured account. See
[the unified MCP migration guide](../unified-mcp-migration.md) for the default
client configuration. See [standalone MCP compatibility](mcp.md) only when
validating or using the opt-in rollback surface.

## Submit, poll, retrieve, and replay

With `CAPABILITY` resolved from Keychain, submit an authorization already issued
by the independently managed Hubu service:

```json
{
  "schema_version": 2,
  "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
  "input": {"prompt": "A small blue circle", "image_count": 1},
  "input_schema_version": 1,
  "workload_type": "image_generation",
  "provider": "google",
  "adapter": "gemini_image",
  "model": "gemini-2.5-flash-image"
}
```

Gongbu resolves the authoritative account, operation identity, amount, currency,
scope, workload profile, task correlation, reason, and expiry from Hubu. It
derives price and scope again from operator configuration and persists only
after exact agreement; the durable workflow claims afterward.

```sh
curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  -H 'Content-Type: application/json' \
  -d @execution.json \
  http://127.0.0.1:8788/v2/executions
curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  http://127.0.0.1:8788/v1/executions/EXECUTION_ID
curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  http://127.0.0.1:8788/v1/executions/EXECUTION_ID/artifacts
curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  -o artifact.bin http://127.0.0.1:8788/v1/artifacts/ARTIFACT_ID
```

Submit the exact same body again to replay the tuple. Gongbu returns the same
execution identity; stable database uniqueness, ProviderAttempt identity, Hubu
operation key, and Temporal workflow ID prevent a second provider or financial
side effect. A changed immutable field returns a conflict. Inspect the claim and
settlement with the normal Hubu CLI/API; Gongbu does not provide a bypass.

On restart, Gongbu opens the same database and artifact root, reconnects to the
same Temporal namespace and queue, and resubmits every nonterminal execution to
its stable workflow ID using `UseExisting`. This closes the database-commit /
workflow-schedule crash window without duplicating a live workflow.

## Safe shutdown, restart, and backup

Send SIGINT or SIGTERM. Gongbu removes readiness, stops accepting requests,
gracefully shuts down and bounds its worker drain, then stops only a managed
Temporal child. It retains the database, artifacts, Temporal state, and logs.
Hubu is untouched.

For a consistent cold backup, stop Gongbu gracefully, then copy the SQLite
database, artifact root, and managed Temporal data directory together. Restart
with the unchanged config. Do not restore only one member of that state set.

## Guarded real Gemini smoke

Ordinary CI and deterministic tests must use no external provider spend. A real
Gemini smoke requires all of the following: a Keychain credential referenced by
the selected active Gemini target, a frozen pricing rule for that exact target,
an existing Hubu authorization, a positive `maximum_spend_minor`, and the literal
`I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND` in the server configuration. Start local Hubu
independently, start Gongbu, submit one request at or below both the configured
ceiling and Hubu authorization, retrieve the artifact, and inspect the bounded
settlement in Hubu. Never place this acknowledgement in CI configuration.

## Troubleshooting

- Startup rejects relative paths, unknown JSON keys, mock/fixture targets,
  missing Keychain items, unavailable active adapters, incomplete prices,
  account mismatches, unsafe hosts, or incompatible Hubu/Temporal versions.
- If `/readyz` becomes unavailable, inspect the managed Temporal log under its
  persistent data directory or the independently managed external service, then
  verify Hubu `/health` and `/version`. Gongbu does not restart either external
  dependency and does not restart a failed managed Temporal child.
- If an outcome is ambiguous, use the authenticated reconciliation API after
  operator inspection. Gongbu preserves evidence and never blindly repeats
  provider work, settlement, or release.
- Safe errors and metadata omit filesystem paths, endpoint values, credentials,
  and credential references. Do not add shell tracing around Keychain commands.
