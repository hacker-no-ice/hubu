# Hubu Documentation

This directory keeps durable technical documentation at the top level. Temporary
planning, improvement, and handoff notes live under [`notes/`](notes/).

## Technical Docs

- [agent-registration-protocol.md](agent-registration-protocol.md): v1 agent
  registration envelope, guidance payload, canonicalization, and review flow
- [registration-flow.md](registration-flow.md): registration persistence and
  runtime flow
- [policy-engine.md](policy-engine.md): policy rule format, validation, and
  deterministic evaluation behavior
- [budget-controls.md](budget-controls.md): advisory spending targets,
  agent-owned budgets, recurring periods, hold lifecycle, and spend enforcement
- [payment-ledger-flow.md](payment-ledger-flow.md): payment orchestration and
  ledger recording flow
- [spend-executor-contract.md](spend-executor-contract.md): external executor
  contract for scoped spend authorization, validation, settlement, and release
- [multi-spend-mandate-protocol.md](multi-spend-mandate-protocol.md): deferred
  v5 design research for multiple provider calls under one authorized maximum
- [mcp-transport.md](mcp-transport.md): MCP adapter setup, approval boundaries,
  and tool mapping
- [unified-mcp-contract.md](unified-mcp-contract.md): accepted unified
  Hubu–Gongbu MCP names, schemas, backend routing, compatibility negotiation,
  partial availability, and standalone migration gates
- [benchmarking.md](benchmarking.md): local benchmark usage and current MVP
  performance notes
- [demo.md](demo.md): scripted local walkthrough and CLI reference
- [releases.md](releases.md): immutable prereleases, stable promotion,
  supported targets, verification, rollback, and retention
- [gongbu-cutover.md](gongbu-cutover.md): unified canary validation, compatibility
  evidence, legacy-repository retirement, and the independent rollback baseline
- [repository-security.md](repository-security.md): dependency scanning,
  immutable action pins, exception review, and post-merge GitHub controls
- [gongbu/README.md](gongbu/README.md): Gongbu execution-plane overview and
  index for its persistent server, sandbox, provider, credential, and MCP
  runbooks; all paths and commands assume this repository root

## Working Notes

The files under [`notes/`](notes/) are non-normative. They preserve useful
context, improvement ideas, and handoff inventories without making the main docs
folder feel like a grab bag.
