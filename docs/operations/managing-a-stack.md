# Managing a local stack

Use this page after the [local stack quick start](../local-stack.md) for
day-to-day operation of a managed profile: inspecting it, reading logs,
stopping it, applying configuration changes, and rolling back. For
configuration fields and design choices, use the
[schema-version-1 configuration reference](https://hubustack.dev/configuration/local-stack/v1/).

## Routine operations

```sh
# List profiles and show machine-readable status.
hubu stack profiles
hubu stack status --json

# Read launcher-owned logs.
hubu stack logs --component all --lines 200
hubu stack logs --component gongbu --execution-id EXECUTION_ID

# Gracefully stop the complete managed stack.
hubu stack stop
```

An explicit `--profile "$profile"` overrides the saved selection for any one
stack command.

There is no `hubu stack restart` command. For an unchanged unhealthy or partial
managed stack, run `hubu stack stop`, then `hubu stack start`.

`stack start` runs doctor and render when needed. For a fully managed profile,
it starts the final Hubu process, completes Gongbu's managed credential
bootstrap, and starts Gongbu and its managed Temporal runtime. The client-owned
`hubu-unified-mcp` process is not part of the managed stack.

## Keep sandbox and live profiles separate

Keep sandbox and live modes in separate profile directories. That isolates
credentials, spend acknowledgement, generated state, databases, artifacts, and
logs while making mode changes an explicit `hubu stack select` operation. See
the [complete examples](https://hubustack.dev/configuration/local-stack/v1/examples#keep-sandbox-and-live-profiles-separate)
for the recommended layout and switching flow.

Local-stack mode requires at least one approved real target and its opaque
credential reference. Inspect the sanitized provider catalog with
`hubu stack catalog --json`; catalog, doctor, and render never call a provider
or claim live qualification. Live provider execution can incur charges; start
with [live provider operations](live-providers.md).

## Apply a configuration change

`hubu stack render` turns the profile's TOML sources into a *generation*: an
identified set of rendered runtime files. Source edits never silently replace
the active generation. Render and review a changed profile before activating
it:

```sh
hubu stack doctor
hubu stack render
# Review the generation ID, changed files, and affected components.
hubu stack stop
hubu stack activate --generation GENERATION_ID
hubu stack start
hubu stack status
```

If the rendered plan reports `hubu-unified-mcp-client-config` as affected,
rerun `hubu init codex --stack-profile "$profile"` after the stack is ready and
restart Codex.

The [active-profile change guide](https://hubustack.dev/configuration/local-stack/v1/decisions#changing-an-active-profile)
explains staging, credential-reference changes, and rollback requirements.

## Roll back

First restore the exact operator-owned TOML and compatible binaries for the
retained generation, then run:

```sh
hubu stack generations
hubu stack render
hubu stack stop
hubu stack rollback --generation PRIOR_GENERATION_ID
hubu stack start
hubu stack status
```

## Doctor reports

Doctor is read-only. `hubu stack doctor --json` emits the version-2 report
contract, which adds the provider contract catalog and keeps configured,
credential-reference-present, production-validated, and live-qualified state
independent.

## How CLI commands find the running stack

A running profile has an active *handoff*: the generated Hubu endpoint and
the authentication, approval, and reconciliation credential file paths that
clients use to reach it.

Once an active handoff exists, normal server-bound `hubu` commands take the
Hubu endpoint and those credential file paths from the selected profile as one
bundle. If there is no explicit selection, an active conventional `default`
profile is used. Ambient `HUBU_URL` and token variables are ignored in either
case. If a selected profile has no valid active handoff, the command fails
instead of silently using another server. Pass an explicit global `--url` to
opt into manual mode, where the existing environment and token-file precedence
remains available. Local-only commands such as profile inspection and policy
file creation do not require an active handoff.

## Terminal color and automation

Human-readable CLI output uses semantic color and emphasis when its destination
is an interactive terminal. Status words remain present: green highlights
ready or successful state, yellow highlights warnings or required action, red
highlights failures, and dimmed text identifies inactive or secondary details.
Color is not the only signal.

The global option `--color auto|always|never` controls rendering and may appear
before or after the command. `auto` is the default, disables color for pipes and
redirects, and also disables color when `TERM=dumb`. A non-empty `NO_COLOR`
environment variable disables automatic color. An explicit `--color always` or
`--color never` takes precedence over the environment.

Machine-readable and raw data paths bypass terminal styling. In particular,
all `--json` reports, version and protocol JSON, exported policy content,
client-configuration dry runs, and individual `stack logs` payload lines remain
ANSI-free even when `--color always` is selected. Hubu-owned log section headers
may still use terminal styling without changing the stored log lines.

## Managed logs

Managed Hubu omits routine request events for successful `GET /health`,
`GET /version`, and Gongbu's marked
`GET /agents?operational_probe=gongbu_credential_check` readiness probe.
Unmarked agent-list reads and failed probes remain logged. The marker changes
logging only; it conveys no caller identity or authorization. Structured logs
remain bounded to one 10 MiB active file and four retained generations.

## Release lineage and signing

All four binaries must share one non-`unknown` full source commit and executor
contract. Release-stamped source installations use the normal production
lineage checks; do not enable `allow_development_builds`. The locally compiled
executables are not Developer ID-signed, Apple-notarized, or Apple-verified.
See [release operations](releases.md#local-build-trust-model-and-cost) for the
trust model.
