# Hubu / 户部

Hubu is an open-source spending control plane for AI agents.

The name comes from `户部`, the Ministry of Revenue in ancient China, which was
responsible for state revenue, budgeting, household registration, and approving
government expenditures. Hubu borrows that metaphor for autonomous software:
agents may act on behalf of users, but their spending should happen inside a
governed financial system with identity, budgets, approvals, and audit trails.

The core idea is simple: agents can have budgets, but they should not hold
private keys.

Humans fund a shared wallet and define spending policies. Agents register as
separate accounts and submit structured spend requests. Hubu validates each
request through deterministic policies and budget controls before executing
payment through the configured rail and recording successful money movement in
an audit ledger. The current local rail is intentionally mocked, but the policy,
budget, authorization, and ledger boundaries are built for real spend control.

This repository contains the Rust workspace for Hubu's policy engine, wallet
logic, shared models, local HTTP server, human developer CLI, benchmark tool,
and MCP transport adapter.

## What Hubu Does Today

- Registers agents with stable identity, version, account, and session records
- Evaluates spend requests through deterministic policy rules
- Issues spend authorization tokens for allowed requests
- Can authorize spend and freeze cap/budget capacity without executing payment,
  so a future Hubu-hosted vendor proxy can consume scoped authorization
- Creates user caps and agent-scoped single or recurring budgets
- Reserves cap/budget capacity before payment, then settles or releases both
  holds from the payment result
- Orchestrates mock payments after spend authorization
- Records successful payments in an immutable double-entry SQLite ledger
- Exposes a local `hubu-server`, human-facing `hubu` CLI, and MCP tools that
  agents can discover through configured MCP clients
- Protects local write APIs with a bearer token and separates agent-callable
  spend tools from human-gated setup, policy, and budget changes

## Crates

- `hubu-common`: shared agent identity, ownership, and session/account models
- `hubu-core`: registration, policy, and spend authorization logic
- `hubu-wallet`: payment orchestration, mock rails, and ledger recording
- `hubu-api`: local HTTP API and `hubu-server` binary
- `hubu-cli`: human developer `hubu` CLI binary
- `hubu-bench`: local benchmark tool for spend approval throughput and correctness
- `hubu-mcp`: MCP stdio transport adapter and `hubu-mcp-server` binary

## Quick Start

### 1. Set Up the Project and Binaries

From a local checkout, verify the workspace and install the local binaries:

```sh
cargo test --workspace
cargo install --path crates/hubu-cli
cargo install --path crates/hubu-api
cargo install --path crates/hubu-mcp
```

Start the local Hubu server:

```sh
hubu-server
```

### 2. Human Admin Setup

In another terminal from the same working directory, run the human/admin setup:
create the human account, register the agent, attach policy, set the user cap,
and create an agent budget.

```sh
hubu health

hubu register human --username alice-example --display-name "Alice Example" --email alice@example.com
hubu user list

hubu protocol agent-registration
hubu register agent --name local-agent --version local-dev
hubu agent list

hubu policy new-template --path policies/starter.yaml
hubu policy validate --path policies/starter.yaml
hubu policy add --path policies/starter.yaml
hubu policy list

hubu user cap set --amount 100
hubu budget create --agent-id AGENT_ID --amount 25
hubu user cap show
hubu budget list
```

Replace `AGENT_ID` with the public agent id printed by `hubu register agent` or
`hubu agent list`. The starter policy allows small spend, denies a blocked
merchant, and defaults everything else to `needs_approval`; edit the YAML before
`hubu policy add` for your local rules.

For human-initiated setup and administration, you can either run the CLI
commands yourself or ask an agent to do the work behind a human approval prompt.

### 3. Agent Spend Path

In normal use, agents call `hubu_authorize_spend` or `hubu_submit_spend`
through MCP. Humans can run the CLI spend commands to verify policy, budget
holds, settlement, and ledger behavior, but manually submitting operational
spend defeats the purpose of letting pre-approved agent spend tools flow
through Hubu's policy and budget controls.

To smoke-test the agent-initiated spend path from the CLI:

```sh
hubu spend authorize --agent-id AGENT_ID --amount 5 --reason "Reserve model API credits"
hubu spend --agent-id AGENT_ID --amount 20 --reason "Purchase API credits"
hubu user cap show
hubu budget list
hubu ledger list
```

Codex is one supported MCP harness, not a Hubu requirement. To make Hubu tools
discoverable to Codex agents, initialize the Codex MCP config:

```sh
hubu init codex --token-file ~/.hubu/hubu.auth-token
HUBU_AUTH_TOKEN_FILE=~/.hubu/hubu.auth-token hubu-server
```

If the server from step 1 is already running with a different token file,
restart it with the same `HUBU_AUTH_TOKEN_FILE` before restarting Codex. Codex
should then be able to discover Hubu MCP tools and call spend tools without
holding wallet credentials. For other MCP clients, use Hubu's tool annotations
or `hubu_client_approval_profile`; see
[docs/mcp-transport.md](docs/mcp-transport.md).

## Local Developer Tools

Hubu also includes local tools for understanding, exercising, and measuring the
system independently of any specific agent harness.

Open the interactive architecture visualizer:

```sh
open architecture/index.html
```

Run the scripted local walkthrough when you want a repeatable end-to-end trace:

```sh
./scripts/demo.sh
```

The script starts Hubu locally, registers a human and agent, adds a policy, sets
a user cap, creates an agent budget, submits allowed, failed-payment,
approval-required, and denied spend requests, then prints the resulting cap,
budget, and ledger state.

Run the conservative local benchmark:

```sh
./scripts/benchmark-local.sh
```

The benchmark starts an isolated local server, simulates multiple agents owned
by one user submitting paced spend requests, samples server CPU/RSS, and writes
a report under `target/hubu-bench/`. See
[docs/benchmarking.md](docs/benchmarking.md) for options and the current MVP
results.

## Local HTTP Server

`hubu-server` owns the local API, SQLite-backed state, policy and budget
managers, wallet orchestration, and ledger writes. By default it listens on
`http://127.0.0.1:8787`.

On startup the server reads `HUBU_AUTH_TOKEN`, or creates/reads
`hubu.auth-token` in the current directory. The CLI, MCP adapter, and benchmark
client read the same token automatically and send it as a local bearer token for
protected API routes. Use `HUBU_AUTH_TOKEN_FILE` when the server and clients
need to share a token file at a different path.

Restart `hubu-server` after rebuilding API or storage changes; reinstalling the
CLI only updates the client binary. To start over with clean local state:

```sh
./scripts/reset-local-state.sh --yes
```

## CLI

Install the local CLI so `hubu ...` works from your shell:

```sh
cargo install --path crates/hubu-cli
```

The CLI is the convenient human developer surface for Hubu. Use it to create
starter policy files, register humans and agents, attach policies, set user
caps, create agent budgets, test agent-initiated spend paths, inspect ledger
entries, and run client setup helpers such as Codex MCP configuration:

```sh
hubu --help
hubu init --help
hubu register --help
hubu policy --help
hubu budget --help
hubu spend --help
```

`hubu init codex` writes a managed `[mcp_servers.hubu]` block to Codex's
`config.toml`, creates or reuses a Hubu auth token file, and points Codex at the
`hubu-mcp-server` executable. It configures Codex to pre-approve Hubu spend
tool calls, while Hubu policy can still return `needs_approval` without
executing payment.

For human-initiated setup and administration, you can either run the CLI
commands yourself or ask an agent to do the work behind a human approval prompt.
Use the CLI for the default low-friction path. Use
`hubu init codex --trust-client-approval` only when the Codex client is trusted
to prompt before protected tools such as registration, policy changes, and
cap/budget creation.

`hubu spend authorize` and `hubu spend` exist in the CLI for local testing and
debugging of the agent spend path. In product usage, those requests should
normally originate from agents through MCP so the agent can act autonomously
while Hubu still enforces policy, caps/budgets, and audit trails.

See [docs/demo.md](docs/demo.md) for the scripted local walkthrough, expected
output, CLI reference, script pacing options, and current local limitations.

## MCP Transport

Hubu exposes agent-facing tools through a thin MCP stdio adapter over the local
HTTP API. Any MCP-compatible harness can launch `hubu-mcp-server`, inspect the
tool annotations, and apply Hubu's approval profile. Codex users can generate
that setup with:

```sh
hubu init codex
```

Agents can discover Hubu tools, inspect read-only state, and submit spend
requests without holding wallet credentials. Setup/admin actions such as
registration, policy changes, and cap/budget creation remain human-gated: humans
can run them directly with the CLI, or ask an agent to invoke protected MCP
tools after the client shows a human approval prompt. If policy returns
`needs_approval`, the MCP response reports that no payment was executed.

See [docs/mcp-transport.md](docs/mcp-transport.md) for install details,
approval profiles, manual MCP setup, and the current tool map.

## Policy Engine

Hubu's policy engine deterministically evaluates a structured `SpendRequest`
against a human-authored `Policy`. Policies are validated before evaluation, and
matching rule effects are merged with this precedence:

```txt
deny > needs_approval > allow > policy default
```

See [docs/policy-engine.md](docs/policy-engine.md) for the evaluation strategy,
rule format, validation behavior, and examples.

## Budget Controls

Budgets are hard spending limits for an agent or task over a time period. A
user cap is the owner-level guardrail for total spend owned by the current
user, not a fallback used only when an agent budget is missing. Agent budgets
can add narrower limits for a specific agent; the user cap is the outer limit
that keeps one user's aggregate spend below the configured amount.

Allowed spend reserves both cap and budget capacity before payment or
authorization. Successful payment settles both holds into consumed balance;
failed payment or unused authorization releases both holds. If the active cap or
applicable budget is exhausted, or policy returns `deny` / `needs_approval`,
Hubu does not execute payment.

See [docs/budget-controls.md](docs/budget-controls.md) for scope selection,
period overlap rules, hold lifecycle, cap renewal, recurring budgets, and CLI
examples.

## Documentation

- [docs/README.md](docs/README.md): docs index separating durable technical
  docs from working notes
- [architecture/index.html](architecture/index.html): interactive sketch-style
  architecture map with drill-down component diagrams and GitHub code links
- [docs/agent-registration-protocol.md](docs/agent-registration-protocol.md):
  v1 registration envelope, fingerprint fields, server validation, and
  low-friction human review flow
- [docs/demo.md](docs/demo.md): scripted local walkthrough and CLI reference
- [docs/benchmarking.md](docs/benchmarking.md): local spend benchmark usage and
  the latest MVP performance, scalability, and reliability report
- [docs/registration-flow.md](docs/registration-flow.md): agent registration
  model and flow
- [docs/policy-engine.md](docs/policy-engine.md): policy rule format and
  evaluation behavior
- [docs/budget-controls.md](docs/budget-controls.md): user caps, cap/budget
  scopes, recurring periods, hold lifecycle, and spend enforcement
- [docs/payment-ledger-flow.md](docs/payment-ledger-flow.md): payment
  orchestration and ledger recording flow
- [docs/mcp-transport.md](docs/mcp-transport.md): MCP stdio transport adapter
  and approval boundaries
- [docs/notes/](docs/notes/): non-normative planning, improvement, and handoff
  notes
