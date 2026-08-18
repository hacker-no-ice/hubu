# Hubu Dogfooding Gaps

This is a consolidated log of usability, product, and architecture gaps found
while following Hubu's Quick Start. It records observations, current
workarounds, and discussed directions; it is not an implementation commitment.

- **Originally observed:** 2026-08-14
- **Last consolidated:** 2026-08-17

## Theme index

| Theme | Original findings | Status |
| --- | --- | --- |
| Policy resources and declarative lifecycle | DG-002, DG-003, DG-004 | Direction discussed |
| Typed and explainable spend requests | DG-005, DG-006, DG-008 | Direction discussed |
| Safe retry after a denied operation | DG-007 | Direction agreed |
| Unified distribution and agent surface | DG-001, DG-009 | Architecture shift proposed |

## Theme 1: Policy resources and declarative lifecycle

**Consolidates:** DG-002, DG-003, DG-004
**Area:** Policy model, persistence, audit, and CLI
**Status:** Direction discussed

### Observed gaps

The current policy surface conflates policy identity, policy content, and policy
assignment:

- `hubu policy list` shows assignment metadata but cannot show or export the
  attached rules.
- YAML supplies a user-authored `id`, such as `test_policy`, which is exposed as
  `policy_id`. Unlike other Hubu resources, it is not an opaque generated public
  ID, and there is no separate human-readable policy name.
- `hubu policy add` writes the submitted policy into a user-default or
  agent-override assignment. Reapplying to the same scope replaces its current
  payload, but the command does not make the replacement behavior clear.
- Replacement is keyed by assignment scope rather than a stable policy
  resource, so one policy cannot be managed independently and assigned to
  multiple scopes.
- Version labels are client-authored, with no server revision history, diff,
  rollback point, or stale-write guard.

These behaviors make it difficult to identify, inspect, update, and audit the
effective policy for an agent.

### Current workaround

Edit the complete YAML, manually increment `version`, validate it, and reapply
it to the appropriate scope:

```sh
hubu policy validate --path policies/policy.yaml
hubu policy add --path policies/policy.yaml
```

For an agent-specific override:

```sh
hubu policy add --agent-id AGENT_ID --path policies/policy.yaml
```

The server updates its in-memory policy and SQLite assignment immediately; a
restart is not required. Existing operation keys still replay their stored
decisions, so policy changes apply to newly evaluated operations.

Until a CLI inspection command exists, normalized policy content can be read
from SQLite. With the default database path:

```sh
sqlite3 -readonly -json hubu.sqlite3 \
  "SELECT scope_type, scope_id, policy_id, policy_version, policy_json
   FROM policy_assignments;" |
  jq 'map(.policy_json |= fromjson)'
```

If the server uses `HUBU_DB_PATH`, use that database path instead. Stored JSON
preserves policy meaning but not the original YAML formatting or comments.

### Consolidated direction

Make policy identity, revision, and assignment distinct concepts:

```text
policy pol_abc123 -> current revision 4
                    revision 1
                    revision 2
                    revision 3
                    revision 4

assignments -> user default follows pol_abc123
            -> agt_xyz follows pol_abc123
```

A policy should have:

- an immutable opaque public ID, potentially `pol_...`
- an immutable owner-scoped declarative key for repeatable application
- a mutable human-readable name
- a server-controlled revision number
- an immutable canonical payload and hash for every accepted revision

Expose one desired-state operation rather than imperative rule-edit commands:

```sh
hubu policy apply --path policy.yaml
```

Reconciliation semantics:

- Unknown policy identity creates the policy.
- Existing identity with changed canonical payload appends a revision and moves
  the current-revision pointer atomically.
- Identical payload is an idempotent no-op.
- Invalid payload leaves the current revision unchanged.
- An expected revision or hash provides compare-and-set protection against
  stale writes.

Each accepted change should record actor, source, timestamp, prior and new
hashes, and affected assignments. Add `policy show`, `export`, `history`, and
`diff` views that display both policy ID and name and make assignment scope
explicit. Define bootstrap and migration behavior for existing YAML-authored
IDs before implementation.

## Theme 2: Typed and explainable spend requests

**Consolidates:** DG-005, DG-006, DG-008
**Area:** Money input, policy scope, provider routing, and CLI explainability
**Status:** Direction discussed

### Observed gaps

Several surprising dogfooding outcomes came from policy-sensitive request
fields being implicit, inconsistent, or overloaded:

- CLI `--amount` accepts decimal USD major units, so `--amount 5` means `$5.00`
  and is sent as `500` cents. HTTP and MCP instead expose integer
  `amount_cents`. The CLI help did not explain the conversion.
- Omitting CLI `--merchant` silently supplies `local-merchant`. A reason such as
  `Test Gemini Image Gen through Gongbu` is only audit text; Hubu does not infer
  `gongbu.image` from it.
- When the silent merchant fails to match an allow rule, the policy falls back
  to `needs_approval`, but the CLI does not print the normalized merchant or
  explain that no allow rule matched.
- A raw caller-controlled merchant such as `google` is too broad and does not
  tell an agent to use Gongbu for Gemini. One string cannot distinguish the
  requested capability, underlying provider, trusted executor, billing party,
  and workload profile.
- Because the caller supplies the value, a merchant match is not authoritative
  evidence of which provider or executor will actually perform the work.

This is one underlying contract problem: the agent is asked to construct
security-sensitive execution scope from loosely documented primitive fields,
and Hubu does not explain the normalized scope it evaluated.

### Current workaround

Treat CLI amounts as decimal major USD units and API/MCP `amount_cents` as
integer minor units:

```text
CLI --amount 5     -> $5.00 -> 500 cents
CLI --amount 1     -> $1.00 -> 100 cents
CLI --amount 0.50  -> $0.50 ->  50 cents
CLI --amount 0.05  -> $0.05 ->   5 cents
```

Supply the exact structured merchant explicitly and ensure the policy version
containing it is applied to the effective user or agent scope:

```sh
hubu spend authorize \
  --operation-key OPERATION_KEY \
  --account-id ACCOUNT_ID \
  --amount 0.05 \
  --merchant gongbu.image \
  --reason "Test Gemini Image Gen through Gongbu"
```

The most specific current executor identity, such as `gongbu.image`, is clearer
than `google`, but it still does not model the underlying provider or make the
identity trusted.

### Immediate improvements

- State the money unit and currency in CLI help, examples, validation errors,
  and review output. Avoid binary floating-point.
- Either require `--merchant` while the raw merchant model exists or display
  any default explicitly before evaluation.
- Print the complete normalized request scope and policy evaluation trace,
  including policy ID/revision, matched rules, unmatched rules, and why the
  default effect was selected.
- Keep human CLI money input explicit and friendly, while defining one
  canonical structured money representation for machine interfaces and future
  multi-currency support.

### Consolidated architecture direction

Replace the overloaded merchant convention with typed execution scope:

- **Capability:** requested outcome, such as `image.generate`
- **Provider/service:** underlying API, such as `google.gemini`
- **Executor:** trusted adapter, such as `gongbu.image`
- **Billing merchant:** party that actually charges the account
- **Workload profile:** operational timing and claim profile
- **Cost ceiling and currency:** normalized money authorization

Policies should constrain these typed fields instead of telling the model which
tool to call. The agent requests a capability; Hubu resolves an eligible route
from a versioned trusted integration registry and snapshots the route metadata
with the authorization:

```text
human policy
    -> agent requests typed capability
    -> Hubu resolves provider, executor, billing party, and workload
    -> Hubu authorizes immutable route and cost scope
    -> trusted executor claims authorization
    -> executor performs provider call and settles or releases
```

Resolution must be deterministic rather than inferred from prose. Executor
adapters supply canonical identities and capability metadata so the model
cannot invent an approved merchant. Hubu governs the execution plan without
owning provider credentials or provider-specific execution.

## Theme 3: Safe retry after a denied operation

**Original finding:** DG-007
**Area:** Spend authorization and idempotency
**Status:** Direction agreed

### Observed gap

An operation key is currently bound to the first evaluated spend scope. After a
mistaken `$5.00` request is denied, retrying the same logical operation at
`$0.05` returns:

```text
error: server returned HTTP 400: operation key was already authorized with different spend scope
```

Exact replay is correctly idempotent, but the client cannot correct the scope
of the same logical operation after a denial. It must generate an unrelated key,
fragmenting audit history and adding key-lifecycle work. A reusable skill helps
generate and recover keys but does not provide a server-enforced retry model.

### Current workaround

Use the original key only for transport retries with exactly the same account,
amount, merchant, task scope, and workload profile. Generate a new key whenever
any bound field changes.

An expired unclaimed authorization also currently requires a new operation key:
replaying the original scope returns the original expired token rather than
issuing a fresh authorization. Its frozen hold is returned to available budget
during expiry reconciliation. A token claimed before authorization expiry uses
the separate claim lease; an expired claim requires human billed/not-billed
reconciliation because vendor work may already have occurred.

### Agreed direction

Keep one client-supplied `operation_key` for the logical operation. Hubu creates
immutable server-controlled attempts beneath it:

```text
operation_key: test-gemini-image-gen
    attempt 1 / scope hash A: $5.00 -> deny
    attempt 2 / scope hash B: $0.05 -> evaluated
```

For an existing operation key:

- Matching canonical scope hash returns the stored attempt as an idempotent
  transport retry.
- A new scope after `deny` appends and evaluates a new immutable attempt because
  denial created no token, hold, claim, payment, or financial side effect.
- A changed scope after `allow` is rejected because authorization or execution
  state may exist. Exact replay remains valid.
- `needs_approval` is side-effect-capable pending state and blocks changed-scope
  retry. Exact replay remains valid.

Audit every attempt, scope, revision, timestamp, actor, final authorization
decision, and reason a new evaluation was allowed. The API returns structured
guidance to reuse the key after safe denial, replay exactly, or create a new
operation. No extra client-supplied `intent_id` is required.

## Theme 4: Unified distribution and agent surface

**Consolidates:** DG-001, DG-009
**Area:** Quick Start, packaging, and execution architecture
**Status:** Historical pre-migration analysis; repository consolidation is
implemented, unified release packaging is tracked by HUB-84, and runtime and
MCP boundaries remain.

### Observed gaps

The Quick Start installs three local Hubu crates without initially explaining
their resulting binaries and responsibilities:

- `hubu-cli` -> human/admin `hubu` command
- `hubu-api` -> local HTTP `hubu-server`
- `hubu-mcp` -> agent-facing `hubu-mcp-server`

At the time of this analysis, Hubu and Gongbu lived in separate repositories.
They still produce separate servers and expose separate agent-facing
interactions. The service split preserves a
clean governance-versus-execution boundary, but users must install, configure,
start, and troubleshoot multiple products. Agents must coordinate Hubu
authorization with Gongbu invocation and carry security-sensitive workflow
state across two MCP surfaces.

The installation problem and agent-choreography problem share one theme: the
internal component boundary is exposed directly as product friction.

### Immediate improvement

Explain what every installed crate produces, why it is needed, where Cargo
places the binary, and when developers should rerun `cargo install` versus use
`cargo run`.

### Architecture-shift note

The longer-term proposal changes Hubu from only a policy, budget, and
authorization service into the unified agent-facing control plane for governed
execution. Gongbu remains the separate execution plane. This requires an
explicit architectural decision, executor and capability protocol design,
threat-model review, and an architecture visualizer update when implementation
begins.

### Consolidated direction

Distribute Hubu and Gongbu together as one product while preserving strict
domain ownership:

```text
agent
  -> Hubu MCP and control plane
       - identity, policy, budgets, authorization
       - capability discovery and route resolution
       - durable dispatch, status, and audit
  -> authenticated versioned executor protocol
       -> Gongbu execution plane
            - provider credentials and integrations
            - provider retries, artifacts, and receipts
            -> provider such as Gemini
```

Authorization and execution remain separate semantics:

- A pure authorize route returns governance state and never dispatches work.
- An explicit execute route authorizes and durably enqueues work.
- Gongbu claims the job before irreversible execution, then settles or releases
  the authorization.

Provide one installation and lifecycle experience while retaining separate
binaries:

```text
hubu
hubu-server
hubu-mcp-server
gongbu-server
```

```sh
hubu init
hubu stack start
hubu stack status
hubu stack logs
hubu stack stop
```

A signed bundle manifest should pin Hubu and Gongbu versions, checksums, and
compatible executor/capability protocol versions. A launcher may configure and
supervise both processes while exposing only Hubu to the agent.

Bundling must not collapse the domain boundary. Preserve:

- one source repository and planned product release, with separate processes
  and failure domains
- separate databases, configuration, data directories, and logs
- exclusive Gongbu ownership of provider credentials and provider execution
- no direct cross-service database or credential-file access
- a narrow authenticated versioned wire protocol and fail-closed handshake
- scope-bound authorization and mandatory executor claim before work
- correlation identifiers without shared mutable state

For local use, prefer a private Unix-domain socket or authenticated loopback
connection. Gongbu's direct MCP surface may remain for development and
diagnostics, but normal governed execution should use Hubu as the only agent
surface. Large or sensitive payloads can travel through opaque references,
encrypted envelopes, or signed transfer URLs so Hubu does not own provider
request data or artifacts.

The intended principle is: **unified installation and agent interaction;
isolated governance and execution domains**.

## New-theme template

```text
## Theme N: Short description

**Original findings:** DG-NNN
**Area:**
**Status:** Observed

### Observed gaps

### Current workaround

### Consolidated direction
```
