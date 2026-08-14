# Hubu Release Runbook

Hubu publishes immutable GitHub Releases from `.github/workflows/release.yml`.
Gongbu and other compatibility consumers must pin an exact release tag and the
matching SHA-256 checksum. A moving checkout of `main` is not a supported
routine integration-test dependency.

## Versions and channels

Hubu has two independently visible versions:

- `product_version` identifies the Hubu binaries. Stable releases use SemVer
  tags such as `v0.1.0`; `main` builds use
  `<cargo-version>-main.<12-character-commit>`.
- `executor_contract` identifies the negotiated external execution protocol.
  It remains `hubu-spend-executor-v4` and does not change merely because the
  Hubu product version changes.

At 10:00 America/Los_Angeles each day, the release workflow checks the current
`main` commit. If that commit does not already have a canary, a successful run
creates a prerelease tagged `main-<full-40-character-source-commit>`. If `main`
has not advanced since the previous canary, the build and publication jobs are
skipped. These releases are for early compatibility testing. There is
deliberately no mutable `latest-main` asset.

The schedule uses GitHub's timezone-aware cron support, which keeps publication
at 10:00 local time through daylight-saving transitions.

Stable release promotion is an explicit workflow dispatch. Stable and
prerelease tags are checked before publication and the workflow refuses to
replace an existing tag. GitHub Release assets are uploaded without overwrite
behavior.

## Supported binary targets

Each release contains the `hubu` CLI and `hubu-server` in a target-specific
archive:

| Platform | Target | Asset suffix |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `x86_64-apple-darwin.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `aarch64-apple-darwin.tar.gz` |

Every archive includes `PROVENANCE.json` with the product version, full source
commit, executor contract, Rust target, repository, workflow run, and locked
dependency declaration. It also includes `LICENSE-MIT`, `LICENSE-APACHE`, and
`THIRD-PARTY-NOTICES.md` and the exact `Cargo.lock` dependency inventory so the
applicable project licenses and dependency notice material travel with the
binaries. The release includes `SHA256SUMS` covering every archive.

## Pin, verify, and install

Choose an exact tag from GitHub Releases. For example:

```sh
repo=hacker-no-ice/hubu
tag=main-FULL_40_CHARACTER_COMMIT_SHA
asset=hubu-0.1.0-main.SHORTCOMMIT-aarch64-apple-darwin.tar.gz

gh release download "$tag" --repo "$repo" --pattern "$asset" --pattern SHA256SUMS
grep "  $asset" SHA256SUMS | shasum -a 256 -c -
tar -xzf "$asset"
```

On Linux, replace `shasum -a 256 -c -` with `sha256sum -c -`. Inspect the
provenance before installing:

```sh
package=${asset%.tar.gz}
cat "$package/PROVENANCE.json"
"$package/hubu-server" --version
install "$package/hubu" "$package/hubu-server" /usr/local/bin/
```

The reported `source_commit` must equal the pinned revision and the reported
`executor_contract` must be `hubu-spend-executor-v4`. Consumers should record
the exact tag, asset filename, and checksum in their lock/configuration file.

## Promote a stable release

First validate a commit-addressed prerelease, including its release smoke jobs.
Then dispatch the workflow with a new SemVer tag and the exact full commit SHA:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  -f version=v0.1.0 \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

The source must be an ancestor of `main`. Promotion reruns formatting, Clippy,
the locked workspace tests, the core integration flow, and a locked release
build before creating platform artifacts. After publication, clean GitHub
runners download the release, verify `SHA256SUMS`, require the project license
and third-party notice files, start an isolated `hubu-server`, and check
`/health`, `/version`, and local `--version` metadata.
HTTP probes use bounded connection and total-request timeouts so an unavailable
or non-responsive server fails the smoke job promptly.

## Rollback, deprecation, and retention

Rollback means changing the consumer pin to an older validated tag and
checksum. Never move a tag, replace an asset, or edit an old checksum to perform
a rollback.

If a release is unsafe, mark it deprecated in its GitHub Release notes and
publish a replacement version. Leave its tag and artifacts intact so existing
pins remain auditable and failures are explicit. Stable and commit-addressed
GitHub Releases are retained indefinitely under the current policy; temporary
Actions build artifacts are retained for seven days because the release assets
are the durable copies. Any future retention change must preserve tags,
checksums, and provenance and be announced before assets are removed.
