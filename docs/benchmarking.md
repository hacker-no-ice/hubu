# Local Spend Benchmark

`hubu-bench` simulates multiple agents owned by one user submitting spend
approval requests to a local Hubu server at a fixed request rate.

The benchmark covers the local MVP HTTP path:

1. Create or select the local user.
2. Register `N` agents.
3. Attach a policy to each agent.
4. Create one budget per registered agent.
5. Submit paced `POST /spend` requests at `Y` requests/sec.
6. Report throughput, latency, response correctness, budget consistency, and
   ledger consistency.

## Quick Run

```sh
./scripts/benchmark-local.sh
```

The script builds `hubu-server` and `hubu-bench`, starts an isolated local
server on `127.0.0.1:8790`, samples server CPU/RSS once per second with `ps`,
and writes artifacts under `target/hubu-bench/`.
The server creates/reads the local Hubu bearer token, and `hubu-bench` reads
`HUBU_AUTH_TOKEN` or `HUBU_AUTH_TOKEN_FILE`/`hubu.auth-token` before calling
protected routes.

Useful environment overrides:

```sh
HUBU_BENCH_AGENTS=8 \
HUBU_BENCH_RPS=16 \
HUBU_BENCH_DURATION_SECONDS=20 \
HUBU_BENCH_WORKERS=8 \
./scripts/benchmark-local.sh
```

For direct use:

```sh
cargo run --bin hubu-bench -- \
  --url http://127.0.0.1:8787 \
  --agents 4 \
  --rps 8 \
  --duration-seconds 10 \
  --workers 4
```

## Correctness Checks

The benchmark fails when any selected guardrail is violated. It verifies:

- planned requests were recorded
- request error rate is under the configured limit
- allowed decisions receive auth tokens
- settled budget holds match successful payments
- released budget holds match failed payments
- ledger transaction count matches successful payments
- consumed budget equals successful payment spend
- no budget remains frozen after the run

## MVP Report

Latest collected run:

- Date: 2026-06-03 local run
- Scenario: one local server, one owner user, 4 agents, 8 target spend approval
  requests/sec, 10 seconds, 4 client workers
- Request path: HTTP `POST /spend` through policy evaluation, auth token
  issuance, budget reservation, mock payment, budget settlement, and ledger
  recording

Performance metrics:

- Requests completed: 80
- Achieved throughput: 8.09 requests/sec
- p50 latency: 1.31 ms
- p95 latency: 1.74 ms
- p99 latency: 1.86 ms
- max latency: 2.40 ms
- Server average CPU: 0.83%
- Server peak CPU: 2.40%
- Server peak RSS: 11,072 KB

Correctness metrics:

- Correctness checks passed: true
- Request errors: 0
- Allowed decisions: 80
- Auth tokens issued: 80
- Payments succeeded: 80
- Holds settled: 80
- Ledger transactions: 80
- Budget consumed: 8,000 cents
- Budget frozen after run: 0 cents
- Per-agent successful requests: 20 each across 4 agents

## Scalability Assessment

The MVP server is intentionally simple and handles accepted TCP connections in a
single blocking loop. This benchmark measures the complete demo path, but it
also highlights that concurrency is serialized before route handling. Higher
request rates will queue at the client/server socket layer rather than
exercising parallel policy, budget, payment, or ledger work.

The default load is intentionally conservative to avoid stressing a laptop.
Step-load runs should increase `HUBU_BENCH_RPS` gradually while watching
`target/hubu-bench/system-stats.tsv`.

The reported numbers are a single local demo observation, not a capacity claim,
service-level objective, production load test, or security test. The benchmark
uses the localhost bearer capability and mock payment rail, and the serialized
server prevents it from exercising concurrent policy, budget, payment, or
ledger execution. Results should not be extrapolated to a deployed or
real-money system.

## Reliability Assessment

The selected run stayed internally consistent under light load: every spend was
allowed, every auth token led to one successful mock payment, every payment
settled one agent-budget hold, every successful payment wrote one ledger
transaction, and all agent budgets ended with no frozen balance.

This does not yet prove retry safety or saturation behavior. The current run
does not include duplicate idempotency keys, mixed policy decisions, concurrent
budget exhaustion, or server restarts during load.

## Top Things To Address

1. Replace the blocking one-connection-at-a-time local server with a concurrent
   HTTP runtime.
2. Move shared state from coarse `Mutex` guards to storage and synchronization
   boundaries that match the domain invariants, especially spend tokens, budget
   holds, and ledger writes.
3. Extend the load benchmark with duplicate operation keys and restart
   injection to exercise the existing idempotency tests under concurrency.
4. Add first-class telemetry: structured request logs, latency histograms,
   error counters, and budget/ledger consistency gauges.
5. Extend this benchmark with mixed allow, deny, needs-approval, and
   payment-failure scenarios plus a step-load mode to find saturation points
   safely.
