# Task budgets

A task budget is a short-lived spending cap for one task, such as "design this
landing page for $3". It is drawn from the agent's budget, caps spend for that
task, and returns its unused amount when it closes.

A task budget is not a second agent budget. It owns no money and has no
separate balance. While it is open, it holds back part of the agent budget's
remaining balance, so other spend cannot use that part. Every reservation is
still an ordinary hold on the agent budget. Holds made under a task budget
also record its id.

## Lifecycle

```text
open ──(no outstanding authorizations)──> closed
```

**Open.** Opening needs the agent's currently available budget in the
requested currency (`usd` by default). The requested limit must fit in that
budget's unallocated balance: its remaining balance minus the unused part of
any other open task budgets. The task budget records which agent budget it
draws from.

An agent can have only one open task budget per task and currency. Repeating an
identical open request (same agent, task, currency and exact limit) returns the
existing task budget with `idempotent_replay: true`. A request for the same task
with a different limit is a conflict. The open fingerprint uses accounting
units only, with no cent form.

**Spend.** Hubu checks a spend request against an open task budget when its
trusted `task_id` matches the task budget's task, agent and currency:

- The request must fit the task budget's remaining amount (limit minus spent
  minus frozen). Otherwise it is denied with `task budget does not have enough
  remaining balance`. It is not escalated.
- The request must reserve from the agent budget the task budget draws from.
  If that budget is no longer the agent's available budget (for example, its
  period ended), the request is denied with `task budget is drawn from an agent
  budget that is no longer available`.
- The hold is attributed to the task budget. Spend allowed by the task budget
  also passes the agent budget's own check, because the task's allocation
  already sits inside that budget.

Spend without a matching open task budget draws from the agent budget as
before. It cannot use the unused part of open task budgets: a request that fits
the agent budget's remaining balance but not its unallocated balance is denied
with `budget balance is held for open task budgets`.

All three denials are automatic admission denials with the
`insufficient_task_budget` cause in governance evidence, separate from
`insufficient_budget`. A human approval that would exceed a task budget is
refused, and the request stays pending.

**Close.** Closing ends the allocation, so the unused amount (`returned_amount`)
is available to the agent budget again. Closing is refused while any
authorization under the task budget is outstanding (frozen or claimed). Settle
or release it first. Closing a closed task budget returns it unchanged. Closed
task budgets are retained and never reopen. A later spend for the same task
draws from the agent budget, unless a new task budget is opened.

## Spend so far

A task budget's spend comes from its attributed holds, so no settlement path
keeps a second balance:

- **frozen**: the amounts of its frozen or claimed holds;
- **spent** (`consumed_amount`): for each settled hold, the budget charge the
  ledger recorded for its spend decision, including corrections. A settled hold
  with no recorded charge counts its full reserved amount. Released and expired
  holds count nothing;
- **remaining**: limit minus spent minus frozen.

This is the same exact micro-unit charge the agent budget consumed. A USD
1.234 settlement against a 2.50 authorization spends exactly 1.234 of the task
budget and frees the rest.

## Surfaces

| Surface | Open | Close | Read |
| --- | --- | --- | --- |
| HTTP | `POST /task-budgets` | `POST /task-budgets/close` | `GET /task-budgets[?agent_id=][&all=true]`, `GET /task-budgets/show` |
| CLI | `hubu budget task open` | `hubu budget task close` | `hubu budget task list`, `hubu budget task show` |
| MCP | `hubu_open_task_budget` | `hubu_close_task_budget` | `hubu_list_task_budgets` |

`POST /task-budgets` takes `agent_id`, `task_id`, an exact decimal `amount`, and
optional `currency`, `reason` and `source`. Responses carry a `task_budget` with
its `tbg_` id, agent and drawn-from budget ids, `state`, exact money objects
for `amount_limit`, `consumed_amount`, `frozen_amount` and `remaining_amount`,
a `returned_amount` once closed, and counts of outstanding and settled
authorizations. There are no `*_cents` fields. Errors use the codes
`invalid_task_budget`, `no_available_budget`, `insufficient_unallocated_budget`,
`task_budget_conflict`, `unknown_task_budget` and
`task_budget_outstanding_authorizations`.

A spend response's `budget_hold` includes `task_budget_id` when the hold was
reserved under a task budget.

`hubu watch` shows each open task budget under its agent as spend so far
(spent plus frozen) against the limit, and the amount left.

### Agents and MCP sessions

Opening or closing a task budget only re-partitions the agent's own budget and
never authorizes spend, so the MCP tools need no human click.

Codex and Claude Code calls carry no trusted task correlation, and models may
not supply `task_id` in spend arguments. Instead, `hubu_open_task_budget` binds
the opened task budget to the MCP session. Spend operations created afterward
in that session (`hubu_authorize_spend`, `hubu_submit_spend` and governed
execution) carry its `task_id`, so the task budget caps them. Closing it, or
opening another, ends or replaces the binding. A redelivered harness call keeps
the task recorded when it was first allocated. A platform-invocation envelope
keeps its own trusted `task_id`.

`hubu_operation_status` adds a `task_budget` projection to an operation that
drew from one: the limit, `spent_amount`, `frozen_amount` and
`remaining_amount`. The projection uses the task budget recorded on the
operation's hold, which the authorization record reports as
`budget_hold.task_budget_id`. It never looks the budget up by task name.

## Storage and locking

`task_budgets` lives in the Hubu governance database. Its triggers allow only
the open-to-closed transition and forbid deletes. A partial unique index keeps
one open task budget per agent, task and currency. `budget_holds.task_budget_id`
records attribution. The column is added before any later schema phase reads
holds. Gongbu, the executor contract and the ledger schema are unchanged.

Open, close and spend reservation all take the budget manager lock before the
governance repository lock, matching
[budget administration](budget-architecture.md#lock-and-transaction-order). An
allocation check therefore cannot interleave with a reservation.

## Limits

- A task budget's limit cannot be changed. Close it and open a new one.
- Task budgets do not expire on their own. Close them when the task ends.
- Ownership follows the agent: only the agent's owner can open, close or read
  its task budgets.
