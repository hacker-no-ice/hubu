# Repository security baseline

This document records the reviewable repository-side controls and the GitHub
settings that an administrator must apply after the change containing this
document merges. Repository settings are deliberately not changed by the pull
request that introduces the policy.

## Dependency and action update policy

Dependabot checks the Rust workspace and GitHub Actions every Monday. Rust
updates must keep `Cargo.lock` committed and pass the MSRV, locked workspace
test, Clippy, release-build, and security jobs. Minor and patch Rust updates are
grouped; major updates remain separate so their compatibility impact is
reviewable. GitHub Actions updates are grouped and must preserve full commit-SHA
pins.

Every `uses:` reference to a third-party action must use a full 40-character
commit SHA followed by a comment naming the corresponding release tag. A tag is
useful review context but is not an immutable trust boundary. Before accepting
an update, verify the SHA against the action publisher's GitHub release/tag and
review upstream release notes. The current pins use Node 24 action releases;
this deliberately replaces `actions/checkout@v4`, whose Node 20 runtime was
being force-migrated by GitHub Actions.

## Advisory and license exceptions

The `Security policy` CI job runs cargo-deny against all features and checks
RustSec advisories, licenses, and dependency sources. `deny.toml` is the only
exception surface. Checks must not be made non-blocking and lint levels must not
be downgraded merely to make CI pass.

An exception requires a focused pull request that includes:

1. the advisory ID or exact crate and version range;
2. an owner and a concrete technical justification;
3. the affected Hubu paths and compensating controls;
4. a removal condition and review date; and
5. evidence that the exception is narrower than ignoring the dependency.

Use cargo-deny's structured entries with a `reason` for advisory ignores. Use a
license exception only after verifying the upstream license text and recording
why it is compatible with Hubu's MIT/Apache-2.0 distribution. Never use a broad
source allow-list exception for an unreviewed Git dependency.

The unified Hubu/Gongbu dependency graph has two exact-version license
exceptions owned by the Hubu maintainers:

- `option-ext@0.2.0` is available under MPL-2.0 and is used only transitively by
  Temporal's configuration-directory support. MPL-2.0 is file-level copyleft
  and does not alter the license of Hubu or Gongbu. Cargo-deny pins the
  exception to this exact crate version, and the release bundle reproduces the
  crate's license material. Remove the exception when Temporal no longer
  selects `option-ext`, or re-review it before 2026-11-17.
- `webpki-roots@1.0.9` is available under CDLA-Permissive-2.0 and supplies the
  public Mozilla trust-root data used by Gongbu's Rustls HTTP clients. The
  exception is exact-version only, Cargo-deny continues to reject unknown
  sources, and the release bundle reproduces the distributed license material.
  Remove the exception when the HTTP graph no longer selects this version, or
  re-review it before 2026-11-17.

## Audited GitHub baseline

Read-only audit on 2026-08-13 (America/Los_Angeles):

- repository visibility: public;
- default branch: `main`;
- active repository ruleset: `main branch protection`, targeting the default
  branch with no bypass actors;
- rules: prevent deletion and non-fast-forward updates, require pull requests,
  and require strict/up-to-date `Test workspace` and
  `Check MSRV (Rust 1.78.0)` status checks;
- GitHub private vulnerability reporting: disabled;
- secret scanning, non-provider patterns, validity checks, and push protection:
  disabled;
- Dependabot security updates: disabled; and
- latest audited `main` commit `6eb4032f683f0a08f837d8faf0115df608befef4`
  passed both required CI checks.

The repository has a ruleset, so the legacy branch-protection endpoint returning
"Branch not protected" does not mean `main` is unprotected.

## Required post-merge GitHub actions

An administrator must perform these steps only after this change is merged.
Record screenshots or API output in the release-readiness evidence.

1. Confirm the merged `main` workflow completes successfully and emits the
   exact check name `Security policy`.
2. In **Settings → Code security**, enable **Private vulnerability reporting**.
   Then open the public **Security** tab in a signed-out browser and confirm
   **Report a vulnerability** is available before advertising the channel.
3. In **Settings → Code security**, enable **Dependency graph**, **Dependabot
   alerts**, and **Dependabot security updates**. Confirm Dependabot recognizes
   both ecosystems in `.github/dependabot.yml` and can open a lockfile-bearing
   Rust update PR.
4. In **Settings → Code security**, enable **Secret scanning**, **Push
   protection**, **Validity checks**, and **Non-provider patterns** wherever the
   repository plan exposes them. Resolve any pre-existing findings through the
   Security tab; never paste a detected secret into an issue or PR.
5. Edit repository ruleset **main branch protection**, replace the retired
   `Check MSRV (Rust 1.78.0)` requirement with `Check MSRV (Rust 1.88.0)`, and
   add required status check `Security policy` from GitHub Actions (integration
   ID `15368`). Preserve strict/up-to-date checks, the existing `Test workspace`
   requirement, deletion and non-fast-forward prevention, the pull-request
   rule, no bypass actors, and active enforcement.
6. Re-read the settings through the GitHub API and verify private vulnerability
   reporting, Dependabot security updates, secret scanning, and push protection
   all report enabled. Open a test PR to confirm all three required checks block
   merging until successful.

Prerequisites are repository administrator access, a successful security job on
merged `main` so its status context is selectable, and a plan/visibility that
supports the requested GitHub security features. If a feature is unavailable,
record the exact unavailable control and do not claim it is enabled.
