# HUB-109 final unified MCP zero-user cutover canary

Date: 2026-08-18 (America/Los_Angeles)

Decision: **NO-GO for standalone MCP deprecation.**

Every locally runnable source and packaged-candidate row passed on
`aa88744c2fde2ed1b1f4ecaa05949422aee41c91`. The decision remains NO-GO because
the required immutable four-platform GitHub canary release does not exist. The
expected tag is
`main-aa88744c2fde2ed1b1f4ecaa05949422aee41c91`; a read-only remote tag check
returned no matching ref. A local release-mode archive is useful technical
evidence but is explicitly not a substitute for the required release URL,
workflow run, four published archives, top-level checksums, and four
published-asset smoke jobs.

This record does not modify the historical
[HUB-96 NO-GO](HUB-96-unified-mcp-migration-canary.md), deprecate or remove the
standalone binaries, or change runtime behavior. HUB-97 remains blocked.

## Candidate and ancestry

| Field | Recorded value |
| --- | --- |
| Candidate | `aa88744c2fde2ed1b1f4ecaa05949422aee41c91` |
| Candidate subject | `HUB-108: Amend unified MCP retirement gate for immediate cutover (#156)` |
| Branch base | Exact fetched `origin/main` at canary start |
| HUB-107 ancestry | `66f6894` is an ancestor of the candidate |
| HUB-106 ancestry | `fdc2bbecc0eee3402892fce6046262a14b916bc9` is the candidate's parent |
| HUB-108 identity | The candidate itself is the HUB-108 merge |
| Product version | `0.1.0-main.aa88744c2fde` |
| Native target exercised | `aarch64-apple-darwin` |
| Provider execution | Disabled; deterministic loopback fixtures only |

## Artifact identity

The release-mode local archive was
`hubu-0.1.0-main.aa88744c2fde-aarch64-apple-darwin.tar.gz`, with SHA-256
`9155ffc4e3568deab3440a0c2c05933127fd62c79b61a8b91b5a3e64fe4a5aa6`.
All six binaries reported the candidate SHA, product version, and executor
contract `hubu-spend-executor-v4.2`. The archive verifier checked internal
checksums, manifest, provenance, legal files, lockfile, unified default
discovery, distinct credential inputs, atomic migration, and both standalone
rollback catalogs. The local provenance deliberately has no qualifying
workflow-run identity, and the archive is not presented as immutable release
evidence.

The retained older rollback release is
[`main-959018f1dacc5e80f60cec209da5a5b360e9f095`](https://github.com/hacker-no-ice/hubu/releases/tag/main-959018f1dacc5e80f60cec209da5a5b360e9f095).
Its published top-level checksums are recorded in the adjacent
[machine-readable manifest](HUB-109-evidence.json). The release policy retains
commit-addressed GitHub Releases indefinitely; this rollback identity must in
all cases remain available through HUB-98 removal verification.

## Evidence matrix

| Evidence | Result | Observation |
| --- | --- | --- |
| Package identity and complete catalog | **NO-GO** | The native release-mode archive, six version surfaces, manifests, checksums, exact 33-name catalog, schemas, annotations, and ownership passed. The required immutable four-platform published release and native smoke jobs are missing. |
| Golden behavior parity | PASS | All 32 mapped tools passed success and representative error parity against packaged unified and standalone binaries. Approval metadata and image content matched. Both reconciliation tools used schema-valid zero-cost requests. |
| `tools/list_changed` notification | PASS | Packaged stdio covered initialized baseline, stop/recovery transitions, exactly-one notification behavior, unchanged-refresh suppression, concurrent single-flight refresh, and payload-free notifications. |
| Failure isolation | PASS | Full backend-state/version matrices, one-backend outages, not-ready behavior, list/call state changes, persistent terminal-failure isolation, and governed execution fail-closed behavior passed. Unit and integration tests proved ambiguous mutations are not automatically retried or cross-routed. |
| Redaction and credential isolation | PASS | Distinct Hubu, Gongbu, approval, and reconciliation credentials reached only their owners. MCP results, diagnostics, stderr, notifications, stored metadata, paths, and backend errors were redacted. Default credential files are ignored at repository root and nested checkouts. |
| Migration and rollback | PASS | Unified-only generation, atomic two-entry migration, refusal without replacement Gongbu settings, unrelated-entry preservation, exact config restore, and initialization of packaged 30-tool Hubu and four-tool Gongbu standalone catalogs passed. No backend database or artifact state was copied or merged. |
| Workspace and release policy | PASS | Locked all-target workspace tests, formatting, Clippy, Rust 1.88 MSRV, four-target packaging tests, native archive runtime, release-workflow policy, Cargo metadata, dependency boundary, provider-safety, Cargo deny advisories/licenses/sources, and Rust docs passed. Two live-provider tests remained ignored. |
| Documentation and stale references | PASS | All 195 local documentation/architecture links passed. Retired Gongbu repository references are confined to the historical import audit and repository-cutover/rollback record; the only current `separate Gongbu checkout` hit explicitly says none is required. No stale operational dependency remains. |
| Findings and decision | **NO-GO** | No runtime P0/P1 defect was observed. One non-waivable release-evidence gate is open: publish and smoke the expected immutable four-platform canary. HUB-97 remains blocked and the HUB-96 NO-GO remains unchanged. |

## Verification summary

The exact commands, outcomes, artifact checksums, rollback identity, and
deviation are recorded in
[`HUB-109-evidence.json`](HUB-109-evidence.json). Highlights:

- the packaged candidate passed 13 unified end-to-end tests, three complete
  golden-parity tests, and three packaged stdio lifecycle tests;
- the locked workspace passed 472 executed tests, with 18 integration tests
  subsequently run by their dedicated scripts and the two explicit live
  provider tests left ignored;
- all dedicated core, executor, unified-MCP, and persistent terminal-failure
  integration suites passed;
- no provider credential, live endpoint, spend acknowledgement, provider call,
  or billable operation was used.

## Required follow-up

Dispatch `.github/workflows/release.yml` on `main` with channel `canary` and
source commit `aa88744c2fde2ed1b1f4ecaa05949422aee41c91`. Verify the published
prerelease targets that exact SHA, contains all four non-empty native archives
plus `SHA256SUMS`, and has four successful published-asset smoke jobs. Then
rerun or attach this evidence without changing candidates and issue a new
reviewed decision. Any failure remains NO-GO.

This task changes only reusable test harnesses and evidence. It does not alter
major components, request flows, public interfaces, storage ownership, or
runtime behavior, so the architecture visualizer is unchanged.
