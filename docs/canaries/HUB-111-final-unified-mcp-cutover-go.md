# HUB-111 final unified MCP cutover GO

Date: 2026-08-19 (America/Los_Angeles)

Decision: **GO, conditional on explicit approval and merge of this evidence PR.**

The owner-approved zero-user cutover may proceed to HUB-97 after that
precondition is met. Until then HUB-97 remains blocked. This record changes no
runtime behavior and does not deprecate or remove either standalone MCP binary.

## Immutable release evidence

GitHub Actions [run 32281312909](https://github.com/hacker-no-ice/hubu/actions/runs/32281312909)
was independently checked against release
[`main-aa88744c2fde2ed1b1f4ecaa05949422aee41c91`](https://github.com/hacker-no-ice/hubu/releases/tag/main-aa88744c2fde2ed1b1f4ecaa05949422aee41c91).
The scheduled run completed successfully on attempt 1 with head SHA
`aa88744c2fde2ed1b1f4ecaa05949422aee41c91`. The lightweight tag and release
`targetCommitish` both equal that exact candidate; the release is a non-draft
prerelease.

Required release checks, all four platform builds, publication, and all four
published-archive smoke jobs succeeded. The four archives are non-empty. A
fresh download verified every archive against `SHA256SUMS`; those hashes also
match GitHub's asset digests. Each archive's provenance identifies the same
candidate, workflow attempt, product version, repository, and expected target.
Exact job IDs, asset sizes, and hashes are in the adjacent
[machine-readable manifest](HUB-111-evidence.json).

## Reconciliation and rollback

This is a fresh GO record. It does not modify PR
[#157](https://github.com/hacker-no-ice/hubu/pull/157), its commits, or its
historical HUB-109 NO-GO. That reviewed record already passed packaged behavior
and error parity, stdio lifecycle, notifications, failure isolation, redaction,
credential separation, migration and rollback, workspace/MSRV, packaging,
provider-safety, and documentation checks with zero unresolved P0/P1 findings.
Its sole non-waivable open gate was the then-absent immutable release. The
evidence above closes that gate for the unchanged candidate.

The rollback release
[`main-959018f1dacc5e80f60cec209da5a5b360e9f095`](https://github.com/hacker-no-ice/hubu/releases/tag/main-959018f1dacc5e80f60cec209da5a5b360e9f095)
was rechecked as present with four non-empty archives and `SHA256SUMS`. Preserve
that identity through HUB-98 removal verification. Rollback continues to mean
changing the consumer pin and MCP configuration; it does not merge processes,
databases, credentials, provider execution, or artifacts.

## Operator handoff

1. Require explicit approval and merge of the HUB-111 GO PR; do not merge it as
   part of this evidence task.
2. After merge, advance the owner-approved zero-user cutover to HUB-97 under
   the exact release tag and checksums recorded here.
3. Do not deprecate or remove standalone surfaces in HUB-111. Preserve the
   rollback identity until HUB-98 completes its verification.

This evidence-only change does not alter components, request flows, storage
boundaries, public interfaces, or code ownership, so the architecture
visualizer is unchanged.
