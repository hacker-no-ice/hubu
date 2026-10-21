# Break Hubu · Episode 01

**One agent. 100 concurrent requests. $1 budget.**

This is a recorded walkthrough of an executable Hubu test. The main view shows
what was configured, the code that ran, its actual output, and the resulting
budget and ledger. Chapter timing is editorial; runtime milestones retain their
captured elapsed times.

## Watch a recording

Unzip a generated recording bundle and open `index.html` in a current browser.
The captured run is embedded in that generated page, so playback needs no
account, provider keys, Rust installation, or local server. The source viewer in
this directory has no prefilled passing results; generate a recording first or
import a schema-v2 `evidence/run.json`.

Use **Play walkthrough**, choose a chapter, or scrub the 75-second timeline.
**Focus view** fits the stage to a desktop screen; Escape restores the full page.
The recording shows:

1. Actual agent/account/budget IDs, the applied policy, and the initial budget.
2. Exact Rust source for the barrier and concurrent authorization requests.
3. The actual Cargo command, captured stdout, and process exit status.
4. One captured allow and deny, correlated to the complete decision list.
5. Synthetic executor settlement and a matching canonical ledger posting.
6. Initial, reserved, final, and reopened SQLite state, with persistence checks.

The receipt includes all decisions and transactions, complete captured source
files, source line references, and SHA-256 hashes. The viewer checks those
hashes and reconciles the recorded facts before enabling playback. Hashes detect
inconsistent bytes within a recording; they do not independently attest its
origin. Inspect the source and rerun the test to verify the claim yourself.

## Reproduce the test

Anyone can reproduce the run from the public repository. The receipt supplies
concrete clone/checkout commands for its full source SHA. For example, replace
`FULL_COMMIT_FROM_RECORDING` with the SHA shown in that recording:

```sh
git clone https://github.com/hacker-no-ice/hubu.git
cd hubu
git checkout FULL_COMMIT_FROM_RECORDING
python3 scripts/break-hubu-episode-01.py --serve
```

Prerequisites for rerunning: Git, Python 3.9 or newer, and Rust through rustup
(the repository pins its toolchain). Cargo downloads locked dependencies on the
first run. `protoc` is needed if you also build the Gongbu/Temporal workspace
components. No Hubu account, active profile, API keys, or paid provider is needed.
The runner ignores active Hubu/Gongbu shell configuration and uses fresh
SQLite storage and built-in test authentication.

The command prints a loopback preview URL, creates a new recording directory
under `target/break-hubu/`, and creates an adjacent `.zip` for sharing. Existing
recording directories and archives are refused. A failed test preserves its log
and does not create a passing replay bundle.

To save a recording without starting a server:

```sh
python3 scripts/break-hubu-episode-01.py --output target/break-hubu/my-run
```

Share `target/break-hubu/my-run.zip`, or serve the generated directory on an
HTTPS static host. It needs no backend. HTTPS, localhost, or a local file allows
the browser to verify the recorded source and log hashes. The bundle contains the viewer, embedded
recording, `scenario.json`, `evidence/run.json`, `evidence/test.log`, and the three
captured source files. For a permanent public replay, retain that whole bundle;
CI artifacts are retained for 30 days and GitHub may require sign-in to download
those artifacts. The public repository and pinned source remain the independent
reproduction path.

Generated IDs and scheduling can differ between runs. The accounting invariant
must remain the same. Clean committed captures link to exact GitHub source;
working-tree captures are labeled and include the captured files, but checking
out their base SHA alone will not reproduce uncommitted changes.

## What the test proves

The [scenario](./scenario.json) configures one registered agent, 100 callers,
a $1 budget, and $0.03 per request. The [regression](../../crates/hubu-api/src/tests/break_hubu.rs)
uses real Hubu application functions for setup and releases 100 threads from one
barrier into the real authenticated `/spend/authorize` route. A permissive policy
leaves the budget as the enforcement gate. It then verifies:

- Exactly 33 allows and 67 insufficient-budget denials.
- Before settlement: $0.99 reserved, $0 consumed, and $0.01 available.
- Denied requests have no authorization token, budget hold, or executor claim.
- All 33 authorizations settle through synthetic executor receipts, with distinct claims and settlement IDs.
- Exact $0.03 expense debits and clearing credits reconcile to $0.99 consumed, with zero unaccounted consumption.
- Final state: $0.99 consumed, $0 reserved, $0.01 available, and no pending holds.
- Reopening SQLite preserves the budget, all transactions, settled claims, and denied-attempt records.

The remaining cent cannot fund another three-cent request. This is an isolated
in-process API contention test, rather than the user's running stack. Production
TCP serving currently serializes connections; no HTTP throughput claim is made.
Executor receipts are synthetic, with no Gongbu/provider network call or actual
invoice. The demonstrated ceiling covers Hubu's own authorizations, reservations,
and internally recorded settlements. Direct provider access outside Hubu and
thread-bound identity enforcement are outside this episode.

## Validation and CI

The regression runs in the ordinary workspace test suite. To run only it:

```sh
cargo test --lib -p hubu-api --locked break_hubu_episode_01
```

With Node.js 18 or newer, test rejection of inconsistent or corrupted evidence
against a generated recording:

```sh
HUBU_BREAK_TEST_EVIDENCE=target/break-hubu/my-run/evidence/run.json node --test examples/break-hubu/episode.test.cjs
```

The [CI workflow](../../.github/workflows/ci.yml) runs the recording command and
viewer evidence tests, and uploads the standalone replay plus underlying logs as
`break-hubu-episode-01`. Failed episode steps retain their test logs when present.
The capture schema is version 2; regenerate old version-1 recordings to include
setup, source, stdout, representative responses, and persistence evidence.

## Recording a narrated cut

No human screen recording is needed to share or replay this version. An optional
voiceover can follow the six chapters: setup → actual script → captured run →
allow/deny evidence → synthetic settlement → reopened outcome. Keep the recorded
run label and synthetic-settlement boundary visible. For a public cut, generate
from a clean published commit and include the source and reproduction command.
