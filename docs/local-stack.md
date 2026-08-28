# Local stack quick start

Use this guide to initialize, start, inspect, and connect a local Hubu stack.
For configuration fields and design choices, use the public
[schema-version-1 configuration reference](https://hubustack.dev/configuration/local-stack/v1/).

## Check the binaries

On macOS, install `hubu`, `hubu-server`, `gongbu-server`, and
`hubu-unified-mcp` together from one exact tag with the repository's
[source installer](operations/releases.md#install-an-exact-release-from-source-macos).
Put the selected prefix's `bin` directory on `PATH`, and confirm that all four
report the intended release:

```sh
for binary in hubu hubu-server gongbu-server hubu-unified-mcp; do
  command -v "$binary"
  "$binary" --version
done
```

They must share one non-`unknown` full source commit and executor contract.
Release-stamped source installations use the normal production lineage checks;
do not enable `allow_development_builds`. The locally compiled executables are
not Developer ID-signed, Apple-notarized, or Apple-verified.

If the profile will use managed-local Temporal, install the version-pinned
Temporal CLI described in the
[Temporal decision guide](https://hubustack.dev/configuration/local-stack/v1/decisions#managed-local-versus-external-temporal).

## Initialize and select a profile

Choose an absolute profile path, initialize it, and select it for later stack
commands:

```sh
profile=/absolute/path/to/profile
hubu stack init --profile "$profile"
hubu stack select --profile "$profile"
```

Initialization creates starter files without overwriting existing files or
starting services:

```text
PROFILE_ROOT/
  README.md
  stack.toml
  credentials.toml
  providers.toml
  generated/
  state/
    credentials/
      .gitignore
```

The three TOML files are the editable sources. A provider-disabled managed
profile normally leaves `credentials.toml` at its generated schema-only
content. Do not edit `generated/` or `state/`.

## Complete and validate the profile

Follow the comments in the starter files and use the detailed reference when a
choice is unclear:

| File | What to choose | Detailed reference |
| --- | --- | --- |
| `stack.toml` | Binaries, managed or external services, Temporal, and local paths | [`stack.toml`](https://hubustack.dev/configuration/local-stack/v1/stack-toml) |
| `credentials.toml` | Provider references or advanced external-service overrides | [`credentials.toml`](https://hubustack.dev/configuration/local-stack/v1/credentials-toml) |
| `providers.toml` | Disabled or live mode, targets, pricing, and spend ceiling | [`providers.toml`](https://hubustack.dev/configuration/local-stack/v1/providers-toml) |

For a first local evaluation, start with the
[provider-disabled example](https://hubustack.dev/configuration/local-stack/v1/examples#provider-disabled-local-profile).
Never put bearer tokens, provider API keys, or other raw secrets in the TOML
files. Live provider execution can incur charges.

Check the profile and follow the reported field paths until it is ready:

```sh
hubu stack doctor
```

Doctor is read-only. An explicit `--profile "$profile"` can override the saved
selection for any one stack command.

## Start and inspect the stack

Start the stack, then confirm that its managed components are ready:

```sh
hubu stack start
hubu stack status
```

`stack start` runs doctor and render when needed. For a fully managed profile,
it starts the final Hubu process, completes Gongbu's managed credential
bootstrap, and starts Gongbu and its managed Temporal runtime. The client-owned
`hubu-unified-mcp` process is not part of the managed stack.

Once an active handoff exists, normal server-bound `hubu` commands take the
Hubu endpoint and authentication, approval, and reconciliation credential file
paths from the selected profile as one bundle. If there is no explicit
selection, an active conventional `default` profile is used. Ambient
`HUBU_URL` and token variables are ignored in either case. If a selected
profile has no valid active handoff, the command fails instead of silently
using another server. Pass an explicit global `--url` to opt into manual mode,
where the existing environment and token-file precedence remains available.
Local-only commands such as profile inspection and policy file creation do not
require an active handoff.

### Terminal color and automation

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

## Connect Codex

After the stack is ready, write the managed MCP configuration:

```sh
hubu init codex --stack-profile "$profile"
```

Restart Codex so it launches the unified MCP process with the new handoff. See
[Unified MCP setup](unified-mcp.md#setup) for discovery and compatibility
details.

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

Managed Hubu omits routine request events for successful `GET /health`,
`GET /version`, and Gongbu's marked
`GET /agents?operational_probe=gongbu_credential_check` readiness probe.
Unmarked agent-list reads and failed probes remain logged. The marker changes
logging only; it conveys no caller identity or authorization. Structured logs
remain bounded to one 10 MiB active file and four retained generations.

There is no `hubu stack restart` command. For an unchanged unhealthy or partial
managed stack, run `hubu stack stop`, then `hubu stack start`.

## Apply a configuration change

Render and review a changed profile before activating it:

```sh
hubu stack doctor
hubu stack render
# Review the generation ID, changed files, and affected components.
hubu stack stop
hubu stack activate --generation GENERATION_ID
hubu stack start
hubu stack status
```

For rollback, first restore the exact operator-owned TOML and compatible
binaries for the retained generation, then run:

```sh
hubu stack generations
hubu stack render
hubu stack stop
hubu stack rollback --generation PRIOR_GENERATION_ID
hubu stack start
hubu stack status
```

If the rendered plan reports `hubu-unified-mcp-client-config` as affected,
rerun `hubu init codex --stack-profile "$profile"` after the stack is ready and
restart Codex.

The [active-profile change guide](https://hubustack.dev/configuration/local-stack/v1/decisions#changing-an-active-profile)
explains staging, credential-reference changes, and rollback requirements.

## More detail

Use the managed lifecycle commands above for the persistent execution plane.

- [Configuration reference](https://hubustack.dev/configuration/local-stack/v1/)
- [Configuration decision guides](https://hubustack.dev/configuration/local-stack/v1/decisions)
- [Complete profile examples](https://hubustack.dev/configuration/local-stack/v1/examples)
- [Unified MCP surface](unified-mcp.md)
