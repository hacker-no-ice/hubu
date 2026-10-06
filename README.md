# Hubu / 户部

[![CI](https://github.com/hacker-no-ice/hubu/actions/workflows/ci.yml/badge.svg)](https://github.com/hacker-no-ice/hubu/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/hacker-no-ice/hubu)](https://github.com/hacker-no-ice/hubu/releases)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![Docs](https://img.shields.io/badge/docs-hubustack.dev-informational)](https://hubustack.dev/)

Hubu is an open-source spending control plane for AI agents. Humans define
policies and budgets; agents submit structured spend requests; and Hubu
authorizes, executes, and records approved spending without giving agents
private keys.

> [!WARNING]
> **Project status: experimental and local-first.** Hubu runs a complete managed
> governance and execution stack, including configured third-party provider
> workloads that can incur real charges. Use live mode cautiously, with reviewed
> targets, conservative policies, and strict budgets. No production direct payment
> rail is supported yet. The built-in direct payment path remains a mock, and Hubu
> is not approved as money-grade production financial infrastructure.

## How it works

```text
 agent (Codex or another MCP client)
   │  hubu_submit_governed_execution
   ▼
 hubu-unified-mcp ── one agent-facing MCP surface
   │
   ▼
 Hubu  ── checks identity, policy, and budget; reserves funds; records the ledger
   │  authorized work only (versioned executor contract)
   ▼
 Gongbu ── runs the provider call durably (Temporal), stores artifacts, reports cost
   │
   ▼
 Hubu settles the actual cost and returns the result to the agent
```

- **Agents never hold keys.** Provider credentials stay with Gongbu; payment
  authority stays with Hubu.
- **Every spend is checked before it runs.** A request is allowed and reserved,
  denied without consuming funds, or held for an explicit human decision.
- **Everything is recorded.** Authorizations and ledger entries can be
  inspected from the CLI or MCP.

The names come from two ministries of imperial China: **Hubu (户部)**, the
Ministry of Revenue, governs resources; **Gongbu (工部)**, the Ministry of Works,
performs the work. Both live in this Rust workspace but run as separate
processes with separate storage, credentials, and failure domains.

| Component | Role |
| --- | --- |
| `hubu-server` | Governance: registration, policies, budgets, authorization, ledger |
| `gongbu-server` | Execution: provider calls, retries, artifacts, cost reporting |
| `hubu-unified-mcp` | The single MCP server agents connect to |
| `hubu` | CLI for stack management, administration, and independent verification |

[Explore the interactive architecture →](https://hubustack.dev/architecture/)

## What works today

| Area | Status |
| --- | --- |
| Platform | macOS, built from an exact release tag |
| Agent clients | Codex via `hubu init codex`; other MCP clients via [manual configuration](docs/unified-mcp.md#setup) |
| Live providers | Gemini Developer API and FLUX.2 Pro ([live provider operations](docs/operations/live-providers.md)) |
| Sandbox | Complete stack with a deterministic, non-billable provider fixture |
| Governance only | `hubu-only` mode, without Gongbu |
| Your own executor | Any service implementing `hubu-spend-executor-v4.4` ([external executor](docs/external-executor.md)) |
| Direct payments | Mock only |

## Requirements

- macOS with Git and Xcode Command Line Tools
- [`rustup`](https://rustup.rs/) (the checkout pins the exact toolchain)
- `protoc`, for example `brew install protobuf`
- The Temporal CLI (`hubu stack init --install-temporal` can install it)
- [Codex](https://github.com/openai/codex) for the guided path below, or any MCP
  client configured manually

## Quick start

This path runs the complete stack in **sandbox** mode, which needs no provider
credentials and cannot incur charges. For a step-by-step walkthrough with
CLI-only administration, see the [local stack quick start](docs/local-stack.md).

### 1. Install from an exact release

Pick an immutable `vX.Y.Z` tag from
[Releases](https://github.com/hacker-no-ice/hubu/releases) and copy its full
source commit, then build all four binaries locally:

```sh
tag=vX.Y.Z
expected_commit=FULL_40_CHARACTER_COMMIT_SHA

git clone --depth 1 --branch "$tag" https://github.com/hacker-no-ice/hubu.git
cd hubu
./scripts/install-from-source.sh --expected-commit "$expected_commit"
```

Binaries install to `~/.local/bin`. They are compiled locally and are not
Developer ID-signed or notarized; the supported flow does not ask you to bypass
Gatekeeper. With `~/.local/bin` on your `PATH`, confirm each binary resolves to
this installation and reports the intended release:

```sh
for binary in hubu hubu-server gongbu-server hubu-unified-mcp; do
  command -v "$binary"
  "$binary" --version
done
```

See [release installation](docs/operations/releases.md#install-an-exact-release-from-source-macos)
for trust, custom-prefix, update, and uninstall details.

### 2. Create, start, and check a sandbox stack

```sh
profile="$HOME/hubu-sandbox"
hubu stack init --mode sandbox --install-temporal --profile "$profile"
hubu stack select --profile "$profile"
hubu stack doctor
hubu stack start
hubu stack status
```

Use `--mode local-stack` for approved live provider targets, or `--mode
hubu-only` for governance alone. The
[complete local stack examples](docs/configuration/local-stack/v1/examples.md)
show each mode.

### 3. Connect Codex

```sh
hubu init codex --stack-profile "$profile" --trust-client-approval
```

`--trust-client-approval` lets Codex use Hubu setup and administration tools, so
it can register identities, apply policies, and create budgets in step 4; Codex
still asks for your confirmation before each of those calls. To do that
administration from the terminal instead, pass `--no-trust-client-approval` and
follow the [CLI administration reference](docs/cli.md). Restart Codex
afterwards.

### 4. Set up governance from Codex

In a new Codex thread (any repository), ask:

```text
Read hubu_registration_guidance. Register the human Alice Example with username
alice-example and an agent named image-designer with version local-dev. Then
read and follow /absolute/path/to/hubu/skills/hubu-policy-authoring/SKILL.md to
draft a user-default policy that allows image generation only through the
configured execution targets, caps each request at $0.10, and requires approval
for anything unmatched. Show me the policy and wait for my approval before
applying it. After I approve, create a $1 USD budget for the agent.
```

Replace `/absolute/path/to/hubu` with your clone. Then verify the result
independently from the terminal:

```sh
hubu agent list
hubu policy show
hubu budget list
```

### 5. Run a governed request

```text
Call gongbu_list_execution_targets and choose the sandbox fixture. Use
hubu_submit_governed_execution with the agent's account ID and the target's
returned execution scope to generate one image of a blue circle. Show any
approval request before asking me to approve or deny it, and return the artifact.
```

Hubu evaluates the policy and reserves budget before Gongbu runs the fixture.
The result is `succeeded`, `in_progress`, `approval_required`, `denied`, or
`failed`; [composite governed execution](docs/unified-mcp.md#composite-governed-execution)
explains each one. Inspect what Hubu recorded with
`hubu spend authorizations --limit 5`.

## Next steps

- [Local stack quick start](docs/local-stack.md): the full first-run guide.
- [Managing a local stack](docs/operations/managing-a-stack.md): logs, stopping,
  configuration changes, and rollback.
- [Live provider operations](docs/operations/live-providers.md): move to real,
  billable providers in a separate profile.
- [Policy engine](docs/policy-engine.md): write your own spending rules.
- [Unified MCP surface](docs/unified-mcp.md): every agent-facing tool.
- [Full documentation](https://hubustack.dev/): concepts, architecture,
  operations, and protocol references.

## Development

Hubu and Gongbu share one Cargo workspace (MSRV Rust 1.88). Install `protoc`,
then run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Use package selectors such as `-p gongbu-api` for focused work. Hubu and Gongbu
must not depend on each other directly; they communicate through the
[versioned executor contract](docs/spend-executor-contract.md). See
[AGENTS.md](AGENTS.md) for repository conventions and the
[changelog](CHANGELOG.md) for release history.

## Feedback and security

- [Send feedback](docs/feedback.md): report a bug or suggest an idea, or run
  `hubu feedback`.
- Suspected vulnerabilities: follow the [security policy](SECURITY.md) and
  [report privately](https://github.com/hacker-no-ice/hubu/security/advisories/new).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option. See
[third-party notices](THIRD-PARTY-NOTICES.md).
