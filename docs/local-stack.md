# Local stack configuration

The local stack profile is an operator-owned configuration boundary for Hubu,
Gongbu, Temporal, and the unified MCP client handoff. It coordinates compatible
inputs without merging component ownership, runtime state, credentials, or
failure domains.

The initial profile workflow is:

```text
stack init -> operator edit -> stack start -> stack status -> init codex
```

`stack start` runs doctor and render as needed, then starts only missing managed
components after their dependencies pass. The stack profile never starts or
supervises the client-owned `hubu-unified-mcp` stdio process.

Later source or credential-reference changes use a reviewable two-phase flow:

```text
operator edit -> stack render -> review plan -> stack stop -> stack activate -> stack start
```

There is intentionally no `hubu stack restart` command. A changed, partial, or
unhealthy managed stack is recovered as one dependency-ordered unit with an
explicit graceful stop and start.

## Component ownership

| Component | Owner | State and credentials |
| --- | --- | --- |
| `hubu` CLI | Human/operator | No service state |
| `hubu-server` | Hubu control plane | Hubu database and capabilities |
| `gongbu-server` | Gongbu execution plane | Gongbu database, provider credentials, pricing, and artifacts |
| Gongbu Temporal worker | Gongbu | Workflow and activity code, always owned by `gongbu-server` |
| Temporal service/UI | Gongbu or external operator | Workflow history and service state |
| `hubu-unified-mcp` | Agent client | Two isolated backend clients and no domain data |

Gongbu is the only process that starts or stops its worker. In `managed_local`
mode Gongbu also owns the Temporal child; in external mode it connects without
lifecycle authority. Hubu and Gongbu continue to communicate through the
versioned HTTP executor contract and never open one another's databases.

## Profile layout

`hubu stack init` creates an operator-owned profile:

```text
PROFILE_ROOT/
  README.md
  stack.toml
  credentials.toml
  providers.toml
  generated/
```

- `stack.toml` describes topology, binary selection, loopback addresses,
  persistent state roots, Temporal mode, and lifecycle policy.
- `credentials.toml` contains opaque credential references or absolute
  credential-file paths, never bearer or provider-secret values.
- `providers.toml` contains operator-selected targets, pricing, spend ceilings,
  and the explicit live-execution gate.
- `generated/generations/` contains immutable validated runtime generations,
  each with a redacted manifest.
- `generated/active-manifest.json` is the atomic pointer to the selected
  generation. Generated content is never an editing surface.

Live profiles use pricing catalog schema v2 exclusively. Each pricing rule in
`providers.toml` supplies exact rational components and may qualify an image
price with a normalized `1k`, `2k`, or `4k` selector:

```toml
[[pricing_rules]]
rule_id = "gemini-image-1k"
provider = "google"
model = "gemini-image"
currency = "USD"
selector = { image_size = "1k" }
components = [
  { unit = "image", rate_numerator_minor = 1, rate_denominator = 1 },
]
```

Define one selector-qualified rule for every enabled image size. Managed-stack
production validation does not accept or translate the retired flat `unit` and
`unit_amount_minor` pricing shape.

The default profile is `hubu/stacks/default` under the host platform's user
configuration directory. `HUBU_HOME` overrides Hubu's configuration root. An
operator may instead provide an absolute profile path:

```sh
hubu stack init --profile /absolute/path/to/profile
```

## Safe initialization

Initialization creates annotated starter files, proposes safe local paths and
ports, and reports missing operator decisions. It deliberately supports an
incomplete profile.

It never:

- overwrites an existing operator-owned file;
- starts, stops, or signals a service;
- connects to a provider or performs provider work;
- creates, copies, reveals, or tests a raw credential;
- selects a provider, model, price, spend ceiling, account, agent, or Temporal
  ownership mode; or
- enables live execution.

Repeated initialization leaves existing inputs byte-for-byte unchanged.
Missing values remain omitted or documented as comments rather than fake IDs,
placeholder secrets, or acknowledgements that could pass runtime validation.

## Validation and readiness

`hubu stack doctor` is read-only. It never writes source or generated files,
creates credentials, starts services, or repairs dependencies:

```sh
hubu stack doctor --profile /absolute/path/to/profile
hubu stack doctor --profile /absolute/path/to/profile --json
```

Doctor evaluates four layers:

1. Source syntax and schema compatibility.
2. Completeness, reported by starter file and stable field path.
3. Renderability, including paths, ports, binary provenance, component
   compatibility, identities, credential references, and managed-Gongbu
   provider and artifact contracts.
4. Runtime readiness for already-running components and external dependencies.

The report classification is `incomplete`, `invalid`, `ready_to_render`,
`ready_to_start`, or `running_ready`. Provider readiness is reported separately
as `unknown`, `disabled`, `fixture_only`, or `live_ready`.

Human output may show the selected local profile path. The versioned JSON report
contains only the classification, provider readiness, and ordered checks with
stable reason codes, owning components, optional source fields, and remedies.
It omits configured endpoints, binary paths, service/account values, credential
values, and raw service responses.

For a complete profile, doctor uses bounded subprocesses and network probes to
check binary provenance, opaque credential-reference existence, active
generation digests, service-owned production validators, backend liveness and
version compatibility, protected reads, Gongbu worker readiness, and required
Temporal reachability. A failed probe never causes doctor to start or repair a
component.

External Gongbu retains authority for its provider and artifact configuration,
so doctor reports local provider readiness as `unknown` rather than certifying
unused local provider inputs.

## Rendering

After editing the starter files, render the profile:

```sh
hubu stack render --profile /absolute/path/to/profile
```

Rendering validates source syntax, required decisions, path and port safety,
binary provenance, component compatibility, provider catalogs, pricing, spend
gates, identities, and credential references. It then uses service-owned
production validators to stage a complete generation.

A first successful render:

- writes immutable output below `generated/generations/`;
- validates it with the selected service-owned production binaries;
- atomically creates `generated/active-manifest.json` only when no generation
  is active and no launcher-owned process state exists;
- records source and output digests, schema versions, binary provenance, and
  affected-component impact in a redacted per-generation manifest; and
- leaves the source TOML unchanged.

When a generation is already active, render validates and stages the new
generation without changing the active manifest. Its redacted change plan
contains changed source filenames, affected component names, the generation
ID, and the exact activation command. Comment-only changes may have no affected
component even though their source digest creates a distinct recoverable
generation.

An incomplete profile or validation failure leaves the previous active
generation untouched. Generated files never contain raw bearer tokens,
provider keys, human-approval capabilities, or reconciliation capabilities.

## Updates and credential-reference rotation

The three TOML files remain the source of truth. To update configuration or
rotate one of the temporary file-based credential references:

1. Create the replacement credential file with operator-controlled permissions.
2. Edit only its absolute path in `credentials.toml`; do not put the credential
   value in TOML.
3. Run `stack doctor`, then `stack render` and review the reported source files,
   affected components, and generation ID.
4. If launcher-owned services are running, stop the whole managed stack.
5. Activate the reviewed generation and start the whole managed stack.

```sh
hubu stack doctor --profile /absolute/path/to/profile
hubu stack render --profile /absolute/path/to/profile
hubu stack stop --profile /absolute/path/to/profile
hubu stack activate --generation GENERATION_ID --profile /absolute/path/to/profile
hubu stack start --profile /absolute/path/to/profile
```

Rendering and activation never read, copy, compare, or rewrite credential
values. They validate reference shape and the owning service's configuration
contract. Overwriting a credential value at the same path is therefore not a
detectable or supported rotation: create a new file and change the reference so
Hubu and the client-owned MCP handoff move together. Keep the previous
credential available until the new generation is running and verified. If the
client handoff is affected, rerun
`hubu init codex --stack-profile ...` and restart Codex after backend startup.
Native macOS Keychain loading and migration are tracked separately; this V1
workflow intentionally preserves file-based Hubu and unified-MCP references.
Gongbu's credential references remain Gongbu-owned opaque Keychain coordinates.
The renderer does not read those secrets or prove that a Gongbu-to-Hubu caller
value matches the Hubu bearer, so an operator rotating that shared capability
must update both owning references before the whole-stack stop/start cycle.

## Compatibility

A profile selects these production binaries from one release lineage:

- `hubu`
- `hubu-server`
- `gongbu-server`
- `hubu-unified-mcp`

An external Hubu or Gongbu service does not require or probe its corresponding
local server binary. The local `hubu` and `hubu-unified-mcp` binaries still
establish the client release lineage, and doctor compares each external
service's safe version response with that lineage before reporting readiness.

Product version and source commit must match for a packaged stack. Protocol
checks still apply independently: Hubu and Gongbu must agree on the executor
contract, the router must support both backend schema versions, and the
selected Temporal mode must satisfy Gongbu's compatibility requirements.

Matching product versions never substitute for protocol negotiation. A local
development override must remain explicit and cannot make an unknown executor
contract safe.

## Codex handoff

Once the profile has a valid active generation, configure Codex with:

```sh
hubu init codex --stack-profile /absolute/path/to/profile
```

The command reads the active manifest, verifies the selected unified MCP binary,
and writes the managed `[mcp_servers.hubu]` entry with separate Hubu and Gongbu
endpoint and credential-file references. It does not copy raw credentials into
Codex configuration or start the stdio process. Restart Codex so a new session
loads the updated entry.

## Lifecycle commands

For a complete profile, one command validates, renders, and reconciles the
launcher-owned processes:

```sh
hubu stack start --profile /absolute/path/to/profile
hubu stack status --profile /absolute/path/to/profile
hubu stack status --profile /absolute/path/to/profile --json
```

Hubu must be ready, version-compatible, and accessible through its protected
probe before managed Gongbu starts. Gongbu then owns its Temporal worker and,
in `managed_local` mode, its Temporal child. Stack readiness means both HTTP
backends and the worker are ready; the unified MCP remains client-owned and is
reported as a compatible handoff rather than a running stack process.

Repeated start is a no-op for a healthy, current stack. Start never repairs,
signals, or selectively restarts a partially running, unhealthy, changed, or
drifted managed stack. For an unchanged unhealthy stack it asks for graceful
whole-stack stop then start. For a validated staged update it additionally
requires explicit generation activation while stopped:

```sh
hubu stack stop --profile /absolute/path/to/profile
hubu stack activate --generation GENERATION_ID --profile /absolute/path/to/profile
hubu stack start --profile /absolute/path/to/profile
```

Stop drains and stops the complete launcher-owned managed stack in reverse
dependency order; the following start launches it in forward dependency order.
External or compatible unowned processes are never signalled. When a managed
prerequisite starts but a downstream external component is unavailable, the
launcher leaves the prerequisite running and tells the external operator what
must be restored before start is retried.

Status distinguishes launcher-owned, compatible unowned, external, exited, and
stale-identity processes. It also reports active generation and restart impact,
Temporal ownership and worker readiness, the client-owned MCP handoff, and
exact follow-up commands for doctor, logs, Codex initialization, Temporal
workflow inspection, and artifact retrieval.

Launcher-owned logs are available without mixing external service logs into the
profile:

```sh
hubu stack logs --component all --lines 200 --profile /absolute/path/to/profile
hubu stack logs --component gongbu --execution-id EXECUTION_ID --profile /absolute/path/to/profile
```

Managed Hubu writes structured events directly to its configured JSONL file;
the launcher does not duplicate those events through stderr. Successful
`/health` and `/version` probes are omitted from normal request logging, while
failed probes remain visible. The structured log rotates at 10 MiB and retains
four prior generations (`hubu.jsonl.1` through `hubu.jsonl.4`), bounding the
visible structured history to approximately 50 MiB. Stack lifecycle commands
preserve both the current file and its retained generations. A pre-existing
file larger than the per-file limit is discarded at the next rotation instead
of being retained as an oversized generation. Fatal process diagnostics and
structured-log write errors use the separate
`runtime/logs/hubu-server.stderr.log` capture, which is truncated on each
managed Hubu start so JSONL rotation never orphans its file descriptor. Hubu
creates every active structured-log generation with private `0600` permissions
on Unix, including the new active file opened after rotation.

Stop proceeds in reverse dependency order: Gongbu drains first and shuts down
its managed worker and Temporal child, then Hubu stops. Startup rollback also
touches only children created by that invocation.

```sh
hubu stack stop --profile /absolute/path/to/profile
```

The launcher records a process start identity before it gains signal authority.
If a PID was reused or the recorded identity does not match, lifecycle commands
refuse to signal it. After independently confirming that ownership is gone, the
operator can remove only the stale metadata with `stack stop --forget-stale`.
Databases, artifacts, managed Temporal data, generated configurations, and logs
are never deleted by start, activation, rollback, or stop.

## Generation rollback and interrupted updates

List the validated generations retained by the profile:

```sh
hubu stack generations --profile /absolute/path/to/profile
```

Rollback never silently rewrites operator-owned input. Restore the exact
`stack.toml`, `credentials.toml`, and `providers.toml` bytes for the target
generation from operator version control or backup, select the same compatible
binaries, and render them. Then stop the managed stack and reactivate that
generation explicitly:

```sh
hubu stack render --profile /absolute/path/to/profile
hubu stack stop --profile /absolute/path/to/profile
hubu stack rollback --generation PRIOR_GENERATION_ID --profile /absolute/path/to/profile
hubu stack start --profile /absolute/path/to/profile
```

The rollback command fails closed when the requested generation does not match
the current source and selected-binary provenance, when its manifest or output
digests are invalid, or while launcher-owned process metadata remains. A failed
render or interrupted pre-activation update leaves the active manifest
unchanged. A failed atomic activation leaves the prior active manifest in
place; rerun doctor/render and retry activation after confirming the stack is
stopped. Corrupt active or retained manifests are reported rather than skipped
or replaced.

## Clean-environment acceptance canary

From a source checkout with the Temporal CLI on `PATH`, run:

```sh
./scripts/integration-local-stack-acceptance.sh
```

The canary builds source-only, feature-gated fixture support and then exercises
the real process boundary end to end. It starts from a clean profile, runs
annotated non-starting init, verifies incremental doctor diagnostics, renders
and activates strict service-owned configuration, and starts the actual
`hubu-server`, `gongbu-server`, Gongbu worker, and managed Temporal child. It
then obtains a governed Hubu authorization, submits a deterministic Gongbu HTTP
execution, discovers its Temporal workflow, downloads and verifies its
artifact, gracefully stops the whole stack, starts it again against the same
state, and verifies the completed execution and artifact remain available.

This is a local fixture canary, not an unattended-production credential model.
For V1 it temporarily lets the deterministic execution test use the existing
broad local Hubu bearer, which remains process-owned test data and is never
placed in model input, output, logs, or generated configuration. Gongbu is
never given Hubu's human reconciliation capability. Keychain-backed,
least-privilege execution credentials remain follow-up work in HUB-32.

The canary uses an explicit one-cent fixture catalog, acknowledgement, and
process-owned dummy provider reference to prove the fail-closed live-provider
configuration path without making a billable request. Fixture support is
compiled only when the non-default `local-fixture-canary` feature is selected
and additionally requires an explicit canary environment switch. Normal and
release `gongbu-server` builds omit that feature and continue to reject fixture
providers.

## Runtime and recovery boundaries

The profile coordinates lifecycle without becoming a shared domain state
store. Each component retains its own readiness, shutdown, backup, and recovery
procedure:

- Hubu recovery covers its database and capabilities.
- Gongbu recovery covers its database, artifact root, and managed Temporal data
  as one consistent unit.
- External Temporal instances remain under the external operator's control.
- Restarting the agent client does not restart either backend.
- Re-rendering configuration does not roll back databases, artifacts, or
  workflow history.

Use [Gongbu server operations](operations/gongbu-server.md) for the persistent
execution plane and [the unified MCP guide](unified-mcp.md) for client setup and
backend discovery.
