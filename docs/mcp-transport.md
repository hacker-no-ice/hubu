# MCP Transport

Hubu's default MCP transport is `hubu-unified-mcp`, a thin stdio router over the
separate Hubu and Gongbu HTTP APIs. It is meant to make both backends easy for
agents to use without moving governance into the router or moving provider
execution and artifacts out of Gongbu.
The MCP server publishes an approval profile so agent harnesses can configure
tool approvals up front instead of prompting for every spend request.

Run the HTTP server first:

```sh
cargo run --bin hubu-server
```

The Hubu HTTP server reads `HUBU_AUTH_TOKEN`, or creates/reads
`hubu.auth-token` in its current directory. The unified router reads
`HUBU_UNIFIED_HUBU_BEARER_TOKEN` or
`HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE` and forwards protected Hubu requests with
that bearer credential. Protected reconciliation additionally uses
`HUBU_RECONCILIATION_TOKEN` or `HUBU_RECONCILIATION_TOKEN_FILE`.
`hubu init codex` creates the local capability files and maps the required ones
into the generated unified entry.

When a local stack profile has been rendered, use
`hubu init codex --stack-profile /absolute/path/to/profile`. The command reads
the active client handoff and configures Codex to launch `hubu-unified-mcp`
with that profile's separate Hubu and Gongbu endpoints and credential files.
The stack itself does not start or own the stdio MCP process.

## Cheatsheet

First install or rebuild the local binaries:

```sh
cargo install --path crates/hubu-cli
cargo install --path crates/hubu-api
cargo install --path crates/hubu-unified-mcp
```

Unstamped local builds are for development and intentionally fail the unified
source-commit compatibility check. Use all binaries from one verified release
archive for an operator deployment.

Configure Codex once so agents in any project can discover Hubu MCP tools:

```sh
hubu init codex --token-file ~/.hubu/hubu.auth-token --trust-client-approval
```

Start the Hubu server with the same token file:

```sh
HUBU_AUTH_TOKEN_FILE=~/.hubu/hubu.auth-token \
HUBU_APPROVAL_TOKEN_FILE=~/.hubu/hubu.approval-token \
HUBU_RECONCILIATION_TOKEN_FILE=~/.hubu/hubu.reconciliation-token \
hubu-server
```

Then restart Codex. You do not normally start `hubu-unified-mcp` yourself;
Codex starts it from the generated MCP config when a session begins.

Use this mental model:

```txt
You start:
  hubu-server

The agent harness starts:
  hubu-unified-mcp

The agent sees:
  hubu_* and configured gongbu_* MCP tools

hubu-unified-mcp forwards independently to:
  hubu-server and gongbu-server
```

Rerun `hubu init codex` after upgrading Hubu when the generated Codex MCP config
changes. Reinstall `hubu-unified-mcp` after MCP server changes so new tool
metadata, instructions, and approval profiles are available to agent harnesses.
After initialization, the unified server monitors both backends independently
and emits the payload-free MCP `notifications/tools/list_changed` signal once
when the effective callable catalog changes. Clients should refresh
`tools/list`; backend diagnostics remain available only through the sanitized
`hubu_unified_capabilities` result.

Approval behavior:

- Read tools and spend tools should be callable without a pre-call approval
  prompt.
- Setup/admin tools such as registration, policy changes, spending targets, and budget creation
  should prompt before the tool call.
- For human-initiated setup/admin work, operators can either run the equivalent
  `hubu` CLI command themselves or ask an agent to invoke the protected MCP tool
  after the client shows a human approval prompt.
- If a spend response includes `requires_human_approval: true`, Hubu did not
  execute payment. The harness should show `approval.review`, wait for an
  explicit answer, then call `hubu_resolve_spend_approval` with `approve` or
  `deny`. The adapter attaches the separate human approval capability; executors
  receive neither that capability nor the reconciliation capability.

## Codex Setup

After installing the `hubu` CLI and `hubu-unified-mcp` binary, configure Codex
with:

```sh
hubu init codex
```

The command writes a managed `[mcp_servers.hubu]` block to
`~/.codex/config.toml` by default, points it at the local `hubu-unified-mcp`
executable, creates or reuses a Hubu auth token file, and renders Hubu's generic
approval profile into Codex per-tool approval overrides. `hubu_authorize_spend`
and `hubu_submit_spend` can run without an extra Codex approval prompt; Hubu
policy still returns `needs_approval` without executing payment when review is
required.
Restart Codex after running the command. Start `hubu-server` with the
token-file paths printed by the command so the server and MCP adapter share the
same bearer and human capability files.

After Codex can discover Hubu tools, human-initiated actions have two paths:
run `hubu` CLI commands directly, or ask the agent to perform the same
setup/admin work through MCP with a human approval prompt. The second path
requires a trusted client configuration:

```sh
hubu init codex --trust-client-approval
```

For a custom Codex config or prebuilt MCP server path:

```sh
hubu init codex --config ~/.codex/config.toml --mcp-server /path/to/hubu-unified-mcp
```

Add `--gongbu-endpoint URL --gongbu-token-file FILE` to configure the separate
Gongbu backend in the same MCP entry. Use binaries from one verified release
archive so the router and both backends report matching compatibility metadata.

Leave `--trust-client-approval` off when approval decisions will be resolved
directly with the Hubu CLI. Enable it when the Codex client is trusted to prompt
a human before invoking `hubu_resolve_spend_approval` or other protected tools.

## Manual MCP Setup

For other MCP clients, configure the client to launch the unified surface:

```sh
cargo run --bin hubu-unified-mcp
```

Then configure the harness from Hubu's MCP metadata:

- Read `tools/list` and honor `annotations.x_hubu_client_approval_mode`.
- Or call the read-only `hubu_client_approval_profile` tool during setup and map
  its `auto_approve_tools` and `prompt_before_call_tools` lists into the
  harness's approval settings.
- Auto-approve `hubu_authorize_spend` and `hubu_submit_spend` at the harness
  layer. If Hubu returns `requires_human_approval: true`, no payment was
  executed. Show `approval.review`, wait, and invoke the protected
  `hubu_resolve_spend_approval` tool with the returned `approval_request_id`.
- Prompt before setup/admin tools such as registration, policy changes, and
  spending-target or budget creation.

The unified server reads `HUBU_UNIFIED_HUBU_ENDPOINT` plus either
`HUBU_UNIFIED_HUBU_BEARER_TOKEN` or
`HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE`.
Protected write tools are disabled unless the MCP process is started with
`HUBU_MCP_TRUST_CLIENT_APPROVAL=1`. Only set that variable when the MCP client
is trusted to show a human approval prompt before invoking destructive tools.
The bearer token protects the local HTTP API from arbitrary localhost callers;
the MCP approval gate still protects human-intent workflows from agent-controlled
tool arguments. Hubu separately persists the pending and resolved spend decision.

These controls define a local demo trust boundary, not production
authentication. A same-user process that can read the bearer-token file or
control an authorized MCP client can act with that client's authority, and the
server does not issue scoped, short-lived workload credentials. The client-side
approval gate is useful only when the selected MCP client reliably enforces it;
it is not proof of a distinct human identity. The Hubu decision is durable, but
the MVP attributes it to the existing owner-authenticated client. Do not expose
this arrangement to a network or connect it to a real payment rail.

## Backend Compatibility and Readiness

Use all four runtime binaries from one verified release archive. The router
requires each configured backend to report the same product version and exact
source commit as itself, plus `hubu-spend-executor-v4.2`. Gongbu must also
report the accepted API, MCP schema, and MCP protocol versions. An unstamped
local build intentionally fails the source-commit compatibility check.

Call `hubu_unified_capabilities` for the sanitized cross-backend view. Interpret
each backend independently:

| State | Meaning and operator action |
| --- | --- |
| `available` | Health and all compatibility fields match; eligible tools are callable. |
| `degraded` | Gongbu is compatible but not ready. Reads and artifact retrieval remain available; repair Gongbu before creating executions. |
| `unavailable` | A health or version probe failed. Inspect only the named backend process. |
| `incompatible` | Product, source, executor, protocol, or schema metadata differs. Reinstall every binary from one archive. |
| `unconfigured` | An endpoint/credential pair is absent or incomplete. Configure both values and restart the MCP client. |

Partial availability is intentional. An unhealthy backend does not hide the
router capability tool or compatible tools owned by the other backend.
`gongbu_create_execution` additionally requires Hubu availability because it
consumes Hubu authorization. The router never falls back across backends,
queues a call, or retries an ambiguous mutation.

After the initialize handshake, each effective callable-catalog transition
emits one payload-free `notifications/tools/list_changed`; unchanged probes
emit nothing. Clients refresh `tools/list` after the notification and use
`hubu_unified_capabilities` for diagnostics rather than inferring backend state
from the notification.

## Approval Boundaries

The MCP tool list uses annotations to separate agent-callable reads, protected
human actions, and spend submission.

Human approval is required for:

- `hubu_register_human`
- `hubu_register_agent`
- `hubu_add_policy`
- `hubu_set_spending_target`
- `hubu_revoke_spending_target`
- `hubu_create_budget`
- `hubu_create_recurring_budget`
- `hubu_revoke_budget`
- `hubu_replace_budget`
- `hubu_resolve_spend_approval`
- `hubu_reconcile_vendor_billed_claim`
- `hubu_reconcile_vendor_did_not_bill_claim`

These tools are marked with `x_hubu_human_approval: "required"` and
`destructiveHint: true`. The scaffold does not accept approval as a tool
argument, because tool arguments are controlled by the caller. Instead, the
operator must start the MCP server with `HUBU_MCP_TRUST_CLIENT_APPROVAL=1`
after choosing an MCP client that enforces a human click for destructive tools.
Without that trusted-client gate, protected tools return an error and do not
forward requests to `hubu-server`.

Reconciliation adds a server-side boundary beyond the MCP prompt:
`hubu-server` and the human-facing MCP process must share
`HUBU_RECONCILIATION_TOKEN` or `HUBU_RECONCILIATION_TOKEN_FILE`. The MCP adapter
sends that capability only for the two reconciliation tools. An executor with
only the normal Hubu bearer token cannot reconcile a claim through direct HTTP.

Agents can call directly:

- `hubu_health`
- `hubu_registration_guidance`
- `hubu_client_approval_profile`
- `hubu_list_users`
- `hubu_list_agents`
- `hubu_show_spending_targets`
- `hubu_list_budgets`
- `hubu_list_ledger`
- `hubu_get_spend_approval`
- `hubu_get_executor_claim`
- `hubu_list_claims_requiring_reconciliation`
- `hubu_authorize_spend`
- `hubu_submit_spend`

These spend tools are the normal product path for operational spend: the agent
initiates the request, the client does not add an extra pre-call approval, and
Hubu decides through policy and budget controls. The CLI can call the same
server routes for local testing, but manual spend submission is not the intended
day-to-day workflow.

`hubu_submit_spend` forwards the spend request immediately. If policy returns
`allow`, the existing Hubu server reserves the agent budget, submits mock
payment, settles or releases the hold, and records successful payment in the ledger. If policy
returns `deny`, no payment is executed. If policy returns `needs_approval`, no
payment is executed and the MCP response includes:

```json
{
  "requires_human_approval": true,
  "approval_reason": "policy returned needs_approval; Hubu did not execute payment",
  "approval": {
    "approval_request_id": "<spend decision id>",
    "status": "pending",
    "review": {
      "operation_key": "codex:tool-call:01K2AZNQ",
      "account_id": "aga_example",
      "agent_id": "agt_example",
      "amount_cents": 500,
      "currency": "usd",
      "workload_profile": "default",
      "reason": "Generate the release artwork",
      "policy_summary": "policy defaulted to needs_approval because no automatic-allow rule matched"
    }
  }
}
```

The harness shows the complete review object and waits. It can read the durable
state through `hubu_get_spend_approval`. After the human chooses, it invokes
`hubu_resolve_spend_approval` with the approval request ID and `approve` or
`deny`. Approval reserves the immutable maximum and returns the normal spend
authorization; it does not call a provider. Repeating the same resolution is
idempotent, and a conflicting resolution is rejected.

`hubu_authorize_spend` uses the same policy and budget checks as
`hubu_submit_spend`, but stops after issuing a spend authorization token and
freezing the agent budget. It does not submit payment or write a ledger transaction. This
is the handoff point for future Hubu-hosted vendor/model proxy tools that need
to consume a scoped spend authorization without exposing provider credentials to
the agent. Both tools require a stable, agent-scoped `operation_key` supplied by
the client platform or orchestrator, not invented by the model. Hubu durably
stores workflow state under that key; identical retries recover the original
workflow and changed scope is rejected. An optional `task_id` is a separate
trusted business correlation, while `reason` remains model-visible descriptive
audit context.

The MCP tool schemas expose `reason` but not `operation_key` or `task_id`.
For every spend call, the client attaches trusted metadata outside
model-authored `arguments`:

```json
{
  "name": "hubu_authorize_spend",
  "arguments": {
    "account_id": "aga_example",
    "amount_cents": 500,
    "reason": "Generate the release artwork"
  },
  "_meta": {
    "hubu.dev/platform-invocation": {
      "platform": "codex",
      "installation_id": "install_7f3a",
      "invocation_id": "provider-call-01K2AZNQ",
      "operation_key": "codex:tool-call:01K2AZNQ",
      "task_id": "linear:HUB-73"
    }
  }
}
```

The adapter validates and injects the trusted fields and rejects either field
inside model-authored arguments. A missing or null trusted `task_id` is
forwarded as explicit null, preventing the legacy reason-to-task mapping from
granting model text a trusted identity. The client platform must reuse the same
metadata for retries. Durable platform-wide allocation and recovery remain the
responsibility of HUB-31; this adapter does not allocate operation keys.

## Tool Mapping

| MCP tool | HTTP route | Approval |
| --- | --- | --- |
| `hubu_health` | `GET /health` | none |
| `hubu_registration_guidance` | `GET /registration/guidance` | none |
| `hubu_client_approval_profile` | local MCP profile | none |
| `hubu_register_human` | `POST /init` | required |
| `hubu_list_users` | `GET /users` | none; marks current local user |
| `hubu_register_agent` | `POST /agents/register` | required |
| `hubu_add_policy` | `POST /policies` | required |
| `hubu_apply_policy` | `POST /policies` | required |
| `hubu_show_policy` | `GET /policies/show` | none |
| `hubu_export_policy` | `GET /policies/export` | none |
| `hubu_policy_history` | `GET /policies/history` | none |
| `hubu_policy_diff` | `GET /policies/diff` | none |
| `hubu_set_spending_target` | `POST /user/spending-target` | required |
| `hubu_revoke_spending_target` | `POST /user/spending-target/revoke` | required |
| `hubu_show_spending_targets` | `GET /user/spending-target` | none |
| `hubu_create_budget` | `POST /budgets` | required |
| `hubu_create_recurring_budget` | `POST /budgets/series` | required |
| `hubu_revoke_budget` | `POST /budgets/revoke` | required |
| `hubu_replace_budget` | `POST /budgets/replace` | required |
| `hubu_authorize_spend` | `POST /spend/authorize` | conditional on policy result |
| `hubu_submit_spend` | `POST /spend` | conditional on policy result |
| `hubu_get_spend_approval` | `GET /spend/approval?approval_request_id=...` | none |
| `hubu_resolve_spend_approval` | `POST /spend/approval/resolve` | required |
| `hubu_list_agents` | `GET /agents` | none; defaults to current user |
| `hubu_list_budgets` | `GET /budgets` | none |
| `hubu_list_ledger` | `GET /ledger` | none |
| `hubu_get_executor_claim` | `GET /spend/executor/claim?claim_id=...` | none |
| `hubu_list_claims_requiring_reconciliation` | `GET /spend/executor/reconciliation` | none |
| `hubu_reconcile_vendor_billed_claim` | `POST /spend/executor/settle` | required |
| `hubu_reconcile_vendor_did_not_bill_claim` | `POST /spend/executor/release` | required |

All non-public HTTP routes behind these tools require the Hubu bearer token.
The two reconciliation mutations additionally require the distinct
`X-Hubu-Reconciliation-Capability` header. Public HTTP routes are limited to
health and protocol guidance.
