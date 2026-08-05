# Gongbu Agent Conventions

## Pull Request Review Comments

GitHub connector actions may post through the user's authenticated identity.
Prefix every agent-authored review reply or PR comment with
`🤖 **Codex:**` so readers can distinguish it from a human-authored comment.
Use `🤖 **Codex recommendation:**` when the comment presents a judgment or
scope recommendation rather than reporting an implemented change.

When asked to address or fix pull request review feedback:

1. Inspect all current review threads and identify the comments in scope.
2. Evaluate whether each request is correct, necessary, and proportionate.
3. Implement reasonable required changes and add regression coverage when
   appropriate.
4. Run the relevant formatting, lint, and test checks.
5. Commit and push the fixes to the pull request branch.
6. Reply to every implemented review comment with:
   - what changed;
   - the commit containing the fix; and
   - the relevant verification or regression test.
7. Resolve each implemented review thread after its reply is posted.
8. If a request is unreasonable, conflicts with the intended design, or is only
   a nice-to-have outside the pull request's scope:
   - do not implement it automatically;
   - reply with concise technical reasoning and the relevant tradeoff;
   - explicitly ask for human review or a scope decision; and
   - leave the thread unresolved.
9. Leave ambiguous, conflicting, or otherwise unfixed threads unresolved and
   explain what is still needed.

Never resolve a review thread before its requested change is verified and
pushed, or when it is awaiting human judgment.
