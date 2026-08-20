## Worktree and branch discipline

Before coding:
1. Check `git status`.
2. Confirm the current branch and worktree.
3. Ensure the task branch is based on the latest `origin/main`; fetch/rebase only if needed.
4. Create a new task-specific branch if one has not already been created for this task.

Constraints:
- Do not reuse prior task branches.
- Do not modify unrelated files.
- Keep changes reviewable as one focused PR.
- Run relevant tests before finishing.
- Summarize the diff, tests run, and any caveats.
- If a Codex review is requested, address review comments in a follow-up commit.

## Unified workspace discipline

Hubu and Gongbu live in this repository's single Rust workspace. Run Cargo
commands from the repository root and use package selectors such as
`-p gongbu-api` for component-focused work. Do not require or document a
separate Gongbu checkout, lockfile, toolchain, CI workflow, or release version.

The workspace MSRV is Rust 1.88. Keep root `Cargo.toml`, `Cargo.lock`, CI, and
runbooks consistent when changing dependencies or toolchain requirements.
Install `protoc` before building Gongbu/Temporal code.

Shared source and packaging do not authorize cross-boundary coupling. Preserve
separate Hubu and Gongbu processes, databases, credentials, provider execution,
artifacts, and failure domains. Do not add direct Cargo dependencies across the
Hubu/Gongbu boundary; communicate through the versioned executor contract.
Keep `hubu-unified-mcp` as the only agent-facing MCP surface. Its unified
routing must not collapse the separate Hubu and Gongbu backend boundaries.

After changing Markdown or the architecture visualizer, run
`python3 scripts/check-doc-links.py` and search the repository for stale
operational references to the retired Gongbu repository.

## Architecture visualization

Hubu includes an interactive sketch-style architecture visualizer at
`architecture/index.html`, with supporting files `architecture/architecture.css`
and `architecture/architecture.js`.

When making changes that alter major components, request flows, storage
boundaries, public interfaces, or code ownership links, update the visualizer in
the same task when practical. Keep the top-level diagram, drill-down component
diagrams, responsibility text, and GitHub code links aligned with the current
codebase. If the architecture change is intentionally not reflected in the
visualizer, call that out in the final caveats.

## Agent registration protocol

When implementing or updating agent registration, keep the human flow low
friction and the agent flow structured:

- Humans should only need to provide a small reviewable set of fields, such as
  agent name and version label, unless they opt into advanced configuration.
- Agents should consume a compact registration guidance object, such as
  `hubu registration guidance` or
  `GET /.well-known/hubu-agent-registration.json`, instead of inferring fields
  from prose.
- The guidance should tell the agent which human inputs to collect, which fields
  the client fills from the active Hubu session/runtime, which payload fields
  are required or optional, how to canonicalize and hash, and which fields to
  show in the human review.
- The client should prepare canonical identity and version payloads, compute
  fingerprints, show the compact human review, and submit the envelope.
- The server should recompute fingerprints from the submitted payloads and
  reject mismatches before creating or reusing registration records.

See `docs/agent-registration.md` for the registration protocol and persistence flow.

## GitHub operations

For GitHub remote operations such as creating or updating pull requests,
reading PR metadata, requesting reviews, or adding PR comments:

- Prefer the GitHub connector / GitHub app tools.
- Do not use `gh` CLI for GitHub API operations when the connector is
  available.
- Local `git` commands are fine for status, diff, commit, branch, fetch, and
  push.

## Pull request review comments

Prefix every agent-authored review reply or PR comment with `🤖 **Codex:**`.
Use `🤖 **Codex recommendation:**` when presenting a scope recommendation or
requesting human judgment. Resolve a review thread only after its change is
verified, committed, pushed, and described in a reply that includes the fix
commit and verification. Leave ambiguous, conflicting, optional, or
out-of-scope requests unresolved with concise technical reasoning.
