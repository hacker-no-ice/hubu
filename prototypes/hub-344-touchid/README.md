# HUB-344 Touch ID prototype (throwaway, do not merge)

This is a time-boxed spike for HUB-344. It answers one question: can a Hubu
binary built from source, with no paid Apple signing, raise the macOS Touch ID
dialog with custom text and get a trustworthy yes or no back?

It is not part of the Hubu workspace. It has its own empty `[workspace]`
table and its own `Cargo.lock`, so the root workspace, CI and release
packaging are unchanged. Delete it once HUB-344 records its decision.

## Binding

- `objc2-local-authentication` 0.3.2 (`Zlib OR Apache-2.0 OR MIT`), with
  `objc2` 0.6.5, `objc2-foundation` 0.3.2, `block2` 0.6.2, `objc2-encode`
  4.1.0 (all MIT) and `bitflags` 2.13.2 (`MIT OR Apache-2.0`).
- It is the same objc2 family Hubu already ships (`objc2-core-foundation`,
  `objc2-io-kit` 0.3.2).
- `cargo deny --manifest-path prototypes/hub-344-touchid/Cargo.toml --config deny.toml check`
  passes: advisories, bans, licences and sources are all ok.
- Third-party notices: like the objc2 crates Hubu already uses, these crates
  ship no `LICENSE*` file in the published package. Production use needs the
  five crate names added to the existing objc2 case in
  `scripts/generate-third-party-licenses.sh`, which points at
  `third-party-license-material/objc2-0.3.2-LICENSE.md`. No new licence text
  is needed.
- The binary links only system frameworks: `LocalAuthentication`,
  `Foundation`, `libobjc` and `libSystem`. No entitlements, provisioning
  profile or Team ID are needed.

## Build

```bash
cd prototypes/hub-344-touchid
cargo build --release
codesign -dv target/release/hub-344-touchid   # flags=0x20002(adhoc,linker-signed)
```

## Subcommands

| Command | Interactive? | What it does |
|---|---|---|
| `probe` | No | `canEvaluatePolicy` for both policies, plus the biometry type |
| `noninteractive [--policy biometrics\|owner]` | No | `evaluatePolicy` with `interactionNotAllowed = true`. The system must fail with `NotInteractive` (-1004) instead of showing UI |
| `ask --reason TEXT [...]` | **Yes** | Shows the real dialog and waits. Refuses to run unless `HUB344_INTERACTIVE=1` is set |

Policies: `biometrics` means Touch ID only, with no password fallback.
`owner` means Touch ID or the login password. Each run prints one JSON line.
The exit code is 0 for approved and 1 for denied or failed.

## Non-interactive results (Claude, 2026-11-17)

Machine: MacBook Pro Mac17,9 (M5 Pro), macOS 26.4 (25E246), Touch ID
enrolled. Binary: ad hoc, linker-signed, Rust 1.94.1.

| Run | Result |
|---|---|
| `probe` from a shell | both policies `ok`, `biometry_type: touch_id` |
| `noninteractive --policy biometrics` | `NotInteractive` (-1004) in 3 ms |
| `noninteractive --policy owner` | `NotInteractive` (-1004) in 2 ms |
| The same three as a launchd background job (`launchctl submit`, Aqua session) | identical results |
| Re-signed ad hoc **with hardened runtime** (`codesign -s - -o runtime`) | identical results; `DYLD_*` variables are stripped |

`NotInteractive` comes back only once the request has passed preflight and
would otherwise have shown UI. So the whole call path works from an
unentitled, ad hoc signed background process. The only part left is a person
seeing the dialog and responding.

## Owner check

See the HUB-344 Linear comment that asks for this check. It has the exact
commands and what to look for.
