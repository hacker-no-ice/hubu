# EP05: the denied operation

**Challenge C2.1** (Request an explicitly denied operation) for **C2 · Policy and approval can’t be bypassed**.

**Claim:** A request above the explicit $1.00 deny rule gets no token, hold, claim or posting—on replay, as a new operation, with a forged token, or by claiming an allowed token for the denied amount. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep05-denied-operation/run.sh --release v0.2.5
```

**Setup.** A fresh $1.00 budget and a tiered policy: allow up to $0.05, approval above $0.05, deny above $1.00. One allowed one-cent control operation.

[attack.py](attack.py) does this through the released CLI and real HTTP, recording
every response, including unexpected successes:

1. Request $2.00, replay it exactly, and request it again as a new operation.
2. Resolve and claim a forged authorization token.
3. Authorize one cent, try to claim that token for $2.00, then settle the cent normally.

The attack process holds only the normal agent credential. Owner steps (human
approval and reconciliation) run in the parent process, which keeps those
capabilities. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) reads only the saved public exports: every denial carries no token or hold, every forged or altered claim is refused, and the final ledger holds only the one-cent control.
A contradicted check makes the challenge **does not hold**; missing or unresolved
evidence makes it **not tested**, never a pass.

```sh
python3 break-hubu/ep05-denied-operation/verify.py /absolute/path/to/episode-evidence
```

**Boundary.** This covers Hubu’s decision and executor contract. Whether Gongbu also refuses to run denied work is part of the planned Gongbu harness. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
