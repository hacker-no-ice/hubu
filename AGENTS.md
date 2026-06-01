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
