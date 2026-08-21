# Changelog

This changelog records the key user-facing and operational changes in each
stable Hubu release and versioned release candidate. It is intentionally
curated rather than being a complete list of commits or pull requests.

<!--
Future release template

## vX.Y.Z — YYYY-MM-DD

### Highlights

- Describe the most important new capabilities or product changes.

### Breaking or operational changes

- Describe compatibility changes, migration steps, or operator actions.

### Important fixes

- Describe only fixes that materially affect users or operators.
-->

## v0.2.0-rc.1 — 2026-08-20

### Highlights

- Unified Hubu and Gongbu in one Rust workspace and one verified four-binary
  release archive while preserving separate processes, storage, credentials,
  provider execution, artifacts, and failure domains.
- Made `hubu-unified-mcp` the only supported agent-facing MCP surface, routing
  governance and execution tools across the separate Hubu and Gongbu backends.
- Added durable Gongbu execution, provider adapters, normalized artifacts, and
  explicit pricing, authorization, reconciliation, and failure evidence.
- Added a dependency-aware local stack workflow for initialization, startup,
  readiness diagnosis, safe updates, recovery, and Codex handoff.
- Added immutable manifests, provenance, internal and release checksums, locked
  dependency inventories, license bundles, and published-archive smoke tests.

### Breaking or operational changes

- The original standalone Hubu and Gongbu MCP implementations are retired;
  clients must configure `hubu-unified-mcp`.
- The supported executor contract is `hubu-spend-executor-v4.2`.
- The temporary pre-launch release matrix contains macOS Intel and Apple
  silicon only; Linux is not advertised for this candidate.
- The release remains experimental, local-first, and backed by a mock payment
  rail. It is not approved for production or real-money use.

### Important fixes

- Preserved authorization, task, operation-key, and trusted execution-scope
  boundaries through provider execution and retry flows.
- Hardened credential redaction, immutable target revisions, claim recovery,
  terminal failure isolation, and release completeness checks.

## v0.1.0 — 2026-08-12

### Highlights

- Shipped the first experimental, local-first Hubu spending control plane for
  AI agents.
- Added stable agent identity and registration, deterministic policy
  evaluation, agent-scoped budgets, spend authorization, and budget holds.
- Added idempotent executor claims and finalization, including expired-claim
  reconciliation and settlement using actual provider cost.
- Persisted governance, payment, and immutable double-entry ledger state in
  SQLite, with optional structured file telemetry.
- Added the human-facing `hubu` CLI, local `hubu-server`, and the original Hubu
  MCP adapter for agent spend tools.
- Published immutable macOS and Linux release artifacts with checksums, build
  provenance, commit-addressed canaries, and an explicit stable promotion flow.

### Breaking or operational changes

- This release was experimental and used a mock local payment rail; it was not
  approved for production or real-money use.
- Protected local APIs required shared bearer-token configuration between the
  server, CLI, and MCP adapter.
