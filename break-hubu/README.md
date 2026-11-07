# Break Hubu: public proof kits

**All claims are about Hubu's own ledger and records, not provider invoices.**
These bounded experiments run an unchanged released Hubu server, set it up
through the public CLI, submit real HTTP requests, and verify saved public
exports. The executor receipts are explicitly synthetic. No model subscription,
provider key, provider invocation or paid workload is needed.

Each kit is one reproducible challenge against one of Hubu's six claims. The
catalog in [`site/app/releases/claims.json`](../site/app/releases/claims.json)
lists every claim and challenge, including challenges that are planned,
blocked or out of scope.

| Kit | Challenge | Question |
| --- | --- | --- |
| [EP01: Runaway](ep01-runaway/README.md) | C1.1 | Does Hubu stop authorizing when a fixed budget is consumed, despite 100 further attempts? |
| [EP02: Last cent](ep02-last-cent/README.md) | C1.2 | How does Hubu account for a reported bill above the authorized maximum? |
| [EP03: Crash recovery](ep03-crash-recovery/README.md) | C1.3 | Does a claimed obligation survive a server crash and explicit reconciliation retries? |
| [EP04: Many clients](ep04-many-clients/README.md) | C1.4 | What happens when ten client processes request three dollars from one dollar? |
| [EP05: Denied operation](ep05-denied-operation/README.md) | C2.1 | Is there any way around a policy denial? |
| [EP06: Approval reuse](ep06-approval-reuse/README.md) | C2.2 | Does one human approval cover more than one operation or amount? |
| [EP07: Impersonation](ep07-impersonate-agent/README.md) | C3.1 | Can one agent spend from another agent's account? |
| [EP08: Second session](ep08-cross-session/README.md) | C3.2 | Can a different session draw on this session's budget? |
| [EP09: Raise own budget](ep09-raise-own-budget/README.md) | C5.1 | Can the agent raise its own budget or loosen its own policy? |
| [EP10: Self-approval](ep10-self-approval/README.md) | C5.2 | Can the agent approve its own escalation? |
| [EP11: Ledger rewrite](ep11-rewrite-ledger/README.md) | C5.3 | Can the agent delete, reverse or re-price recorded spend? |
| [EP12: Misstated approval](ep12-misstated-approval/README.md) | C5.4 | Can the agent make the human approve something other than what runs? |

A claim is **Verified** when every in-scope challenge holds, **Partially
verified** when some do, **Unverified** when none have been shown, and **Does
not hold** when any challenge fails. Out-of-scope challenges are not counted;
planned, blocked and untested ones count as not holding.

Each stable release gets a card in [cards/](cards/), starting with
[v0.2.4](cards/v0.2.4.md), that ties measured outcomes to their exact scripts,
environment and evidence. Targets in the episode READMEs are assertions to test,
not guarantees. A failing or incomplete experiment stays visible. The public
website presents each card under release notes: `/releases/vX.Y.Z` summarizes
the changelog, claims and system card, then links to the full analysis at
`/releases/vX.Y.Z/report`. The [System card
workflow](../.github/workflows/system-card.yml) measures every new stable
release and opens a PR for review; see the [release-card
checklist](release-checklist.md).

## Prerequisites and release identity

The initial qualified environment is macOS. Linux has not been qualified for
this card; Windows is unsupported by these process/signal helpers. Have these
installed before starting:

- Git, Python 3 and a POSIX shell.
- Xcode Command Line Tools, including a C compiler and linker.
- Rustup with the exact toolchain pinned by the release's `rust-toolchain.toml`
  (**Rust 1.94.1** for v0.2.4 and v0.2.5).
- Network access for the initial GitHub source/metadata and Cargo dependency
  downloads. The experiment itself talks only to loopback.

The scripts do not install prerequisites. Check the tools with:

```sh
git --version
python3 --version
xcode-select -p
rustup run 1.94.1 cargo --version
```

Only `hubu` and `hubu-server` are built. This component-focused build does not
start or build Gongbu/Temporal. A broader workspace build may additionally
require `protoc`; it is not part of this kit's two-binary command.

[bootstrap.py](bootstrap.py) reads the published GitHub release for the
requested tag, which must be a stable (non-draft, non-prerelease) release whose
notes name one full source commit. It clones the tag into a separate checkout
for that release and requires the tag to resolve to exactly that commit. For
example:

```text
Hubu:   v0.2.4
Source: 5344ab565171e1aa3d3ec628641cdbfce18372af
Contract: hubu-spend-executor-v4.5
```

The build uses the release's `Cargo.lock` with `--locked`, its pinned toolchain,
`--release`, and explicit version/source stamps. Both binaries must report the
expected version and source commit, and an executor contract the kits support
(currently `hubu-spend-executor-v4.5`). Absolute binary paths prevent a
normal installed Hubu from being selected accidentally. The scripts do not
switch your checkout, overwrite installed binaries, attach to your active
profile, or change Codex configuration.

A cold build may take much longer than the experiment. The cache retains source,
Cargo artifacts, published metadata and `build.log`; later invocations verify
and reuse it. Concurrent invocations serialize cache preparation with a file
lock while retaining separate experiment instances.

## Run and reproduce

From a checkout of the **kits commit linked by the release card**:

```sh
./break-hubu/ep01-runaway/run.sh --release v0.2.5
./break-hubu/run-all.sh --release v0.2.5
```

`--release` is required and names a published stable tag. The kits commit
differs from the older release commit being tested. `run.json`
records both identities and whether the kits checkout had uncommitted changes.
Use a clean kits checkout when publishing results.

To choose retained locations explicitly:

```sh
./break-hubu/run-all.sh --release v0.2.5 \
  --cache /tmp/my-break-hubu-build \
  --output /tmp/my-break-hubu-evidence
```

`--output` must be absent or an empty directory; a previous run is never replaced.
By default, fresh evidence and private runtime directories are created under
the OS temporary directory. The build cache is `break-hubu-build-<uid>` under
that directory, with one subdirectory per release. On macOS this is commonly under `/var/folders`, not `/tmp`.
Every invocation prints the evidence and private-runtime paths.

The entry point validates an explicit server configuration, selects an unused
loopback port, creates fresh credential files and a fresh SQLite database, and
runs the released CLI to register a synthetic owner/agent, apply a fixture-only
policy and create one budget. The fixed configuration is exported before and
after the attack. Ordinary claims use a 900-second lease; EP02/EP03 explicitly
configure and verify a 30-second lease through public executor guidance.

The driver persists a fresh operation identity before each logical request and
reuses the exact request body on ambiguous transport retries. A timeout or server
error is never classified as a budget denial. EP04 uses ten OS processes, each
sending ten requests; these are HTTP clients, not ten AI agents.

## Read the evidence

Each output directory contains:

| File | Meaning |
| --- | --- |
| `run.json` | Tested release/source, separate kits commit, binary versions, build command, tools and workload. |
| `transcript.txt` | Actual setup commands, CLI output, labeled requests, responses and operator steps. |
| `requests.jsonl` | HTTP status/body records, explicit transport errors and CLI reconciliation outcomes. Continuation tokens and private operation keys are redacted. |
| `snapshots/<phase>/` | Raw paginated public ledger and authorization JSON, structured budgets, and CLI budget/policy output. |
| `fixtures/` | Clearly labeled synthetic receipts and crash markers; these are not provider invoices. |
| `process-events.jsonl` | Owned server starts, confirmed crash exit and durable fixture checkpoints. |
| `verification.txt`, `result.json` | Saved-evidence assertions, verdict, execution status and any failure cause. |
| `summary.json` | Run-all summary including the known identity and broad-bearer limitations. |

Read `result.json` first, then inspect the phase snapshots and trace. Each episode's
`verify.py` can be run against its saved output directory without a running
server. Verification uses exact integer amounts/scales, full pagination,
transaction identities and balanced ledger entries. It does not read SQLite or
trust driver success counters. Full-budget coverage repeats on each ledger page
and is counted once, not summed across pages.

Claim verdicts are **holds**, **holds with caveat**, **does not hold**, or
**not tested**. Execution status is separate. Run-all continues through every
kit and exits with 1 if any claim is contradicted; otherwise 2 for incomplete
experiments; otherwise 0 for supported runnable claims with caveats printed.
The documented identity gap always remains visible even when all four kits
support their narrower claims. Failure evidence is retained, and verification
still runs after an attack error so an earlier contradiction cannot become a
passing result.

## Boundaries and cleanup

Policy, budget version, account and credentials remain fixed during each attack.
v0.2.4's normal bearer can also administer policy and budgets: this is not proof
that a malicious bearer holder cannot raise a budget. Runtime/thread-bound
account authority is a documented gap that will be addressed in future releases;
more narrowly scoped executor authority is also planned.

The public catalog labels the fixture scope `executor:gongbu:image`. That is
stored execution-scope metadata here: the Python fixture performs the executor
HTTP calls and **no Gongbu process runs**. These kits do not test Gongbu,
Temporal, MCP transport, provider adapters, actual billing, task quality,
provider-side deduplication or arbitrary hostile executors. The released Hubu
server handles TCP connections serially; EP04 is competing network demand, not
a benchmark of simultaneous database writers or distributed locking.

Owner reconciliation uses a separate capability retained in the parent process,
never supplied in the attack child's environment or context. Its CLI step is
visible and automatic only because the synthetic receipt is known fixture
evidence. This exercises the human-authorized resolution path, not autonomous
judgment about an uncertain real charge. The process split does not provide
security isolation against another process running as the same OS user.

The runner stops its owned server at completion or failure and leaves evidence
and private runtime files for inspection. The private directory contains live
fixture credentials, unredacted operation/continuation records and the database;
**publish only the evidence directory**, never the private directory. After
review, delete the exact printed private runtime directory and any unneeded
evidence/cache directories. None is the user's normal Hubu profile.

The implementation is intentionally inspectable: scenario steps live in each
`attack.py`, assertions in each `verify.py`, and shared setup/transport/export in
[runtime.py](runtime.py), [bootstrap.py](bootstrap.py), [run-all.sh](run-all.sh)
and [verification.py](verification.py). Those four shared files total about
650 lines, in addition to scenario code and episode assertions; the shared work
is not hidden from episode-size accounting. The separate
[report renderer](render-report.py) only turns captured public evidence into
website data and a plain ZIP; it is not part of the attack runtime. Run verifiers with ordinary Python,
without `-O`, `-OO` or `PYTHONOPTIMIZE`; optimized mode cannot produce a passing verdict.
There is no automatic PR proof-kit job or custom replay viewer.
