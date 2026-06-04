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
request through deterministic policies, merchant verification, risk guardrails,
and concurrency-safe budget controls before executing payment and recording the
result in an audit ledger.

This repository contains the Rust workspace for Hubu's policy engine, wallet
logic, MCP integration layer, shared models, optional HTTP API, and local demo
CLI.

## What Hubu Does Today

- Registers agents with stable identity, version, account, and session records
- Evaluates spend requests through deterministic policy rules
- Issues spend authorization tokens for allowed requests
- Creates human-scoped single or recurring budgets and tracks available balance
- Reserves budget before payment, then settles or releases the hold from the payment result
- Orchestrates mock payments after spend authorization
- Records successful payments in an immutable double-entry SQLite ledger
- Exposes a local `hubu-server` and `hubu` CLI for live demos

## Crates

- `hubu-common`: shared agent identity, ownership, and session/account models
- `hubu-core`: registration, policy, and spend authorization logic
- `hubu-wallet`: payment orchestration, mock rails, and ledger recording
- `hubu-mcp`: MCP server adapter layer
- `hubu-api`: local demo HTTP API and `hubu-server` binary
- `hubu-cli`: demo-friendly `hubu` CLI binary
- `hubu-bench`: local benchmark tool for spend approval throughput and correctness

## Quick Start

```sh
cargo test --workspace
```

Build the workspace:

```sh
cargo build
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

Then use the CLI from another terminal:

```sh
hubu register human --display-name "Alice Example" --email alice@example.com
hubu registration guidance
hubu register agent
hubu init --policy policy.yaml
hubu policy add --agent-id AGENT_ID --path policy.yaml
hubu agent list
hubu spend --agent-id AGENT_ID --amount 20 --reason "Purchase API credits"
hubu ledger list
```

`hubu register agent` uses the guidance-provided vendor/workspace name template
and the git short SHA as defaults. Pass `--name`, `--version`, or `--dry-run` to
override or inspect the computed registration envelope.

See [docs/demo.md](docs/demo.md) for the full walkthrough, expected output,
CLI installation notes, demo script pacing options, and known limitations.

Start a local Anvil chain:

```sh
./scripts/start-anvil.sh
```

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
