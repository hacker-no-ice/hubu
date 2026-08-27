# Release operations

Hubu publishes immutable unified Hubu/Gongbu releases through
`.github/workflows/release.yml`. Consumers pin an exact tag and matching
SHA-256 checksum; a moving checkout of `main` is not a supported integration
dependency.

Current releases are experimental local-first builds. A stable SemVer tag means
the immutable artifact passed the repository's release checks, not that Hubu is
approved for production money movement.

## Versions and channels

The distribution exposes two independent versions:

- `product_version` identifies all four production binaries. Stable releases
  use SemVer tags; `main` builds include the source commit.
- `executor_contract` identifies the Hubu-to-executor protocol and changes only
  with that protocol. The current value is `hubu-spend-executor-v4.3`.

A scheduled workflow publishes at most one prerelease for each exact `main`
commit, tagged `main-<full-source-commit>`. There is no mutable latest-main
artifact. Versioned release candidates use immutable `vX.Y.Z-rc.N` tags for
human validation before stable promotion. Stable promotion is an explicit
workflow dispatch, and no channel replaces an existing tag or asset.

The changelog presents stable release history. While a release line is under
validation, its top entry must use the active candidate version so the release
checker can keep the runbook and source package version aligned. After stable
promotion, fold the candidate notes into one self-contained stable entry that
describes the complete change from the previous stable release, and remove
candidate-specific chronology.

## Release contents

Every target archive contains exactly these production binaries:

| Binary | Responsibility |
| --- | --- |
| `hubu` | Human and developer CLI |
| `hubu-server` | Hubu control-plane HTTP process and governance state |
| `hubu-unified-mcp` | Agent-facing router over separate backends |
| `gongbu-server` | Gongbu execution plane, Temporal worker, providers, and artifacts |

Development binaries such as `hubu-bench` and `gongbu-sandbox` are excluded.
A shared archive does not merge databases, credentials, provider execution,
artifacts, lifecycle, or failure domains.

Archives also include:

- `MANIFEST.json` and `PROVENANCE.json`;
- internal checksums for every manifested file;
- `LICENSE-MIT`, `LICENSE-APACHE`, and third-party notices;
- a target-specific third-party license bundle; and
- the exact `Cargo.lock` dependency inventory.

The GitHub Release publishes a top-level `SHA256SUMS` for its target archives.
The current pre-launch platform matrix is macOS Intel and Apple silicon. Linux
targets must be restored and verified before Linux is advertised as supported.

## Pin, verify, and install

Choose an exact tag and archive from GitHub Releases:

```sh
repo=hacker-no-ice/hubu
tag=main-FULL_40_CHARACTER_COMMIT_SHA
asset=hubu-PRODUCT_VERSION-aarch64-apple-darwin.tar.gz

gh release download "$tag" --repo "$repo" --pattern "$asset" --pattern SHA256SUMS
grep "  $asset" SHA256SUMS | shasum -a 256 -c -
tar -xzf "$asset"
```

On Linux, use `sha256sum -c -`. Before installation, inspect the manifest and
provenance and compare all binary version surfaces:

```sh
package=${asset%.tar.gz}
cat "$package/PROVENANCE.json"
cat "$package/MANIFEST.json"
for binary in hubu hubu-server hubu-unified-mcp gongbu-server; do
  "$package/$binary" --version
done
```

Every binary must report the product version and source commit recorded in the
archive. Hubu and Gongbu must both report `hubu-spend-executor-v4.3`.
`LOCAL-STACK.md` is the packaged, command-focused path for initializing,
starting, inspecting, and connecting the four-binary stack outside a source
checkout. It links to the public configuration reference for detailed choices.
Its linked `unified-mcp.md` guide is included at the same relative path. The
standalone Gongbu operations guide remains in the source documentation but is
intentionally excluded from the managed-stack release bundle.

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

After validating a commit-addressed canary, publish a versioned candidate for
human testing:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=candidate \
  -f version=v0.2.0-rc.2 \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

Release-candidate tags and assets are immutable. If testing finds a problem,
publish the fix from a new `main` commit as the next candidate number. Never
replace an existing candidate.

## Promote a stable release

Validate the candidate and all published-archive smoke jobs, then dispatch the
stable promotion from the same source commit:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=stable \
  -f version=v0.2.0 \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

Promotion reruns formatting, Clippy, locked workspace tests, integration and
packaging checks, release builds, archive verification, and native published
asset smoke tests. Ordinary release validation never supplies provider
credentials or enables provider spend.

## Rollback and retention

Rollback changes the consumer pin to another validated unified release tag and
checksum. Never move a tag, replace an asset, edit an old checksum, mix binaries
from releases, or copy backend state between versions.

If a release is unsafe, mark it deprecated in its release notes and publish a
replacement. Stable and commit-addressed releases remain immutable and
retained; short-lived Actions artifacts are not the durable distribution
surface.
