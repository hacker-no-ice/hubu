# Hubu Release Runbook

Hubu publishes immutable releases from `.github/workflows/release.yml`. Today,
the target-specific archives contain `hubu` and `hubu-server`. Gongbu builds
from the same source repository and locked workspace but is not yet packaged in
those published archives; HUB-84 tracks the five-binary unified distribution.
Consumers of a published archive must pin an exact release tag and matching
SHA-256 checksum; a moving checkout of `main` is not a supported routine
integration-test dependency.

All current releases are experimental, local-first builds for the localhost
demo server and mock payment rail. They are not approved for real-money
production use. A `stable` SemVer channel means that the artifact has an
intentional immutable version and passed the listed repository checks; it does
not mean production security, capacity, or payment-rail readiness.

## Versions and channels

The current Hubu archive has two independently visible versions:

- `product_version` identifies the packaged Hubu binaries. Stable releases use
  SemVer tags such as `v0.1.0`; `main` builds use
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

Release notes for both channels must retain the experimental/non-production
status. Checksums and smoke tests establish artifact integrity and basic local
startup behavior only; they do not validate a production threat model or live
money movement.

## Supported binary targets

Each current target-specific archive contains two production binaries:

| Binary | Runtime responsibility |
| --- | --- |
| `hubu` | Human/developer control-plane CLI |
| `hubu-server` | Hubu control-plane HTTP process and governance storage |

## Unified archive target (HUB-84)

HUB-84 will extend the archive to five production binaries built from one
source commit and lockfile under one tag, product version, checksum set, and
provenance identity:

| Binary | Runtime responsibility |
| --- | --- |
| `hubu` | Human/developer control-plane CLI |
| `hubu-server` | Hubu control-plane HTTP process and governance storage |
| `hubu-mcp-server` | Hubu's agent-facing MCP adapter |
| `gongbu-server` | Gongbu execution-plane process, storage, workflow, credentials, providers, and artifacts |
| `gongbu-mcp` | Gongbu's separate agent-facing MCP adapter |

`hubu-bench` and `gongbu-sandbox` will remain development tools rather than
release artifacts. Unified packaging must not merge the production binaries'
processes, databases, credentials, provider boundary, failure domain, or MCP
surfaces.

| Platform | Target | Asset suffix |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `x86_64-apple-darwin.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `aarch64-apple-darwin.tar.gz` |

Every archive includes `PROVENANCE.json` with the product version, full source
commit, executor contract, Rust target, repository, workflow run, and locked
dependency declaration. It also includes `LICENSE-MIT`, `LICENSE-APACHE`, and
`THIRD-PARTY-NOTICES.md`, a target-specific `THIRD-PARTY-LICENSES.txt` bundle,
and the exact `Cargo.lock` dependency inventory so the applicable project
licenses and dependency notice material travel with the binaries. Packaging
fails if any included crate lacks license material. The release includes
`SHA256SUMS` covering every archive.

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
for binary in hubu hubu-server; do
  "$package/$binary" --version
done
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
and third-party notice files, verify both binaries' local `--version` metadata,
start an isolated `hubu-server`, and check `/health` and `/version`. HUB-84 owns
the future published-archive smoke coverage for Gongbu.
HTTP probes use bounded connection and total-request timeouts so an unavailable
or non-responsive server fails the smoke job promptly.

Before any real-money deployment, complete the security, authority, payment
rail, storage, observability, concurrency, reliability, and independent-review
work listed in the top-level README. Release promotion must not be used as a
substitute for those deployment gates.

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
