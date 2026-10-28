# EP02 — The last cent

This kit asks what happens when an executor reports a three-cent charge after
Hubu authorized only one cent. **All claims concern Hubu's own ledger and
records, not provider invoices.** A synthetic executor supplies the receipt;
no provider is called or paid.

From the repository root, after reading the [shared prerequisites](../README.md):

```sh
./break-hubu/ep02-last-cent/run.sh --release v0.2.4
```

The entry point builds unchanged released binaries in a separate checkout and
starts a fresh isolated instance. It preserves its output directory and prints
that location, including when a step fails. A cold build needs network access
and can take substantially longer than the experiment.

## Setup, attack and evidence

The owner uses the released CLI to register one synthetic agent, apply a fixed
fixture policy and create a USD 0.01 budget. The public lease configuration gives
the claim 30 seconds. The attack then:

1. Authorizes and claims USD 0.01 through the public HTTP contract, and saves an
   immutable synthetic USD 0.03 receipt.
2. Submits that receipt through normal settlement while the claim is active.
   Exports the state, then asks for one further cent.
3. Polls the returned lease deadline and public reconciliation state.
4. Runs the visibly labeled owner CLI reconciliation with the saved receipt and
   a distinct reconciliation capability, then repeats that exact resolution.
5. Makes ten further one-cent attempts and exports the final public state.

[attack.py](attack.py) contains these steps. [verify.py](verify.py) reads saved
public evidence rather than driver success counts or the database. The normal
overrun response must be the specific overrun rejection, not an expired-lease
error. Before reconciliation, one cent must remain held with no partial or
false settlement. The held probe and ten final attempts must explicitly be
budget denials; transport failures do not count as denials.

The final target is one USD 0.03 ledger charge, original authorization USD 0.01,
visible overrun USD 0.02, remaining budget USD **-0.02**, no pending hold and zero
unaccounted consumption. Replaying reconciliation must preserve the original
settlement and ledger identities. The negative remaining amount represents
truthful retrospective accounting of the supplied fixture bill.

Inspect `transcript.txt`, `requests.jsonl`, `fixtures/synthetic-receipt.json`, the public
`snapshots/held-after-overrun/`, `snapshots/expired/`, `snapshots/reconciled/` and
`snapshots/final/` exports, and `verification.txt` in the printed run directory.
`result.json` records assertion-level outcomes. Published evidence excludes
credential files, operation keys and the SQLite database.

You can also verify an exported evidence directory without starting Hubu:

```sh
python3 ./break-hubu/ep02-last-cent/verify.py /path/to/evidence
```

## Interpretation

A fully supported result is **holds with caveat**: posting an overrun requires
explicit owner reconciliation after expiry. The automatic operator step in
this controlled fixture exercises the human-authorized API path; it does not
decide whether an actual provider billed. This does not test automatic invoice
ingestion, Gongbu recovery, provider truthfulness or actual external billing.

The attack keeps policy and budget versions fixed. v0.2.4's normal bearer also
permits administration; separating the driver and owner capability in this kit
does not prove server-enforced isolation from a malicious bearer holder.
