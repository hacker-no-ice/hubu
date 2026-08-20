# Repository security controls

This runbook defines the current repository-side dependency, workflow, and
release controls. Dated audits and one-time settings changes belong in review
evidence, not in this document.

## Dependencies and GitHub Actions

Dependabot checks the Rust workspace and GitHub Actions weekly. Rust updates
keep `Cargo.lock` committed and must pass the exact MSRV, locked workspace
tests, Clippy, release-build, and security jobs. Major dependency updates remain
separate; compatible minor and patch updates may be grouped.

Every third-party `uses:` reference in GitHub Actions is pinned to a full
40-character commit SHA with the corresponding release tag recorded as a
comment. Before accepting an update, verify the commit against the publisher's
release and review its changes.

## Advisory and license exceptions

The `Security policy` job runs cargo-deny across all features and checks
advisories, licenses, and dependency sources. `deny.toml` is the only exception
surface. Checks must not become non-blocking merely to make CI pass.

An exception requires a focused review that records:

1. the advisory ID or exact crate and version range;
2. an owner and technical justification;
3. affected Hubu paths and compensating controls;
4. a removal condition and review date; and
5. evidence that the exception is narrower than ignoring the dependency.

Use structured cargo-deny entries with reasons. Verify upstream license text
before adding a license exception, and never broadly allow an unreviewed Git
dependency source.

The current exact-version license exceptions are documented with their owners,
review dates, and removal conditions in `deny.toml`. Release packaging must
include the corresponding license material.

## Required repository settings

The default branch must use an enforced ruleset with:

- pull requests required;
- strict, up-to-date required checks;
- `Test workspace`, exact Rust 1.88 MSRV, and `Security policy` required;
- deletion and non-fast-forward updates prohibited; and
- no unreviewed bypass actor.

Where the repository plan supports them, keep private vulnerability reporting,
dependency graph, Dependabot alerts and security updates, secret scanning, push
protection, validity checks, and non-provider patterns enabled.

Settings audits should capture current API output or screenshots in the PR or
release evidence that requested the audit. Do not copy a dated snapshot into
this durable runbook, and never paste detected secrets into an issue or PR.

## Review checklist

For dependency, workflow, or repository-policy changes:

- verify action and dependency provenance;
- run the locked workspace, MSRV, lint, and security checks affected by the
  change;
- confirm provider credentials and live-spend acknowledgements remain absent
  from ordinary CI;
- confirm generated default credentials cannot be committed;
- update exception ownership and review dates when applicable; and
- re-read repository settings after any administrator-side change.
