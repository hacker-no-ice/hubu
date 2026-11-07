# EP07: the borrowed identity

**Challenge C3.1** (Supply another agent’s identity) for **C3 · No impersonating another agent**.

**Claim:** A request that names another registered agent’s account is refused, and that agent’s budget is never charged. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep07-impersonate-agent/run.sh --release v0.2.5
```

**Setup.** Two registered agents with their own allow policies and budgets ($0.10 and $1.00). The attacking process holds the same normal credential every agent uses.

[attack.py](attack.py) does this through the released CLI and real HTTP, recording
every response, including unexpected successes:

1. Request $0.05 while naming the other agent’s account_id.
2. If authorized, claim and settle it.
3. Export both budgets and this agent’s public ledger.

The attack process holds only the normal agent credential. Owner steps (human
approval and reconciliation) run in the parent process, which keeps those
capabilities. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) reads only the saved public exports: the impersonated request is refused and the other agent’s budget shows nothing consumed.
A contradicted check makes the challenge **does not hold**; missing or unresolved
evidence makes it **not tested**, never a pass.

```sh
python3 break-hubu/ep07-impersonate-agent/verify.py /absolute/path/to/episode-evidence
```

**Boundary.** Hubu agents share one bearer credential and Hubu does not bind an account to an authenticated runtime or thread; this experiment measures that known gap. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
