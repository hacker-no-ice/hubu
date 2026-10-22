# Manual release-card checklist

Use this after selecting a stable release's exact final source and before
promoting its claims. These experiments are a manual release activity, not a PR
CI job or required gate. Product regression tests remain in normal bug-fix PRs.

1. Confirm the published tag, full source commit, lockfile, pinned toolchain and
   binary version outputs. When qualifying a new release, review the four claims
   and public contract, update the explicit release pin in
   [bootstrap.py](bootstrap.py), and commit the reviewed kits revision first.
   The current scripts accept only v0.2.4; changing `--release` alone is insufficient.
2. Start from a clean checkout of that kits revision and follow the
   [prerequisites](README.md). Run every kit against the unchanged release:

   ```sh
   ./break-hubu/run-all.sh --release v0.2.4 --output /tmp/new-release-card-evidence
   ```

3. Inspect every result, including incomplete or contradicted assertions. Check
   raw public ledger/authorization pages, budget coverage, request outcomes,
   operator interventions, exact arithmetic, tool/host metadata and cleanup.
   Reuse earlier evidence only if tested source and configuration match exactly;
   otherwise rerun. Never carry a stale verified label forward.
4. Write the versioned repository card and website release page: a concise
   summary of the repository changelog, claims and system card, with a link to
   the full analysis. Add the release to `/releases`, linked from the landing
   page, and update the version link below the landing-page wordmark. Start
   with v0.2.4; earlier releases are not backfilled. Record release
   source and kits commit separately. Keep failures, known identity and
   bearer limitations, shortened leases, mock execution and untested surfaces
   visible. Link genuine product failures to engineering issues; preserve the
   original release's result when a later release fixes them.
5. Run the repository documentation/site checks and review the PR. A human
   decides whether to release with a disclosed failure or fix it first. A broken
   experiment stays **not tested**. Do not alter acceptance criteria to manufacture
   a pass.
6. After approved source/site changes merge, verify the live report, attach the
   plain evidence archive to the corresponding GitHub release, and add its card
   and reproduction links to the release notes. Preserve existing release assets
   and notes. Check downloads from an unauthenticated browser. Promotion follows
   publication; videos are optional. Posting or messaging needs separate authorization.

The v0.2.4 card is a follow-up to an existing stable release. Publication remains
pending until its source/site PR has merged and the live links are checked.
