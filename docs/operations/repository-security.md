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

## Proactive advisory remediation

The `Security advisory maintenance` workflow checks the default branch every
six hours and can also be started with `workflow_dispatch` on the default
branch. GitHub scheduled runs are best effort. The required `Security policy` PR check remains
blocking; scheduled detection does not replace it.

For an end-to-end detection and repair exercise, dispatch the workflow with
`exercise` enabled. It creates an isolated temporary Rust project locked to the
known vulnerable `rustls 0.23.45`, detects its real RustSec advisories, prepares a
compatible fixed lockfile, and checks policy, Rust 1.88, and a TLS-provider test.
The fixture does not change the workspace or publish vulnerable dependencies.
Ordinary PR tests use deterministic diagnostic and GitHub API fixtures so a
historical vulnerable-version exercise cannot become a new global CI blocker.

The monitor scans the committed `Cargo.lock` against RustSec with the same
all-feature advisory policy used in CI. When a compatible fix is available, it
attempts only the smallest targeted dependency updates and validates the result
with the exact Rust 1.88 MSRV check, locked workspace tests and Clippy, the full
advisory/license/source policy, and builds of all four production binaries.
It publishes the verified lockfile change to one reusable
`codex/security-advisories` branch and PR. It never changes manifests, raises the
MSRV, edits `deny.toml`, or merges the PR automatically. Existing exact-version
license exceptions can therefore require a separately reviewed policy change.
An existing Dependabot PR is reused only when its complete Git tree matches
the validated candidate. Broader grouped updates keep the separate validated
remediation PR available for review.

If no compatible fix is available, or any validation fails, the workflow opens
or updates one remediation issue instead. The issue records the advisory IDs,
affected versions and dependency paths, available fix information, the failure,
and the workflow evidence. Repeated runs update existing findings instead of
opening duplicate PRs or issues. Set the repository variable
`SECURITY_MAINTAINER` to the GitHub login responsible for triage; the repository
owner is the fallback notification target. The owner should inspect new
findings promptly, choose a dependency replacement or upstream fix when needed,
and follow the exception review requirements below if no safe update exists.

Scanning, dependency resolution, and validation run without provider credentials
or write permissions, with the same mock-only provider settings as ordinary CI.
Only a separate publication job receives narrowly scoped repository-write
permissions. It publishes the validated lockfile and finding metadata from the
trusted default-branch run; it does not execute provider requests or accept
arbitrary PR code as a remediation input. When it creates or changes the
candidate, it explicitly dispatches ordinary CI for that branch so required
checks run even when a bot-created PR does not automatically start them. Its
Actions write permission is reserved for this dispatch. Human review and merge
remain required.

Before activating automatic fix PRs, verify **Settings → Actions → General →
Workflow permissions → Allow GitHub Actions to create and approve pull
requests** is enabled. Keep default workflow permissions restricted; only the
publication job requests the writes it needs, and it never approves a PR.
GitHub documents this separate prerequisite in its
[repository Actions settings reference](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository).
If PR creation is prohibited, publication fails visibly and creates an assigned
tracking issue so a maintainer can open the prepared fix manually.

Ordinary PR security failures must identify whether the committed default-branch
lockfile also fails the advisory policy. This distinguishes a newly introduced
dependency problem from an advisory published since the last successful run;
both still require the security gate to pass before merge.

Dependabot version-update configuration is separate from repository security
settings. Enable the dependency graph, Dependabot alerts, and Dependabot security
updates in the repository settings, and verify they remain enabled during a
settings audit. Cargo security updates have their own group, separate from
weekly version updates. RustSec findings such as unmaintained dependencies may
not have an equivalent Dependabot alert, so the scheduled policy scan remains
necessary.

The daily [Dependabot PR triage](dependabot-triage.md) routine keeps Dependabot
PRs current with `main`, merges patch and minor updates that pass every
required check, and reports major updates, failures and alerts without a PR in
one tracking issue.

For manual recovery, start `Security advisory maintenance` on the default
branch and inspect its summary and linked PR or issue. If automation cannot
publish, open a focused dependency PR from current `main`, reproduce the advisory with
`cargo deny --all-features check advisories licenses sources`, inspect the
reverse dependency path with `cargo tree --locked -i <crate>@<version>`, and use
`cargo update -p <crate>@<version> --precise <first-compatible-fixed-version>`
for a targeted fix. Review the lockfile diff and run the same validation listed
above before requesting review. Never relax the gate to clear an unrelated PR.

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

There are currently no advisory exceptions. RUSTSEC-2024-0449 (unmaintained
`unsafe-libyaml`) was ignored briefly and removed when HUB-270 replaced
`serde_yaml_ng` with `serde-saphyr`.

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
