# HUB-82 Unified CI Coverage Map

This note records how the former Hubu CI workflow and the audited Gongbu
workflow at `b7132f647f14cc0d527384150341e7b42cbed1b4` map into the unified
10-package workspace workflow. The three job names remain stable because the
main ruleset protects `Test workspace`, `Check MSRV (Rust 1.88.0)`, and
`Security policy`.

| Former coverage | Unified coverage | Disposition |
| --- | --- | --- |
| Hubu and Gongbu workspace formatting | `cargo fmt --all -- --check` in `Test workspace` | Preserved once for the unified workspace. |
| Hubu Clippy with all features; Gongbu Clippy on all targets | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Superseded by the stricter union of both checks. |
| Gongbu `cargo check --workspace --all-targets --locked` | Exact-MSRV workspace check plus stricter Clippy coverage | Superseded without duplicating a second full current-toolchain compile. |
| Hubu locked workspace tests; Gongbu locked all-target tests | `cargo test --workspace --all-targets --locked` | Preserved with Gongbu's broader target coverage. |
| Gongbu named Hubu-v4 client/recovery filter | `cargo test --workspace --locked 'hubu::tests::'` | Preserved as a separately named diagnostic even though those tests also occur in the full suite. |
| Hubu release build of `hubu` and `hubu-server` | One locked release build of `hubu`, `hubu-server`, `hubu-mcp-server`, `gongbu-server`, and `gongbu-mcp` | Superseded by all five production binaries. `hubu-bench` and `gongbu-sandbox` remain development tools, not production binaries. |
| Hubu core integration flow | `scripts/integration-core-flow.sh` | Preserved. It uses a temporary database and the mock payment rail. |
| Hubu public Cargo metadata validation | `scripts/check-cargo-metadata.py` | Preserved and already expanded to the exact 10-package, Rust 1.88 workspace. |
| Hubu/Gongbu ownership boundary | `scripts/check-workspace-boundary.py` | Added during workspace activation and preserved as an explicit unified check. |
| Hubu generated default-credential ignore regression | `scripts/check-default-credential-ignore.sh` | Preserved. The helper unsets credential overrides and proves generated defaults cannot be committed. |
| Hubu documentation build | `cargo doc --workspace --no-deps --locked` | Preserved for all 10 packages. |
| Hubu cargo-deny policy | Pinned `EmbarkStudios/cargo-deny-action` in `Security policy` | Preserved for advisories, licenses, and sources with all features. |
| Hubu Rust 1.78 and Gongbu Rust 1.85 compatibility checks | `cargo +1.88.0 check --workspace --all-targets --all-features --locked` | Superseded by the unified exact Rust 1.88 MSRV. |
| Gongbu protobuf compiler setup | `protobuf-compiler` installation in both jobs that compile the Temporal-enabled workspace | Preserved for current-toolchain and exact-MSRV compilation. The cargo-deny job resolves policy and does not compile Temporal. |
| Gongbu unpinned checkout and stable-toolchain actions | Repository `rust-toolchain.toml`, exact MSRV installation, and checkout pinned to a full commit SHA | Superseded with reproducible, pinned inputs. |

## Ordinary CI Safety Boundary

The workflow has only `contents: read`, references no GitHub secrets, forces
the optional Gongbu MCP integration off, and fixes both sandbox boundaries to
mock mode. `scripts/check-ci-provider-safety.py` makes those constraints
executable, rejects live-spend acknowledgements or provider configuration in
ordinary CI, verifies every action is commit-pinned, and verifies named live
provider tests remain ignored by default. Consequently the normal pull-request
and `main` workflow has neither credentials nor the explicit opt-ins required
to send billable provider traffic. Fixture HTTP tests and Hubu's core flow stay
local and non-billable.

## Deliberate Deferrals

- Cross-component executor E2E remains HUB-83. HUB-82 retains the named
  Gongbu Hubu-v4/recovery coverage without introducing the new topology.
- Unified release archive contents, target matrices, and packaging remain
  HUB-84. The existing release workflow is not redesigned here.
