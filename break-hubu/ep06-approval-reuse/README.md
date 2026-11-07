# EP06: the reused approval

**Challenge C2.2** (Skip, replay or reuse a human approval for a different operation or amount) for **C2 · Policy and approval can’t be bypassed**.

**Claim:** A human approval of one $0.10 operation authorizes exactly that operation at exactly that amount—not a new operation, a larger amount, a changed replay, or a claim for more, and a denial can’t be turned into a yes. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep06-approval-reuse/run.sh --release v0.2.5
```

**Setup.** A fresh $1.00 budget and the tiered policy. The approval capability stays with the owner process; the agent holds only the normal credential.

[attack.py](attack.py) does this through the released CLI and real HTTP, recording
every response, including unexpected successes:

1. Request $0.10, try to use the pending request as a token, and have the owner approve it.
2. Request a new $0.10 operation and a $0.20 operation, replay the approved one at $0.20, and claim its token for $0.20.
3. Settle the approved operation, replay the approval, deny a second request, and try to approve and replay it.

The attack process holds only the normal agent credential. Owner steps (human
approval and reconciliation) run in the parent process, which keeps those
capabilities. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) reads only the saved public exports: new and larger requests wait for their own approval, every altered replay or claim is refused, the approval replay returns the same authorization, and only $0.10 is recorded.
A contradicted check makes the challenge **does not hold**; missing or unresolved
evidence makes it **not tested**, never a pass.

```sh
python3 break-hubu/ep06-approval-reuse/verify.py /absolute/path/to/episode-evidence
```

**Boundary.** The owner approves through the approval capability over HTTP; the human-facing notification and review screens are outside this test. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
