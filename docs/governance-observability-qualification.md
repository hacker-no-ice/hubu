# Governance observability qualification

Target: v0.2.4, with all project PRs left open for human review and merge.
The [read contract](governance-observability.md) defines the public semantics.

## Baseline and evidence

Integrated release qualification is pending the published Gongbu v4.5 work in
HUB-281 (including HUB-279). Core capture and HTTP/MCP reads are independently
validated; this does not yet qualify the full v0.2.4 execution baseline.

Current tested runtime stack: `f04fad7b94a4e014f4da4162da273f2bbae3ae94`,
based on main `6addfc8bb63b2576259ca24ca1b2b12c004fe34c`.
The real-history fixture is committed as
`a00e0472c496d98ac8697d9106fc6f08a0937073`.

| Layer | Evidence |
| --- | --- |
| Durable capture | Core tests cover automatic admission, real human branches, failed approval budget acquisition, claims, release, expiry and billed/no-bill reconciliation; replay and restart preserve immutable evidence. |
| Shared queries | Core tests cover distinct attempts/operations, corrected revisions, policy/budget edits, late facts, parsed time boundaries, frozen totals/backlog, exact USD/USDC sums, owner isolation, legacy gaps and redaction. |
| HTTP | 155 API tests and one logging integration pass. Nine new transport scenarios include authenticated malformed raw requests, recursive wide money, frozen reads and database write traps. |
| Unified MCP | 190 unit tests, 10 other package tests, three golden cases and 29 stamped process cases pass, including summary → pages → detail with identical snapshots, exact amounts and coverage. |
| Compatibility | Rust 1.88 full locked workspace check (all targets/features) passes at `bd2e7496314e7ef5807e8f0c84511173ff512e25`, including the repaired Gongbu fixture. Component suites, strict Clippy, formatting and runtime dependency-boundary checks pass. |
| Real history | 121 real HTTP/MCP read responses pass in explicit provisional v4.4 mode, including both human branches, late expiry and closure, corrected attempts, exact wide USD/USDC, isolation and redaction. |
| Published Hubu v4.5 compatibility | All 121 real read responses pass on local composition `c21b78aeb43a17317bac1c821f897e231d5cf189`: observability `d2e15263f18b92aca321c5fed9abc4db05ff2258` plus published HUB-279/PR #328 head `5b3f859c23268748d9c355a6f4c31f19b0044436`. Exact v4.5 USDC claim/settlement passes without runtime edits. No PR was merged. |
| Existing execution checks | Current-baseline conformance passes 12 built-in and 12 plugin scenarios; standalone MCP, deterministic Hubu–Gongbu execution and Temporal terminal-failure isolation pass. These retain v4.4 compatibility coverage. |
| Integrated Gongbu v4.5 execution | Pending unpublished HUB-281 and the final baseline. The default v4.5 fixture correctly rejects the original v4.4 server before creating business records. |

## Deterministic checks

Run from the single repository workspace. These checks use local stubs and do
not authorize paid-provider work:

```sh
scripts/integration-governance-observability.sh
scripts/integration-executor-conformance.sh
scripts/integration-standalone-mcp-executor.sh
scripts/integration-hubu-gongbu-executor.sh
scripts/integration-hub-71-terminal-failure-isolation.sh
scripts/integration-unified-mcp.sh
python3 scripts/check-doc-links.py
```

The default governance check requires v4.5; `--expected-contract v4.4` is an
explicit provisional smoke run, not release qualification.

The governance fixture proves weekly automatic denials separately from
human declines, both human branches, pre-period backlog, late expiry observation,
resolution after a frozen snapshot, replay/restart and exact per-currency money
through real Hubu and stdio MCP reads. Positive USDC v4.5 settlement must consume
3,101 of 5,000 reserved units, release 1,899, and leave the USD budget untouched.
The Gongbu checks reuse Amount Precision qualification rather than implementing
another executor or amount representation.

The terminal-failure fixture previously selected scenarios through the retired
private operation identity. Test-only commit
`bd2e7496314e7ef5807e8f0c84511173ff512e25` selects the consumed authorization
continuation and checks the exact failure cause. The repaired Temporal test, its
workflow regression and strict focused Clippy pass; production behavior is
unchanged.

## Limits retained for review

Provider failure and pre-expiry executor uncertainty lack durable source
history and remain unsupported, rather than zero. Earlier policy/human history
is not backfilled. Record and explanation coverage remain separate. Legacy
inventory describes scoped gap presence, not additional enumerable events;
its observation time is not a polling heartbeat.

Collection can lag across downtime or write failures, and its cost can grow with
history and unresolved claims. Financial and inventory failures are reported
separately; a failed owner inventory does not block other owners.

Two nonblocking review follow-ups remain: the generic foundation grant validator
accepts a broader source pairing than the restricted production writers use
([PR #323](https://github.com/hacker-no-ice/hubu/pull/323)), and recurring financial
collection should eventually avoid rescanning the complete unresolved backlog
([PR #326](https://github.com/hacker-no-ice/hubu/pull/326)). Neither changes the
supported read semantics or authorizes provider work.
