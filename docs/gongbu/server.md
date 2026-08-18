# Persistent local Gongbu server

`gongbu-server` is Gongbu's supported persistent local runtime. It owns the
Gongbu HTTP API, SQLite database, artifact store, Temporal execution worker,
and—only in `managed_local` mode—the configured local Temporal child. Hubu is a
separate service. Start and provision Hubu before Gongbu; stopping Gongbu never
stops, upgrades, provisions, migrates, or otherwise changes Hubu's lifecycle.

## Install and configure

Build the operator binaries:

```sh
cargo build --release --locked --bin gongbu-server --bin gongbu-mcp
```

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

Raw tokens never belong in JSON, command arguments, shell tracing, SQLite, or
Temporal payloads. The config names only opaque macOS Keychain service/account
references. First create each provider credential in Keychain Access as
described in [Local Keychain secrets](local-keychain-secrets.md). Then, with
Hubu already running, bootstrap the remaining references:

```sh
target/release/gongbu-server credentials bootstrap \
  --config /absolute/path/gongbu.json
```

The command uses the production config and provider-catalog parsers, confirms
all enabled provider references resolve, generates the caller-to-Gongbu
capability, and persists it directly through the Keychain API. It discovers the
Hubu executor/service credential in this explicit precedence order:

1. `--hubu-token-file FILE`
2. `HUBU_AUTH_TOKEN`
3. `HUBU_AUTH_TOKEN_FILE`
4. `./hubu.auth-token`
5. `$HUBU_HOME/hubu.auth-token`
6. `~/.hubu/hubu.auth-token`

Before persisting that handoff, setup calls Hubu's protected
`GET /spend/executor/credential-check` endpoint. Public `/health` and `/version`
are compatibility signals only and cannot prove the bearer. Setup reports only
the credential class and discovery source category, never a value, path, or
Keychain reference. Until HUB-32 issues a narrower executor capability, this
handoff necessarily uses Hubu's protected local service bearer; the config name
and endpoint are ready to accept the narrower credential later without changing
spend-authorization request semantics.

The authenticated caller account must exactly equal the configured Hubu account.
Requests contain no account override. Provider endpoints, credentials, target
revisions, prices, spend ceiling, Hubu endpoint, and Hubu agent identity are all
operator-owned and cannot be changed by HTTP or MCP callers.

## Start Hubu independently

Start the already installed Hubu server using its own persistent database and
authentication configuration. Register or select the account and agent, create
the relevant budget/policy, and issue the authorization used by the execution.
Verify Hubu reports the exact `product_version` configured in Gongbu and the
`hubu-spend-executor-v4.1` executor contract. Gongbu additionally verifies its
Hubu executor/service credential on the protected credential-check route before
Temporal or the Gongbu listener starts. The startup policy may exit immediately
or wait for the configured bounded interval; it never repairs Hubu.

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

## Point MCP at the live server

`gongbu-mcp` remains a thin authenticated client and cannot choose an account.
Resolve the same configured capability for the MCP process:

```sh
export GONGBU_MCP_ENDPOINT=http://127.0.0.1:8788
export GONGBU_MCP_BEARER_TOKEN="$(security find-generic-password -s gongbu.caller -a local-mcp -w)"
target/release/gongbu-mcp
```

The server maps that capability to the operator-configured account. See
[MCP usage](mcp.md) for the JSON-RPC tool schemas.

## Submit, poll, retrieve, and replay

With `CAPABILITY` resolved from Keychain, submit an authorization already issued
by the independently managed Hubu service:

```sh
curl -fsS -H "Authorization: Bearer $CAPABILITY" \
  -H 'Content-Type: application/json' \
  -d @execution.json \
  http://127.0.0.1:8788/v1/executions
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

### Credential rotation, rollback, and revocation

Caller and Hubu credentials are loaded once because they become authenticators
and long-lived clients. Gongbu re-resolves both Keychain items while running; a
change or removal closes readiness and stops new admission with a
`restart required` diagnostic. Provider credentials remain separately owned and
are checked at startup and re-resolved at provider preflight.

Rotate the generated caller capability or hand off the currently discoverable
Hubu credential:

```sh
target/release/gongbu-server credentials rotate caller --config /absolute/path/gongbu.json
target/release/gongbu-server credentials rotate hubu --config /absolute/path/gongbu.json
```

Each replacement copies the previous value to an opaque `.rollback` Keychain
account without printing either value. Stop or allow the running process to
close admission, restart Gongbu, and verify authenticated execution admission.
If validation fails, swap the previous value back and restart:

```sh
target/release/gongbu-server credentials rollback caller --config /absolute/path/gongbu.json
target/release/gongbu-server credentials rollback hubu --config /absolute/path/gongbu.json
```

After the new credential is proven and all clients have moved, revoke the
rollback copy:

```sh
target/release/gongbu-server credentials revoke-rollback caller --config /absolute/path/gongbu.json
target/release/gongbu-server credentials revoke-rollback hubu --config /absolute/path/gongbu.json
```

For Hubu rotation, rotate the source used by the Hubu server first, restart Hubu,
then run Gongbu's `rotate hubu`; the protected check rejects a stale or placeholder
candidate before Keychain replacement. Never copy the human reconciliation
capability: it remains only in Hubu operator clients and Gongbu has no config,
header, database column, or Temporal field for it.

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
  missing caller-to-Gongbu, Hubu executor/service, or provider credentials,
  rejected Hubu credentials, unavailable active adapters, incomplete prices,
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
