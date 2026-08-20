# Local stack configuration

The local stack profile is an operator-owned configuration boundary for Hubu,
Gongbu, Temporal, and the unified MCP client handoff. It coordinates compatible
inputs without merging component ownership, runtime state, credentials, or
failure domains.

The supported profile workflow is:

```text
stack init -> operator edit -> stack doctor -> stack render -> stack doctor -> init codex
```

Service lifecycle remains explicit: operators start Hubu and Gongbu using their
respective runbooks. The stack profile does not start or supervise the
client-owned `hubu-unified-mcp` stdio process.

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
- `generated/` contains validated runtime artifacts and a redacted active
  manifest. It is never an editing surface.

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

A successful render:

- writes immutable output below `generated/generations/`;
- atomically replaces only `generated/active-manifest.json`;
- records source and output digests, schema versions, binary provenance, and
  restart impact in a redacted manifest; and
- leaves the source TOML unchanged.

An incomplete profile or validation failure leaves the previous active
generation untouched. Generated files never contain raw bearer tokens,
provider keys, human-approval capabilities, or reconciliation capabilities.

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

## Runtime and recovery boundaries

The profile is configuration, not a shared state store. Each component retains
its own startup, readiness, shutdown, backup, and recovery procedure:

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
