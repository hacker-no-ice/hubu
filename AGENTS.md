# Gongbu Agent Conventions

## Pull Request Review Comments

When asked to address or fix pull request review feedback:

1. Inspect all current review threads and identify the comments in scope.
2. Implement each requested change and add regression coverage when appropriate.
3. Run the relevant formatting, lint, and test checks.
4. Commit and push the fixes to the pull request branch.
5. Reply to every addressed review comment with:
   - what changed;
   - the commit containing the fix; and
   - the relevant verification or regression test.
6. Resolve each addressed review thread after its reply is posted.
7. Leave ambiguous, conflicting, or unfixed threads unresolved and explain what
   is still needed.

Never resolve a review thread before its requested change is verified and
pushed.
