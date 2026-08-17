# Gongbu History Import (HUB-80)

Date: 2026-08-17 (America/Los_Angeles)

## Pinned inputs and topology

HUB-80 imports only the object graph reachable from audited Gongbu
`refs/heads/main` commit
`b7132f647f14cc0d527384150341e7b42cbed1b4`. The Hubu base is merged HUB-79
commit `d911e7a4f01843dbfdd1b3d57389764da83c77b2`.

The unsquashed subtree import first placed the complete approved tip tree under
`.gongbu-import/`. Its merge commit has the Hubu base as first parent and the
exact audited Gongbu commit as second parent. A separate relocation commit then
moved live component files into their final paths and reconciled root
collisions. The two commits can be reverted independently before workspace
activation.

## Live-tree relocation

| Audited Gongbu path | Hubu path |
| --- | --- |
| `crates/gongbu-api/` | `crates/gongbu-api/` |
| `crates/gongbu-build-info/` | `crates/gongbu-build-info/` |
| `crates/gongbu-mcp/` | `crates/gongbu-mcp/` |
| `docs/` | `docs/gongbu/` |
| `examples/` | `examples/gongbu/` |
| `README.md` | `docs/gongbu/README.md` |

Relocation-induced links and command paths in the moved documentation, plus two
source-level ignored-test messages that point readers to those runbooks, were
updated to the new locations. These path-only edits are the live-content parity
allowlist; crate behavior, protocol, configuration formats, and Cargo manifests
remain unchanged.

## Root-collision allowlist

Hubu's root files remain authoritative:

- `LICENSE-MIT` and `LICENSE-APACHE` are retained unchanged. Gongbu had no
  license-text file at the audited tip.
- Gongbu's `README.md` is retained as `docs/gongbu/README.md`; Hubu's root
  `README.md` is unchanged.
- Gongbu's `.gitignore` contributed only the previously missing root-local
  `.config/`, `.rustc_info.json`, and `debug/` exclusions. Hubu's existing
  ignore rules remain intact.
- The generally applicable GitHub connector and review-comment conventions
  from Gongbu's `AGENTS.md` were appended to Hubu's `AGENTS.md`. Hubu-specific
  worktree, architecture, and registration instructions remain intact.
- Gongbu's root `Cargo.toml` and `Cargo.lock` were intentionally not installed.
  Unified workspace membership, dependency reconciliation, and lockfile
  regeneration belong to HUB-81.
- Gongbu's `.github/workflows/ci.yml` was intentionally not installed. Unified
  CI coverage belongs to HUB-82.
- Gongbu's `.github/pull_request_template.md` was intentionally not installed;
  Hubu's existing template remains authoritative, with broader contributor and
  monorepo documentation reconciliation deferred to HUB-85.

Every omitted source file remains recoverable from preserved history, for
example:

```sh
git show b7132f647f14cc0d527384150341e7b42cbed1b4:Cargo.toml
git show b7132f647f14cc0d527384150341e7b42cbed1b4:Cargo.lock
git show b7132f647f14cc0d527384150341e7b42cbed1b4:.github/workflows/ci.yml
git show b7132f647f14cc0d527384150341e7b42cbed1b4:.github/pull_request_template.md
```

## Deferred integration

The Gongbu crates are intentionally absent from the root Cargo workspace in
this import. HUB-81 activates and reconciles the unified workspace, HUB-82
consolidates CI, and HUB-85 completes repository-wide documentation and updates
the architecture visualizer. Gongbu remains a distinct execution-plane
component rather than part of the Hubu control-plane process or core crates.
