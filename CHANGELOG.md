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

## Unreleased

## v0.2.0 — 2026-08-26

### Highlights

- Promoted the source commit validated by `v0.2.0-rc.2` to the stable
  `v0.2.0` release with no intervening source changes. The stable artifacts
  were rebuilt with the stable product version and revalidated; see the
  `v0.2.0-rc.2` entry below for the complete user-facing and operational
  changes.

### Breaking or operational changes

- Compatibility requirements and experimental-use limitations are unchanged
  from `v0.2.0-rc.2`.

### Important fixes

- No additional source fixes were introduced after `v0.2.0-rc.2`.

## v0.2.0-rc.2 — 2026-08-25

### Highlights

- Removed the temporary Hubu bootstrap from managed local-stack setup. Managed
  users no longer configure service credential locations: `stack start`
  launches the final Hubu once, lets it create private capabilities, completes
  a Gongbu-owned credential handoff, and then starts Gongbu.
- Made Gongbu principal-neutral so multiple Hubu agents can execute against the
  same running stack while Hubu remains authoritative for per-operation
  identity, authorization, and settlement.
- Added durable normalized operations to `hubu-unified-mcp`, including private
  continuation binding, restart recovery, public status, and background
  resolution of accepted work to a durable terminal adapter state while
  preserving outcomes that still require financial reconciliation.
- Added selectable stack-profile registration and a versioned local-stack
  configuration reference covering topology, credentials, providers, examples,
  and operator decisions.

### Breaking or operational changes

- Advanced the executor contract to `hubu-spend-executor-v4.3`: Hubu's
  `workload_profile` is now `lease_profile`, `default_profile` is now
  `default_lease_profile`, and `profiles` is now `lease_profiles`. Authorization
  TTL is global, lease profiles configure claim TTL only, and Gongbu workload
  types no longer have to equal Hubu lease profiles. `HUBU_SPEND_TIMING_CONFIG`
  is now `HUBU_LEASE_CONFIG`; existing local profiles and Hubu/Gongbu databases
  must be recreated, and all four production binaries must be installed
  together.
- Recreated managed profiles generate Gongbu server configuration schema v3,
  which no longer selects an account or agent at startup. Manually managed
  configurations must migrate to v3 and remove `hubu.account_id`,
  `hubu.agent_id`, and `authentication.caller_account_id`; schemas v1 and v2
  are rejected.
- Retired pricing catalog schema v1. Live profiles must define schema-v2 exact
  rational price components and selector-qualified rules for every enabled
  image size, replacing flat `unit` and `unit_amount_minor` fields with
  `components` entries that use `rate_numerator_minor` and `rate_denominator`.
- `gongbu_create_execution` now returns a durable acknowledgement instead of
  waiting for provider completion; clients observe the returned public handle
  with `hubu_operation_status` and never submit a replacement
  (`replacement_safe` is false). An ambiguous initial call must be redelivered
  exactly with the same harness identity. Unified-MCP operation-registry schema
  v4 does not upgrade earlier registry state, so start this candidate with fresh
  adapter registry state.
- The temporary pre-launch release matrix remains limited to macOS Intel and
  Apple silicon. This candidate remains experimental, local-first, backed by a
  mock payment rail, and unsuitable for production or real-money use.

### Important fixes

- Gongbu admission errors now distinguish unselectable target tuples from
  unmatched image-size pricing selectors through bounded API, unified-MCP, and
  process-log diagnostics without echoing submitted values.
- Corrected Gongbu admission for sequential and concurrent executions that
  share one agent-scoped budget while retaining per-hold validation.
- Gongbu now withdraws readiness and admission immediately on dependency loss
  but allows a 30-second continuous-failure grace before process shutdown.
  Managed Hubu structured logs are bounded, and repeated capability-probe load
  is reduced.

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
