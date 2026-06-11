# MCP Transport

Hubu's MCP transport is a thin stdio adapter over the local `hubu-server` HTTP
API. It is meant to make Hubu easy for agents to use without moving registration,
policy, budget, spend, payment, or ledger logic out of the existing server.
The MCP server publishes an approval profile so agent harnesses can configure
tool approvals up front instead of prompting for every spend request.

Run the HTTP server first:

```sh
cargo run --bin hubu-server
```

The HTTP server reads `HUBU_AUTH_TOKEN`, or creates/reads `hubu.auth-token` in
its current directory. The MCP adapter reads `HUBU_AUTH_TOKEN` or the same token
file and forwards protected HTTP requests with `Authorization: Bearer ...`.
Use `HUBU_AUTH_TOKEN_FILE` when the server and adapter run from different
working directories.

## Cheatsheet

First install or rebuild the local binaries:

```sh
cargo install --path crates/hubu-cli
cargo install --path crates/hubu-api
cargo install --path crates/hubu-mcp
```

Configure Codex once so agents in any project can discover Hubu MCP tools:

```sh
hubu init codex --token-file ~/.hubu/hubu.auth-token --trust-client-approval
```

Start the Hubu server with the same token file:

```sh
HUBU_AUTH_TOKEN_FILE=~/.hubu/hubu.auth-token hubu-server
```

Then restart Codex. You do not normally start `hubu-mcp-server` yourself; Codex
starts it from the generated MCP config when a session begins.

Use this mental model:

```txt
You start:
  hubu-server

The agent harness starts:
  hubu-mcp-server

The agent sees:
  hubu_* MCP tools

hubu-mcp-server forwards to:
  hubu-server
```

Rerun `hubu init codex` after upgrading Hubu when the generated Codex MCP config
changes. Reinstall `hubu-mcp-server` after MCP server changes so new tool
metadata, instructions, and approval profiles are available to agent harnesses.

Approval behavior:

- Read tools and spend tools should be callable without a pre-call approval
  prompt.
- Setup/admin tools such as registration, policy changes, and budget creation
  should prompt before the tool call.
- If a spend response includes `requires_human_approval: true`, Hubu did not
  execute payment; the harness should show the response to the human.

## Codex Setup

After installing the `hubu` CLI and `hubu-mcp-server` binary, configure Codex
with:

```sh
hubu init codex
```

The command writes a managed `[mcp_servers.hubu]` block to
`~/.codex/config.toml` by default, points it at the local `hubu-mcp-server`
executable, creates or reuses a Hubu auth token file, and renders Hubu's generic
approval profile into Codex per-tool approval overrides. `hubu_authorize_spend`
and `hubu_submit_spend` can run without an extra Codex approval prompt; Hubu
policy still returns `needs_approval` without executing payment when review is
required.
Restart Codex after running the command. Start `hubu-server` with the
`HUBU_AUTH_TOKEN_FILE` path printed by the command so the server and MCP adapter
share the same bearer token.

For a custom Codex config or prebuilt MCP server path:

```sh
hubu init codex --config ~/.codex/config.toml --mcp-server /path/to/hubu-mcp-server
```

Leave `--trust-client-approval` off for normal agent spend workflows. Add it
only when the Codex client is trusted to prompt a human before invoking
destructive MCP tools such as registration, policy changes, or budget creation.

## Manual MCP Setup

For other MCP clients, configure the client to launch:

```sh
cargo run --bin hubu-mcp-server
```

Then configure the harness from Hubu's MCP metadata:

- Read `tools/list` and honor `annotations.x_hubu_client_approval_mode`.
- Or call the read-only `hubu_client_approval_profile` tool during setup and map
  its `auto_approve_tools` and `prompt_before_call_tools` lists into the
  harness's approval settings.
- Auto-approve `hubu_authorize_spend` and `hubu_submit_spend` at the harness
  layer. If Hubu returns `requires_human_approval: true`, no payment was
  executed and the harness should surface the response to the human.
- Prompt before setup/admin tools such as registration, policy changes, and
  budget creation.

The MCP server reads `HUBU_URL` and defaults to `http://127.0.0.1:8787`.
Protected write tools are disabled unless the MCP process is started with
`HUBU_MCP_TRUST_CLIENT_APPROVAL=1`. Only set that variable when the MCP client
is trusted to show a human approval prompt before invoking destructive tools.
The bearer token protects the local HTTP API from arbitrary localhost callers;
the MCP approval gate still protects human-intent workflows from agent-controlled
tool arguments.

## Approval Boundaries

The MCP tool list uses annotations to separate agent-callable reads, protected
human actions, and spend submission.

Human approval is required for:

- `hubu_register_human`
- `hubu_register_agent`
- `hubu_add_policy`
- `hubu_create_budget`
- `hubu_create_recurring_budget`

These tools are marked with `x_hubu_human_approval: "required"` and
`destructiveHint: true`. The scaffold does not accept approval as a tool
argument, because tool arguments are controlled by the caller. Instead, the
operator must start the MCP server with `HUBU_MCP_TRUST_CLIENT_APPROVAL=1`
after choosing an MCP client that enforces a human click for destructive tools.
Without that trusted-client gate, protected tools return an error and do not
forward requests to `hubu-server`.

Agents can call directly:

- `hubu_health`
- `hubu_registration_guidance`
- `hubu_client_approval_profile`
- `hubu_list_users`
- `hubu_list_agents`
- `hubu_list_budgets`
- `hubu_list_ledger`
- `hubu_authorize_spend`
- `hubu_submit_spend`

`hubu_submit_spend` forwards the spend request immediately. If policy returns
`allow`, the existing Hubu server reserves budget, submits mock payment, settles
or releases the hold, and records successful payment in the ledger. If policy
returns `deny`, no payment is executed. If policy returns `needs_approval`, no
payment is executed and the MCP response includes:

```json
{
  "requires_human_approval": true,
  "approval_reason": "policy returned needs_approval; Hubu did not execute payment"
}
```

This first scaffold exposes the approval boundary but does not yet implement a
durable approval queue or a follow-up endpoint that resumes payment after a
human approves a `needs_approval` spend decision.

`hubu_authorize_spend` uses the same policy and budget checks as
`hubu_submit_spend`, but stops after issuing a spend authorization token and
freezing budget. It does not submit payment or write a ledger transaction. This
is the handoff point for future Hubu-hosted vendor/model proxy tools that need
to consume a scoped spend authorization without exposing provider credentials to
the agent.

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
| `hubu_create_budget` | `POST /budgets` | required |
| `hubu_create_recurring_budget` | `POST /budgets/series` | required |
| `hubu_authorize_spend` | `POST /spend/authorize` | conditional on policy result |
| `hubu_submit_spend` | `POST /spend` | conditional on policy result |
| `hubu_list_agents` | `GET /agents` | none; defaults to current user |
| `hubu_list_budgets` | `GET /budgets` | none |
| `hubu_list_ledger` | `GET /ledger` | none |

All non-public HTTP routes behind these tools require the Hubu bearer token.
Public HTTP routes are limited to health and protocol guidance.
