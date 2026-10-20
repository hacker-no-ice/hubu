# Break Hubu · Episode 01

**One agent. 100 concurrent requests. $1 budget.**

This 45-second visual replay is backed by an executable concurrency regression,
not prerecorded outcomes. The test releases 100 threads from one barrier into
Hubu's actual authorization route. Every request belongs to the same registered
agent account and asks for $0.03. A permissive policy leaves the budget as the
enforcement gate.

## Run and preview

From the repository root, with the repository's pinned Rust toolchain, Python 3,
and `protoc` installed:

```sh
python3 scripts/break-hubu-episode-01.py --serve
```

Open the loopback URL printed by the command. Press **Play replay** to watch,
or scrub the timeline. **Focus view** fits the stage to a desktop screen for
recording; Escape returns to the full page. The evidence drawer exposes all decisions and the
canonical ledger. Animation time is editorial pacing, not request latency.

Each run creates a new directory under `target/break-hubu/`, containing the
viewer, `scenario.json`, `evidence/run.json`, and `evidence/test.log`. Share the
whole directory to preserve the evidence. Without a local server, open
`index.html` and select `evidence/run.json` with the file picker. The source
viewer starts without evidence and does not manufacture a passing result.

For an explicit new output directory:

```sh
python3 scripts/break-hubu-episode-01.py --output target/break-hubu/my-run
```

Existing directories are refused so failed runs cannot inherit old passing
evidence. A failed test preserves its log and does not package a passing replay.
Development runs are marked as a dirty working tree; commit the implementation
and rerun to link results to exact source. The runner records the source SHA,
toolchain version, lockfile/scenario hashes, and UTC run time. Thread scheduling
and generated IDs may differ between runs; the asserted totals must not.

The scenario lives in [scenario.json](./scenario.json); the regression reads it
at compile time and validates this episode's fixed parameters. The exporter
allowlists trace fields and omits bearer and authorization tokens. It uses a
fresh temporary SQLite database and synthetic executor receipts, without an
active profile, provider credentials, a network listener, or provider calls.

## Exact claim and evidence

The [regression](../../crates/hubu-api/src/tests/break_hubu.rs) checks:

- 100 requests complete: 33 allowed and 67 denied for insufficient budget.
- Before settlement, all 33 holds exist: $0.99 reserved, $0 consumed, $0.01 available.
- Denied requests create no authorization token, budget hold, or executor claim.
- Synthetic executor claims settle exactly $0.99; no funds are released.
- 33 canonical transactions have balanced entries and account for every consumed unit.
- Final state is $0.99 consumed, $0 reserved, $0.01 available, with zero unaccounted consumption.
- The budget and ledger remain identical after SQLite is reopened.

The remaining cent cannot fund another three-cent request. The tested ceiling
is Hubu's authorization/reservation and internally recorded settlement amount.
This is an in-process API route contention test. Production TCP serving currently
serializes connections; this episode makes no HTTP throughput claim. Executor
receipts are synthetic, so this is not evidence of an external provider bill,
provider execution, or thread-bound identity enforcement. Direct provider access
outside Hubu remains outside this claim.

The normal workspace test suite runs the regression even without evidence export:

```sh
cargo test --lib -p hubu-api --locked break_hubu_episode_01
```

With Node.js 18 or newer, validate the viewer's rejection of corrupted evidence
against a generated run (replace the path with your run directory):

```sh
HUBU_BREAK_TEST_EVIDENCE=target/break-hubu/my-run/evidence/run.json node --test examples/break-hubu/episode.test.cjs
```

The [CI workflow](../../.github/workflows/ci.yml) also packages a fresh replay and
logs as the `break-hubu-episode-01` artifact on each CI run. It preserves failed
test logs when the episode step fails. The artifact is run evidence, not an
independent attestation; inspect the source and reproduce it.

## Recording plan

No recording is needed to preview this version. For a public cut, capture the
generated viewer at 1920×1080 after a clean committed run. Enter **Focus view**,
keep the mock label visible, press Play, and record the 45-second playback. Suggested narration:

1. **0–8 seconds:** “One agent. One dollar. A hundred requests at once.”
2. **8–22 seconds:** “Each request asks for three cents. Can they race past the budget?”
3. **22–34 seconds:** “Thirty-three get through. Sixty-seven are denied. Ninety-nine cents reserved.”
4. **34–45 seconds:** “The ledger reconciles to ninety-nine cents. One cent remains. Run the same test—or try to break it.”

Voiceover is optional; the preview carries its own captions. A final recording
should include the evidence view and exact source commit. No live paid-provider
recording is needed for this episode.
