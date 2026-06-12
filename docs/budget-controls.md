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
hubu budget create-recurring --amount 100 --recurrence daily --period-count 7
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
  -> check any narrower agent/task budget
  -> reserve allowed balance into frozen hold state
  -> issue spend authorization or execute payment
  -> settle hold on success, release hold on failure
```

`hubu spend authorize` stops after policy and budget reservation. It returns a
scoped spend authorization token and freezes budget without executing payment or
writing a ledger transaction.

`hubu spend` continues into the wallet rail. In the current local server, that
rail is mocked. Successful payment settles the hold into consumed budget and
records a ledger transaction; failed payment releases the hold back to
remaining budget.

Hubu does not execute payment when:

- policy returns `deny`
- policy returns `needs_approval`
- no active cap applies
- the active cap or applicable budget has insufficient remaining balance
- the cap or budget is inactive, expired, or in the wrong currency

## CLI Inspection

Use the CLI to create and inspect user caps and agent budgets:

```sh
hubu budget create --amount 50
hubu budget create-recurring --amount 100 --recurrence monthly --period-count 3
hubu budget list
```

Without `--agent-id`, `hubu budget create` and `hubu budget create-recurring`
create a user cap. With `--agent-id`, they create an agent budget. `hubu budget
list` shows each cap or budget's scope, status, limit, consumed balance, frozen
balance, remaining balance, and period. Pair it with `hubu ledger list` to
compare settled payment movement against consumed balance.

## Current Limits

The current local API persists budgets in SQLite and uses a mock payment rail.
There is not yet a durable human approval queue for `needs_approval` decisions,
task-scoped budget creation is not exposed through the CLI, and the local spend
response still reports one budget hold. A follow-up implementation should make
the user cap an always-on aggregate guardrail when agent/task budgets are also
configured. The budget manager already enforces the reserve, settle, release,
and overlap invariants used by the local server and MCP tools.
