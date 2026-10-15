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
| Compatibility | Rust 1.88 checks and component suites, strict Clippy, formatting and runtime dependency-boundary checks pass. |
| Real history | 121 real HTTP/MCP read responses pass in explicit provisional v4.4 mode, including both human branches, late expiry and closure, corrected attempts, exact wide USD/USDC, isolation and redaction. |
| Integrated v4.5 execution | Pending HUB-281 and its published dependency baseline. The default v4.5 fixture correctly rejects the current v4.4 server before creating business records. |

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
