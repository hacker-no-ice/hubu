# Release-card checklist

Every stable release gets a measured system card. The
[System card workflow](../.github/workflows/system-card.yml) does the
measurement; a human reviews and publishes it. These experiments are a release
activity, not a PR CI gate. Product regression tests remain in normal bug-fix
PRs.

## Before publishing the release

1. Write the release notes page, `site/app/releases/vX.Y.Z/page.tsx`, with a
   `report.ts` beside it that exports `report = null as ReleaseReport | null`.
   The page's system card uses `MeasuredClaims`, so until the measured report
   lands no challenge counts as holding: claims show **Unverified**, or **Does
   not hold** for challenges the page marks as documented known gaps. Add the release to
   `/releases`. Point the navbar version badge at it after stable publication.
2. Review the four claims, caveats and known limits against the new release.
   If the public contract the kits exercise changed (for example a new
   executor contract), update the kits first; `bootstrap.py` refuses a release
   whose binaries report a contract the kits do not support, and every claim
   then stays **not tested**.

3. For v0.3.0, run the C5 proof kits on the exact intended source SHA before
   dispatching the stable release:

   ```sh
   ./break-hubu/run-all.sh --commit "$source_commit" \
     --kits ep09-raise-own-budget,ep10-self-approval,ep11-rewrite-ledger,ep12-misstated-approval \
     --output /tmp/v0.3.0-candidate-evidence
   ```

   Block stable on a failed or incomplete C5 result. EP09 must pass all four
   checks and EP10 must show that access to the owner bearer and retired
   approval capability does not substitute for owner confirmation. Capture the
   real macOS approval flow separately. Run the full suite to review changes
   against v0.2.5, including the documented gaps scheduled for later releases.
   See [v0.3.0 qualification](../docs/specs/v0.3.0-owner-agent-separation.md#13-testing).

## After the stable release is published

4. The Release workflow's completion starts the System card workflow. It
   resolves the newest stable release (drafts and prereleases never count),
   skips it if a card already exists or a card PR is open, and otherwise:
   - builds the exact published tag on macOS with its pinned toolchain,
     verifying the tag commit against the published release notes;
   - runs every kit from a clean checkout of `main` and uploads the evidence
     as an encrypted workflow artifact;
   - renders `site/app/releases/vX.Y.Z/report.ts`, the
     `/releases/vX.Y.Z/report` analysis page and `cards/vX.Y.Z.md` with
     claim tiers and reproduction instructions;
   - lints and tests the site, then opens a `system-card/vX.Y.Z` PR.

   When the catalog gains kits after a release was measured, the workflow
   runs only the missing kits and renders with `--supplement`, which adds
   their results without changing any published one.

   To measure a release by hand, dispatch the workflow with its tag, or run
   the kits locally, then move the output directories to private storage:

   ```sh
   ./break-hubu/run-all.sh --release vX.Y.Z --output /tmp/vX.Y.Z-card-evidence
   python3 break-hubu/render-report.py /tmp/vX.Y.Z-card-evidence
   # later, for kits the published card lacks:
   ./break-hubu/run-all.sh --release vX.Y.Z --kits ep05-denied-operation --output /tmp/vX.Y.Z-more
   python3 break-hubu/render-report.py --supplement /tmp/vX.Y.Z-more
   ```

5. Review the PR. Save its run’s encrypted artifact to private storage before
   the 30-day Actions expiry. Inspect every result, including incomplete or
   contradicted assertions. Review ledger and authorization pages, budget coverage,
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

6. After the PR merges and the site deploys, verify the live methodology and
   reproduction commands. Link the analysis and proof kits in the release notes.
   Keep captured logs, request traces, state exports, and archives out of the
   website, public repository, and GitHub release attachments.

## Private evidence archive

The workflow uses [the public archive recipient certificate](archive-recipient.pem)
to encrypt the compressed run directory. It saves its console output inside
that encrypted archive. Keep the corresponding private key in owner-only
storage and keep a secure backup. Preserve old keys for archives that still
need them when rotating the recipient certificate.

Download the encrypted artifact from its workflow run. Extract the GitHub
artifact ZIP to obtain `proof-kit-evidence.cms`. Use OpenSSL 3 and the matching
private key to decrypt it in a private directory.

```sh
umask 077
openssl cms -decrypt -binary -inform DER -in proof-kit-evidence.cms \
  -recip /path/to/archive-recipient.pem -inkey /path/to/archive-key.pem \
  -out evidence.tar.gz
tar -xzf evidence.tar.gz
```

Label each saved archive with the release and run ID. Review and sanitize any
copy before sharing it on request, then rerun the verifier on the sanitized
copy to check that it remains usable.

The workflow needs the repository setting that allows GitHub Actions to create
pull requests. The renderer refuses to replace an existing measured report or
card.
