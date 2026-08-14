# Hubu Local Demo

This demo runs Hubu locally and exercises onboarding, registration, policy,
advisory spending targets, agent-budget creation, spend evaluation, mock payment
orchestration, the budget-hold lifecycle, and ledger recording through the
`hubu` CLI.

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

The server reads `HUBU_AUTH_TOKEN`, or creates/reads `hubu.auth-token` in the
current directory. The CLI reads `HUBU_AUTH_TOKEN` or the same token file and
sends it as a bearer token for protected routes. Set `HUBU_AUTH_TOKEN_FILE` if
the server and CLI run from different working directories.

Restart `hubu-server` after rebuilding API or storage changes; reinstalling the
CLI only updates the client binary. To reset local demo data:

```sh
./scripts/reset-local-state.sh --yes
```

The reset command is a dry run without `--yes` and preserves both credential
files by default. Add `--include-auth-token` and/or
`--include-reconciliation-token` to explicitly remove those files too. Override
their paths with `HUBU_AUTH_TOKEN_FILE` and
`HUBU_RECONCILIATION_TOKEN_FILE`, respectively.

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

The script builds the binaries, starts `hubu-server`, registers a human user,
registers an agent, generates and attaches a policy, creates a recurring budget,
submits allowed, failed-payment, over-limit, and denied spend requests, prints
the budget balance and ledger, and stops the server on exit.

To adjust pacing:

```sh
HUBU_DEMO_STEP_DELAY=2 HUBU_DEMO_READ_DELAY=4 ./scripts/demo.sh
```

### 1. Register the Human User

```sh
hubu register human \
  --username alice-example \
  --display-name "Alice Example" \
  --email alice@example.com
```

Expected output:

```txt
Human registered
  user_id: usr_6qqcj94w6pr5
  username: alice-example
  display_name: Alice Example
```

The demo server creates a fallback default user on startup so core workflows can
run in single-user MVP mode. Calling `hubu register human` explicitly creates a
new human user from the provided username, display name, and email, then makes
that user the active default for subsequent policy, spend, payment, and ledger
operations.

List users when you need to confirm which local user is current:

```sh
hubu user list
```

### 2. Register an Agent

```sh
hubu register agent \
  --name codex-agent \
  --version 1.0
```

Expected output:

```txt
Agent registered
  agent_id: agt_8x7k2m4q9v1c
  version_id: agv_5f0p3tn8wqj2
  account_id: aga_c6q3d9m1v8ra
  session_id: ags_2h7rx0cq4p9w
```

Copy the public `agent_id` for budgets and `account_id` for spend commands.
Internally, Hubu still uses UUID-backed IDs; the CLI and HTTP demo API
use shorter public IDs such as `usr_...`, `agt_...`, and `aga_...`. The suffixes
will differ for each new user and agent because they are derived from internal
UUIDs.

### 3. Generate and Add a Policy

```sh
hubu policy new-template --path policy.yaml
hubu policy validate --path policy.yaml
hubu policy add --path policy.yaml
hubu policy list
```

Expected output:

```txt
Hubu policy template created
  path: policy.yaml
  next: edit the file, then run hubu policy add --path policy.yaml
Policy valid
  path: policy.yaml
  policy_id: starter_spending_policy
  policy_version: starter-1
  default_decision: needs_approval
  rules: 2
Policy added
  scope: user_default
  policy_id: starter_spending_policy
  policy_version: starter-1
  default_decision: needs_approval
SCOPE         AGENT ID  POLICY ID             VERSION  DEFAULT         RULES  ATTACHED AT                 UPDATED AT
------------  --------  --------------------  -------  --------------  -----  --------------------------  --------------------------
user_default  -         starter_spending_policy  starter-1   needs_approval  2      2026-06-09T14:32:10-07:00  2026-06-09T14:32:10-07:00
```

`hubu policy new-template` generates an editable YAML policy file.
`hubu policy validate --path` checks the file locally before it is attached to
the current human user. Human registration lives under `hubu register human`,
while `hubu policy add --path` loads the policy file. The server stamps the
policy to the active human user during add, so authored policy YAML does not
need an owner id. `hubu policy list` shows which policy is attached and when it
was attached.

### 4. List Registered Agents

```sh
hubu agent list
```

Expected output:

```txt
agt_8x7k2m4q9v1c  codex-agent  account: aga_c6q3d9m1v8ra  status: active
```

### 5. Set an Advisory Spending Target and Create an Agent Budget

```sh
hubu user spending-target set --amount 50
hubu budget create --agent-id agt_8x7k2m4q9v1c --amount 75
```

Expected output:

```txt
Spending target set (advisory)
  target_id: tgt_6qqcj94w6pr5  status: active
    target: $50.00  allocated: $0.00  exceeded by: $0.00
    period: 2026-06-03T10:19:20.123456-07:00 -> open-ended
    enforcement: advisory only
Budget created
  budget_id: bgt_8x7k2m4q9v1c  agent_id: agt_8x7k2m4q9v1c  status: active
    limit: $75.00  consumed: $0.00  frozen: $0.00  remaining: $75.00
    period: 2026-06-03T10:19:20.123456-07:00 -> open-ended
Spending target warning (advisory)
  target_id: tgt_6qqcj94w6pr5
  target: $50.00  allocated: $75.00  exceeded by: $25.00
  agent budget allocations exceed the advisory spending target by 2500 cents; budget creation was not blocked
```

Hubu uses the budget as the agent's hard spending allocation. The target is an
advisory comparison across concurrent agent budget allocations, so exceeding it
produces a warning without blocking the $75 budget.

### 6. Submit an Allowed Spend Request

```sh
hubu spend \
  --operation-key demo-allow \
  --account-id aga_c6q3d9m1v8ra \
  --amount 20 \
  --reason "Purchase API credits"
```

Expected output:

```txt
Spend evaluated
  operation_key: demo-allow
  account_id: aga_c6q3d9m1v8ra
  agent_id: agt_8x7k2m4q9v1c
  decision: allow
  decision_id: 7da692a8-d5a7-4028-b5db-fc8b0de79d10
  reason: amount is within the starter single-spend limit of 10000 cents
Payment
  status: succeeded
  payment_id: dfd9a10f-e80f-4c17-b3ec-944d2114d4b9
  owner_user: Alice Example (usr_6qqcj94w6pr5)
  account_id: aga_c6q3d9m1v8ra
  ledger_transaction_id: 87b1cb7f-0fdf-40c8-b260-343cf4939be9
  rail_reference: fiat_mock_7da692a8-d5a7-4028-b5db-fc8b0de79d10:Purchase API credits
Budget hold
  status: settled
  hold_id: e9ee93b7-dac7-4c23-946f-2a7bc2835c24
  budget_id: bgt_6qqcj94w6pr5
  amount: $20.00
  consumed: $20.00
  frozen: $0.00
  remaining: $55.00
```

The owner shown on the payment is the initialized human user. The agent spends
under authority delegated by that user. Allowed spend reserves the active agent
budget before payment; successful payment settles that hold into consumed
balance.

### 6a. Authorize Spend Without Executing Payment

```sh
hubu spend authorize \
  --operation-key demo-logo \
  --account-id aga_c6q3d9m1v8ra \
  --amount 5 \
  --reason "Generate Project Hubu logo" \
  --merchant hubu-model-proxy
```

Expected output:

```txt
Spend evaluated
  operation_key: demo-logo
  account_id: aga_c6q3d9m1v8ra
  agent_id: agt_8x7k2m4q9v1c
  decision: allow
  decision_id: 7da692a8-d5a7-4028-b5db-fc8b0de79d10
  auth_token_id: 1e48e2ec-564e-4519-9db4-d7892012ca78
  reason: amount is within the starter single-spend limit of 10000 cents
Budget hold
  status: frozen
  hold_id: e9ee93b7-dac7-4c23-946f-2a7bc2835c24
  budget_id: bgt_6qqcj94w6pr5
  amount: $5.00
  consumed: $20.00
  frozen: $5.00
  remaining: $50.00
```

This is the first slice of the logo-generation demo path: Hubu evaluates policy,
issues a spend authorization token and freezes the agent budget, but does
not submit payment or write a ledger transaction. A later vendor/model proxy can
consume the token while keeping provider API keys inside Hubu.

### 7. Submit an Allowed Spend Whose Mock Payment Fails

```sh
hubu spend \
  --operation-key demo-failed \
  --account-id aga_c6q3d9m1v8ra \
  --amount 15 \
  --reason "Test failed merchant payout" \
  --merchant fail
```

Expected output:

```txt
Spend evaluated
  operation_key: demo-failed
  account_id: aga_c6q3d9m1v8ra
  agent_id: agt_8x7k2m4q9v1c
  decision: allow
  decision_id: 9ed7d2a1-782f-45d3-9262-a69f7e610d7d
  reason: amount is within the starter single-spend limit of 10000 cents
Payment
  status: failed
  payment_id: 11f94960-2b30-429b-8d2f-0069eac928b5
  owner_user: Alice Example (usr_6qqcj94w6pr5)
  account_id: aga_c6q3d9m1v8ra
  failure_reason: mock rail declined merchant
Budget hold
  status: released
  hold_id: 098cbd0f-ff3d-4817-a741-1e58158f65df
  budget_id: bgt_6qqcj94w6pr5
  amount: $15.00
  consumed: $20.00
  frozen: $0.00
  remaining: $55.00
```

Failed mock payments release the frozen amount back to the active agent budget.
Only successful payments become ledger transactions.

### 8. Submit an Over-Limit Spend Request

```sh
hubu spend \
  --operation-key demo-needs-approval \
  --account-id aga_c6q3d9m1v8ra \
  --amount 120 \
  --reason "Large API credit purchase"
```

Expected output:

```txt
Spend evaluated
  operation_key: demo-needs-approval
  account_id: aga_c6q3d9m1v8ra
  agent_id: agt_8x7k2m4q9v1c
  decision: needs_approval
  decision_id: b58e1683-6707-41ec-b8c8-1ab1c52c2632
```

The demo server only orchestrates a payment when the existing spend manager
returns `allow`.

### 9. Submit a Denied Spend Request

```sh
hubu spend \
  --operation-key demo-deny \
  --account-id aga_c6q3d9m1v8ra \
  --amount 20 \
  --reason "Attempt blocked merchant purchase" \
  --merchant blocked-merchant
```

Expected output:

```txt
Spend evaluated
  operation_key: demo-deny
  account_id: aga_c6q3d9m1v8ra
  agent_id: agt_8x7k2m4q9v1c
  decision: deny
  decision_id: 6f8625e8-426f-40f1-91ff-bef0efb9600b
  reason: merchant is blocked by the starter policy
  reason: amount is within the starter single-spend limit of 10000 cents
```

The policy engine gives `deny` precedence over `allow`, so no payment is
orchestrated.

### 10. Inspect the Budget and Spending Target

```sh
hubu budget list
hubu user spending-target show
```

Expected output:

```txt
  budget_id: bgt_6qqcj94w6pr5  agent_id: agt_8x7k2m4q9v1c  status: active
    limit: $75.00  consumed: $20.00  frozen: $0.00  remaining: $55.00
    period: 2026-06-03T10:19:20.123456-07:00 -> 2026-07-03T10:19:20.123456-07:00
  target_id: tgt_8x7k2m4q9v1c  status: active
    target: $50.00  allocated: $75.00  exceeded by: $25.00
    period: 2026-06-03T10:19:20.123456-07:00 -> open-ended
    enforcement: advisory only
```

The budget balance tracks reserved and consumed agent spend. The spending target
compares the user's concurrent agent allocations without tracking a balance or
participating in spend authorization.

### 11. Inspect the Ledger

```sh
hubu ledger list
```

Expected output:

```txt
2026-06-01T23:49:51.098978+00:00  87b1cb7f-0fdf-40c8-b260-343cf4939be9  payment dfd9a10f-e80f-4c17-b3ec-944d2114d4b9 via fiat_mock  owner: Alice Example (usr_6qqcj94w6pr5)
  debit      $20.00  2d2221f0-2a82-458c-bbd1-6a777a3fc6f8  owner: Alice Example (usr_6qqcj94w6pr5)
  credit     $20.00  6fa2b721-c070-4d75-8689-1703dcbb0e9c  owner: Alice Example (usr_6qqcj94w6pr5)
```

Only successful mock payments create ledger transactions. The ledger output
shows the owning human user for both the transaction and its entries.

## CLI Reference

```sh
hubu [--url http://127.0.0.1:8787] init [--policy FILE] [--force]
hubu [--url http://127.0.0.1:8787] register human --username USERNAME --display-name NAME [--email EMAIL]
hubu [--url http://127.0.0.1:8787] user list
hubu [--url http://127.0.0.1:8787] user spending-target set --amount AMOUNT [--starting-at RFC3339] [--ending-before RFC3339]
hubu [--url http://127.0.0.1:8787] user spending-target show [--all]
hubu [--url http://127.0.0.1:8787] user spending-target revoke --target-id ID
hubu [--url http://127.0.0.1:8787] protocol agent-registration
hubu [--url http://127.0.0.1:8787] register agent [--name NAME] [--version VERSION] [--dry-run]
hubu [--url http://127.0.0.1:8787] policy new-template [--path FILE] [--force]
hubu [--url http://127.0.0.1:8787] policy validate --path FILE
hubu [--url http://127.0.0.1:8787] policy add --path FILE
hubu [--url http://127.0.0.1:8787] policy list
hubu [--url http://127.0.0.1:8787] agent list [--all]
hubu [--url http://127.0.0.1:8787] budget create --amount AMOUNT --agent-id ID [--starting-at RFC3339] [--ending-before RFC3339]
hubu [--url http://127.0.0.1:8787] budget create-recurring --amount AMOUNT --agent-id ID --recurrence daily|monthly|yearly --period-count N [--starting-at RFC3339]
hubu [--url http://127.0.0.1:8787] budget revoke --budget-id ID
hubu [--url http://127.0.0.1:8787] budget replace --budget-id ID --amount AMOUNT
hubu [--url http://127.0.0.1:8787] budget list [--all]
hubu [--url http://127.0.0.1:8787] spend --operation-key KEY --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
hubu [--url http://127.0.0.1:8787] spend authorize --operation-key KEY --account-id ID --amount AMOUNT --reason TEXT [--merchant NAME]
hubu [--url http://127.0.0.1:8787] ledger list
hubu [--url http://127.0.0.1:8787] health
```

Without `cargo install --path crates/hubu-cli`, you can still run the CLI from
the repository with `cargo run --bin hubu -- ...` or `./target/debug/hubu ...`
after `cargo build`.

## Known Limitations

- Local demo state is stored in SQLite at `HUBU_DB_PATH`, defaulting to
  `hubu.sqlite3` in the server working directory. Use
  `./scripts/reset-local-state.sh --yes` to clear it.
- `hubu user spending-target show` shows the current user's advisory target and
  maximum concurrent agent budget allocation. `hubu budget list` shows active
  budgets owned by agents belonging to the current user. Use `--all` to include
  inactive records.
- The server uses a minimal local HTTP adapter for demo use, not a production
  web framework.
- Payments use the existing mock rail only. No real payment provider is called.
- `needs_approval` is surfaced as the over-limit outcome; there is no human
  approval queue in this demo.
