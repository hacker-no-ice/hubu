# Hubu Local Demo

This demo runs Hubu locally and exercises the existing registration, policy,
spend evaluation, mock payment orchestration, and ledger recording flow through
the `hubu` CLI.

## Setup

Build the workspace:

```sh
cargo build
```

Install the local CLI so `hubu ...` works from your shell:

```sh
cargo install --path crates/hubu-cli
```

This installs the `hubu` binary into Cargo's local bin directory, usually
`~/.cargo/bin`. Make sure that directory is on your `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

Start the demo server:

```sh
cargo run --bin hubu-server
```

By default the server listens on `http://127.0.0.1:8787`. To use another bind
address:

```sh
cargo run --bin hubu-server -- 127.0.0.1:8788
```

In another terminal, check that the CLI can reach it:

```sh
hubu health
```

Expected output:

```txt
Hubu server: ok
```

If the server is running on a non-default port, pass `--url`:

```sh
hubu --url http://127.0.0.1:8788 health
```

## Demo Workflow

For an automated walkthrough with colorful progress output, run:

```sh
./scripts/demo.sh
```

The script builds the binaries, starts `hubu-server`, registers an agent, adds a
policy, submits allowed and over-limit spend requests, prints the ledger, and
stops the server on exit.

To adjust pacing:

```sh
HUBU_DEMO_STEP_DELAY=2 HUBU_DEMO_READ_DELAY=4 ./scripts/demo.sh
```

### 1. Register an Agent

```sh
hubu register-agent \
  --name codex-agent \
  --version 1.0
```

Expected output:

```txt
Agent registered
  agent_id: agt_1p8x7k2m4q9v1c6d
  version_id: agv_0r5f0p3tn8wqj2az
  account_id: aga_3hc6q3d9m1v8ra7k
  session_id: ags_9g2h7rx0cq4p9w5e
```

Copy the public `agent_id` for the next commands. Internally, Hubu still uses a
UUID-backed `AgentId`; the CLI and HTTP demo API use the shorter public ID. The
suffix will differ for each new registration because it is derived from the
internal UUID.

### 2. Add a Policy

```sh
hubu add-policy \
  --agent-id agt_1p8x7k2m4q9v1c6d \
  --daily-limit 100
```

Expected output:

```txt
Policy added
  agent_id: agt_1p8x7k2m4q9v1c6d
  policy_id: demo_policy_agt_1p8x7k2m4q9v1c6d
  per_request_limit: $100.00
  default_decision: needs_approval
```

### 3. Submit an Allowed Spend Request

```sh
hubu spend \
  --agent-id agt_1p8x7k2m4q9v1c6d \
  --amount 20 \
  --reason "Purchase API credits"
```

Expected output:

```txt
Spend evaluated
  decision: allow
  decision_id: 7da692a8-d5a7-4028-b5db-fc8b0de79d10
  reason: amount is at or below the configured demo limit of 10000 cents
Payment
  status: succeeded
  payment_id: dfd9a10f-e80f-4c17-b3ec-944d2114d4b9
  ledger_transaction_id: 87b1cb7f-0fdf-40c8-b260-343cf4939be9
  rail_reference: fiat_mock_7da692a8-d5a7-4028-b5db-fc8b0de79d10:Purchase API credits
```

### 4. Submit an Over-Limit Spend Request

```sh
hubu spend \
  --agent-id agt_1p8x7k2m4q9v1c6d \
  --amount 120 \
  --reason "Large API credit purchase"
```

Expected output:

```txt
Spend evaluated
  decision: needs_approval
  decision_id: b58e1683-6707-41ec-b8c8-1ab1c52c2632
```

The demo server only orchestrates a payment when the existing spend manager
returns `allow`.

### 5. Submit a Denied Spend Request

```sh
hubu spend \
  --agent-id agt_1p8x7k2m4q9v1c6d \
  --amount 20 \
  --reason "Attempt blocked merchant purchase" \
  --merchant blocked-merchant
```

Expected output:

```txt
Spend evaluated
  decision: deny
  decision_id: 6f8625e8-426f-40f1-91ff-bef0efb9600b
  reason: merchant is blocked by the demo policy
  reason: amount is at or below the configured demo limit of 10000 cents
```

The policy engine gives `deny` precedence over `allow`, so no payment is
orchestrated.

### 6. Inspect the Ledger

```sh
hubu ledger list
```

Expected output:

```txt
2026-05-28T22:16:36.508390+00:00  87b1cb7f-0fdf-40c8-b260-343cf4939be9  payment dfd9a10f-e80f-4c17-b3ec-944d2114d4b9 via fiat_mock
  debit      $20.00  2d2221f0-2a82-458c-bbd1-6a777a3fc6f8
  credit     $20.00  6fa2b721-c070-4d75-8689-1703dcbb0e9c
```

Only successful mock payments create ledger transactions.

## CLI Reference

```sh
hubu [--url http://127.0.0.1:8787] register-agent --name NAME --version VERSION
hubu [--url http://127.0.0.1:8787] add-policy --agent-id ID --daily-limit AMOUNT
hubu [--url http://127.0.0.1:8787] spend --agent-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
hubu [--url http://127.0.0.1:8787] ledger list
hubu [--url http://127.0.0.1:8787] health
```

Without `cargo install --path crates/hubu-cli`, you can still run the CLI from
the repository with `cargo run --bin hubu -- ...` or `./target/debug/hubu ...`
after `cargo build`.

## Known Limitations

- Server state is in memory. Restarting `hubu-server` clears registered agents,
  policies, spend decisions, payments, and ledger records.
- `--daily-limit` is demo wording for an existing per-request amount policy
  rule. The current demo does not aggregate spend across a calendar day.
- The server uses a minimal local HTTP adapter for demo use, not a production
  web framework.
- Payments use the existing mock rail only. No real payment provider is called.
- `needs_approval` is surfaced as the over-limit outcome; there is no human
  approval queue in this demo.
