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

See `docs/agent-registration-protocol.md` for the full protocol draft.
