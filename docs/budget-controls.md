# Budget Controls

Hubu budgets are hard spending limits for agent-controlled spend. A user cap is
the owner-level guardrail for total spend owned by the current user, not a
fallback that applies only when an agent budget is absent. Agent and task
budgets can narrow that cap to a specific actor or work boundary. These controls
sit after policy evaluation and before payment execution: policy decides
whether a request may proceed, then cap and budget controls decide whether
enough scoped balance can be reserved.

## Core Model

A cap or budget has:

- a scope: user cap, agent, or task
- an immutable limit in cents and currency
- a time period with optional end
- a lifecycle status such as active, exhausted, expired, or revoked
- a balance split into consumed, frozen, and remaining amounts

A budget hold is created when Hubu reserves funds for an allowed spend decision.
The hold starts as `frozen`, then becomes `settled` after successful payment or
`released` when payment fails, authorization is unused, or the hold is canceled.

## Cap And Budget Selection

Human operators can create a user cap for all spend owned by the current user,
or an agent budget for one registered agent:

```sh
hubu user cap set --amount 100
hubu budget create --agent-id AGENT_ID --amount 25
```

When an agent submits spend, the user cap is the outer owner-level guardrail.
An agent budget, when present, adds a narrower agent-level guardrail; it does
not replace or bypass the user's cap. Task-scoped budgets exist in the core
model for future project/workflow boundaries.

A cap or budget scope may only have one limit for a currency at any instant.
Single budgets and recurring series reject overlapping periods for the same
scope and currency. Recurring periods use half-open boundaries, so the end of
one period is the start of the next.

## Spend Flow

Allowed spend follows this intended path:

```txt
policy allow
  -> check active user cap
  -> check active agent/task budget
  -> reserve cap balance and budget balance into frozen hold state
  -> issue spend authorization or execute payment
  -> settle both holds on success, release both holds on failure
```

`hubu spend authorize` stops after policy and budget reservation. It returns a
scoped spend authorization token and freezes cap and budget balances without
executing payment or writing a ledger transaction.

`hubu spend` continues into the wallet rail. In the current local server, that
rail is mocked. Successful payment settles the cap hold and budget hold into
consumed balance and records a ledger transaction; failed payment releases both
holds back to remaining balance.

Hubu does not execute payment when:

- policy returns `deny`
- policy returns `needs_approval`
- no active cap applies
- the active cap or applicable budget has insufficient remaining balance
- the cap or budget is inactive, expired, or in the wrong currency

## CLI Inspection

Use the CLI to create and inspect user caps and agent budgets:

```sh
hubu user cap set --amount 50
hubu user cap show
hubu budget create --agent-id AGENT_ID --amount 25
hubu budget create-recurring --agent-id AGENT_ID --amount 25 --recurrence monthly --period-count 3
hubu budget list
```

`hubu user cap set` creates or renews the current user's cap. `hubu budget
create` and `hubu budget create-recurring` require `--agent-id` and create agent
budgets. `hubu user cap show` and `hubu budget list` show status, limit,
consumed balance, frozen balance, remaining balance, and period. Pair them with
`hubu ledger list` to compare settled payment movement against consumed balance.

## Current Limits

The current local API persists budgets and caps in SQLite and uses a mock
payment rail. There is not yet a durable human approval queue for
`needs_approval` decisions, and task-scoped budget creation is not exposed
through the CLI. The local spend response reports both `budget_hold` and
`cap_hold`; the budget manager enforces the reserve, settle, release, and
overlap invariants used by the local server and MCP tools.
