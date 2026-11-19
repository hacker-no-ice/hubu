# EP08: the second session

**Challenge C3.2** (Spend from another session’s budget) for **C3 · No impersonating another agent**.

**Claim:** A second, separately started client session cannot draw on a budget opened for the first session unless Hubu attributes it to its own runtime-bound session. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep08-cross-session/run.sh --release v0.2.5
```

**Setup.** One agent with a $0.10 budget. Session 1 reserves $0.05; session 2 is a separate OS process with the same credential.

[attack.py](attack.py) does this through the released CLI and real HTTP, recording
every response, including unexpected successes:

1. Session 1 authorizes and claims $0.05.
2. Session 2 asks for the remaining $0.05.
3. Inspect both authorizations for any session attribution.

The attack process holds only the normal agent credential. Owner steps (human
approval and reconciliation) run in the parent process, which keeps those
capabilities. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) reads only the saved public exports: session 2 is refused, or its authorization carries a distinct runtime-bound session identity.
A contradicted check makes the challenge **does not hold**; missing or unresolved
evidence makes it **not tested**, never a pass.

```sh
python3 break-hubu/ep08-cross-session/verify.py /absolute/path/to/episode-evidence
```

**Boundary.** Hubu has no session identity of its own in this release; strong session binding is planned for a future release. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
