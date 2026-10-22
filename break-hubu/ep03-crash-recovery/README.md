# EP03 — Kill Hubu with unresolved work

This kit tests whether a claimed financial obligation survives a Hubu process
crash and whether an explicitly reconciled bill is recorded once across retries
and another restart. **All claims concern Hubu's own ledger and records, not
provider invoices.** The billed receipt is a synthetic fixture; no provider is
called or paid.

From the repository root, after reading the [shared prerequisites](../README.md):

```sh
./break-hubu/ep03-crash-recovery/run.sh --release v0.2.4
```

The entry point builds unchanged released binaries in a separate checkout and
starts an isolated instance with its own SQLite database, credentials and
loopback port. It never attaches to your selected Hubu profile. The printed
output directory survives failures. A cold build needs network access and may
take substantially longer than the experiment.

## Setup and exact fault point

The released CLI creates one synthetic agent, a fixed fixture policy and a
USD 0.01 budget. The public lease configuration gives the claim 30 seconds.
[attack.py](attack.py) authorizes and claims one cent, then durably writes a
synthetic billed receipt **outside Hubu's database**. A public authorization
lookup must show the same claim and held funds with no receipt posted.

Only after that observable checkpoint does the parent send `SIGKILL` to the
exact Hubu child PID it owns and confirm the process exited. No settlement
request has been sent. This is a fault at a public protocol boundary, not an
attempt to interrupt a private database transaction.

The kit restarts the same binary against the same database and credentials,
exports state, and probes for a new one-cent authorization. It then polls the
actual lease deadline and reconciliation visibility. Both normal settlement
and release are attempted after expiry and must reject the unresolved claim
without releasing the held cent.

The visibly labeled operator step uses the distinct owner CLI capability to
reconcile the saved one-cent fixture bill. It repeats the exact resolution,
restarts Hubu once more and exports the final state.

## What the verifier checks

[verify.py](verify.py) reads the public snapshots and recorded responses. The
original authorization, claim and hold identities must survive the first
restart. There must be no new spending capacity, release or expense before
explicit resolution, including after lease expiry. The final target is one
USD 0.01 settlement and canonical expense, zero frozen funds, USD 0.00 remaining
and zero unaccounted consumption. Identical reconciliation retries and the
final restart must retain the same settlement and ledger identities.

Inspect `transcript.txt`, `process-events.jsonl`, `requests.jsonl`, the durable
`fixtures/synthetic-receipt.json` fixture, `snapshots/before-crash/`,
`snapshots/after-restart/`, `snapshots/expired/`,
`snapshots/after-expired-probes/`, `snapshots/reconciled/` and `snapshots/final/`
in the printed run directory. `verification.txt` and `result.json` preserve the
individual assertions. Credential files, operation keys and the database are
excluded from published evidence.

You can verify an exported evidence directory without starting Hubu:

```sh
python3 ./break-hubu/ep03-crash-recovery/verify.py /path/to/evidence
```

## Interpretation

This tests Hubu process recovery and idempotent accounting under the declared
fixed configuration. It does not prove Gongbu or Temporal recovery, provider
deduplication, recovery from a lost HTTP response, disk corruption, power-loss
durability or automatic reconciliation. The operator resolves a known synthetic
fixture, not an uncertain real-world invoice. A missed checkpoint makes the
experiment incomplete; the kit does not substitute a timed kill.

v0.2.4's normal bearer also permits administration. Separating the driver and
owner capability here does not prove isolation from a malicious bearer holder.
