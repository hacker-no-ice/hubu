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
request through deterministic policies and budget controls before executing a
mock payment and recording successful money movement in an audit ledger.

This repository contains the Rust workspace for Hubu's policy engine, wallet
logic, shared models, local demo HTTP API, demo CLI, benchmark tool, and MCP
transport adapter.

## What Hubu Does Today

- Registers agents with stable identity, version, account, and session records
- Evaluates spend requests through deterministic policy rules
- Issues spend authorization tokens for allowed requests
- Can authorize spend and freeze budget without executing payment, so a future
  Hubu-hosted vendor proxy can consume scoped authorization
- Creates human-scoped single or recurring budgets and tracks available balance
- Reserves budget before payment, then settles or releases the hold from the payment result
- Orchestrates mock payments after spend authorization
- Records successful payments in an immutable double-entry SQLite ledger
- Exposes a local `hubu-server` and `hubu` CLI for live demos

## Crates

- `hubu-common`: shared agent identity, ownership, and session/account models
- `hubu-core`: registration, policy, and spend authorization logic
- `hubu-wallet`: payment orchestration, mock rails, and ledger recording
- `hubu-api`: local demo HTTP API and `hubu-server` binary
- `hubu-cli`: demo-friendly `hubu` CLI binary
- `hubu-bench`: local benchmark tool for spend approval throughput and correctness
- `hubu-mcp`: MCP stdio transport adapter and `hubu-mcp-server` binary

## Quick Start

```sh
cargo test --workspace
```

Build the workspace:

```sh
cargo build
```

Open the interactive architecture visualizer:

```sh
open architecture/index.html
```

Run the automated local demo:

```sh
./scripts/demo.sh
```

The demo starts Hubu locally, registers an agent, adds a policy, creates a
recurring budget, submits allowed, failed-payment, approval-required, and denied
spend requests, then prints the resulting budget balance and ledger.

Run the conservative local benchmark:

```sh
./scripts/benchmark-local.sh
```

The benchmark starts an isolated local server, simulates multiple agents owned
by one user submitting paced spend requests, samples server CPU/RSS, and writes
a report under `target/hubu-bench/`. See
[docs/benchmarking.md](docs/benchmarking.md) for options and the current MVP
results.

## CLI Demo

Install the local CLI so `hubu ...` works from your shell:

```sh
cargo install --path crates/hubu-cli
```

Start the local demo server:

```sh
cargo run --bin hubu-server
```

On startup the server reads `HUBU_AUTH_TOKEN`, or creates/reads
`hubu.auth-token` in the current directory. The CLI, MCP adapter, and benchmark
client read the same token automatically and send it as a local bearer token for
protected API routes. Use `HUBU_AUTH_TOKEN_FILE` if the server and clients need
to share a token file at a different path.

Then use the CLI from another terminal:

```sh
hubu register human --display-name "Alice Example" --email alice@example.com
hubu registration guidance
hubu register agent
hubu init --policy policy.yaml
hubu policy add --agent-id AGENT_ID --path policy.yaml
hubu agent list
hubu budget create-recurring --amount 100 --recurrence daily --period-count 1
hubu spend authorize --agent-id AGENT_ID --amount 5 --reason "Generate Project Hubu logo"
hubu spend --agent-id AGENT_ID --amount 20 --reason "Purchase API credits"
hubu ledger list
```

`hubu register agent` uses the guidance-provided vendor/workspace name template
and the git short SHA as defaults. Pass `--name`, `--version`, or `--dry-run` to
override or inspect the computed registration envelope.

See [docs/demo.md](docs/demo.md) for the full walkthrough, expected output,
CLI installation notes, demo script pacing options, and known limitations.

## MCP Transport

Hubu includes an MCP stdio transport scaffold for agent-facing tool calls. Start
the local Hubu server first:

```sh
cargo run --bin hubu-server
```

Then point an MCP client at:

```sh
cargo run --bin hubu-mcp-server
```

Set `HUBU_URL` to target a non-default Hubu server URL. The MCP transport
forwards to the existing local HTTP API with the Hubu bearer token, marks
read-only tools as safe for agent inspection, and marks human/agent
registration, policy creation, and budget creation as human-approval-required
tools. Protected write tools are disabled unless the MCP process is started with
`HUBU_MCP_TRUST_CLIENT_APPROVAL=1` behind a trusted client that prompts the
human before destructive calls. Agents can submit spend requests directly; if
policy returns `needs_approval`, the MCP response includes
`requires_human_approval: true` and no payment is executed.

See [docs/mcp-transport.md](docs/mcp-transport.md) for the current tool and
approval model.

## Policy Engine

Hubu's policy engine deterministically evaluates a structured `SpendRequest`
against a human-authored `Policy`. Policies are validated before evaluation, and
matching rule effects are merged with this precedence:

```txt
deny > needs_approval > allow > policy default
```

See [docs/policy-engine.md](docs/policy-engine.md) for the evaluation strategy,
rule format, validation behavior, and examples.

## Documentation

- [architecture/index.html](architecture/index.html): interactive sketch-style
  architecture map with drill-down component diagrams and GitHub code links
- [docs/agent-registration-protocol.md](docs/agent-registration-protocol.md):
  v1 registration envelope, fingerprint fields, server validation, and
  low-friction human review flow
- [docs/demo.md](docs/demo.md): local server and CLI demo walkthrough
- [docs/demo-findings.md](docs/demo-findings.md): findings and improvement
  opportunities from the demo implementation
- [docs/benchmarking.md](docs/benchmarking.md): local spend benchmark usage and
  the latest MVP performance, scalability, and reliability report
- [docs/registration-flow.md](docs/registration-flow.md): agent registration
  model and flow
- [docs/policy-engine.md](docs/policy-engine.md): policy rule format and
  evaluation behavior
- [docs/payment-ledger-flow.md](docs/payment-ledger-flow.md): payment
  orchestration and ledger recording flow
- [docs/mcp-transport.md](docs/mcp-transport.md): MCP stdio transport scaffold
  and approval boundaries
