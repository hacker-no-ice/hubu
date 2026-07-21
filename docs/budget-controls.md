# Budget Controls

Hubu agent budgets are hard spending limits. A user spending target is a
separate advisory signal: it helps a human compare aggregate agent allocations
with a preferred amount, but it never blocks budget creation or spend.

These controls sit after policy evaluation and before payment execution.
Policy decides whether a request may proceed. If policy allows it, Hubu checks
the active budget for the spending agent and reserves that budget before issuing
an authorization or submitting payment.

## Agent Budget Model

An agent budget has:

- exactly one owning agent
- an immutable limit in cents and currency
- a time period with an optional end
- a lifecycle status such as active, exhausted, expired, or revoked
- a balance split into consumed, frozen, and remaining amounts

A budget hold is created when Hubu reserves funds for an allowed spend
decision. The hold starts as `frozen`, becomes `settled` after successful
payment, or becomes `released` when payment fails, the authorization is unused,
or the hold is canceled.

Every spend decision may have at most one budget hold and requires an active USD
budget belonging to the spending agent. A spend may still carry `task_id` as
audit and executor metadata, but that metadata does not select or own a budget.

An agent may only have one budget for a currency at any instant. Single budgets
and recurring series reject overlapping periods for the same agent and
currency. Recurring periods use half-open boundaries, so the end of one period
is the start of the next.

## Advisory Spending Targets

Humans can optionally set a user spending target:

```sh
hubu user spending-target set --amount 100
hubu user spending-target show
```

A spending target has an owner, amount, currency, period, and lifecycle status.
It is persisted separately from budgets and does not have consumed, frozen, or
remaining balances.

When a budget is created, replaced, or created as part of a recurring series,
Hubu finds spending targets whose periods overlap the new budget periods. For
each target, it calculates the maximum concurrent sum of overlapping,
non-revoked agent budget limits. If that allocation exceeds the target, the API
returns a structured `spending_target_warnings` entry and the CLI prints it.
The budget is still created.

For example, a $50 target followed by a $75 agent budget produces a $25
advisory warning. Two adjacent $50 budget periods count as a maximum concurrent
allocation of $50, not $100.

Legacy user-cap records are migrated into spending targets when the governance
database opens. Legacy cap holds are removed; existing agent-budget holds remain
available for executor settlement or release.

## Spend Flow

Allowed spend follows this path:

```txt
policy allow
  -> find the active agent budget
  -> reserve budget balance into one frozen hold
  -> issue spend authorization, claim for executor work, or execute payment
  -> settle the hold on success, release it on failure
```

`hubu spend authorize` stops after policy and budget reservation. It returns a
scoped spend authorization token and freezes the agent budget without executing
payment or writing a ledger transaction.

External executors must turn that frozen authorization into an exclusive
`claimed` hold before irreversible work. Claiming assigns a separate execution
lease based on the authorization's workload profile. The original authorization
may expire while the claim remains active. Expired claimed holds are not
automatically returned because vendor work may already have completed; they
remain frozen for reconciliation.

`hubu spend` continues into the wallet rail. In the current local server, that
rail is mocked. Successful payment settles the budget hold into consumed
balance and records a ledger transaction; failed payment releases the hold back
to remaining balance.

Hubu does not execute payment when:

- policy returns `deny`
- policy returns `needs_approval`
- no active agent budget applies
- the active agent budget has insufficient remaining balance
- the budget is inactive, expired, or in the wrong currency

The spending target is intentionally absent from this list because it is never
an authorization condition.

## CLI Inspection

Use the CLI to create and inspect spending targets and agent budgets:

```sh
hubu user spending-target set --amount 50
hubu user spending-target show
hubu budget create --agent-id AGENT_ID --amount 25
hubu budget create-recurring --agent-id AGENT_ID --amount 25 --recurrence monthly --period-count 3
hubu budget list
```

`hubu user spending-target show` reports the target amount, maximum concurrent
allocation, exceeded amount, period, and the fact that enforcement is advisory.
`hubu budget list` reports budget status, limit, consumed balance, frozen
balance, remaining balance, and period. Pair it with `hubu ledger list` to
compare settled payment movement against consumed budget balance.

## Current Limits

The current local API persists spending targets, budgets, and holds in SQLite
and uses a mock payment rail. There is not yet a durable human approval queue
for `needs_approval`. Shared budgets are intentionally outside the MVP. If a
real shared-allocation use case emerges, Hubu can add an explicit budget-pool
model with agent membership instead of overloading spend task metadata.
