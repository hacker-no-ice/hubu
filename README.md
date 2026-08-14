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

- **Agent identity:** Registers stable agent identities, versions, accounts, and sessions
- **Policy and budgets:** Applies deterministic spend rules, advisory spending targets,
  and per-agent single or recurring budgets
- **Reliable authorization:** Issues spend tokens, reserves budget capacity, and makes
  authorization, claim, and finalization retries idempotent
- **Safe execution:** Grants exclusive executor claims and supports human-gated
  billed/not-billed reconciliation with durable evidence
- **Payments and accounting:** Orchestrates mock payments and records successful ones
  in an immutable double-entry SQLite ledger
- **Secure integrations:** Exposes local HTTP, CLI, and MCP interfaces with bearer-token
  protection and clear human-versus-agent permission boundaries

## Crates

- `hubu-common`: shared agent identity, ownership, and session/account models
- `hubu-core`: registration, policy, spend approval, and executor claim lifecycle services
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
create the human account, register the agent, attach policy, set an optional
advisory spending target,
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

hubu user spending-target set --amount 100
hubu budget create --agent-id AGENT_ID --amount 25
hubu user spending-target show
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
hubu spend authorize --operation-key PLATFORM_OPERATION_KEY --account-id ACCOUNT_ID --amount 5 --reason "Reserve model API credits"
hubu spend --operation-key PLATFORM_OPERATION_KEY --account-id ACCOUNT_ID --amount 20 --reason "Purchase API credits"
hubu user spending-target show
hubu budget list
hubu ledger list
```

The agent platform supplies a stable, namespaced operation key. Hubu stores the
authoritative workflow state under `(agent_id, operation_key)`, so replaying
authorization, claim, or finalization recovers the same result.

Codex is one supported MCP harness, not a Hubu requirement. To make Hubu tools
discoverable to Codex agents, initialize the Codex MCP config:

```sh
hubu init codex --token-file ~/.hubu/hubu.auth-token
HUBU_AUTH_TOKEN_FILE=~/.hubu/hubu.auth-token \
HUBU_RECONCILIATION_TOKEN_FILE=~/.hubu/hubu.reconciliation-token \
hubu-server
```

If the server from step 1 is already running with a different token file,
restart it with the same auth and reconciliation token files before restarting Codex. Codex
should then be able to discover Hubu MCP tools and call spend tools without
holding wallet credentials. For other MCP clients, use Hubu's tool annotations
or `hubu_client_approval_profile`; see
[docs/mcp-transport.md](docs/mcp-transport.md).

## Releases

A daily workflow checks `main` at 10:00 America/Los_Angeles and publishes an
immutable canary prerelease tied to the full source commit only when that commit
does not already have a canary. Stable SemVer releases are promoted
intentionally from a validated `main` revision. Consumers should pin an exact
release and checksum, not a rolling newest build. See
[docs/releases.md](docs/releases.md) for the supported targets, verification
and installation steps, promotion workflow, and rollback/retention policy.

Both binaries expose safe build metadata locally, and the server publishes the
same metadata without authentication:

```sh
hubu --version
hubu-server --version
curl http://127.0.0.1:8787/version
```

`product_version` versions Hubu itself, while `executor_contract` remains the
independently negotiated `hubu-spend-executor-v4` identifier.

## License

Hubu is dual-licensed under the [MIT License](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. Binary release archives
include both project licenses and the applicable
[third-party notices](THIRD-PARTY-NOTICES.md).

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
an advisory spending target, creates an agent budget, submits allowed,
failed-payment, approval-required, and denied spend requests, then prints the
resulting target, budget, and ledger state.

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

Human claim reconciliation additionally requires a distinct capability from
`HUBU_RECONCILIATION_TOKEN` or `HUBU_RECONCILIATION_TOKEN_FILE` (default
`hubu.reconciliation-token`). Executors should receive only the normal bearer
token. The CLI and approved MCP reconciliation tools send the second capability
only on reconciliation requests.

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
starter policy files, register humans and agents, attach policies, set advisory
spending targets, create agent budgets, test agent-initiated spend paths, inspect ledger
entries, and run client setup helpers. Command help is the best reference for
options and examples:

```sh
hubu --help
hubu init --help
hubu register --help
hubu user --help
hubu policy --help
hubu budget --help
hubu spend --help
hubu ledger --help
```

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
registration, policy changes, spending targets, and budget creation remain human-gated: humans
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

Each budget is a hard spending limit for exactly one agent over a time period. Every
allowed agent spend reserves exactly one active agent budget before payment or
authorization. Successful payment settles that hold into consumed balance;
failed payment or unused authorization releases it. If the applicable budget is
exhausted, or policy returns `deny` / `needs_approval`, Hubu does not execute
payment.

External executor work moves the hold from `frozen` to `claimed` with a separate
workload-profile lease. Expired claims remain frozen until a human reviews
provider billing and explicitly settles or releases them; operators can use
`hubu spend claim` and `hubu spend reconcile`. See
[docs/spend-executor-contract.md](docs/spend-executor-contract.md) for claim,
settle, release, reconciliation, and timing configuration.

A user spending target is advisory. Hubu compares it with the maximum
concurrent allocation of overlapping agent budgets and returns a warning when
the allocations exceed the target. The warning does not block budget creation
or spend, and the target never creates a hold.

See [docs/budget-controls.md](docs/budget-controls.md) for spending-target
advisories, agent ownership, period overlap rules, recurring budgets,
the hold lifecycle, and CLI examples.

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
- [docs/budget-controls.md](docs/budget-controls.md): advisory spending targets,
  agent-owned budgets, recurring periods, hold lifecycle, and spend enforcement
- [docs/payment-ledger-flow.md](docs/payment-ledger-flow.md): payment
  orchestration and ledger recording flow
- [docs/mcp-transport.md](docs/mcp-transport.md): MCP stdio transport adapter
  and approval boundaries
- [docs/notes/](docs/notes/): non-normative planning, improvement, and handoff
  notes
