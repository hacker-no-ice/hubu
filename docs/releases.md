# Hubu Release Runbook

Hubu publishes immutable unified Hubu/Gongbu releases from
`.github/workflows/release.yml`. Consumers must pin an exact release tag and the
matching SHA-256 checksum. A moving checkout of `main` is not a supported
routine integration-test dependency.

All current releases are experimental, local-first builds for the localhost
demo server and mock payment rail. They are not approved for real-money
production use. A `stable` SemVer channel means that the artifact has an
intentional immutable version and passed the listed repository checks; it does
not mean production security, capacity, or payment-rail readiness.

## Versions and channels

The distribution has two independently visible versions:

- `product_version` identifies all six production binaries. Stable releases
  use SemVer tags such as `v0.1.0`; `main` builds use
  `<cargo-version>-main.<12-character-commit>`.
- `executor_contract` identifies the negotiated external execution protocol.
  It is `hubu-spend-executor-v4.2` and does not change merely because the
  Hubu product version changes.

At 10:00 America/Los_Angeles each day, the release workflow checks the current
`main` commit. If that commit does not already have a canary, a successful run
creates a prerelease tagged `main-<full-40-character-source-commit>`. If `main`
has not advanced since the previous canary, the build and publication jobs are
skipped. These releases are for early compatibility testing. There is
deliberately no mutable `latest-main` asset.

An operator can publish the same immutable canary on demand for an exact commit
that is already contained in `main`:

```sh
source_commit=FULL_40_CHARACTER_COMMIT_SHA
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=canary \
  -f source_commit="$source_commit"
```

The resulting tag remains `main-<full-source-commit>`. Repeating the request
does not replace the tag or assets. The workflow exits without publishing only
after it confirms that the existing release is a published prerelease targeting
the requested commit with all four non-empty archives and `SHA256SUMS`; a draft,
partial, or mismatched release fails closed for operator recovery. Use this path
for a time-sensitive cutover instead of waiting for the next scheduled run.

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

Each release contains exactly six production binaries in a target-specific
archive:

| Binary | Separate runtime responsibility |
| --- | --- |
| `hubu` | Human/developer control-plane CLI |
| `hubu-server` | Hubu control-plane HTTP process, governance database, policy, budgets, claims, settlement, and ledger |
| `hubu-unified-mcp` | Default agent-facing MCP router over independently configured Hubu and Gongbu backends |
| `hubu-mcp-server` | Opt-in compatibility Hubu MCP adapter to the control plane |
| `gongbu-server` | Gongbu execution-plane HTTP process, database, Temporal worker, provider credentials/calls, artifacts, and recovery |
| `gongbu-mcp` | Opt-in compatibility Gongbu MCP adapter to the execution plane |

`hubu-bench` and `gongbu-sandbox` are development tools and are explicitly
excluded. A shared archive does not merge backend processes, databases,
credentials, provider execution, artifacts, or failure domains. The unified MCP
binary communicates with each backend only through its versioned HTTP contract.

| Platform | Target | Asset suffix |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `x86_64-apple-darwin.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `aarch64-apple-darwin.tar.gz` |

Every archive includes `MANIFEST.json` and `PROVENANCE.json`. Both enumerate the
six binaries, the default and compatibility agent surfaces, product version,
full source commit, executor contract, and Rust target; provenance also records
the repository, workflow run, and locked
dependency declaration. The archive carries its own `SHA256SUMS` for every
manifested file plus `LICENSE-MIT`, `LICENSE-APACHE`,
`THIRD-PARTY-NOTICES.md`, a target-specific `THIRD-PARTY-LICENSES.txt` bundle,
and the exact `Cargo.lock` dependency inventory. License generation covers the
locked normal dependency graph of all six production binaries and fails when
an included third-party crate lacks license material. The GitHub Release also
publishes a top-level `SHA256SUMS` covering all four target archives.

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
cat "$package/MANIFEST.json"
for binary in hubu hubu-server hubu-unified-mcp hubu-mcp-server gongbu-server gongbu-mcp; do
  "$package/$binary" --version
done
install \
  "$package/hubu" \
  "$package/hubu-server" \
  "$package/hubu-unified-mcp" \
  "$package/hubu-mcp-server" \
  "$package/gongbu-server" \
  "$package/gongbu-mcp" \
  /usr/local/bin/
```

Configure the default single agent entry after installation with
`hubu init codex`. Migrate an existing two-entry configuration deterministically
with `hubu init codex --migrate-standalone --gongbu-endpoint URL
--gongbu-token-file FILE`; migration refuses to change the config when a
standalone Gongbu entry lacks replacement settings. The packaged standalone
binaries remain available only through explicit compatibility configuration such as
`hubu init codex --compatibility-standalone` or a manually retained
`gongbu-mcp` entry.

Every binary's reported `product_version` and `source_commit` must match the
archive provenance. Its `executor_contract` (or Gongbu's equivalent
`hubu_executor_contract` field) must be `hubu-spend-executor-v4.2`. Consumers
should record the exact tag, asset filename, and checksum in their
lock/configuration file.

## Promote a stable release

First validate a commit-addressed prerelease, including its release smoke jobs.
Then dispatch the workflow with a new SemVer tag and the exact full commit SHA:

```sh
gh workflow run release.yml \
  --repo hacker-no-ice/hubu \
  --ref main \
  -f channel=stable \
  -f version=v0.1.0 \
  -f source_commit=FULL_40_CHARACTER_COMMIT_SHA
```

The source must be an ancestor of `main`. Promotion reruns formatting, Clippy,
the locked all-target workspace tests, the core integration flow, packaging
negative tests, and a locked six-binary release build before creating platform
artifacts. Before publication, a deterministic native archive smoke verifies
the six binaries, starts an isolated `hubu-server`, initializes the unified MCP
server, verifies default config generation and unified tool discovery, then
initializes both standalone compatibility adapters without making provider
calls or spend requests. After
publication, native runners for all four supported targets download the release,
verify both checksum layers, manifests, provenance, licenses, notices, lockfile,
all six `--version` surfaces, Hubu `/health` and `/version`, unified MCP tool
discovery, and standalone compatibility initialization. No smoke test enables
provider credentials or spend.
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

### Unified MCP zero-user retirement canary

For the dated HUB-108 owner-approved zero-user cutover, one fresh immutable
packaged canary from the final `main` commit containing HUB-106, HUB-107, and
HUB-108 replaces the former two-canary/14-day pre-deprecation wait. Its release
URL, tag, full source SHA, workflow run, platform archives, `SHA256SUMS`, and
evidence index form one immutable candidate. The canary must satisfy the
[complete retirement evidence matrix](unified-mcp-contract.md#fresh-packaged-canary-evidence-matrix),
show zero unresolved P0/P1 findings, and receive an explicit GO before HUB-97.
This policy change does not itself run or approve that canary.

After GO, HUB-97 and then HUB-98 may proceed sequentially without the former
90-day/two-stable-release wait. At least one immutable rollback release with
checksums and provenance must remain available until HUB-98 verifies removal,
workspace integrity, and stale operational references. Retaining that artifact
does not permit mixed Hubu/Gongbu versions or merged backend state; rollback
continues to change consumer pins and MCP configuration only.
