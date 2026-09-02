# Release operations

Hubu publishes immutable unified Hubu/Gongbu releases through
`.github/workflows/release.yml`. Consumers pin an exact tag and its published
full source commit; a moving checkout of `main` is not a supported installation
or integration dependency.

Current releases are experimental local-first builds. A stable SemVer tag means
the exact source passed the repository's release checks, not that Hubu is
approved for production money movement or verified by Apple. For `v0.2.1` and
later, compiling that exact source locally is the primary supported macOS
installation path for initial technical users.

## Versions and channels

The distribution exposes two independent versions:

- `product_version` identifies all four production binaries. Stable releases
  use SemVer tags; `main` builds include the source commit.
- `executor_contract` identifies the Hubu-to-executor protocol and changes only
  with that protocol. The current value is `hubu-spend-executor-v4.3`.

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

## Published archive contents

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

`LOCAL-STACK.md` is the packaged, command-focused path for initializing,
starting, inspecting, and connecting the four-binary stack outside a source
checkout. Its linked `unified-mcp.md` guide is included at the same relative
path. The standalone Gongbu operations guide remains in the source
documentation but is intentionally excluded from the managed-stack archive.

The GitHub Release publishes a top-level `SHA256SUMS` for its target archives.
The current pre-launch platform matrix is macOS Intel and Apple silicon. Linux
targets must be restored and verified before Linux is advertised as supported.

The downloadable macOS archives are not Developer ID-signed or Apple-notarized.
They remain useful as immutable release and automation evidence, but they are
not the recommended initial-user installation path. Existing archives,
including `v0.2.0` and earlier, remain immutable and will not be replaced with
differently signed bytes.

## Install an exact release from source (macOS)

The installer checks prerequisites but does not install or update them. Before
cloning Hubu, provide:

- Git and the standard macOS `install` utility;
- Xcode Command Line Tools, visible to `xcode-select -p`;
- `rustup`, with Cargo able to use the exact toolchain pinned by the checkout's
  `rust-toolchain.toml`; and
- the Protocol Buffers compiler, visible as `protoc`.

For example, Homebrew users can install the Protocol Buffers compiler with
`brew install protobuf`. Install Xcode Command Line Tools and `rustup` from
their official distribution channels. The Hubu installer never invokes another
installer, requests `sudo`, or changes system security settings.

Choose an exact stable or release-candidate tag. On its GitHub Release page,
copy the complete 40-character `Source commit` value. Clone the tag into a new
directory and pass that published commit to the repository installer:

```sh
tag=vX.Y.Z
expected_commit=FULL_40_CHARACTER_COMMIT_SHA

git clone --depth 1 --branch "$tag" https://github.com/hacker-no-ice/hubu.git
cd hubu
./scripts/install-from-source.sh --expected-commit "$expected_commit"
```

The checkout must be clean, `HEAD` must equal the expected commit, and a strict
release tag must select that commit. When exactly one stable tag and one or more
candidate tags intentionally share a commit, the installer selects the stable
tag. Multiple stable tags, or multiple candidate tags without a stable tag,
fail as ambiguous; non-release checkouts also fail before compilation.

The installer uses `cargo build --release --locked` once for the unified
workspace, stamps one release identity into exactly these production binaries,
and stages and verifies the complete set before installing it:

- `hubu`;
- `hubu-server`;
- `hubu-unified-mcp`; and
- `gongbu-server`.

Development binaries such as `hubu-bench` and `gongbu-sandbox` are not
installed. The default prefix is `~/.local`; choose another absolute prefix
when needed:

```sh
./scripts/install-from-source.sh \
  --expected-commit "$expected_commit" \
  --prefix /absolute/install/prefix
```

The destination is `PREFIX/bin`. Put it on `PATH`, then verify the installed
lineage before creating or updating a profile:

```sh
for binary in hubu hubu-server hubu-unified-mcp gongbu-server; do
  command -v "$binary"
  "$binary" --version
done
```

Every binary must report the selected tag as its product version, the same
non-`unknown` full source commit, and `hubu-spend-executor-v4.3`. The installer
performs this verification before replacing the destination files. These
release-stamped local builds work with the normal managed-stack lineage checks;
keep `allow_development_builds = false`.

### Local-build trust model and cost

This path avoids recommending an unsigned downloaded executable. Because the
executables are compiler outputs created on the user's Mac, normal use requires
no quarantine deletion, alternate Open action, or Security Settings exception.
It does not make Hubu Developer ID-signed, Apple-notarized, or Apple-verified.

The expected tag and full commit identify the source exactly, while `--locked`
uses the reviewed dependency versions in `Cargo.lock`. Compilation still runs
that source and its build-time dependencies with the user's permissions; review
the selected release and its dependency changes according to your trust needs.
After the prerequisite toolchain is installed, the first build downloads Rust
crate dependencies that are not already cached, then compiles the unified
workspace. Expect it to take materially more time, network transfer, and disk
space than unpacking a prebuilt archive.

### Update

Read the target release notes for state or configuration compatibility before
updating. Stop the managed stack, clone the new exact tag into a fresh checkout,
copy that release's published full source commit, and run the same installer
against the same prefix:

```sh
hubu stack stop

tag=vNEW.VERSION
expected_commit=NEW_FULL_40_CHARACTER_COMMIT_SHA
git clone --depth 1 --branch "$tag" https://github.com/hacker-no-ice/hubu.git hubu-update
cd hubu-update
./scripts/install-from-source.sh --expected-commit "$expected_commit"

hubu stack doctor
hubu stack start
```

Pass the same `--prefix` used for the original installation when it was not
`~/.local`. Never update from a moving branch or mix binaries built from
different tags. `stack start` renders an updated generation when the selected
release changes.

### Uninstall

Stop the stack, then remove only the four installed files from the selected
prefix:

```sh
prefix="$HOME/.local"
hubu stack stop
rm -f \
  "$prefix/bin/hubu" \
  "$prefix/bin/hubu-server" \
  "$prefix/bin/hubu-unified-mcp" \
  "$prefix/bin/gongbu-server"
```

Uninstalling binaries does not delete operator-owned profiles, Hubu or Gongbu
databases, artifacts, Temporal state, or logs. Retain, archive, or remove each
explicit profile path separately according to its data-retention requirements.

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
  -f version=v0.2.1-rc.1 \
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
  -f version=v0.2.1 \
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
