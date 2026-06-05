# MCP Transport

Hubu's MCP transport is a thin stdio adapter over the local `hubu-server` HTTP
API. It is meant to make Hubu easy for agents to use without moving registration,
policy, budget, spend, payment, or ledger logic out of the existing server.

Run the HTTP server first:

```sh
cargo run --bin hubu-server
```

Then configure an MCP client to launch:

```sh
cargo run --bin hubu-mcp-server
```

The MCP server reads `HUBU_URL` and defaults to `http://127.0.0.1:8787`.

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
`destructiveHint: true`. The scaffold also requires the tool arguments to
include `human_approved: true` before forwarding the request to `hubu-server`.
That field is removed before the HTTP request is sent.

Agents can call directly:

- `hubu_health`
- `hubu_registration_guidance`
- `hubu_list_agents`
- `hubu_list_budgets`
- `hubu_list_ledger`
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

## Tool Mapping

| MCP tool | HTTP route | Approval |
| --- | --- | --- |
| `hubu_health` | `GET /health` | none |
| `hubu_registration_guidance` | `GET /registration/guidance` | none |
| `hubu_register_human` | `POST /init` | required |
| `hubu_register_agent` | `POST /agents/register` | required |
| `hubu_add_policy` | `POST /policies` | required |
| `hubu_create_budget` | `POST /budgets` | required |
| `hubu_create_recurring_budget` | `POST /budgets/series` | required |
| `hubu_submit_spend` | `POST /spend` | conditional on policy result |
| `hubu_list_agents` | `GET /agents` | none |
| `hubu_list_budgets` | `GET /budgets` | none |
| `hubu_list_ledger` | `GET /ledger` | none |
