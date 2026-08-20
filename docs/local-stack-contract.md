# Local stack configuration and lifecycle contract

Status: accepted V1 design for HUB-100. The commands and starter workspace in
this document are not implemented yet. Until the dependent milestone tasks
land, the existing [Hubu MCP](mcp-transport.md) and
[persistent Gongbu server](gongbu/server.md) runbooks remain authoritative.

## Decision

Hubu will expose one local operator workflow for configuring and running the
packaged Hubu stack without merging the ownership or runtime boundaries of its
components. The V1 workflow is:

```text
scaffold -> operator edit -> doctor -> render -> start
```

The intended command surface is:

```sh
hubu stack init
hubu stack doctor
hubu stack render
hubu stack start
hubu stack status
hubu stack logs
hubu stack stop
```

Exact option spelling may evolve without changing the phase boundaries below.
In particular, `init` never becomes an interactive setup wizard that requires
every operator decision in one session.

## Product and process boundaries

The local stack is an operator abstraction over separate components, not a new
combined server:

| Component | Owner | Lifecycle in V1 | State and credentials |
| --- | --- | --- | --- |
| `hubu` CLI | Human/operator | Invoked on demand | No service state |
| `hubu-server` | Hubu control plane | Stack-managed or explicitly external | Hubu database and Hubu capabilities |
| `gongbu-server` | Gongbu execution plane | Stack-managed or explicitly external | Gongbu database, provider credentials, pricing, and artifacts |
| Gongbu Temporal worker | Gongbu | Always owned by `gongbu-server` | Gongbu workflow/activity code |
| Temporal service/UI | Gongbu or external operator | `managed_local` child of Gongbu, or external | Temporal workflow history and service data |
| `hubu-unified-mcp` | Agent client, or optional stack wrapper | Client-owned stdio by default; optional foreground child | Two isolated backend clients; no domain data |

The stack command may launch `hubu-server` and `gongbu-server`. By default it
does not launch an orphaned `hubu-unified-mcp` daemon: Codex or another MCP
client owns that stdio process. Stack setup renders the client launch
configuration and stack diagnostics verify the binary, configuration, backend
compatibility, and capability contract.

For agent-harness testing, `hubu stack start --with-mcp` is the optional MCP
entry point. An MCP client may configure that command instead of invoking
`hubu-unified-mcp` directly. The wrapper first reconciles the backend stack,
then launches the unified MCP as a foreground child with stdin and stdout
reserved for MCP transport. It must not mix lifecycle output into MCP stdout;
operator diagnostics go to stderr or the profile logs. The wrapper remains
alive for the MCP child and forwards normal termination. This mode does not
turn the stdio server into a background daemon or make the unified MCP own
either backend.

Gongbu remains the only process that starts or stops its Temporal worker. In
`managed_local` mode, Gongbu also owns its Temporal child. The outer stack
launcher must not bypass or duplicate either responsibility.

Shared source, release packaging, discovery, and orchestration do not authorize
direct Hubu-to-Gongbu Cargo dependencies. Hubu and Gongbu continue to
communicate through their versioned HTTP executor contract and retain separate
databases, credentials, artifacts, failure domains, and shutdown behavior.

## Operator-owned starter workspace

`hubu stack init` creates a profile directory. The default root should follow
the host platform's user configuration convention; an explicit absolute root
may be selected when creating the profile. A profile contains these
operator-owned inputs:

```text
PROFILE_ROOT/
  README.md
  stack.toml
  credentials.toml
  providers.toml
  generated/
```

The three TOML files are normative V1 source files:

- `stack.toml` describes topology, component ownership, binary selection,
  loopback addresses, persistent state roots, Temporal mode, and lifecycle
  policy.
- `credentials.toml` contains only opaque credential references or absolute
  credential-file paths. It never contains bearer or provider secret values.
- `providers.toml` contains operator-selected targets, pricing rules, spend
  ceilings, and the explicit live-execution gate.

The generated `README.md` lists every starter file, which files still require
input, the next safe command, and the location of durable documentation. It may
be regenerated only when absent or when an explicit reviewed update preserves
the previous file.

The source schema supports incomplete profiles. Required decisions should be
shown as commented examples and omitted values, not strings such as
`REQUIRED`, fake UUIDs, placeholder credentials, invented prices, or a spend
acknowledgement that could accidentally pass a runtime validator.

## `stack init`

Initialization is a safe scaffolding operation.

It may:

- create the profile and `generated/` directories;
- write annotated starter TOML and a local README;
- discover installed packaged binaries and their safe version/provenance
  output;
- propose unused loopback ports and platform-appropriate absolute state paths;
- record discovered facts whose meaning is stable and reviewable; and
- report which files and fields still require operator input.

It must not:

- start, stop, restart, or signal a service;
- connect to a provider or perform provider work;
- mint, copy, reveal, or test raw credentials;
- select a provider, model, price, spend ceiling, account, agent, or Temporal
  ownership mode for the operator;
- emit strict production runtime configuration;
- enable live provider execution or write the live-spend acknowledgement; or
- overwrite an existing operator-owned file.

Re-running initialization is idempotent. Existing source files are reported and
left byte-for-byte unchanged. A future migration command may propose a
versioned source-schema update, but `init --force` must not become a shortcut
for destroying operator input.

Initialization succeeds when the scaffold is usable even if every operator
decision is still missing. Its output distinguishes discovered facts from
required operator choices.

## Source validation and doctor

`hubu stack doctor` is read-only. It never writes a source or generated file,
creates a credential, starts a service, or repairs a dependency.

Doctor evaluates the profile in layers so an operator can make progress over
multiple sessions:

1. **Source syntax**: TOML parses, schema versions are supported, and unknown or
   contradictory fields are identified.
2. **Completeness**: missing decisions are reported by source filename and
   stable field path with a concise remedy.
3. **Renderability**: paths, ports, binary provenance, component compatibility,
   catalog coverage, pricing, spend gates, identities, and opaque credential
   references satisfy the target runtime schemas.
4. **Runtime readiness**: when components are already running, doctor performs
   safe liveness/version checks and authenticated protected checks without
   exposing operator details.

Diagnostics classify the profile rather than collapsing every result into
success or failure:

- `incomplete`: valid starter syntax, but operator decisions are missing;
- `invalid`: a supplied value is contradictory, unsafe, or unsupported;
- `ready_to_render`: all source decisions required by the selected topology are
  present and valid;
- `ready_to_start`: rendered output is current and required external
  dependencies are reachable; or
- `running_ready`: all selected components pass their readiness contract.

Provider readiness is reported separately as `disabled`, `fixture_only`, or
`live_ready`. A profile may be useful for no-spend configuration and dependency
work without being live-provider ready. Production Gongbu execution remains
closed until explicit target, pricing, credential-reference, maximum-spend, and
live-spend gates all pass.

Human output may show operator-owned paths because doctor is a local operator
surface. Machine-readable and public server surfaces must use stable reason
codes and omit raw secret values. Secret values must never appear in either
form.

## Rendering

`hubu stack render` converts a complete starter profile into strict runtime
inputs below `generated/`. Generated files are implementation artifacts, not a
second editing surface.

Rendering must:

- refuse incomplete or invalid starter configuration;
- resolve cross-file references once and write consistent component inputs;
- keep raw secrets out of every generated file;
- use the production parser and validator owned by each target service;
- stage and validate the complete output set before atomically activating it;
- preserve the previously active generated set when any write or validation
  fails; and
- write a redacted manifest containing source digests, schema versions,
  selected binary provenance, generated-file digests, and restart impact.

Current service interfaces imply different generated forms. Gongbu owns a
strict JSON server configuration and provider/pricing catalogs. Hubu currently
owns environment/launch inputs and token-file references. The unified MCP owns
client launch environment containing separate Hubu and Gongbu endpoints and
credential-file references. V1 may add service-native validation entry points,
but it must not create a single runtime configuration parser shared across
Hubu and Gongbu.

Rendering never modifies the starter TOML. If source digests no longer match
the active manifest, status reports generated output as stale and start renders
a new complete generation before changing process state.

## Compatibility contract

A stack-managed profile selects all four production binaries from one verified
release lineage:

- `hubu`;
- `hubu-server`;
- `gongbu-server`; and
- `hubu-unified-mcp`.

Doctor and render compare safe `--version` metadata. Product version and source
commit must match for a packaged local stack unless a development-only override
is explicit and prominently reported. Independent protocol checks still apply:

- Gongbu's expected Hubu executor contract must match Hubu;
- the unified MCP contract and backend schema versions must be supported; and
- Temporal CLI/service compatibility must satisfy Gongbu's selected Temporal
  mode.

Matching product versions do not replace protocol negotiation, and an explicit
development override never permits an unknown executor contract.

## Startup and readiness

`hubu stack start` is an idempotent desired-state operation. It runs source
validation and rendering when needed before it changes process state, and
refuses an incomplete, invalid, or incompatible profile. Re-running it starts
missing components and leaves healthy, current components alone.

For stack-managed backends, startup order is:

1. start Hubu and wait for liveness, version compatibility, and the selected
   protected operator check;
2. start Gongbu, which starts or connects to Temporal and starts its own worker;
3. wait for Gongbu readiness, including Hubu compatibility, provider policy,
   Temporal readiness, and a polling Gongbu worker; and
4. verify the unified MCP executable and rendered client configuration. If an
   MCP client is already running, its existing capability monitor reports the
   backend catalog transition; with `--with-mcp`, launch the foreground MCP
   child only after the backend gates pass.

Stack readiness means every selected managed backend is ready and every
external dependency is compatible. Default start does not claim that a
client-owned MCP process is currently running. With `--with-mcp`, the wrapper
also reports the optional child lifecycle through stderr/status metadata while
leaving MCP stdout protocol-clean.

If startup fails, the launcher stops only children started by that invocation,
in reverse dependency order. It retains databases, artifacts, Temporal state,
generated configuration, and logs. Processes that were already running or are
configured as external remain untouched.

## Status and logs

`hubu stack status` presents one summary while keeping component ownership
visible. At minimum it reports:

- profile and generated-manifest identity;
- source/render drift and restart requirements;
- Hubu and Gongbu liveness, readiness, safe versions, and ownership mode;
- unified MCP binary/configuration compatibility and whether it is client-owned
  or an optional stack-launched foreground child;
- Temporal ownership mode, safe UI URL, namespace, and task queue;
- whether the Gongbu worker is polling; and
- exact local commands for relevant logs, doctor, repeated start, Temporal
  workflow discovery, and authenticated artifact retrieval.

Public unauthenticated health/version endpoints remain intentionally smaller.
They do not expose configured endpoints, filesystem paths, credential
references, account/agent identity, provider configuration, or stack topology.

`hubu stack logs` reads only logs owned by the selected managed profile. It
supports component filtering and execution correlation without concatenating
secret-bearing shell commands. External-service logs remain the external
operator's responsibility.

## Repeated start and shutdown

Users run `hubu stack start` again after editing starter configuration or when
they want to recover missing components. If generated inputs are stale while a
component is running, start first shows the affected-component plan and
requires explicit confirmation before gracefully replacing those processes.
It never restarts a component during render, and a current healthy stack is a
successful no-op. This keeps one start command as the normal reconcile path
without making an unexpected restart implicit.

Normal managed shutdown proceeds in reverse dependency order:

1. Gongbu removes readiness, stops accepting new executions, and drains/stops
   its worker;
2. Gongbu stops Temporal only when it owns the `managed_local` child;
3. the stack launcher waits for Gongbu to exit; and
4. the launcher stops Hubu only when this profile owns the Hubu process.

The agent client owns the normal stdio MCP process and its session lifecycle.
Stack shutdown does not kill Codex or another MCP client. When the optional
foreground wrapper launched the MCP child, it may stop only that child and
must leave the agent harness itself untouched. External Hubu, Gongbu, and
Temporal processes are never stopped by the stack command.

Hard termination may bypass graceful cleanup. The next doctor/status run must
detect stale ownership metadata, validate the real process identity before
signalling anything, and provide recovery guidance rather than trusting an old
PID blindly.

## Updates, credentials, and recovery

Operator-owned starter files remain the source of truth. Update and migration
workflows show a redacted plan, validate the candidate generation, preserve a
recoverable backup, and report component restart impact before activation.

Credential creation, issuance, rotation, revocation, and rollback belong to the
Credential and Authentication Architecture project. The stack consumes its
stable contracts and stores only opaque references. Until those contracts are
available, starter files may remain incomplete and doctor must identify the
missing credential class without suggesting that one broad bearer be copied
into every component.

Rollback restores one internally consistent generated set and its manifest. It
does not roll back databases, artifacts, or Temporal history. State recovery
continues to follow each owning service's runbook; a cold Gongbu backup includes
its database, artifact root, and managed Temporal data together.

## Security and spend invariants

- Starter and generated files never contain raw bearer tokens, provider keys,
  human approval capabilities, or reconciliation capabilities.
- Generated launch metadata must avoid shell interpolation and command strings
  containing secret values.
- Operator-owned files and manifests use restrictive user-only permissions.
- Hubu, Gongbu, and the unified MCP each receive only their own credential
  references; one component's credential is never forwarded to another.
- Gongbu never receives the human approval or reconciliation capability.
- Initialization and rendering never discover live prices, choose providers,
  raise spend limits, or write the live-spend acknowledgement.
- Live execution remains closed until the operator supplies an exact target,
  complete pricing, an opaque provider credential reference, a positive spend
  ceiling, and the explicit acknowledgement required by Gongbu.
- Deterministic tests and ordinary CI never enable external provider spend.

## Failure-domain matrix

| Failure | Required behavior |
| --- | --- |
| Hubu unavailable | Gongbu and unified capabilities report Hubu unavailable; no repair or substitution |
| Gongbu unavailable | Hubu governance remains usable; Gongbu tools are unavailable |
| Temporal unavailable or worker not polling | Gongbu readiness and new execution admission close |
| Unified MCP client exits | Backends continue independently; the client may restart the stdio process |
| One source file is incomplete | Doctor reports exact fields; init and existing source files remain unchanged |
| Render validation fails | Previously active generated set remains intact |
| Managed startup partially fails | Stop only newly started owned children; retain durable state and evidence |
| External dependency fails | Never stop, replace, download, or reconfigure it implicitly |

## Issue delivery boundaries

- HUB-101 implements starter scaffolding and rendering.
- HUB-102 implements read-only doctor and readiness diagnostics.
- HUB-103 implements managed lifecycle, status, and logs.
- HUB-104 implements reviewed updates, credential-reference rotation
  integration, drift, backup, rollback, and recovery.
- HUB-105 supplies deterministic milestone coverage, packaging, documentation,
  architecture updates, and the acceptance canary.

This contract does not authorize a GUI, automatic dependency installation,
hosted deployment, multi-node supervision, live pricing discovery, raw secret
management, or a combined Hubu/Gongbu service process.
