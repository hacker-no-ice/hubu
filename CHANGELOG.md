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

<!-- Draft release target: v0.2.1-rc.1. -->

### Highlights

- Added a durable governed-execution path through unified MCP, including human
  approval and exact-operation resume. Gongbu is now principal-neutral, so
  multiple agents can use one runtime and shared logical budgets while Hubu
  remains authoritative for each operation's identity and settlement.
- Added versioned updates and history for stable logical budgets, preserving
  cumulative usage, immutable provenance, and exact historical replay.
- Added three outcome-oriented `hubu stack init` modes: `sandbox` for the full
  stack with a non-billable provider fixture, `local-stack` for approved real
  provider targets, and `hubu-only` for governance without Gongbu or Temporal.
- Added runtime provider selection and the supported
  `hubu.flux-2-pro.text-to-image/v1` profile, with frozen dimension-qualified
  prices, zero generation retries, no fallback, bounded artifacts, and durable
  asynchronous recovery.
- Added a reusable Hubu policy-authoring skill and versioned language reference.

### Breaking or operational changes

- Unified MCP routing advances to revision 4. Budget replacement surfaces are
  removed; use approval-gated `hubu_update_budget` and read-only
  `hubu_budget_history`.
- The Vertex AI Gemini adapter is removed. Gemini Developer API remains
  available through its separate developer adapter and raw target
  configuration. FLUX.2 Pro is the only packaged supported provider profile;
  targets require explicit active revisions and pricing schema version 2.
- Policy denials are terminal, corrected work uses a new logical operation, and
  approval-required work resumes only by public operation handle.
- Release publication is manual-only. Exact-tag source installation is the
  primary macOS path, but source-built binaries are not Apple-signed or
  notarized.
- Live providers remain explicitly opt-in and billable. The FLUX profile stays
  `live_qualified = false` until guarded qualification is complete; CI, demos,
  and release validation remain provider-free.

### Important fixes

- Preserved precise vendor costs and bound FLUX dimensions, target revision,
  and pricing evidence before transmission.
- Hardened FLUX polling, artifacts, redaction, deadlines, reconciliation,
  replay, and restart recovery.
- Fixed selected-profile CLI authentication, reduced idle stack log and probe
  load, and improved bounded diagnostics and terminal output.

## v0.2.0 — 2026-08-26

### Highlights

- Integrated Hubu and Gongbu into one Rust workspace and one verified
  four-binary distribution while preserving separate processes, databases,
  credentials, provider execution, artifacts, and failure domains.
- Made `hubu-unified-mcp` the only supported agent-facing MCP surface. It
  routes governance and execution to their owning backends, negotiates backend
  compatibility and health, and replaces the standalone Hubu MCP adapter from
  `v0.1.0`.
- Added durable normalized MCP operations with private backend operation keys
  and continuation binding, restart recovery, public status and artifact
  retrieval, and background resolution of acknowledged work without losing
  outcomes that still require financial reconciliation.
- Added the Gongbu execution plane for durable provider work, including
  Temporal-backed recovery, operator-controlled targets and exact pricing,
  normalized artifacts, and image adapters for the Gemini Developer API,
  Ideogram, and Flux 2. Sequential and concurrent executions
  can share an agent-scoped budget while retaining per-hold validation.
- Added dependency-aware local-stack profiles for initialization, rendering,
  startup, readiness diagnosis, safe updates, recovery, managed service
  credential handoff, and Codex configuration. Multiple Hubu agents can use
  one running stack while Hubu remains authoritative for per-operation
  identity, authorization, and settlement.
- Expanded governance with revisioned declarative policy resources, typed
  trusted execution scopes, resumable spend authorization after owner approval,
  and safe corrected-scope retries after side-effect-free denials.
- Strengthened release verification with immutable manifests, provenance,
  internal and release checksums, locked dependency inventories, license
  bundles, and published-archive smoke tests.

### Breaking or operational changes

- Clients must install `hubu`, `hubu-server`, `hubu-unified-mcp`, and
  `gongbu-server` together from the same release archive and regenerate their
  MCP configuration for `hubu-unified-mcp`; `hubu-mcp-server` is removed.
- There is no supported in-place upgrade for a `v0.1.0` Hubu database. Retain
  or back up the old database for audit, then start `v0.2.0` with a fresh Hubu
  database and fresh local-stack state. Do not point the new server at the old
  database file.
- The executor contract advances from `hubu-spend-executor-v4` to
  `hubu-spend-executor-v4.3`. External integrations must adopt the new lease
  terminology: `workload_profile`, `default_profile`, `profiles`, and
  `HUBU_SPEND_TIMING_CONFIG` become `lease_profile`,
  `default_lease_profile`, `lease_profiles`, and `HUBU_LEASE_CONFIG`. Typed
  execution scope is canonical for new clients and required for provider-backed
  Gongbu work; legacy merchant-only requests remain readable for migration.
  Authorization TTL is global, lease profiles configure claim TTL only, and
  Gongbu workload types are independent of Hubu lease profiles.
- The CLI no longer supplies an implicit `local-merchant` when merchant or
  execution scope is omitted. Merchant-dependent policies may therefore yield
  `needs_approval` until the caller supplies the intended typed scope. Money
  inputs now use checked non-negative decimal parsing that rejects signs,
  malformed decimal forms, and overflow.
- `gongbu_create_execution` durably acknowledges accepted work. Observe its
  public handle with `hubu_operation_status`; exact redelivery must reuse the
  same harness identity, and acknowledged work must not be replaced.
- Resumable approvals require an owner-only approval capability distinct from
  both the ordinary Hubu bearer and the reconciliation capability. Managed
  local-stack profiles create and wire it automatically; manual or external
  Hubu setups must ensure the server and approved client share it through
  `HUBU_APPROVAL_TOKEN`, `HUBU_APPROVAL_TOKEN_FILE`, or the generated default
  capability file, and keep it from executors.
- Published archives are limited to macOS Intel and Apple silicon. The Linux
  archives available for `v0.1.0` are no longer published or advertised as
  supported. Source builds require Rust 1.88; building the unified workspace or
  Gongbu's Temporal components also requires `protoc`. Gongbu additionally
  needs either a separately installed Temporal CLI configured by absolute path
  and exact version pin for `managed_local` mode, or an operator-owned Temporal
  service in external mode; release archives do not bundle Temporal.
- Hubu remains experimental and local-first, uses a mock payment rail, and has
  a same-user local capability boundary; it is not approved for production or
  real-money use. Explicitly enabled live Gongbu adapters can still incur
  provider charges and require conservative spend ceilings.

### Important fixes

- Preserved authorization, task, operation-key, and trusted execution-scope
  boundaries through provider execution and retry flows.
- Hardened credential redaction, immutable target revisions, claim recovery,
  terminal failure isolation, ambiguous-outcome reconciliation, and release
  completeness checks.
- Gongbu admission errors distinguish unselectable target tuples from unmatched
  image-size pricing selectors through bounded diagnostics without echoing
  submitted values.
- Gongbu withdraws readiness and admission immediately on dependency loss
  while allowing a 30-second continuous-failure grace before shutdown. Managed
  Hubu structured logs are bounded, and repeated capability-probe load is
  reduced.

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
