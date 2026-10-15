# Changelog

This changelog records the key user-facing and operational changes in each
stable Hubu release. Candidate notes are folded into the corresponding stable
entry. It is intentionally curated rather than being a complete list of commits
or pull requests.

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

### Highlights

- USDC is a budget currency. Budgets are per currency: USDC spend counts
  only against a USDC budget and USD spend only against a USD budget, with no
  conversion between them. USDC amounts use the same micro-unit accounting
  (one micro-unit is one USDC atomic unit), so an x402-style
  `maxAmountRequired` hold is exact. Executors may report USDC actual costs
  against USDC holds; a receipt in another currency than its hold is rejected.
  Ledger postings and coverage stay per currency. Wallet rails remain USD
  cents only and refuse USDC payments before any money moves. Policy amount
  values compare in the request's own currency (scope them with a `currency`
  condition), and `POST /policies` returns `warnings` for amount values finer
  than one cent. Spend authorization (`/spend/authorize`, `hubu_authorize_spend`,
  `hubu_submit_spend`) accepts an exact decimal `amount` such as `"0.005"`
  (the whole-cent `amount_cents` is deprecated; sending both is rejected) and
  a `currency`. `POST /budgets` and the `hubu_create_budget` MCP tool accept
  `currency` (`usd` by default); executor resolve/claim responses report the
  authorization's currency, and an executor may assert `currency`.

- Policies can state exact amounts with `value: {money: "0.005"}`: a quoted
  decimal in the budget currency with up to 6 decimal places. Unquoted
  numbers, signs, exponents, and more than 6 decimal places are rejected when
  the policy is written. The legacy `money_cents` form still loads and
  compares with `money` values in the same policy, and stored policy
  revisions are never rewritten. `POST /policies` accepts
  `daily_limit: "5.00"` for the starter policy (`daily_limit_cents` is
  deprecated), and policy explanations show `USD 0.005` instead of minor
  units.
- Budgets are accounted in micro-units (`10⁻⁶` USD). Each exact provider cost
  is rounded up once, to the nearest micro-unit, when it settles, so a
  USD 0.001 executor call now consumes USD 0.001 of budget instead of a full
  cent, and 1,000 such calls fit a USD 1.00 budget. Wallet payments consume
  what the rail actually charged and release the rest of the hold.
- New ledger postings record the budget charge in micro-units
  (`budget_charge_delta` with `budget_charge_scale: 6`), and the rounding
  delta is measured against that charge. Existing postings keep their cent
  charge and are read exactly as `cents × 10⁴`; stored ledger evidence is
  never rewritten. The budget coverage report reconciles in micro-units, so
  budget consumption matches recorded charges exactly and differs from the
  exact provider total by less than one micro-unit per settlement.

- The HTTP API speaks exact amounts. Budgets (`amount`), budget versions
  (`amount_limit`), spending targets (`amount`), spend (`amount`) and the
  starter policy (`daily_limit`) accept decimal strings with up to 6
  decimal places, and the matching MCP tools expose the same fields.
  Responses add exact money objects
  (`{"units": "5000", "scale": 6, "currency": "usd", "display": "0.005"}`)
  beside every legacy `*_cents` field for budgets, versions, holds,
  spending targets, approval reviews, settlement receipts, authorization
  records, ledger coverage and the watch projection. See
  [docs/api-amounts.md](docs/api-amounts.md).

- Executor contract `hubu-spend-executor-v4.5` moves executor amounts to
  exact units. Validate and claim requests send
  `contract_version: "hubu-spend-executor-v4.5"` with
  `amount: {"units", "scale": 6, "currency"}`, which must equal the
  authorization exactly; a missing or unknown `scale`, an unknown
  `contract_version`, or `amount_cents` alongside `amount` is rejected.
  Executor responses add the exact `amount` money object. Requests without
  `contract_version` are v4.4 and keep working with whole cents. The guidance
  lists both in `supported_protocol_versions`, the conformance suite gains a
  v4.5 corpus with a sub-cent scenario (the v4.4 corpus still runs in CI), and
  the `hubu_governed_execution` MCP tool accepts a decimal `amount` and
  `currency`. See [docs/spend-executor-contract.md](docs/spend-executor-contract.md#v45-exact-amounts).

### Breaking or operational changes

- All binaries now report and require executor contract
  `hubu-spend-executor-v4.5`. Managed stacks regenerate Gongbu's
  `expected_executor_contract`; hand-written Gongbu configurations must
  change it to `hubu-spend-executor-v4.5`.
  Gongbu still sends v4.4 cent requests until its client moves to v4.5, and
  until then it refuses to admit an authorization that is not a whole number
  of cents.

- Whole-cent amount fields (`amount_cents`, `amount_limit_cents`,
  `daily_limit_cents`) are deprecated. Requests that use only them still work
  for this release, and their responses carry `Deprecation: true` with a
  `Link` to the replacement fields. `*_cents` response fields are now
  conservative projections: amounts that use budget round up, and amounts
  that permit spend round down.

- **One-way storage migration.** On first start, Hubu adds micro-unit
  (`10⁻⁶` USD) amount columns to budget versions, balances, holds, spending
  targets, and executor settlement receipts, and backfills them exactly from
  the existing cent columns. Before migrating a database that holds budget
  data, Hubu writes a copy to `<database>.pre-accounting-scale.bak` and logs
  the path. The migration refuses to run if any stored amount exceeds
  USD 9,223,372,036,854.77, and it rolls back if any backfilled value or
  budget invariant does not match. The cent columns are still written as
  conservative projections, but restore the backup rather than downgrading
  in place.

## v0.2.3 — 2026-10-13

### Highlights

- Added `hubu watch`, a read-only terminal view of agent budgets and
  governance state for live demos. It shows per-agent budget bars with
  consumed, frozen, and free capacity and a feed of the newest operation
  decisions, polls the selected profile's authenticated endpoint about once per
  second, keeps the last snapshot when a poll fails, and fits its layout to the
  terminal width (70–100 columns, measured by grapheme cluster so emoji and
  wide characters align). `--compact`, `--events`, `--agent`, `--once`, and
  `--currency` narrow the view.
- Added `hubu gallery export` (and `hubu-unified-mcp gallery export`) to save
  generated images from a completed public operation handle into a local
  folder with Hubu-aware filenames such as `01-flux-2k-draft-5.8c.png`, a
  receipt carrying the exact settled operation total, and a hidden gallery
  manifest. Export fetches artifacts natively through the selected stack
  profile, validates artifact digests, and refuses to write if the operation's
  accounting changes during export.
- Human owners can rename an agent with `hubu agent rename` while keeping its
  `agt_...` ID, account, budgets, policy assignments, and ledger history.
  `hubu agent history` shows the append-only revision audit. Clients that
  still register under a previous name resolve to the same agent and receive a
  `stale_agent_identity` warning. Rename requires the human approval
  capability and is not exposed through the unified MCP.

### Breaking or operational changes

- Local stacks now support two modes, `sandbox` and `live`, and both start
  Gongbu by default. `hubu stack init --mode local-stack` and `mode =
  "local-stack"` still work as a deprecated alias for `live`; doctor warns and
  new profiles write `live`. Legacy `hubu-only` profiles are rejected with a
  migration message: create a separate sandbox profile or explicitly configure
  a live profile with approved providers. No automatic migration enables
  billable execution.
- Agent registration is now idempotent over HTTP: registering an agent whose
  identity and version content already exist returns the existing `agt_...`,
  `agv_...`, and `aga_...` records with a new session instead of failing with
  `agent is already registered for this owner`. Responses include
  `identity_resolution` (`created`, `reused`, or `alias`), and
  `hubu register agent` prints `Agent already registered; reused existing
  records` for a repeat. Owner, agent-type, and version-content conflicts are
  still rejected.
- Agent names are now unique per owner (trimmed, case-insensitive).
  Registering a new identity under an existing agent's name fails with HTTP
  409 `display_name_conflict` naming the existing `agt_...`. Existing local
  databases that already contain duplicate names are not migrated: the server
  refuses to start and lists the conflicting agents.
- Policy and lease-config YAML now use `serde-saphyr` (pure Rust) instead of
  `serde_yaml_ng`, removing the unmaintained `unsafe-libyaml`
  (RUSTSEC-2024-0449) and its `cargo deny` exception. Policy exports
  (`GET /policies/export`, `hubu_show_policy` with `include_yaml`) now write
  enum values in the authored map form (`value: {money_cents: 500}` as a
  nested block) instead of YAML tags (`value: !money_cents 500`); older
  tagged exports still load. String fields no longer accept `null`, `Null`,
  or `~` as literal text, `10_000`-style integers are accepted, and parse
  errors include the line, column, and a source excerpt.

## v0.2.2 — 2026-10-05

### Highlights

- Renamed Hubu history to authorization records: `/spend/authorizations`,
  `authorization_id`, `hubu_list_authorization_records`,
  `hubu_get_authorization_record`, and `hubu spend authorizations` /
  `hubu spend show --authorization-id`. The unreleased old names are removed.
  History remains `hubu-history-v1`; unified MCP routing advances to revision 13.
- Added an executor-neutral, black-box conformance suite for the
  `hubu-spend-executor-v4.4` contract: a versioned fixture corpus of
  requests, responses, error codes and retry decisions, and a
  standard-library runner that targets any Hubu base URL with a pluggable
  executor side. It covers authorization, claims, settlement, release, lease
  expiry, human reconciliation, restart recovery and budget limits, including
  exactly-once ledger postings. See `docs/executor-conformance.md`.
- Qualified Hubu-only MCP authorization with your own executor. An agent
  authorizes through unified MCP and hands the continuation token to any
  executor, which claims and settles or releases it through the v4.4 executor
  API. See `docs/external-executor.md`.

### Breaking or operational changes

- The executor contract is now `hubu-spend-executor-v4.4`, and it is
  intentionally startup-incompatible with v4.3. Executors identify an
  operation only by values Hubu issued:
  - claim with the token plus the account, amount and scope assertions;
  - settle or release with the `claim_id`; an executor that lost the claim
    response replays the identical claim to recover it.

  Hubu rejects `operation_key` and `agent_id` on executor routes, and rejects
  the token on settle and release. A claim replay returns the existing claim
  even after the authorization expires. Gongbu, unified MCP and the
  release tooling move to v4.4 together, and every binary must report the same
  version. External v4.3 executors must migrate; see the migration notes in
  `docs/spend-executor-contract.md`. Human reconciliation and the operation
  key's role in authorization retries are unchanged.
- No `/spend/executor/*` response returns `operation_key` any more, including
  `resolve`. Executors receive only the token, `claim_id` and public IDs.
  Gongbu identifies new executions by Hubu's `decision_id`, which also seeds
  the vendor idempotency key; executions created earlier keep their stored
  identity. Gongbu responses report the consumed `spend_auth_token_id`, which
  unified MCP verifies against the continuation it bound. The `hubu spend
  claim` output shows the public `authorization_id` instead of the private key.
- Removed recurring-budget creation from core (`CreateBudgetSeriesRequest`,
  `CreateBudgetSeriesResponse`, `BudgetRecurrence`, and the series creation
  methods), HTTP (`POST /budgets/series`), CLI (`hubu budget create-recurring`),
  and unified MCP (`hubu_create_recurring_budget`). Create individual budgets
  with explicit periods instead. Existing budgets retain their IDs, versions,
  balances, holds, and history and continue as independent budgets; no data
  migration or recurring-series lifecycle is introduced.
- Consolidated the unified MCP tool set; see `docs/mcp-tool-consolidation.md`.
  These tools are removed from discovery, capabilities and public dispatch.
  Calls fail as unknown tools before backend access, with no hidden aliases,
  compatibility handlers or grace period. Reconnect clients to refresh cached
  discovery and update callers:
  - `hubu_add_policy`: use `hubu_apply_policy` with explicit policy YAML.
    Human approval and optional revision/hash compare-and-set checks still
    apply. Inspect existing policies before replacing the old
    `daily_limit_cents` shortcut: it encoded a **single-spend** threshold, a
    blocked-merchant denial and a needs-approval fallback, not a cumulative
    daily budget, so a cumulative daily budget is not an equivalent
    translation.
  - `hubu_export_policy`: use `hubu_show_policy` with `include_yaml: true`.
  - `hubu_health`: use `hubu_unified_capabilities` and adopt its structured
    response shape, which differs from the removed tool. HTTP health probes
    remain.
  - `gongbu_get_provider_catalog`: use `gongbu_list_execution_targets` for
    agent-selectable targets and pricing; operators use `hubu stack catalog`,
    `hubu stack doctor` or authenticated `GET /v1/provider-catalog`.
  - `gongbu_get_redaction_attestation`: operators qualify through
    authenticated `GET /v1/executions/{id}/redaction-attestation`.
  - `hubu_client_approval_profile`: use tool approval annotations, generated
    client configuration and the approval boundary in
    `docs/unified-mcp.md`.

  The operator endpoints retain their authentication, validation and safety
  checks. Removing MCP wrappers does not remove stored policy records, backend
  HTTP APIs or CLI functionality, and does not restore recurring-budget
  creation.

### Important fixes

- `hubu_operation_status` now reports a standalone authorization from Hubu's
  authorization record (`authorized`, `executing`, `settled`, `released`, `expired`,
  `reconciliation_required`). Previously a handle stayed `authorized` and
  `replacement_safe` after another executor claimed or settled it, and turned
  `failed` once the authorization expired even while a claim was active. An
  agent following that answer could submit a replacement and double-spend. If
  Hubu is unreachable, the status is now `unverified` and never
  replacement-safe.
- `hubu-server` no longer fails to start with `UNIQUE constraint failed:
  spend_decisions.agent_id, spend_decisions.operation_key` after an agent
  corrected a denied authorization under the same operation key.

## v0.2.1 — 2026-09-04

### Highlights

- Added a durable governed-execution path through unified MCP, including human
  approval and exact-operation resume. Gongbu is now principal-neutral, so
  multiple agents can use one runtime while logical budgets remain agent-scoped
  and Hubu remains authoritative for each operation's identity and settlement.
- Added versioned updates and history for stable logical budgets, preserving
  cumulative usage, immutable provenance, and exact historical replay.
- Added three outcome-oriented `hubu stack init` modes: `sandbox` for the full
  stack with a non-billable provider fixture, `local-stack` for approved real
  provider targets, and `hubu-only` for governance without Gongbu or Temporal.
- Added one immutable provider-contract framework for two Gemini Developer API
  image models and FLUX.2 Pro. Agents select an opaque `target_id`; contracts
  freeze capabilities, prices, retry and fallback policy, and transport and
  recovery behavior.
- Added a reusable Hubu policy-authoring skill and versioned language reference.
- Added complete, mode-specific stack configuration examples and an exact-tag,
  source-SHA-verified macOS installation workflow for the four-binary cohort.
- Added discoverable bug and idea reporting, private vulnerability reporting,
  and offline feedback preparation through CLI and unified MCP. Reports expose
  their destination and full content for review; preparation never submits them.

### Breaking or operational changes

- Unified MCP routing advances to revision 8. Budget replacement surfaces are
  removed; use approval-gated `hubu_update_budget` and read-only
  `hubu_budget_history`.
- Supported provider profiles are renamed to provider contracts. Configuration
  must replace `[[supported_profiles]]` with `[[contract_bindings]]`; there are
  no compatibility aliases.
- The Vertex AI Gemini adapter is removed. Packaged contracts now cover Gemini
  3.1 Flash Lite Image, Gemini 3.1 Flash Image, and FLUX.2 Pro. Public execution
  uses discovered `target_id` values, and `POST /v1/executions` is retired.
- In unified MCP, policy denials are terminal, corrected work uses a new logical
  operation, and approval-required work resumes only by public operation handle.
- Release publication is manual-only. Exact-tag source installation is the
  primary macOS path, but source-built binaries are not Apple-signed or
  notarized.
- Live providers remain explicitly opt-in and billable. The FLUX contract stays
  `live_qualified = false` in the packaged catalog; successful live tests do not
  automatically change that readiness declaration. CI, demos, and release
  validation remain provider-free.
- Install the four binaries from the same release and refresh generated Codex
  MCP configuration to discover the updated tool surface. Use a separate
  profile when changing stack modes; examples document the complete source
  configuration for each mode.
- Hubu remains experimental and local-first. The stable release tag does not
  change the mock payment rail or qualify it for production money handling.

### Important fixes

- Preserved precise vendor costs and bound FLUX dimensions, target revision,
  and pricing evidence before transmission.
- Hardened FLUX polling, artifacts, redaction, deadlines, reconciliation,
  replay, and restart recovery.
- Fixed supported BFL polling and artifact-origin handling so accepted FLUX
  requests can complete artifact retrieval. Preserved sanitized recovery
  context for failures without exposing provider credentials or signed URLs.
- Kept Gongbu live during runtime Temporal or Hubu degradation, automatically
  withdrew and restored readiness, and preserved pending execution identity
  across retryable scheduling failures.
- Fixed selected-profile CLI authentication, reduced idle stack log and probe
  load, and improved bounded diagnostics and terminal output.
- Fixed sandbox secret validation rejecting generated provider version values.

### Known limitations

- Large image artifacts may not render inline in some MCP client display paths.
  Generated originals remain retrievable through `gongbu_list_artifacts` and
  `gongbu_get_artifact`; a missing preview does not require another billable
  generation. The client rendering boundary is still under investigation.
- Private billing/support intake is not yet configured. Keep sensitive drafts
  private; the private vulnerability-reporting channel is for security reports
  only. See [feedback guidance](docs/feedback.md).

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
