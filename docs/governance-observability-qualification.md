# Governance observability qualification

Target: v0.2.4 preparation. This records deterministic qualification, not a
published release or permission to run paid providers. The
[read contract](governance-observability.md) defines the public semantics.

## Baseline and evidence

The integrated runtime baseline is main
`9482bb6e17721622a05ac36be7a9d6ca247c93d9`. It includes the merged observability
stack and Amount Precision work, including HUB-277, HUB-279 and HUB-281; those
are no longer pending dependencies. This qualification adds test fixtures, a CI
hook and documentation without changing production behavior. The project
outcome is already recorded in the existing Unreleased changelog entry.

The checks below were run on 2026-10-18 with workspace product version `0.2.3`
and executor contract `hubu-spend-executor-v4.5`. Hubu and unified MCP process
fixtures carry matching product/source stamps from the baseline above. Rust
commands run from the repository root with locked dependencies and an isolated
`CARGO_TARGET_DIR`; business records use fresh temporary databases. No user
profile, installed stack, provider credentials or paid provider was used.

| Acceptance | Evidence |
| --- | --- |
| Automatic decisions and real human intervention | Core capture tests and the real-history fixture cover allow, policy/no-budget/insufficient-budget denials, genuine escalation, confirmed approval/decline and approval-time budget failure. Initial policy, terminal admission and human resolution remain separate facts. |
| Financial outcomes | Core tests cover atomic claim/release/reconciliation, evidence rollback and replay. Real history covers safe void/release, late expiry observation, and billed/no-bill confirmation. Existing Gongbu E2E and Temporal failure-isolation fixtures cover deterministic execution failures and subsequent healthy work without implying those failures have supported governance capture. |
| Immutable history and full-filter reads | Core and transport suites cover corrected revisions, policy/budget edits, parsed time bounds, owner isolation, redaction and legacy missing/unlinked evidence. The real-history fixture reuses one snapshot across summary/pages/detail, verifies pre-period backlog, late facts, closure after snapshot, exact replay and restart. |
| Exact native currencies | Existing Amount Precision E1–E15 pass. Real history reserves 5,000 µUSDC, charges 3,101, releases 1,899, then checks replay leaves consumed=3,101, frozen=0 and remaining=996,899; the USD budget is unchanged. Requested aggregates include 18,446,744,073,709,551,614 USD units without narrowing. |
| Agent questions | 121 recorded HTTP/MCP governance read responses, including HTTP parity, answer an explicit past-week window: seven automatic denials (five policy, one no available budget, one insufficient budget), one human approval and one decline. Supporting records explain these counts; a remaining pending human approval and frozen open/closed reconciliation snapshots qualify backlog separately from the event period. |
| Compatibility and boundaries | Locked workspace tests, strict all-target/all-feature Clippy, Rust 1.88 all-target/all-feature check, stamped MCP golden/routing matrices, executor conformance and dependency-boundary checks pass. Hubu and Gongbu retain separate processes, databases and credentials. |
| Documentation | Local links, site rendering and the repository stale-reference scan pass. Qualification evidence stays out of the published documentation site. Architecture metadata and source links identify the tested integrated baseline. |

Recorded results: 1,177 workspace tests passed; the ordinary suite leaves 37
opt-in tests ignored. The explicit integration runs passed one Gongbu E2E case,
one Temporal failure-isolation case, three MCP golden cases and 29 MCP routing
cases. Conformance passed 13 v4.5 built-in, 13 v4.5 plugin and 12 legacy v4.4
scenarios. All E1–E15 precision cases, 42 rendered-site tests and 456 local
documentation/architecture links passed. Formatting, strict Clippy, MSRV,
workspace documentation, CI provider-safety and dependency-boundary checks
also passed.

## Repeatable checks

These commands use local deterministic fixtures, not paid providers:

```sh
scripts/integration-governance-observability.sh
python3 scripts/qualify-amount-precision.py --server-bin target/debug/hubu-server
scripts/integration-executor-conformance.sh
scripts/integration-standalone-mcp-executor.sh
scripts/integration-hubu-gongbu-executor.sh
scripts/integration-hub-71-terminal-failure-isolation.sh
scripts/integration-unified-mcp.sh
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo +1.88.0 check --workspace --all-targets --all-features --locked
python3 scripts/check-workspace-boundary.py
python3 scripts/check-ci-provider-safety.py
python3 scripts/check-doc-links.py
```

The governance wrapper derives its binary directory from Cargo metadata, so it
also supports a custom `CARGO_TARGET_DIR`. Older integration wrappers assume
`target/debug`; when using a custom target, invoke their equivalent Cargo/Python
commands with that target's explicit binary paths. The Temporal check requires
`temporal` on `PATH`, and Gongbu builds require `protoc`. The default governance
check requires v4.5; `--expected-contract v4.4` is explicitly provisional, not the
integrated qualification result above. Conformance separately retains the
legacy v4.4 compatibility corpus.

The governance runner emits a JSON report and `--keep-state` preserves its
fresh diagnostic directory. A test-only SQLite trigger delays reconciliation
journal appends until the frozen boundary is captured, making late observation
independent of the background collector's polling schedule. All business
records still enter through authenticated HTTP. API tests separately reject
all database writes during reads and hold mutable manager locks to prove GET
handlers do not acquire them.

The repaired terminal-failure fixture selects scenarios by the consumed
authorization continuation, rather than the opaque private operation identity.
Its synthetic resolver supplies the exact v4.5 amount matching its cent
compatibility projection. It checks each exact failure cause, a proven release's
retained provider-attempt cause, process health and subsequent successful work.
This is a test repair; production failure handling is unchanged.

## Limits retained for review

Provider failures and pre-expiry executor uncertainty lack durable source
history and remain unsupported, rather than zero. Earlier policy/human history
is not backfilled. Record and explanation coverage remain separate. Legacy
inventory describes scoped gap presence, not additional enumerable events;
its observation time is not a polling heartbeat. Requested totals are neither
costs nor savings. No dashboard, alerting or monitoring system is delivered.

Collection can lag across downtime or write failures, and its cost can grow with
history and unresolved claims. Financial and inventory failures are reported
separately; a failed owner inventory does not block other owners.

Two nonblocking review follow-ups remain: the generic foundation grant validator
accepts a broader source pairing than the restricted production writers use
([PR #323](https://github.com/hacker-no-ice/hubu/pull/323)), and recurring financial
collection should eventually avoid rescanning the complete unresolved backlog
([PR #326](https://github.com/hacker-no-ice/hubu/pull/326)). Neither changes the
supported read semantics or authorizes provider work. Upgrade and read-only
verification of the selected local profile are a separate rollout step;
this fixture does not claim to have performed them.
