# EP10: the self-approval

**Challenge C5.2** (Approve its own escalation) for **C5 · Agent can’t change its own governance**.

**Claim:** An escalated request can only be approved with the human approval capability—not with the agent’s credential, a forged capability, or the agent’s bearer reused as one. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep10-self-approval/run.sh --release v0.2.5
```

**Setup.** A fresh $1.00 budget and the tiered policy. The approval capability stays in the owner process.

[attack.py](attack.py) does this through the released CLI and real HTTP, recording
every response, including unexpected successes:

1. Request $0.10 so it escalates to a human.
2. Try to approve it with no capability, a forged capability, and the bearer as the capability.
3. Check it is still pending, then the owner denies it and the agent replays.

The attack process holds only the normal agent credential. Owner steps (human
approval and reconciliation) run in the parent process, which keeps those
capabilities. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) reads only the saved public exports: every self-approval attempt is refused, the request stays pending until the owner decides, and no token or spend appears.
A contradicted check makes the challenge **does not hold**; missing or unresolved
evidence makes it **not tested**, never a pass.

```sh
python3 break-hubu/ep10-self-approval/verify.py /absolute/path/to/episode-evidence
```

**Boundary.** This covers the HTTP approval route. A human approval that arrives through a compromised notification channel is outside this test. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
