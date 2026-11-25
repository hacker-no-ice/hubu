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

## PR completion workflow

For requests to implement, fix, execute, or take work to completion, continue
through the full PR lifecycle unless the user says otherwise:

1. Complete the implementation and required validation.
2. Rebase onto the latest `origin/main` if needed, then publish a non-draft PR.
3. Request review with `@codex review`.
4. Address only P0/P1 findings and re-request review after remediation, for at
   most 2 remediation rounds.
5. After the requested review completes and required checks pass, follow the
   merge policy below.
6. If P0/P1 findings remain after 2 rounds, or remediation requires an ambiguous
   product or design decision, stop and wait for human input.

P2+ findings do not block handoff for human review unless they expose a
correctness or safety issue that should reasonably be treated as P0/P1. Requests
only to scope, investigate, review, plan, or discuss do not trigger this
lifecycle.

## Release specs

Some releases have a tech spec in `docs/specs/` (index: `docs/specs/README.md`).
A spec records the technical decisions for its release, so that agents working
on different issues make the same choices.

- Before working on an issue in a release that has a spec, read the spec
  sections the issue links to, plus any section covering the routes, records or
  surfaces you change.
- Where the issue and the spec disagree, follow the spec.
- Do not change behaviour the spec decides without changing the spec. If the
  implementation has to depart from it, update the spec in the same PR and say
  so under "Spec" in the PR description. If the departure needs a product or
  security decision, stop and ask the owner instead.
- A behaviour the spec does not cover is fine to add, but name it under "Spec"
  in the PR description so a reviewer can decide whether the spec needs it.

## Merge policy

Use the Linear issue's explicit `Merge` label when one exists. An explicit
instruction from the user in the current task overrides that decision.

- `Merge → auto`: use `merge:auto`.
- `Merge → manual`: use `merge:manual`.
- Without an explicit merge decision, use `merge:auto` for eligible maintenance
  PRs and `merge:manual` for everything else.
- Include the Linear issue ID in the PR title when there is one. Add the
  matching GitHub merge label to the PR.

A maintenance PR is eligible for the auto-merge default when it is narrowly
scoped to a routine dependency update, a mechanical compatibility fix for
that update, or upkeep of existing docs or development tooling. Keep the PR
manual if it needs a product or security decision, changes supported toolchain
versions or intentionally changes runtime behavior, or touches a path on the
auto-merge blocklist. Follow any stricter eligibility criteria in the
[Dependabot triage runbook](docs/operations/dependabot-triage.md) when running
that workflow.

Before enabling auto-merge for any maintenance PR, verify that Codex review
completed on the current head, no review findings remain outstanding, and all
applicable checks passed against current `main`. A newer head or stale checks
require fresh verification.

Handoff:
- `auto`: once the requested review is done and required checks pass, enable
  GitHub auto-merge (squash) on the PR. Do not merge it directly or bypass
  checks; branch protection decides when it lands. Report that auto-merge is
  enabled.
- `manual`: leave the PR open and report that it is ready for human review and
  merge. Do not merge unless the user explicitly asks.

### Auto-merge blocklist

`.github/auto-merge-blocklist` lists the paths a `merge:auto` PR may not change.
The `Merge guard` CI check enforces it, so agents do not judge this themselves:

- Do not edit the blocklist or the `Merge guard` workflow.
- Do not change the PR's merge label to get past a failing `Merge guard`.
  Report the failure and leave the PR for the owner.

## Unified workspace discipline

Hubu and Gongbu live in this repository's single Rust workspace. Run Cargo
commands from the repository root and use package selectors such as
`-p gongbu-api` for component-focused work. Do not require or document a
separate Gongbu checkout, lockfile, toolchain, CI workflow, or release version.

The workspace MSRV is Rust 1.89. Keep root `Cargo.toml`, `Cargo.lock`, CI, and
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

Prefix every agent-authored review reply or PR comment with
`🤖 **<Agent>:**`, where `<Agent>` names the agent actually posting, such as
`🤖 **Codex:**` or `🤖 **Claude:**`. Use `🤖 **<Agent> recommendation:**` when
presenting a scope recommendation or requesting human judgment. Never post
under another agent's name. A comment that only issues a bot command, such
as `@dependabot rebase`, may omit the prefix when the bot needs the command
alone. Resolve a review thread only after its change is
verified, committed, pushed, and described in a reply that includes the fix
commit and verification. Leave ambiguous, conflicting, optional, or
out-of-scope requests unresolved with concise technical reasoning.

## Review guidelines

These apply to Codex code review and to any agent reviewing a PR.

- When a PR touches work covered by a release spec in `docs/specs/`, compare
  the diff with the relevant spec sections.
- Report as **P1** any departure from the spec that the PR description's
  "Spec" section does not name. A departure that is named there is for the
  owner to judge and is not a finding.
- PRs that only change files under `docs/specs/` are reviewed for internal
  consistency and agreement with the current code.

## Owner identity

Refer to the repository owner by the GitHub handle `hacker-no-ice`, never by
their legal name. This applies to specs, documentation, runbooks, code
comments, and owner fields in configuration, such as the owners of
`deny.toml` exceptions.

Leave the existing copyright line in `LICENSE-MIT` and recorded evidence,
such as committed transcripts, as they are.
