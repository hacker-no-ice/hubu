# Unified MCP surface

`hubu-unified-mcp` is Hubu's only supported agent-facing MCP server. It is a
thin stdio adapter over the separate Hubu and Gongbu HTTP APIs; it does not own
domain state or merge the two backends.

The implemented public contract is `hubu-gongbu-mcp-v1`. The server reports
`serverInfo.name = "hubu-unified-mcp"` and implements MCP protocol version
`2024-11-05`.

## Ownership boundary

- Hubu owns humans, agents, policies, budgets, spending targets,
  authorizations, claims, reconciliation, payments, and ledger state.
- Gongbu owns provider configuration and credentials, executions, Temporal
  state, provider calls and retries, pricing, artifacts, and recovery.
- The router owns discovery and static name-to-backend routing only.

The processes retain separate endpoints, bearer credentials, databases,
configuration, lifecycle, readiness, and failure domains. The router never
forwards one backend credential to the other, copies artifact bytes into Hubu,
or exposes provider credentials.

A tool call is validated against its public schema and forwarded to exactly one
owner. The router never composes a Hubu governance mutation and Gongbu execution
mutation into one call.

## Tool catalog

The router exposes one local read-only tool:

- `hubu_unified_capabilities`

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

The router preserves backend result semantics:

- Hubu successes contain pretty-printed JSON text and identical
  `structuredContent`.
- Gongbu JSON successes retain their compact text result and
  `isError: false`.
- `gongbu_get_artifact` returns safe metadata followed by PNG or JPEG content.
- Gongbu application errors remain `isError: true` with their sanitized error
  object.

The router does not add a success envelope, rename fields, translate currency
units, expose filesystem locations, or convert an application error into a
successful payload.

## Discovery and compatibility

Before `initialize`, and on a bounded interval afterward, the router probes
Hubu and Gongbu independently. `hubu_unified_capabilities` returns a sanitized
snapshot containing the unified contract and routing revision, each backend's
state and compatible version metadata, and all 33 tool names with owner and
availability.

The version-1 compatibility boundary requires:

| Surface | Required value |
| --- | --- |
| Unified contract | `hubu-gongbu-mcp-v1` |
| Routing revision | `1` |
| MCP protocol | `2024-11-05` |
| Hubu and Gongbu executor contract | `hubu-spend-executor-v4.2` |
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

The command writes the managed MCP entry, creates or reuses local capability
files, and renders Hubu's approval profile into client tool settings. Restart
Codex after changing the generated configuration.

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

- `HUBU_UNIFIED_HUBU_ENDPOINT`
- `HUBU_UNIFIED_HUBU_BEARER_TOKEN` or
  `HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE`
- the corresponding Gongbu endpoint, token, and account configuration
- `HUBU_RECONCILIATION_TOKEN` or its file form when reconciliation is enabled

Endpoint and credential values are never returned by capability discovery.

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
and reason. `operation_key` and optional `task_id` travel outside model-authored
arguments under `_meta["hubu.dev/platform-invocation"]`.

The router validates and injects those fields, rejects them when supplied in
ordinary arguments, and forwards an explicit null task ID when none exists.
The client platform must reuse the same invocation metadata for retries. The
router does not allocate operation keys.

The router implementation and ownership map live in
[`crates/hubu-unified-mcp`](../crates/hubu-unified-mcp).
