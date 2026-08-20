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
key. An identical retry recovers the existing operation; a retry that changes
the account, amount, reason, workload profile, task correlation, or execution
scope is rejected.

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

Human approval operates on the immutable review snapshot. Approval resumes the
same operation; denial ends it. Repeating the same resolution is idempotent,
while a conflicting resolution is rejected.

## Budgets and spending targets

An agent budget is a hard spending limit. A user spending target is advisory:
it helps a human compare aggregate allocations with a preferred amount but does
not block budget creation or spend.

An agent budget has one owner, an immutable limit and currency, a time window,
a lifecycle status, and balances split into consumed, frozen, and remaining
amounts. Only one budget for a given agent and currency may cover an instant;
recurring periods use half-open boundaries so adjacent periods do not overlap.

After policy allows a request, Hubu atomically reserves the authorized maximum
as a budget hold. A hold moves through:

```text
frozen -> settled
       -> released
```

Settlement converts actual billable cost to consumed balance and releases any
unused maximum. Failure, unused authorization, expiry, or a confirmed
non-billable outcome releases the hold. Ambiguous provider outcomes enter
reconciliation instead of being released optimistically.

## Authorization paths

Hubu exposes two related paths:

- Authorize-only evaluates policy, freezes the budget, and issues a spend
  authorization token. It does not call a provider, submit payment, or write a
  ledger transaction.
- Submit-spend performs the same controls and then invokes Hubu's configured
  payment orchestration path.

An authorization token points to the immutable spend decision and expires. It
can be claimed only by the executor and scope authorized by that decision.
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
6. It settles with a stable receipt after confirmed billing, or releases after
   confirmed non-billing.

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
- Never settle more than the authorized maximum.
- Never release a hold merely because a timeout made billing ambiguous.
- Never retry an ambiguous provider mutation under a new operation key.
- Keep Hubu and executor databases, credentials, artifacts, and failure domains
  separate.
- Preserve enough immutable evidence to reconcile billed and non-billed
  outcomes without trusting caller assertions.

Core spend and budget behavior lives in [`crates/hubu-core`](../crates/hubu-core),
while payment and ledger behavior lives in
[`crates/hubu-wallet`](../crates/hubu-wallet).
