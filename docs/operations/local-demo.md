# Local demo

The local demo exercises human and agent registration, policy assignment,
spending targets, agent budgets, spend evaluation, mock payment, budget holds,
and ledger recording without real provider credentials or money movement.

## Run the scripted flow

From the workspace root:

```sh
./scripts/demo.sh
```

The script builds the required binaries, starts an isolated `hubu-server`, runs
the workflow, and prints the records needed to inspect each transition. Useful
speed controls are:

```sh
HUBU_DEMO_STEP_DELAY=0.1 \
HUBU_DEMO_READ_DELAY=0.1 \
./scripts/demo.sh
```

Use `HUBU_DEMO_ADDR` and `HUBU_DB_PATH` when an isolated address or persistent
demo database is required. Do not reuse a production database or credential
file for the demo.

## Workflow

The script demonstrates:

1. Register or select a human owner.
2. Register an agent and account.
3. Apply a declarative policy.
4. Set an advisory user spending target.
5. Create the agent's hard budget.
6. Submit an allowed spend through the mock payment rail.
7. Authorize an external spend without executing payment.
8. Confirm failed mock payment releases its budget hold.
9. Confirm over-limit and policy-denied requests do not move money.
10. Inspect budgets, spending targets, decisions, and the ledger.

The expected lifecycle is documented in [Spend lifecycle](../spend-lifecycle.md).
Use `hubu --help` and subcommand help for the current CLI syntax instead of
copying command output from this guide.

## Manual development setup

To inspect individual steps, build the workspace and start the server:

```sh
cargo build
cargo run --bin hubu-server
```

The server listens on `http://127.0.0.1:8787` by default. It reads
`HUBU_AUTH_TOKEN`, or creates and reuses `hubu.auth-token` in its working
directory. The CLI reads the same environment variable or token file. Set
`HUBU_AUTH_TOKEN_FILE` when the processes use different working directories.

The local HTTP server and mock rail are development surfaces. They do not
provide a production authentication, concurrency, payment, or threat model.
