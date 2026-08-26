# Unified MCP surface

`hubu-unified-mcp` is Hubu's only supported agent-facing MCP server. It is a
stdio adapter over the separate Hubu and Gongbu HTTP APIs. It owns only local
harness-operation identity state; it does not own either backend's domain state
or merge the backends.

The implemented public contract is `hubu-gongbu-mcp-v1`. The server reports
`serverInfo.name = "hubu-unified-mcp"` and implements MCP protocol version
`2024-11-05`.

## Ownership boundary

- Hubu owns humans, agents, policies, budgets, spending targets,
  authorizations, claims, reconciliation, payments, and ledger state.
- Gongbu owns provider configuration and credentials, executions, Temporal
  state, provider calls and retries, pricing, artifacts, and recovery.
- The router owns discovery, static name-to-backend routing, its local
  normalized harness-operation registry, and the durable adapter worker that
  submits and observes acknowledged executions.

The processes retain separate endpoints, bearer credentials, databases,
configuration, lifecycle, readiness, and failure domains. The router never
forwards one backend credential to the other, copies artifact bytes into Hubu,
or exposes provider credentials.

A tool call is validated against its public schema and forwarded to exactly one
owner. The router never composes a Hubu governance mutation and Gongbu execution
mutation into one call.

## Tool catalog

The router exposes two local read-only tools:

- `hubu_unified_capabilities`
- `hubu_operation_status`

Hubu-owned tools cover health, registration, policies, budgets, spending
targets, spend authorization and submission, ledger reads, executor claims,
and reconciliation:

```text
hubu_add_policy
hubu_apply_policy
hubu_authorize_spend
hubu_client_approval_profile
hubu_create_budget
hubu_create_recurring_budget
hubu_export_policy
hubu_get_executor_claim
hubu_health
hubu_list_agents
hubu_list_budgets
hubu_list_claims_requiring_reconciliation
hubu_list_ledger
hubu_list_users
hubu_policy_diff
hubu_policy_history
hubu_reconcile_vendor_billed_claim
hubu_reconcile_vendor_did_not_bill_claim
hubu_register_agent
hubu_register_human
hubu_registration_guidance
hubu_replace_budget
hubu_revoke_budget
hubu_revoke_spending_target
hubu_set_spending_target
hubu_show_policy
hubu_show_spending_targets
hubu_submit_spend
```

Gongbu-owned tools cover execution and artifacts:

```text
gongbu_create_execution
gongbu_get_execution
gongbu_list_artifacts
gongbu_get_artifact
```

The static ownership table is authoritative; prefix inference is not a routing
rule. Unknown names fail closed until a routing revision assigns them. Exact
input schemas are checked against the router's catalog and the Gongbu
[golden fixture](../crates/hubu-unified-mcp/tests/fixtures/gongbu-tool-definitions-v2.json).

## MCP behavior and responses

The server supports JSON-RPC `initialize`, `ping`, `tools/list`, and
`tools/call` over stdio. Unknown methods return `-32601`; malformed call
parameters return `-32602`. Input schemas reject additional properties.

The router preserves backend result semantics. Hubu spend results additionally
replace private backend identity with a stable public operation handle and
concise recovery guidance:

- Hubu successes contain pretty-printed JSON text and identical
  `structuredContent`.
- Gongbu read successes retain their compact text result and `isError: false`.
  Execution projections replace Gongbu's private operation identity with the
  same stable public operation handle returned by Hubu authorization.
- `gongbu_create_execution` durably acknowledges the bound operation instead
  of waiting for provider work. Its result contains the public handle, adapter
  lifecycle state, terminal flag, replacement-safety flag, and guidance to
  observe the same operation rather than submit a replacement.
- `gongbu_get_artifact` returns safe metadata followed by PNG or JPEG content.
- Gongbu application errors remain `isError: true` with their sanitized error
  object.

For `gongbu_create_execution`, the router preserves Gongbu's two allowlisted
admission diagnostics: `target_not_selectable` with the four target field names,
or `pricing_selector_not_matched` with `input.image_size`. These are field paths,
not request or target values. Because create is acknowledged before background
dispatch, a definitive admission rejection surfaces later in the terminal
`hubu_operation_status.result` as the stable `execution_request_invalid` code
plus the allowlisted `reason_code` and `fields`; the same projection survives
exact redelivery and restart. It explains the failure but does not make the
acknowledged operation replacement-safe. Generic and unknown backend
diagnostics are not promoted into agent-facing detail, and the backend's bounded
process-log event does not cross the MCP boundary.

For example, an agent-facing spend result includes:

```json
{
  "operation_handle": "hubu:public-operation:v1:8e8ca8d0f42a4d0e8a781d61b30f55ce",
  "decision": "allow",
  "auth_token_id": "authorization-record-id",
  "requires_human_approval": false,
  "agent_guidance": {
    "on_ambiguous_result": "redeliver_exact_call",
    "replacement_call": "do_not_submit"
  }
}
```

The authorization token ID is a scoped continuation identifier, not a service
credential. For `gongbu_create_execution`, the router resolves that identifier
to exactly one normalized operation in its registry before contacting Gongbu.
Gongbu authenticates the installation caller independently. For a new execution,
Hubu validates the same identifier over the versioned executor contract and
supplies authoritative account/agent attribution; exact replay of a persisted
token is local to Gongbu before Hubu resolution. Neither the public handle nor
the continuation identifier grants backend access on its own.

After acknowledgement, call `hubu_operation_status` with the public handle.
The adapter lifecycle is:

```text
accepted -> queued -> dispatching -> reconciling -> succeeded
                                             \----> failed
```

`reconciling` covers both ordinary observation of a durable Gongbu execution
and an explicitly ambiguous provider outcome. Terminal `failed` means the
adapter cannot establish successful completion; it does not prove that an
ambiguous provider mutation performed no work. Every acknowledged state sets
`replacement_safe: false`. Gongbu keeps an independent
`reconciliation_required` record after adapter reconciliation exhaustion so
later operator evidence can still settle or release the financial state.

The same status tool safely projects handles before Gongbu acknowledgement:
`awaiting_hubu_result` requires exact redelivery of the original harness call,
`approval_required` requires resolving the existing human approval, an allowed
authorization is `authorized`, and a synchronous `hubu_submit_spend` result is
already terminal. Denied or malformed allowed authorizations are terminal
failures rather than executable continuations.

The router does not add a success envelope, rename fields, translate currency
units, expose filesystem locations, or convert an application error into a
successful payload.

## Discovery and compatibility

Before `initialize`, and on a bounded interval afterward, the router probes
Hubu and Gongbu independently. `hubu_unified_capabilities` returns a sanitized
snapshot containing the unified contract and routing revision, each backend's
state and compatible version metadata, and all 34 tool names with owner and
availability.

The version-1 compatibility boundary requires:

| Surface | Required value |
| --- | --- |
| Unified contract | `hubu-gongbu-mcp-v1` |
| Routing revision | `1` |
| MCP protocol | `2024-11-05` |
| Hubu and Gongbu executor contract | `hubu-spend-executor-v4.3` |
| Gongbu API schema | `2` |
| Gongbu MCP schema | `2` |
| Product versions | Exact match across router and configured backends |
| Source commits | Known, exact, matching 40-character Git SHA values |

Unstamped local builds intentionally fail the source-commit compatibility
check. For an operator deployment, install every runtime binary from one
verified release archive.

Backend states determine the callable catalog:

| State | Behavior |
| --- | --- |
| `available` | Compatible and healthy; all owned tools are eligible |
| `degraded` | Gongbu is compatible but not ready; reads remain available and execution creation is hidden |
| `unavailable` | A health or version probe failed; owned tools are hidden |
| `incompatible` | Required metadata differs; no call is forwarded |
| `unconfigured` | Endpoint or credential configuration is incomplete |

Partial availability is intentional. One unhealthy backend does not hide the
capability tool or compatible tools owned by the other backend.
`gongbu_create_execution` also depends on Hubu availability because it consumes
a Hubu authorization.

After the client completes the initialized lifecycle, the router emits one
payload-free `notifications/tools/list_changed` when the effective catalog
changes. Clients then refresh `tools/list` and use
`hubu_unified_capabilities` for diagnostics.

The initialized monitor probes each backend independently on a jittered
30-second default cadence so multiple client-owned stdio processes do not
synchronize. Repeated unavailable results back off that backend exponentially
to at most five minutes without delaying transitions for the other backend, and
a healthy result resets its cadence. The same per-backend deadline gates
background and request-triggered refreshes, so routine `tools/list` and tool
calls cannot bypass outage backoff. The explicit capability diagnostic and
governed execution admission still force a refresh; when either shortens a
recovered backend's deadline, it wakes and reschedules the background monitor.
Operators may set
`HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS` between 10 and 60000 milliseconds to
replace the base interval. Backoff and jitter still apply to an overridden
interval.

The durable operation worker uses a one-second base tick. Safe create replay
and read-only observation failures retry at bounded exponential delays for at
most five attempts. Explicit Gongbu reconciliation is observed at 30, 60, 120,
240, and 480 ticks before the adapter records
`reconciliation_exhausted`. Operators may set
`HUBU_UNIFIED_OPERATION_TICK_MS` between 10 and 1000 milliseconds; values below
the one-second production default are intended for deterministic local tests.
Every acknowledgement also receives a durable 24-hour adapter deadline, so a
permanently nonterminal backend record resolves to
`operation_deadline_exhausted` rather than being orphaned indefinitely.

## Setup

Install the CLI, Hubu server, Gongbu server, and unified MCP binary from one
release. The preferred Codex setup is:

```sh
hubu init codex --trust-client-approval
```

For a rendered local stack profile:

```sh
hubu init codex --stack-profile /absolute/path/to/profile
```

With `--stack-profile`, the command consumes the verified handoff from an
already running stack and writes the managed MCP entry; managed startup has
already created the required capabilities. The non-stack setup form may create
or reuse its manual local defaults. Both forms render Hubu's approval profile
into client tool settings. Restart Codex after changing the generated
configuration.

The lifecycle is:

```text
operator starts: hubu stack start (managed hubu-server and gongbu-server)
client starts:   hubu-unified-mcp
router calls:    separate Hubu and Gongbu HTTP endpoints
agent sees:      eligible hubu_* and gongbu_* tools
```

The local stack does not start or own the stdio MCP process. The agent harness
starts it from client configuration.

Manual MCP clients configure these inputs for the router:

- `HUBU_UNIFIED_OPERATION_STATE_PATH`, an absolute path to the router-owned
  SQLite registry. Managed setup always renders it. A manual client may omit
  it, but new billable Hubu operations are then unavailable.
- `HUBU_UNIFIED_HUBU_ENDPOINT`
- `HUBU_UNIFIED_HUBU_BEARER_TOKEN` or
  `HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE`
- the corresponding Gongbu endpoint and installation-scoped caller token
- `HUBU_RECONCILIATION_TOKEN` or its file form when reconciliation is enabled

Endpoint and credential values are never returned by capability discovery.
The Gongbu caller token carries no execution identity claim. One configured
installation caller may read known executions and artifacts across the owner's
agents, but the API does not promise owner-wide browse/list and this local
capability model is not strong multi-user or per-agent isolation.

## Approval boundary

The MCP catalog distinguishes reads, spend submission, and protected human
actions through tool annotations and the `hubu_client_approval_profile` result.

Registration, policy mutation, spending-target changes, budget mutation, and
claim reconciliation require a client-enforced human prompt. Protected tools
remain disabled unless the process is started with
`HUBU_MCP_TRUST_CLIENT_APPROVAL=1`. Approval is never accepted as a model-owned
tool argument.

Spend submission and authorization do not receive a generic pre-call prompt.
Hubu evaluates policy and may return `requires_human_approval: true`; in that
case no payment or provider execution has occurred. The client shows the
returned immutable review and resolves the pending decision through the
protected approval path.

Reconciliation requires a separate server-side capability in addition to the
MCP prompt. An executor with only the normal Hubu bearer credential cannot
reconcile a claim.

These controls define a local trust boundary. A same-user process that can read
the capability files or control an authorized MCP client can act with that
client's authority. Do not expose this configuration to an untrusted network or
connect it to a real payment rail without a stronger authentication design.

## Trusted invocation metadata

Spend tools expose business arguments such as account, amount, scope, workload,
and reason. `operation_key` and optional `task_id` remain outside
model-authored arguments. The router accepts exactly one supported trusted
identity source per spend call:

- Codex `_meta.callId`
- Claude Code `_meta["claudecode/toolUseId"]`
- `_meta["hubu.dev/platform-invocation"]` containing only `platform`,
  `invocation_id`, optional `task_id`, and optional diagnostic
  `installation_id`

The controlled envelope does not accept `operation_key`. Its optional
`installation_id` is a bounded typed diagnostic alias only: it neither selects
the registry installation nor participates in operation-key authority or the
core deduplication tuple. On first successful registry startup the router
persists its own stable local installation identity. For each call it validates bounded identifiers,
canonically hashes the tool name and model-authored arguments, and atomically
resolves or allocates a private backend operation key plus an independent public
`hubu:public-operation:v1:*` handle by platform, local installation, and harness
call ID. Exact redelivery reuses the normalized operation. Reusing the same
identity with different arguments or trusted aliases fails before backend
access, while a different call ID creates a distinct operation even when its
arguments are identical.

Only common identity columns and the typed Codex, Claude Code, and controlled
envelope aliases are stored; arbitrary `_meta` content is not persisted. The
router injects the private key and an explicit null task ID when none exists.
Neither field appears in model-authored tool schemas. Spend results recursively
remove every raw `operation_key` location from text content, structured content,
and agent-facing errors. Trusted `task_id` remains visible as non-authoritative
business correlation for compatibility, audit context, and human review. The
router returns the public operation handle and persists the sanitized replay
payload separately from the bounded continuation columns `decision_id`,
`auth_token_id`, and `approval_request_id`.

Before forwarding a spend mutation, the registry durably marks dispatch. A
terminal `allow` or `deny` result is stored before it is returned and exact
redelivery reads that result without another backend mutation. Terminal state is
monotonic, so a delayed pending response cannot overwrite it. The registry
retains the private key for internal continuation verification, claim
idempotency, settlement, and recovery, but never returns it. A
`needs_approval` result or ambiguous dispatch likewise retains it so an exact
redelivery can recover Hubu's durable state. Expired authorization token IDs are
removed opportunistically only before Gongbu create dispatch begins. Once
dispatch starts, the identifier is retained so an ambiguous response can
recover Gongbu's locally persisted execution after restart. The public handle
cannot retrieve or replay an operation: recovery requires the original
normalized harness call identity or its authorized continuation flow.

`gongbu_create_execution` accepts only the opaque `spend_auth_token_id` plus
execution intent. Before acknowledging, the router requires that identifier to
name one allowed normalized operation, canonically binds the first immutable
execution intent to it, and rejects changed intent or nested attempts to supply
operation identity, task correlation, endpoint, credential, retry, or lifecycle
state. It temporarily persists the validated canonical request, marks the
operation `accepted`, and returns the public status projection. The background
worker promotes it through `queued` and `dispatching`, then calls Gongbu over
HTTP. Gongbu independently resolves the identifier from Hubu and returns its
internal operation identity. The router verifies that identity and any existing
execution ID against the registry, then deletes the replay request. Exact
create replay uses the same private operation key and immutable request, so a
lost HTTP response recovers Gongbu's idempotent stored execution without
creating another provider attempt. Conflicting intent fails before Gongbu
access and conflicting returned identity fails closed.

The registry persists the adapter lifecycle (`accepted`, `queued`,
`dispatching`, `reconciling`, `succeeded`, or `failed`), bounded retry counters,
next-attempt time, 24-hour terminal deadline, worker lease, safe result code,
Gongbu execution ID, and
Gongbu's latest lifecycle and optional outcome. Worker leases make interrupted
dispatch recoverable after restart without allowing concurrent adapter
processes to own the same attempt. Create and status results recursively remove
`operation_key` fields and private-key text from content, structured content,
errors, failure messages, and artifact metadata. Read-only status correlation
uses only `operation_handle`; it exposes no continuation identifier, raw
operation key, harness identifier, provider credential, prompt, or storage
path. `task_id` remains trusted, non-authoritative correlation and is not
accepted in model-authored protected inputs.

The worker retries only calls whose safety is known from the versioned HTTP
contract: exact Gongbu create replay and execution GET. It never retries a
provider mutation directly. Transient create or observation failures use
bounded exponential backoff; permanent contract errors fail immediately. A
Gongbu `reconciliation_required` response remains adapter `reconciling` while
Gongbu uses provider idempotency and queryable provider references. When the
bounded observation window is exhausted, the adapter records terminal
`failed` with `reconciliation_exhausted` and `replacement_safe: false`, while
Gongbu retains its separate unresolved record.

One distinct harness spend call remains one distinct potentially billable
operation. The router does not infer retries across call IDs. If acknowledgment
is ambiguous, the returned guidance tells the agent to redeliver the exact call
with the same harness identity and never submit a replacement spend call.

The registry and worker are adapter state and remain separate from both the Hubu
and Gongbu databases, credentials, provider execution, artifacts, and failure
domains. Continuation binding composes agent calls, not backend storage or
process ownership. All interaction remains through bounded, versioned Gongbu
HTTP requests; Gongbu performs its own Hubu resolution, persistence,
scheduling, provider work, and financial recovery. For a managed stack, the
router's client references come from the verified post-start handoff; it does
not participate in service credential bootstrap.

Registry schema v4 intentionally does not upgrade v1, v2, or v3 state. Earlier
schemas cannot prove the complete replay payload and lifecycle needed by the
submit-once contract; v2 terminal authorization rows also erased private
operation identity. Such a profile fails the registry capability closed and
must start with fresh adapter state. This is an intentional pre-live breaking
change. Backend reads remain available while registry-dependent tools are
hidden.

The v1 normalizer fails closed when more than one primary identity source is
present in the same call. It does not apply metadata precedence or correlate
aliases across harnesses. A future contract revision may add explicit alias
correlation without changing v1's fail-closed behavior.

Registry availability is independent of both backend capabilities. Missing or
broken registry state does not stop unified MCP startup and does not hide Hubu
reads, `gongbu_get_execution`, or artifact tools. It hides new Hubu billable
tools, local `hubu_operation_status`, and `gongbu_create_execution` from
discovery and rejects direct registry-dependent calls before backend access. The
capability snapshot reports `operation_registry.state`, its stable reason code,
and `billable_operations_available` for diagnosis.

The router implementation and ownership map live in
[`crates/hubu-unified-mcp`](../crates/hubu-unified-mcp).
