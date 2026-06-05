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
Protected write tools are disabled unless the MCP process is started with
`HUBU_MCP_TRUST_CLIENT_APPROVAL=1`. Only set that variable when the MCP client
is trusted to show a human approval prompt before invoking destructive tools.

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
- `hubu_list_agents`
- `hubu_list_budgets`
- `hubu_list_ledger`
- `hubu_authorize_spend`
- `hubu_generate_image`
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
the agent. Callers may pass `budget_id` to reserve a specific active user or
agent budget, which lets the logo flow use a dedicated `$5` agent budget without
changing the default spend path.

`hubu_generate_image` consumes one spend authorization token through
`POST /model-calls/image`, settles the matching frozen budget hold, records the
payment in the ledger, writes a local demo SVG artifact, and returns image
output metadata. The current provider is a local `hubu-demo` adapter; real
vendor adapters can be added behind the same Hubu-hosted boundary without
passing API keys to agents. Provider name, model, API key, and output directory
are server-side Hubu configuration (`HUBU_IMAGE_PROVIDER_*`,
`HUBU_IMAGE_PROXY_MERCHANT`, `HUBU_IMAGE_OUTPUT_DIR`); agent requests must match
the configured provider/model, and the spend authorization must be scoped to the
configured image proxy merchant.

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
| `hubu_authorize_spend` | `POST /spend/authorize` | conditional on policy result |
| `hubu_generate_image` | `POST /model-calls/image` | conditional on policy result |
| `hubu_submit_spend` | `POST /spend` | conditional on policy result |
| `hubu_list_agents` | `GET /agents` | none |
| `hubu_list_budgets` | `GET /budgets` | none |
| `hubu_list_ledger` | `GET /ledger` | none |
