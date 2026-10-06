# Publishing releases (maintainers)

> This page is for Hubu maintainers who publish releases. To install, update,
> or uninstall Hubu, see [Release operations](releases.md).

Hubu publishes immutable unified Hubu/Gongbu releases through
`.github/workflows/release.yml`.

Every release channel requires an explicit workflow dispatch. A manual canary
dispatch publishes at most one prerelease for each exact `main` commit, tagged
`main-<full-source-commit>`. There is no mutable latest-main artifact.
Versioned release candidates may use immutable `vX.Y.Z-rc.N` tags when a
release needs prerelease testing before stable publication. No channel replaces
an existing tag or asset.

The changelog presents stable release history. While a release line is under
validation, its top entry must use the active candidate version so the release
checker can keep the runbook and source package version aligned. After stable
promotion, fold the candidate notes into one self-contained stable entry that
describes the complete change from the previous stable release, and remove
candidate-specific chronology.

## Publish a canary

To publish the immutable canary for an exact commit already contained in
`main`:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=canary \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

The workflow refuses draft, partial, or mismatched existing releases. Repeating
the request never moves the tag or replaces an asset.

## Publish a release candidate

When a release needs prerelease testing, publish a versioned candidate from a
validated commit-addressed canary:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=candidate \
  -f version=v0.2.2-rc.1 \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

Release-candidate tags and assets are immutable. If testing finds a problem,
publish the fix from a new `main` commit as the next candidate number. Never
replace an existing candidate.

## Publish a stable release

Validate the intended source commit in CI and, when used, as a release
candidate. Then dispatch the stable release from that exact source commit:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=stable \
  -f version=v0.2.2 \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

Stable publication reruns formatting, Clippy, locked workspace tests,
integration and packaging checks, the exact-tag source installer on Intel and
Apple silicon, archive verification, and native published-asset smoke tests.
Ordinary release validation never supplies provider credentials or enables
provider spend.

## Rollback and retention

Rollback rebuilds from another validated unified release tag and its published
full source commit. Automation that intentionally consumes a secondary archive
also pins its checksum. Never move a tag, replace an asset, edit an old
checksum, mix binaries from releases, or copy backend state between versions.

If a release is unsafe, mark it deprecated in its release notes and publish a
replacement. Stable and commit-addressed releases remain immutable and
retained; short-lived Actions artifacts are not the durable distribution
surface.
