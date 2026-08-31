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

Most tool calls are validated against their public schema and forwarded to
exactly one owner. `hubu_submit_governed_execution` is the intentional
cross-backend orchestration operation: the router authorizes through Hubu,
binds one immutable execution intent to the existing durable Gongbu worker
after authorization, observes that operation for a bounded time, and may
deliver its artifacts in the same MCP response. `hubu_resume_operation` is a
router-owned recovery operation: it replays stored Hubu intent for primitive
spend and additionally binds stored Gongbu execution intent only for governed
work. These operations compose service calls and adapter state transitions;
they do not merge backend state, credentials, provider work, artifacts,
processes, or failure domains.

## Tool catalog

The router exposes two local read-only tools:

- `hubu_unified_capabilities`
- `hubu_operation_status`

It also exposes two router-owned workflow tools:

- `hubu_submit_governed_execution`
- `hubu_resume_operation`

The composite is the preferred ordinary path when an agent has both spend
authorization intent and an execution request. `hubu_resume_operation` resumes
an approved pending normalized operation—primitive spend or composite—by its
public handle without requiring the original harness call identity. The
primitive Hubu and Gongbu tools remain available for recovery, diagnostics,
and backward compatibility.

Hubu-owned tools cover health, registration, policies, budgets, spending
targets, spend authorization and submission, ledger reads, executor claims,
and reconciliation:

```text
hubu_add_policy
hubu_apply_policy
hubu_authorize_spend
hubu_budget_history
hubu_client_approval_profile
hubu_create_budget
hubu_create_recurring_budget
hubu_export_policy
hubu_get_executor_claim
hubu_get_spend_approval
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
hubu_resolve_spend_approval
hubu_revoke_budget
hubu_revoke_spending_target
hubu_set_spending_target
hubu_show_policy
hubu_show_spending_targets
hubu_submit_spend
hubu_update_budget
```

Gongbu-owned tools cover the supported-provider catalog, configured-target
discovery, execution, and artifacts:

```text
gongbu_list_execution_targets
gongbu_create_execution
gongbu_get_execution
gongbu_get_provider_catalog
gongbu_list_artifacts
gongbu_get_artifact
```

`gongbu_get_provider_catalog` has strict empty input and forwards only to
Gongbu's authenticated `GET /v1/provider-catalog`. Its sanitized schema-v1
result exposes exact supported target/model, resolutions, currency, rational
pricing, policy versions, and independent readiness facts. It does not expose
credential coordinates or values, call BFL, or convert
`live_qualified = false` into a readiness claim.

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
concise, boundary-specific recovery guidance. A raw Hubu client that owns an
operation key may follow `reuse_operation_key` after a side-effect-free denial,
but the unified MCP router never exposes that key. It translates a definitive
denial to `create_new_operation`: corrected work is a new tool call with a new
trusted harness identity and a newly allocated private operation key.

- Hubu successes contain pretty-printed JSON text and identical
  `structuredContent`.
- Gongbu read successes retain their compact text result and `isError: false`.
  Execution projections replace Gongbu's private operation identity with the
  same stable public operation handle returned by Hubu authorization.
- `gongbu_create_execution` durably acknowledges the bound operation instead
  of waiting for provider work. Its result contains the public handle, adapter
  lifecycle state, terminal flag, replacement-safety flag, and guidance to
  observe the same operation rather than submit a replacement.
- `hubu_submit_governed_execution` returns one composite outcome, the public
  handle, adapter/execution status when available, server-observed timing, and
  eligible inline artifacts when terminal execution and delivery fit its total
  internal budget.
- `hubu_get_spend_approval` reads the owner-scoped immutable review and durable
  `pending`, `approved`, or `denied` status without using approval authority.
- `hubu_resolve_spend_approval` submits exactly one explicit human `approve` or
  `deny` decision behind the client prompt and the separate Hubu approval
  capability. It never starts Gongbu execution or provider work.
- `hubu_resume_operation` accepts only a public operation handle. After an
  approval, it replays the stored immutable Hubu intent. Primitive operations
  return their original authorization or spend outcome; a governed operation
  binds its stored execution intent and wakes the existing worker idempotently.
  It cannot change the approved scope or create a second logical operation.
- `gongbu_get_artifact` returns safe metadata followed by PNG or JPEG content.
- Gongbu application errors remain `isError: true` with their sanitized error
  object.
- Only `hubu_update_budget` and `hubu_budget_history` translate typed Hubu
  application rejections into MCP tool results with `isError: true`. Their text
  and `structuredContent` contain the same sanitized `error`, `http_status`,
  `error_code`, and optional `details` and `retry_guidance`. Other Hubu tools
  retain their existing JSON-RPC application-error behavior.

### Versioned budget tools

`hubu_update_budget` is approval-gated and idempotent. Its strict input requires
`budget_id`, `expected_revision >= 1`, and `amount_limit_cents >= 1`, with an
optional `reason`. The amount is the budget's new cumulative total cap, not an
increment. The router validates the public budget ID, places it only in
`/budgets/{budget_id}/versions`, and forwards only the revision, amount, and
optional reason in the POST body. `hubu_budget_history` is the read-only GET on
the same safe dynamic path.

An update success distinguishes the requested immutable `applied_version` from
the authoritative `current_budget`. This matters when an exact historical
retry recovers its original successor after the head has advanced.
`idempotent_replay` labels that case. History returns the current logical
snapshot once and then immutable versions in ascending revision order.

Typed rejections use HTTP 400 for invalid amount or revision, 404 for unknown
or not-owned budgets, 409 for revision conflict, revocation, or expiry, and 422
when the total would fall below committed usage. Numeric conflict and floor
values remain in `details`. Generic HTTP 500 responses expose no storage or
invariant detail. Every configured Hubu bearer or capability string is removed
recursively from the error, details, retry guidance, and rendered text.

The stable public codes are:

- `budget_update_invalid_amount` (400) and
  `budget_update_invalid_revision` (400) identify boundary validation failures.
- `budget_not_found` (404) deliberately covers both unknown and not-owned IDs.
- `budget_revision_conflict` (409) includes expected and current revisions;
  refresh and review history before creating a new intent.
- `budget_revoked` (409) and `budget_expired` (409) reject lifecycle-invalid
  updates.
- `budget_limit_below_committed` (422) includes requested and committed cents.
- `budget_update_storage_error` (500) is generic. After an ambiguous outcome,
  retry only with the same budget ID, expected revision, amount, and reason.

## Composite governed execution

`hubu_submit_governed_execution` means “submit one governed execution
request.” Hubu evaluates policy first. If Hubu allows the request, Gongbu
executes through the existing durable worker and the router returns the result
when it can. If Hubu requires human approval, no provider work starts and the
request remains resumable. The tool does not assert that approval has already
happened and it never treats an agent tool argument as human approval. Human
resolution is a separate protected MCP or CLI action; the composite never waits
for that decision and resolution itself never starts provider work.

The input combines the existing authorization fields with the existing Gongbu
execution intent. Trusted harness identity remains in MCP `_meta`, outside the
model-authored arguments. Agents should first call
`gongbu_list_execution_targets`, choose one operator-approved target and its
runtime options, and copy the returned `execution_scope` into authorization:

```json
{
  "authorization": {
    "account_id": "agent-account-id",
    "amount_cents": 12,
    "reason": "Generate one product illustration",
    "execution_scope": {
      "schema_version": 1,
      "provider": "provider:local:fixture",
      "executor": "executor:gongbu:image",
      "capability": "capability:image:generate",
      "billing_merchant": "merchant:local"
    }
  },
  "execution": {
    "schema_version": 2,
    "input": {
      "prompt": "A deterministic blue circle",
      "image_count": 1
    },
    "input_schema_version": 1,
    "target_id": "gongbu:target:v1:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  }
}
```

Discovery returns structured content containing only the opaque target ID,
workload type, provider/model labels, authorization scope, supported
`image_size` values, and exact configured price components. It omits adapter
settings, endpoints, headers, credentials, and provider configuration
revisions. The raw workload/provider/adapter/model tuple remains accepted for
backward compatibility, but a request must use exactly one selector form.

The router normalizes that complete request once and durably stores its bounded,
validated intent before authorization dispatch. It performs the existing Hubu
authorization mutation and, only after `allow`, binds the execution intent to
the same private operation and wakes the existing durable operation worker. A
pending review retains that immutable intent so `hubu_resume_operation` can use
the public handle after approval. There is no second execution state machine
and no router retry of a provider mutation. Exact redelivery with the same
trusted harness identity and public-handle resume both recover the same
authorization, Gongbu execution, and provider attempt; neither permits changed
arguments.

The composite outcome is one of:

| Outcome | Meaning |
| --- | --- |
| `succeeded` | Gongbu reached successful terminal execution during the internal budget; the response includes timing and eligible artifacts when delivery fits. Budget exhaustion after terminal success remains `succeeded` with an artifact-delivery warning. |
| `denied` | Hubu denied authorization. This operation is terminal and no Gongbu or provider work starts. Exact redelivery only recovers the same denial; corrected work is submitted as a new tool call and logical operation. |
| `approval_required` | Hubu persisted a pending human decision. The composite returns immediately with its public handle; no Gongbu or provider work starts. Approval makes that same immutable operation eligible for explicit public-handle resume; denial makes it terminal. |
| `in_progress` | The bounded wait ended before terminal execution. The durable worker continues the same operation and the public handle can be observed with `hubu_operation_status`. |
| `failed` | The existing adapter or Gongbu execution reached a terminal failure. This outcome does not make a replacement safe; observe the existing handle and recovery guidance. |

### Human review, resolution, and resume

The approval tools have deliberately separate jobs:

- `hubu_get_spend_approval` accepts only `approval_request_id`. It is an
  owner-scoped read and returns the immutable review plus its durable status.
- `hubu_resolve_spend_approval` accepts only `approval_request_id` and
  `decision`, whose value is `approve` or `deny`. It requires both the normal
  Hubu bearer and the distinct approval capability. Repeating the same decision
  is idempotent; a conflicting decision is rejected. Its public result withholds
  any authorization continuation so resolution cannot bypass the separate
  public-handle resume boundary.
- `hubu_resume_operation` accepts only `operation_handle`. It is not an approval
  operation. It synchronizes the decision if necessary, requires an approved
  immutable operation, and replays only the stored Hubu intent. Primitive
  operations recover their original authorization or spend outcome; governed
  operations bind the stored execution intent and wake the existing durable
  worker. Repeating it observes or resumes that same operation rather than
  creating another Gongbu execution or provider attempt.

In Codex, the human first says `approve` or `deny` in the chat after reviewing
the structured request. Codex then forms a
`hubu_resolve_spend_approval` call, and the native MCP tool prompt asks for a
second confirmation before the resolver is invoked. Canceling or rejecting
that native prompt does not call Hubu: the durable decision remains `pending`.
It is not recorded as a Hubu denial. A denial exists only after an explicit
`decision: "deny"` call reaches Hubu successfully.

Neither reading nor resolving an approval starts Gongbu execution, payment, or
provider work. An approval reserves the original immutable maximum and moves
the public operation to `resume_required`; provider work can begin only after
the separate resume call. Once the original call records `needs_approval`, its
redelivery is permanently replay-only: even a concurrent or later response
cannot advance it to execution. A denial is terminal and resume fails closed
without contacting Gongbu. The CLI remains a supported external decision
surface, and the next status or resume call synchronizes that authoritative
Hubu decision into the router registry. Because the router stores the validated
intent before authorization dispatch, the public handle still resumes that
exact intent after the stdio MCP process restarts; callers do not need to
reconstruct its private key or original arguments.

If the approved authorization lease expires before resume, the router records
terminal `authorization_expired_before_resume`, starts no Gongbu or provider
work, and directs the caller to create a new logical operation. Hubu exposes a
machine-readable expiry code for the submit-spend replay path; unrelated or
ambiguous backend failures remain nonterminal and continue to require a retry
with the same public handle.

The composite handler uses a 45-second default end-to-end response target. Its
clock starts before the forced capability refresh, so capability checks,
authorization, worker wake/observation, and inline artifact work all consume
the reported total and leave less time for the bounded waiter. A synchronous
probe, Hubu request, or SQLite busy wait cannot be interrupted after it starts;
those individually bounded calls may therefore briefly overrun a deliberately
lowered test override. The 45-second production default leaves headroom under
the generated Codex MCP
configuration's 60-second per-tool timeout for final JSON-RPC serialization and
delivery. It is not the provider deadline or the durable operation deadline.
If execution is still nonterminal when the configured budget expires, the tool
returns `in_progress` and the worker keeps the original operation alive under
its existing 24-hour adapter deadline. If execution is already successfully
terminal, artifact-budget exhaustion does not change the outcome to
`in_progress`; the tool returns `succeeded` with a delivery warning and the
primitive artifact recovery guidance. The composite does not wait for a human,
raise the client timeout, derive target pricing, or change worker retry and
reconciliation semantics.

On `succeeded`, the router lists and fetches only PNG and JPEG artifacts and
may append them as MCP image content up to a fixed 8 MiB aggregate raw-byte
ceiling (about 10.7 MiB after base64 encoding) and at most 16 inline images.
The optional `max_inline_artifact_bytes` input defaults to that ceiling and may
lower, but never raise, the per-call aggregate. Artifact metadata remains
sanitized. Artifacts that are unsupported, exceed either limit, or cannot be
delivered inside the remaining internal budget stay available through
`gongbu_list_artifacts` and `gongbu_get_artifact`; a delivery failure or retry
never starts another provider attempt.

The structured `timing` object uses milliseconds and has
`scope: "composite_tool_server_observed"`. It reports `total_ms`,
`hubu_authorization_ms`, `execution_wait_ms`, `artifact_delivery_ms`,
`router_unattributed_ms`, the nullable Gongbu-owned fields
`gongbu_execution_total_ms`, `provider_interaction_ms`, and
`gongbu_non_provider_ms`, plus a display `summary`. The execution wait includes
durable worker scheduling, Gongbu coordination, provider work, and observation;
it must not be presented as provider-only time. The router fields form the
composite handler's envelope view. The Gongbu fields are an owner-attributed
breakdown inside that envelope, usually overlapping `execution_wait_ms`; the two
views must not be added together.

`human_approval_wait_ms` is explicitly null: the composite returns
`approval_required` immediately and cannot measure the external human interval.

The Gongbu fields come from durable execution and provider-attempt boundaries,
not router polling. Gongbu measures execution total from execution creation to
completion, provider interaction from immediately before request transmission
to the durable provider result, and non-provider time as their checked
difference. Those values remain null until both relevant timestamps exist;
malformed, negative, or inconsistent intervals are unavailable. Raw provider
attempt IDs and timestamps do not cross the MCP boundary.

The same Gongbu-owned projection on execution reads is
`timing: { schema_version: 1, scope: "gongbu_execution",
execution_total_ms, provider_interaction_ms, non_provider_ms }`, with the three
durations nullable under the same rules.

The one-call artifact-delivery promise therefore applies to an ordinary
auto-approved execution that reaches terminal state while its eligible
artifacts fit the inline limits and remaining internal budget. A terminal
success still returns in one call when delivery is partial, but the agent uses
the primitive list/get tools to recover the omitted artifacts without rerunning
the provider. The primitive authorization, create, status, list, and get tools
remain the recovery and diagnostic surface for all other paths.

For `gongbu_create_execution`, the router preserves Gongbu's allowlisted
admission diagnostics: `target_not_selectable` with either `target_id` or the
four legacy target field names, and `pricing_selector_not_matched` with
`input.image_size`. These are field paths, not request or target values. Because
create is acknowledged before background
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

A definitive denial instead carries decision-aware guidance without exposing
or asking the agent to reuse the private backend key:

```json
{
  "operation_handle": "hubu:public-operation:v1:edb9e31c9a4245aab93b30bc29607f22",
  "decision": "deny",
  "retry_guidance": {
    "action": "create_new_operation",
    "message": "This denied operation is terminal. Exact redelivery only recovers the same denial. Submit corrected work as a new tool call so the harness creates a new logical operation."
  },
  "agent_guidance": {
    "on_ambiguous_result": "redeliver_exact_call",
    "on_denied_result": "create_new_operation",
    "replacement_call": "create_new_operation"
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
`approval_required` means the authoritative decision is still pending,
`resume_required` means it was approved and the stored intent is ready for
`hubu_resume_operation`, an allowed primitive authorization is `authorized`,
and a synchronous `hubu_submit_spend` result is already terminal. A read of a
pending operation refreshes its approval status from Hubu, so an approval or
denial submitted through the CLI or another owner-authorized surface is not
left stale in local MCP state. The refresh never calls Gongbu or starts provider
work. Denied or malformed allowed authorizations are terminal failures rather
than executable continuations. For `authorization_denied`, `approval_denied`,
and `spend_denied`, status repeats `create_new_operation` guidance. Its
`replacement_safe: false` describes the existing public handle: the handle and
call identity cannot be repurposed, while corrected work may be submitted as a
distinct logical operation. Other terminal failures retain no-replacement
guidance because provider or financial side effects may be unresolved.

The approval branch around the ordinary adapter lifecycle is:

```text
approval_required --approve--> resume_required --resume--> accepted -> queued
                 \--deny-----------------------------------> failed
```

Primitive pass-through routes do not add a success envelope, rename fields,
translate currency units, or expose filesystem locations. Versioned budget
tools make the narrow documented exception of returning typed application
rejections as `isError: true` MCP tool results; other Hubu application errors
remain JSON-RPC errors. The explicit composite uses its documented outcome
envelope without changing either backend's wire contract.

## Discovery and compatibility

Before `initialize`, and on a bounded interval afterward, the router probes
Hubu and Gongbu independently. `hubu_unified_capabilities` returns a sanitized
snapshot containing the unified contract and routing revision, each backend's
state and compatible version metadata, and all 40 other tool names with owner
and availability. Together with `hubu_unified_capabilities`, the stdio surface
exposes 41 tools, 37 of which route to a backend.

The version-1 compatibility boundary requires:

| Surface | Required value |
| --- | --- |
| Unified contract | `hubu-gongbu-mcp-v1` |
| Routing revision | `5` |
| MCP protocol | `2024-11-05` |
| Hubu and Gongbu executor contract | `hubu-spend-executor-v4.3` |
| Gongbu API schema | `2` |
| Gongbu MCP schema | `2` |
| Product versions | Exact match across router and configured backends |
| Source commits | Known, exact, matching 40-character Git SHA values |

Unstamped local builds intentionally fail the source-commit compatibility
check. For an operator deployment, install every runtime binary together from
one exact, release-stamped lineage. The source installer is the recommended
initial-user macOS path.

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
a Hubu authorization. `hubu_submit_governed_execution` requires a healthy
operation registry plus safe Hubu and Gongbu admission boundaries.
`hubu_resume_operation` requires the registry and Hubu; it remains discoverable
when Gongbu is unavailable so approved primitive authorization or spend can be
recovered. A governed handle that still needs its first Gongbu dispatch cannot
advance until Gongbu admission is healthy, while a completed operation replays
its stored sanitized result without either backend. Compatible primitive reads,
including approval lookup, remain independently available.

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

The composite handler's end-to-end response target defaults to 45 seconds.
Deterministic tests may set `HUBU_UNIFIED_GOVERNED_EXECUTION_WAIT_MS` between 10
and 45000 milliseconds. Lower values principally reduce the time left for
observation and artifact delivery; bounded synchronous pre-admission work may
consume or briefly exceed a very low override. The setting does not change the
durable worker or operation deadline.

## Setup

Install the CLI, Hubu server, Gongbu server, and unified MCP binary from one
release. The preferred Codex setup is:

```sh
hubu init codex
```

For a rendered local stack profile:

```sh
hubu init codex --stack-profile /absolute/path/to/profile
```

With `--stack-profile`, the command consumes the verified handoff from an
already running stack and writes the managed MCP entry; managed startup has
already created the required capabilities. The non-stack setup form may create
or reuse its manual local defaults. Both forms render Hubu's approval profile
into client tool settings. The resolver is rendered with
`approval_mode = "prompt"`; spend submission and public-handle resume retain
their non-interactive client policy because Hubu's durable decision remains the
authority. Restart Codex after changing the generated configuration.

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
- `HUBU_APPROVAL_TOKEN` or `HUBU_APPROVAL_TOKEN_FILE` when protected approval
  resolution is enabled
- `HUBU_MCP_TRUST_SPEND_APPROVAL=1` only when the client shows a human prompt
  that confirms the already chosen approve-or-deny resolver call
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
claim reconciliation require a client-enforced human prompt. Those broad
administrative tools remain disabled unless the process is started with
`HUBU_MCP_TRUST_CLIENT_APPROVAL=1`.

Spend approval resolution uses the narrower
`HUBU_MCP_TRUST_SPEND_APPROVAL=1` gate. `hubu init codex` always renders that
gate together with the per-tool native prompt, without enabling the broader
administrative surface. The broad gate also satisfies the resolver gate when
an operator deliberately enables it. Approval authority is loaded from the
separate local capability and is never accepted as a model-owned tool argument.

Spend submission, authorization, and composite governed execution do not
receive a generic pre-call prompt. Hubu evaluates policy and may return
`requires_human_approval: true`; the composite projects that state as
`approval_required` and returns immediately. In that case no payment, Gongbu
execution, or provider work has occurred. The client shows the returned
immutable review. After the human says approve or deny, the client forms the
protected `hubu_resolve_spend_approval` call and shows its native MCP prompt.
The composite never holds its MCP call open while the human decides. A canceled
prompt submits no decision and leaves the request pending; it must not be
reported as a Hubu denial. A successful resolution changes only Hubu and the
router's durable approval projection. It never invokes Gongbu or a provider.
After approval through MCP or an external owner-authorized surface,
`hubu_operation_status` synchronizes the decision and
`hubu_resume_operation` continues the same immutable intent by public handle.

Reconciliation requires a separate server-side capability in addition to the
MCP prompt. An executor with only the normal Hubu bearer credential cannot
reconcile a claim.

These controls define a local trust boundary. A same-user process that can read
the capability files or control an authorized MCP client can act with that
client's authority. Do not expose this configuration to an untrusted network or
connect it to a real payment rail without a stronger authentication design.

## Trusted invocation metadata

Spend tools expose business arguments such as account, amount, scope, workload,
and reason. The composite nests the ordinary Hubu fields under `authorization`
and the existing Gongbu intent under `execution`. `operation_key` and optional
`task_id` remain outside model-authored arguments. The router accepts exactly
one supported trusted identity source per spend call:

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
`auth_token_id`, `approval_request_id`, and durable approval status. For a
governed request it also persists the bounded canonical intent needed for
public-handle continuation; that intent is never returned by status.

Before forwarding a spend mutation, the registry durably marks dispatch. A
terminal `allow` or `deny` result is stored before it is returned and exact
redelivery reads that result without another backend mutation. Terminal state is
monotonic, so a delayed pending response cannot overwrite it. The registry
retains the private key for internal continuation verification, claim
idempotency, settlement, and recovery, but never returns it. A
`needs_approval` result or ambiguous dispatch likewise retains it and the
validated canonical intent. Owner-scoped approval reads synchronize an
externally submitted `approved` or `denied` decision without trusting local
client state. Expired authorization token IDs are removed opportunistically
only before Gongbu create dispatch begins. Once dispatch starts, the identifier
is retained so an ambiguous response can recover Gongbu's locally persisted
execution after restart. The public handle does not grant backend authority,
but `hubu_resume_operation` may use it inside the configured router to select
exactly the stored immutable intent; callers cannot retrieve that intent or
replace it with new arguments.

For a denial, the router replaces any backend-oriented
`reuse_operation_key` guidance before persisting or returning the sanitized
result. Exact redelivery with the original call identity therefore recovers the
same terminal denial. A corrected request uses a distinct harness call identity,
which allocates a different private key and public handle; changing arguments
under the denied identity remains an identity collision rejected before Hubu
access.

For `hubu_submit_governed_execution`, the normalized canonical request covers
both nested objects and the composite tool name. The first invocation routes
the authorization portion through the existing Hubu path. `deny` and
`needs_approval` stop before execution admission. Pending approval retains the
canonical request but does not bind or wake the Gongbu worker. `allow`, whether
returned initially or recovered by `hubu_resume_operation`, binds the execution
portion to that same operation using the existing continuation rules, then the
existing worker performs create replay and observation. Denial clears the
pending intent. The bounded MCP waiter reads adapter state; it is not a second
worker and does not take ownership of Gongbu execution, provider calls,
artifacts, or financial recovery.

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
with the same harness identity and never submit a replacement spend call. Once
a denial is established, that operation is terminal and corrected work is a
new call ID and logical operation rather than a changed replay.

The registry and worker are adapter state and remain separate from both the Hubu
and Gongbu databases, credentials, provider execution, artifacts, and failure
domains. Continuation binding composes agent calls, not backend storage or
process ownership. All interaction remains through bounded, versioned Gongbu
HTTP requests; Gongbu performs its own Hubu resolution, persistence,
scheduling, provider work, and financial recovery. For a managed stack, the
router's client references come from the verified post-start handoff; it does
not participate in service credential bootstrap.

Registry schema v5 performs one explicit forward migration from v4, preserving
existing public handles and approval state while adding durable request intent.
A migrated v4 pending row has no reconstructable intent. Before handle resume,
an exact original-call redelivery can backfill the matching canonical request.
Otherwise the first handle-resume attempt records terminal
`resume_intent_unavailable`; later calls cannot resurrect or replace that
intent. Schemas v1, v2, and v3 still fail closed because they cannot prove the complete replay
payload and lifecycle needed by the submit-once contract; v2 terminal
authorization rows also erased private operation identity. Backend reads remain
available while registry-dependent tools are hidden.

The v1 normalizer fails closed when more than one primary identity source is
present in the same call. It does not apply metadata precedence or correlate
aliases across harnesses. A future contract revision may add explicit alias
correlation without changing v1's fail-closed behavior.

Registry availability is independent of both backend capabilities. Missing or
broken registry state does not stop unified MCP startup and does not hide Hubu
reads, `gongbu_get_execution`, or artifact tools. It hides new Hubu billable
tools, local `hubu_operation_status`, `gongbu_create_execution`, and
`hubu_submit_governed_execution`, and `hubu_resume_operation` from discovery and
rejects direct registry-dependent calls before backend access. Approval lookup
and resolution remain Hubu-owned operations; without matching router state they
cannot offer public-handle continuation. The capability snapshot reports
`operation_registry.state`, its stable reason code, and
`billable_operations_available` for diagnosis.

The router implementation and ownership map live in
[`crates/hubu-unified-mcp`](../crates/hubu-unified-mcp).
