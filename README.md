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
logic, MCP integration layer, shared models, and optional HTTP API.

## Crates

- `hubu-common`: shared agent identity, ownership, and session/account models
- `hubu-core`: core policy engine and budget manager
- `hubu-wallet`: wallet logic, private key handling, signing, and future Alloy integration
- `hubu-mcp`: MCP server adapter layer
- `hubu-api`: optional standalone HTTP API layer

## Quick Start

```sh
cargo test --workspace
```

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
