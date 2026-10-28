# EP04: many clients, one budget

**Claim:** ten clients sharing a fixed USD 1 budget cannot obtain more than
USD 0.99 of USD 0.03 reservations in this 100-operation burst. Normal settlement
then accounts for those admitted costs once. All claims concern Hubu records,
not provider invoices.

Read the [shared prerequisites and isolation model](../README.md), then run:

```sh
./break-hubu/ep04-many-clients/run.sh --release v0.2.4
```

[attack.py](attack.py) creates ten ordinary OS client processes, each with ten
distinct operations. All ten reach a start barrier before it opens. Each sends
real HTTP requests and promptly claims allowed operations. No settlement or
release occurs until every client finishes and the burst snapshot is saved.
Only then does the synthetic executor settle the admitted claims at USD 0.03.
These are request clients; no multi-agent platform or model subscription is needed.

[verify.py](verify.py) checks saved request responses and public accounting:
100 resolved attempts; 33 allows and 67 explicit budget denials; USD 0.99 frozen
and USD 0.01 available at the burst checkpoint; then 33 unique balanced postings,
USD 0.99 consumed, USD 0.01 remaining, zero pending holds and zero unaccounted
consumption. It also checks public identity links, fixed configuration, client
completion, and that settlement followed the burst. An unresolved transport
error is not a denial. Lease expiry makes the declared workload incomplete.

```sh
python3 break-hubu/ep04-many-clients/verify.py /absolute/path/to/episode-evidence
```

Because reservations only accumulate during the burst, the admitted obligations
are monotonic until its final snapshot. This reasoning depends on all admitted
claims remaining held; sparse snapshots alone do not prove every hidden state.

**Boundary:** v0.2.4 handles TCP connections serially. This tests competing
network demand, not simultaneous SQLite writers, throughput, fairness or
distributed locking. Policy/budget stay fixed and the broad normal bearer is
not a restricted spend-only credential. No Gongbu process or provider is called.
See the [release card](../cards/v0.2.4.md) for measured outcomes and exact evidence.
