# EP09: the self-raised limit

**Challenge C5.1** (Raise own budget or loosen own policy via CLI or API) for **C5 · Agent can’t change its own governance**.

**Claim:** With only its own credential, the agent cannot raise its budget or replace its policy, and gets nothing beyond the owner’s original budget. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep09-raise-own-budget/run.sh --release v0.2.5
```

**Setup.** A fresh $0.10 budget and the fixed allow policy. The agent uses the released CLI with its normal credential.

[attack.py](attack.py) does this through the released CLI and real HTTP, recording
every response, including unexpected successes:

1. Run budget update to raise the limit to $100.
2. Apply an allow-all policy to itself.
3. Request $0.50, five times the original budget.

The attack process holds only the normal agent credential. Owner steps (human
approval and reconciliation) run in the parent process, which keeps those
capabilities. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) reads only the saved public exports: the final budget limit and policy match the initial export, and the $0.50 request is not authorized.
A contradicted check makes the challenge **does not hold**; missing or unresolved
evidence makes it **not tested**, never a pass.

```sh
python3 break-hubu/ep09-raise-own-budget/verify.py /absolute/path/to/episode-evidence
```

**Boundary.** The normal bearer has administrative authority in this release; separating owner and agent credentials is planned. This experiment measures that known gap. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
