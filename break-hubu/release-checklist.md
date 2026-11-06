# Release-card checklist

Every stable release gets a measured system card. The
[System card workflow](../.github/workflows/system-card.yml) does the
measurement; a human reviews and publishes it. These experiments are a release
activity, not a PR CI gate. Product regression tests remain in normal bug-fix
PRs.

## Before publishing the release

1. Write the release notes page, `site/app/releases/vX.Y.Z/page.tsx`, with a
   `report.ts` beside it that exports `report = null as ReleaseReport | null`.
   The page's system card uses `MeasuredClaims`, so it shows every claim as
   **Not tested** until the measured report lands. Add the release to
   `/releases` and point the navbar version badge at it.
2. Review the four claims, caveats and known limits against the new release.
   If the public contract the kits exercise changed (for example a new
   executor contract), update the kits first; `bootstrap.py` refuses a release
   whose binaries report a contract the kits do not support, and every claim
   then stays **not tested**.

## After the stable release is published

3. The Release workflow's completion starts the System card workflow. It
   resolves the newest stable release (drafts and prereleases never count),
   skips it if a card already exists or a card PR is open, and otherwise:
   - builds the exact published tag on macOS with its pinned toolchain,
     verifying the tag commit against the published release notes;
   - runs every kit from a clean checkout of `main` and keeps the raw public
     evidence as a workflow artifact;
   - renders `site/app/releases/vX.Y.Z/report.ts`, the
     `/releases/vX.Y.Z/report` analysis page, public evidence under
     `site/public/break-hubu/evidence/vX.Y.Z/`, and `cards/vX.Y.Z.md`;
   - lints and tests the site, then opens a `system-card/vX.Y.Z` PR.

   To measure a release by hand, dispatch the workflow with its tag, or run
   the kits locally:

   ```sh
   ./break-hubu/run-all.sh --release vX.Y.Z --output /tmp/vX.Y.Z-card-evidence
   python3 break-hubu/render-report.py /tmp/vX.Y.Z-card-evidence
   ```

4. Review the PR. Inspect every result, including incomplete or contradicted
   assertions: raw public ledger and authorization pages, budget coverage,
   request outcomes, operator interventions, exact arithmetic, tool and host
   metadata. Check that the caveats and known limits (identity, bearer
   authority, serial request handling, shortened leases, mock execution,
   untested surfaces) still describe this release, and edit the copy if not.
   Link genuine product failures to engineering issues. A human decides
   whether to publish a disclosed failure or fix it first. A broken experiment
   stays **not tested**; never alter acceptance criteria to manufacture a pass,
   and never carry an earlier release's verdict forward.

   PRs opened with `GITHUB_TOKEN` do not start other workflows. The System
   card run already tested the site; push a commit or close and reopen the PR
   to run the required checks.

5. After the PR merges and the site deploys, verify the live report, attach
   the evidence archive to the GitHub release, and add the card and
   reproduction links to the release notes. Preserve existing release assets
   and notes. Check downloads from an unauthenticated browser. Posting or
   messaging needs separate authorization.

The workflow needs the repository setting that allows GitHub Actions to create
pull requests. Earlier cards are never regenerated: the renderer refuses to
replace an existing measured report, card or evidence directory.
