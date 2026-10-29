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
proposed ──approve──> open ──request close──> close pending ──approve──> closed
   │                   │  ▲                         │
   │                   │  └──── reject or expire ───┘
   │                   └──────── human closes directly ───────────────> closed
   └──deny or expire──> nothing opened
```

An agent's task budget follows five steps:

1. **Propose.** The agent proposes a task budget for a task, such as "landing
   page, $3". It is not active until a human approves it.
2. **Approve open.** The human approves or rejects the proposal. On approval,
   the task budget opens and reserves its amount.
3. **Spend.** The agent spends from the task budget. This is not
   pre-approval: each request still goes through the normal approval policy.
   The task budget only caps and reserves the money.
4. **Request close.** When the agent thinks the task is done, it asks to
   close. The human checks that the task is actually complete.
5. **Approve close.** On approval, the task budget closes and returns what is
   unused. On rejection, it stays open and the task continues.

A human who uses the CLI or HTTP routes directly (`hubu budget task open` and
`close`) still opens and closes in one step, with no approval request.

**Open.** Opening needs the agent's currently available budget in the
requested currency (`usd` by default). The requested limit must fit in that
budget's unallocated balance: its remaining balance minus the unused part of
any other open task budgets. The task budget records which agent budget it
draws from.

An agent can have only one open task budget per task and currency. Repeating an
identical open request (same agent, task, currency and exact limit) returns the
existing task budget with `idempotent_replay: true`. A request for the same task
with a different limit is a conflict, and so is a direct open while an agent's
proposal for the task is waiting (`task_budget_proposal_pending`). The open
fingerprint uses accounting units only, with no cent form.

**Proposal.** An agent's proposal records the task, currency, limit and
reason, and returns a `tba_` approval request. It reserves nothing. Its limit
must fit the unallocated balance when proposed, and again when approved.
While it waits, spend carrying its task is denied with `task budget proposal
is waiting for human approval`; it does not fall through to the agent budget.
A repeated identical proposal returns the pending one, or the task budget once
it is open. Approval opens the task budget as the approving human
(`opened_by`), keeping the proposal's `source` and `reason`. A denied or
expired proposal opens nothing, and the task's spend then draws from the agent
budget as for any task without a task budget.

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

Spend for a task whose proposal or close request is waiting for a human is
denied with `task budget proposal is waiting for human approval` or `task
budget close request is waiting for human approval`.

These denials are automatic admission denials with the
`insufficient_task_budget` cause in governance evidence, separate from
`insufficient_budget`. A human approval of spend that a task budget would deny
is refused, and the spend request stays pending.

**Close request.** An agent's close request returns a `tba_` approval request
and leaves the task budget open, still holding its unused part. While the
request waits, new spend for the task is refused. Authorizations already
outstanding can still settle or release. Approval closes the task budget; it is
refused, and the request stays pending, while any authorization under the task
budget is outstanding. Rejection keeps the task budget open, and spending
resumes. A repeated request returns the pending one. A human's direct close
also approves a pending close request.

**Close.** Closing ends the allocation, so the unused amount (`returned_amount`)
is available to the agent budget again. Closing is refused while any
authorization under the task budget is outstanding (frozen or claimed). Settle
or release it first. Closing a closed task budget returns it unchanged. Closed
task budgets are retained and never reopen. A later spend for the same task
draws from the agent budget, unless a new task budget is opened.

## Approval requests

Proposals and close requests are approval requests with `tba_` ids. They use
the same approval link as spend approvals: `GET
/spend/approval?approval_request_id=tba_…` returns the review, and `POST
/spend/approval/resolve` with the human approval capability approves or
denies it. The CLI is `hubu spend approval get|approve|deny
--approval-request-id tba_…`, and MCP clients use `hubu_get_spend_approval`
and the protected `hubu_resolve_spend_approval`.

A task budget approval carries `kind` (`task_budget_open` or
`task_budget_close`), `status` (`pending`, `approved`, `denied` or `expired`),
a `review` with the agent, task, currency, exact `amount_limit`, reason, who
requested it and when, and `expires_at`, plus the `task_budget` it opened or
would close. Repeating a decision returns the request with
`idempotent_replay: true`; a different decision on a resolved request is
`task_budget_approval_resolved`.

Requests expire 24 hours after they are made. An expired proposal opens
nothing. An expired close request leaves the task budget open, and spending
resumes. Task budgets themselves never expire.

`GET /task-budgets` lists pending requests in `pending_approvals`, and an open
task budget with a pending close request shows its
`close_approval_request_id`. `hubu budget task list` and `hubu watch` show
them too.

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
| HTTP, human | `POST /task-budgets` | `POST /task-budgets/close` | `GET /task-budgets[?agent_id=][&all=true]`, `GET /task-budgets/show` |
| HTTP, agent | `POST /task-budgets/proposals` | `POST /task-budgets/close-requests` | as above |
| CLI | `hubu budget task open` | `hubu budget task close` | `hubu budget task list`, `hubu budget task show` |
| MCP | `hubu_open_task_budget` (proposes) | `hubu_close_task_budget` (requests close) | `hubu_list_task_budgets` |

`POST /task-budgets/proposals` takes the same body as `POST /task-budgets`.
`POST /task-budgets/close-requests` takes `task_budget_id` and optional
`reason` and `source`. Both return the pending `approval`, the `task_budget`
when there is one, and `idempotent_replay`.

`POST /task-budgets` takes `agent_id`, `task_id`, an exact decimal `amount`, and
optional `currency`, `reason` and `source`. Responses carry a `task_budget` with
its `tbg_` id, agent and drawn-from budget ids, `state`, exact money objects
for `amount_limit`, `consumed_amount`, `frozen_amount` and `remaining_amount`,
a `returned_amount` once closed, and counts of outstanding and settled
authorizations. There are no `*_cents` fields. Errors use the codes
`invalid_task_budget`, `no_available_budget`, `insufficient_unallocated_budget`,
`task_budget_conflict`, `task_budget_proposal_pending`, `unknown_task_budget`,
`task_budget_outstanding_authorizations`, `unknown_task_budget_approval`,
`task_budget_approval_resolved` and `task_budget_not_open`.

A spend response's `budget_hold` includes `task_budget_id` when the hold was
reserved under a task budget.

`hubu watch` shows each open task budget under its agent as spend so far
(spent plus frozen) against the limit, and the amount left. It marks one with
a pending close request and lists proposals waiting for approval.

### Agents and MCP sessions

An agent cannot open or close a task budget by itself. `hubu_open_task_budget`
sends a proposal and `hubu_close_task_budget` sends a close request. A human
resolves each on the approval link. This replaces HUB-259 decision D, under
which the MCP tools opened and closed with no human click. The MCP router can
reach only the proposal and close-request routes, not the direct ones.

Codex and Claude Code calls carry no trusted task correlation, and models may
not supply `task_id` in spend arguments. Instead, `hubu_open_task_budget` binds
the proposed task to the MCP session. Spend operations created afterward in
that session (`hubu_authorize_spend`, `hubu_submit_spend` and governed
execution) carry its `task_id`. Hubu denies them until the proposal is
approved, then the task budget caps them. A close request keeps the binding, so
Hubu refuses new spend while it is pending. The binding ends when the session
sees the task budget closed, or its proposal denied or expired, through a close
result or `hubu_get_spend_approval` / `hubu_resolve_spend_approval`. Only the
bound proposal and the task budget it opened count: reading an earlier
proposal or closed task budget for the same task leaves the binding in place.
Proposing another task replaces it. A closed task budget's task otherwise draws from the
agent budget. A redelivered harness call keeps the task recorded when it was
first allocated. A platform-invocation envelope keeps its own trusted
`task_id`.

`hubu_operation_status` adds a `task_budget` projection to an operation that
drew from one: the limit, `spent_amount`, `frozen_amount` and
`remaining_amount`. The projection uses the task budget recorded on the
operation's hold, which the authorization record reports as
`budget_hold.task_budget_id`. It never looks the budget up by task name.

## Storage and locking

`task_budgets` lives in the Hubu governance database. Its triggers allow only
the open-to-closed transition and forbid deletes. A partial unique index keeps
one open task budget per agent, task and currency. `budget_holds.task_budget_id`
records attribution. `task_budget_approvals` keeps proposals and close
requests. Its triggers let a request move only once from `pending` and forbid
deletes, and partial unique indexes keep one pending proposal per agent, task
and currency and one pending close request per task budget. Each command first
persists the expiry of requests past `expires_at`; spend checks compare
`expires_at` with the request time, so they do not depend on that sweep. The column is added before any later schema phase reads
holds. Gongbu, the executor contract and the ledger schema are unchanged.

Open, close, approving a request and spend reservation all take the budget
manager lock before the governance repository lock, matching
[budget administration](budget-architecture.md#lock-and-transaction-order). An
allocation check therefore cannot interleave with a reservation.

## Limits

- A task budget's limit cannot be changed. Close it and open a new one.
- Task budgets do not expire on their own. Close them when the task ends.
  Proposals and close requests expire after 24 hours.
- A task budget is not pre-approval. Spend inside it still goes through the
  normal approval policy.
- Ownership follows the agent: only the agent's owner can open, close, read
  or resolve requests for its task budgets.
- The direct `POST /task-budgets` and `/task-budgets/close` routes need only the
  owner's bearer credential, like budget creation. The MCP router does not
  expose them.
