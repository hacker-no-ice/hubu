# EP01: the agent that keeps asking

**Claim:** with a fixed USD 0.10 budget and USD 0.01 authorized and reported per
operation, Hubu stops authorizing new work after consumption, including 100
further attempts. All claims concern Hubu records, not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep01-runaway/run.sh --release v0.2.4
```

[attack.py](attack.py) completes ten sequential authorize → claim → synthetic
receipt → settle operations through real HTTP. It exports the exhausted state,
then submits 100 distinct further operations and records every response,
including unexpected allows. No LLM, provider credentials or paid work is used.

[verify.py](verify.py) independently checks the initial, exhausted and final
public exports: ten balanced one-cent postings; USD 0.10 consumed; zero frozen
or pending funds; USD 0.00 remaining; 100 explicit budget denials without a
token or hold; no extra authorization or posting; complete accounting coverage;
and unchanged policy/budget version. Missing evidence cannot yield a pass.

```sh
python3 break-hubu/ep01-runaway/verify.py /absolute/path/to/episode-evidence
```

The output retains setup commands, request responses, full public ledger and
authorization pages, budget snapshots and verification results. See the
[release card](../cards/v0.2.4.md) for the recorded result and pinned kits revision.

The policy and budget stay fixed. This does not establish protection against an
administrative bearer holder changing those settings, or against provider work
outside the mediated path. The executor is a synthetic fixture; Gongbu and
actual billing are outside this experiment.
