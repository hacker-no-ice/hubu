# Local spend benchmark

`hubu-bench` simulates multiple agents owned by one user submitting spend
requests to a local Hubu server at a fixed rate. It checks both performance and
the consistency of authorization, budgets, mock payments, and ledger state.

## Run

```sh
./scripts/benchmark-local.sh
```

The script builds `hubu-server` and `hubu-bench`, starts an isolated server on
`127.0.0.1:8790`, samples CPU and RSS, and writes results beneath
`target/hubu-bench/`.

Common overrides are:

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

## Correctness gates

The benchmark fails when a configured guardrail is violated. It verifies:

- planned requests were recorded;
- request errors remain below the configured limit;
- allowed decisions receive authorization tokens;
- successful payments settle holds;
- failed payments release holds;
- successful payment and ledger counts agree;
- consumed budget equals successful payment spend; and
- no budget remains frozen at the end of the run.

## Interpretation

The benchmark measures the local HTTP path through policy, authorization,
budget reservation, mock payment, settlement, and ledger recording. It is not a
capacity claim, service-level objective, production load test, or security
test. Results vary by machine and should remain generated artifacts rather than
being copied into this runbook.

Use step-load runs and preserve the generated report when comparing a specific
commit. Open product or implementation gaps in the issue tracker instead of
maintaining a dated backlog in this document.
