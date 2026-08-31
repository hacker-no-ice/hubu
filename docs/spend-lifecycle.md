# Spend lifecycle

Hubu is the spend control plane. It evaluates policy, reserves an agent budget,
issues scoped authorization, tracks executor claims, and records final financial
state. Provider execution remains outside Hubu, while direct payment rails and
the ledger remain owned by `hubu-wallet`.

The end-to-end lifecycle is:

```text
request
  -> policy decision
  -> budget reservation
  -> authorization
  -> payment or external executor claim
  -> settlement or release
  -> ledger and audit state
```

## Request identity and retry safety

Every spend operation has a stable, agent-scoped `operation_key` supplied by a
trusted client platform or orchestrator. Hubu stores the workflow under that
key. An identical retry recovers the existing operation. A trusted direct Hubu
client may change scope under that key only when Hubu explicitly returns
`reuse_operation_key` after an entirely denied, side-effect-free history. The
unified MCP agent surface does not expose or reuse the private key after denial:
corrected work is a new tool call and logical operation. All other retries that
change the account, amount, reason, lease profile, task correlation, or
execution scope are rejected.

`task_id` is optional trusted business correlation. `reason` is descriptive,
model-visible audit context. Neither field owns a budget or replaces the
operation key.

Amounts use integer minor units and a currency. The current local spend path is
USD-only.

## Trusted execution scope

Execution-scope schema version 1 replaces the overloaded `merchant` convention
with four independently addressable identities:

- `provider`: the API or service that performs the work;
- `executor`: the trusted execution-plane adapter allowed to claim the token;
- `capability`: the requested outcome independent of tool or provider name;
- `billing_merchant`: the party expected to charge the governed account.

Each identity has a stable ID and a display name. Hubu resolves all selectors
against one trusted catalog entry and snapshots the canonical entry in the
spend decision:

```json
{
  "schema_version": 1,
  "provider": "provider:google:gemini-developer",
  "executor": "executor:gongbu:image",
  "capability": "capability:image:generate",
  "billing_merchant": "merchant:google"
}
```

Policies address these stable identities directly. Payment and executor
validation exact-match the complete canonical scope; caller-supplied display
names never grant authority.

Legacy requests that contain only `merchant` remain readable and normalize to
a version-1 scope whose provider, executor, and capability are
`legacy:unresolved`. New callers send `execution_scope` and omit `merchant`.
Supplying both is rejected.

## Policy decision

The [policy engine](policy-engine.md) returns one of three effects:

- `allow`: continue to budget reservation;
- `deny`: stop without reserving or moving funds;
- `needs_approval`: persist a pending decision and wait for explicit human
  resolution.

Human approval operates on the immutable review snapshot. Approval makes the
same operation eligible for continuation but does not itself invoke payment or
provider work; the unified MCP flow resumes it separately by public operation
handle. Denial ends it. Repeating the same resolution is idempotent, while a
conflicting resolution is rejected.

## Budgets and spending targets

An agent budget is a hard spending limit. A user spending target is advisory:
it helps a human compare aggregate allocations with a preferred amount but does
not block budget creation or spend.

An agent budget has one owner, a stable logical ID, an immutable currency and
time window, and a current immutable version containing its total limit and
change provenance. Consumed and frozen usage belong to the logical budget;
remaining is checked against the current-version limit. Every hold records both
the logical budget ID and the version that authorized it.

SQLite persists only the budget's administrative state: `active` or `revoked`.
Hubu derives availability from one captured instant and the current balance,
using this precedence:

```text
revoked -> scheduled -> expired -> exhausted -> active
```

Periods are half-open: a budget is scheduled before `starting_at`, is eligible
at `starting_at`, and is expired at `ending_before`. A zero or negative
remaining balance is exhausted only while the period is otherwise live.
Default budget listing includes scheduled, active, and exhausted budgets;
requesting all budgets also includes expired and revoked budgets.

Only an effectively active budget may accept a new reservation. Existing holds
remain attributable and may settle, release, expire, or enter reconciliation
after the budget becomes exhausted, expired, or revoked. Those finalization
paths update hold and balance state without changing the administrative state.
All non-revoked periods participate in overlap prevention for the same agent
and currency, including exhausted periods; revocation is the explicit way to
remove a period from that constraint.

After policy allows a request, Hubu atomically reserves the authorized maximum
as a budget hold. A hold moves through:

```text
frozen -> settled
       -> released
```

Settlement preserves exact external cost as an integer amount, decimal scale,
and currency, then converts that value to budget cents with one checked ceiling
operation. Any positive sub-cent charge therefore consumes at least one cent.
A normal settlement consumes that conservative `budget_charge_cents` and
releases any unused maximum. Failure, unused authorization, expiry, or a
confirmed non-billable outcome releases the hold. Ambiguous provider outcomes
and confirmed costs above the authorized maximum enter reconciliation instead
of being released optimistically.

An executor cannot settle above its maximum. Once the claim lease expires, a
human reviewing provider evidence may confirm a legitimate billed overrun.
Hubu then consumes the full conservatively rounded charge, records the overrun
separately, releases none of the hold, and allows the remaining balance to become negative. That is
retrospective accounting for an external charge that already occurred; it does
not grant new spending authority, and the exhausted budget rejects subsequent
reservations.

## Authorization paths

Hubu exposes two related paths:

- Authorize-only evaluates policy, freezes the budget, and issues a spend
  authorization token. It does not call a provider, submit payment, or write a
  ledger transaction.
- Submit-spend performs the same controls and then invokes Hubu's configured
  payment orchestration path.

An authorization token points to the immutable spend decision and expires. It
can be claimed only by the executor and scope authorized by that decision.
For unified-MCP human approvals, the explicit public-handle resume must happen
before that lease expires. An expired pre-resume authorization is terminal for
that operation and requires a new logical operation; it never starts provider
work.
Detailed external-executor rules are defined by the
[spend executor contract](spend-executor-contract.md).

## External execution

For Gongbu and other external executors:

1. The executor resolves the authorization snapshot without claiming it.
2. It validates the account, amount, currency, workload, operation key, and
   complete execution scope against its operator-controlled target and price.
3. It persists its execution before scheduling provider work.
4. Its durable workflow claims the authorization.
5. It records a provider attempt before irreversible transmission.
6. It persists the exact provider cost, currency, scale, complete frozen pricing
   snapshot, and provider evidence.
7. It settles with that stable receipt after confirmed billing, routes a
   settlement overrun to reconciliation for human resolution after claim
   expiry, or releases after confirmed non-billing.

Provider credentials, provider retries, artifacts, and provider pricing remain
owned by the executor. Sharing a Rust workspace does not turn this wire
boundary into an in-process or shared-database call.

## Payment and ledger path

`hubu-wallet::payment` accepts a payment request, validates it against the spend
authorization through `SpendAuthorizationValidator`, executes a selected
`PaymentRail`, and returns a payment response. The validator boundary keeps
wallet orchestration decoupled from the core spend manager and its storage.

The initial `MockPaymentRail` supports synchronous fiat and stablecoin mock
results. A real rail may add pending states, webhooks, settlement identifiers,
on-chain transaction hashes, and reconciliation metadata without changing the
control-plane ownership boundary.

Successful money movement writes one immutable, balanced, double-entry ledger
transaction. The initial accounting shape is:

```text
debit  AgentSpendExpense
credit UserWalletCash
```

SQLite stores accounts, transactions, and entries. Triggers reject updates and
deletes for transactions and entries, and Rust validates balance before insert.
Failed payment attempts are persisted but do not create successful ledger
movement.

## Failure and reconciliation invariants

- Never execute provider work before a durable claim.
- Never accept a normal executor settlement above the authorized maximum.
- Never discard exact cost or pricing evidence when a legitimate provider
  charge exceeds that maximum; keep the hold claimed for human reconciliation.
- Never convert external cost through floating point or round budget consumption
  down; apply one checked ceiling conversion to the final exact cost.
- Never release a hold merely because a timeout made billing ambiguous.
- Never retry an ambiguous provider mutation under a new operation key.
- Keep Hubu and executor databases, credentials, artifacts, and failure domains
  separate.
- Preserve enough immutable evidence to reconcile billed and non-billed
  outcomes without trusting caller assertions.

Legacy v4.3 cents receipts migrate to exact amount with scale 2. Hubu and Gongbu
upgrade their own databases independently and preserve receipt, operation,
execution, settlement, and provider-request identities. The detailed mapping,
retry behavior, and rollback requirements are defined by the
[spend executor contract](spend-executor-contract.md#persistence-migration-and-v4-compatibility).

Core spend and budget behavior lives in [`crates/hubu-core`](../crates/hubu-core),
while payment and ledger behavior lives in
[`crates/hubu-wallet`](../crates/hubu-wallet).
