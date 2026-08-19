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
- [unified-mcp-migration.md](unified-mcp-migration.md): supported migration from
  deprecated standalone MCP entries to the only supported unified surface and
  health validation
- [unified-mcp-contract.md](unified-mcp-contract.md): accepted unified
  Hubu–Gongbu MCP names, schemas, backend routing, compatibility negotiation,
  partial availability, and standalone migration gates
- [canaries/HUB-96-unified-mcp-migration-canary.md](canaries/HUB-96-unified-mcp-migration-canary.md):
  packaged unified MCP migration evidence and the explicit standalone
  deprecation no-go decision
- [canaries/HUB-111-final-unified-mcp-cutover-go.md](canaries/HUB-111-final-unified-mcp-cutover-go.md):
  final immutable-release evidence and conditional zero-user cutover GO
- [HUB-97-unified-mcp-cutover.md](HUB-97-unified-mcp-cutover.md): immediate
  supported-surface and release-package cutover notes plus HUB-98 follow-up
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
