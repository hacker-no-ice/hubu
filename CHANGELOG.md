# Changelog

This changelog records the key user-facing and operational changes in each
stable Hubu release. It is intentionally curated rather than being a complete
list of commits or pull requests.

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

- Made release publication operator-triggered only. The daily canary schedule
  is removed while manual canary, candidate, and stable dispatches retain their
  immutable source, tag, and asset validation.
- Aligned agent-facing denial recovery across unified MCP spend, governed
  execution, and public operation status. A definitive denial remains terminal,
  exact redelivery recovers that denial, and corrected work is submitted as a
  new logical operation without exposing or asking agents to reuse Hubu's
  private backend operation key.
- Fixed CLI authentication after local-stack startup by resolving the selected
  profile's active endpoint and authentication, approval, and reconciliation
  credential files as one handoff. Explicit `--url` remains the manual-mode
  escape hatch, and legacy environment/file resolution remains unchanged when
  no active selected/default profile exists.
- Made exact-tag, full-commit source installation the primary macOS onboarding
  path for initial technical users. The reviewed installer builds and stamps
  the four production binaries together from the locked workspace without
  requiring Apple signing credentials or Gatekeeper workarounds; the resulting
  local binaries are not Developer ID-signed, notarized, or Apple-verified.
- Made human-readable CLI diagnostics and status summaries easier to scan with
  semantic terminal color, clearer sections, and consistent action styling.
  Automatic TTY detection and `NO_COLOR` keep default redirected output plain,
  while machine-readable output stays ANSI-free for every `--color` choice.

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
  normalized artifacts, and image adapters for Vertex AI Gemini, the Gemini
  Developer API, Ideogram, and Flux 2. Sequential and concurrent executions
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
