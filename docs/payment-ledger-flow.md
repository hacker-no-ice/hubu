# Payment and Ledger Flow

Hubu's payment layer starts as a small orchestration boundary that can grow into
real fiat and crypto rails later.

## Responsibilities

- `hubu-core::spend` evaluates spend requests and issues spend authorization
  tokens.
- `hubu-wallet::payment` accepts payment requests, validates the spend
  authorization through a trait boundary, executes a selected payment rail, and
  returns a payment response.
- `hubu-wallet::rail` defines the payment rail abstraction. The first
  implementation is `MockPaymentRail`, which supports both fiat and stablecoin
  mock rail kinds through one contract.
- `hubu-wallet::ledger` records successful money movement as immutable
  double-entry ledger transactions in SQLite.

## Payment Request

A payment request is payment-facing. It includes:

- idempotency key
- spend authorization token id
- agent id
- amount and currency
- merchant and task context
- selected rail
- rail-specific destination

The request intentionally carries enough context to compare against the original
authorized spend before money moves.

## Rail Abstraction

Rails implement:

```rust
PaymentRail::execute(&PaymentRequest) -> RailPaymentResult
```

The mock rail currently returns a synchronous success or failure. Real fiat or
stablecoin rails can keep the same boundary while adding pending states,
webhooks, settlement IDs, on-chain transaction hashes, and retry metadata.

## Spend Authorization Boundary

`PaymentManager` depends on `SpendAuthorizationValidator` rather than directly
depending on `hubu-core::SpendManager`.

That keeps wallet/payment orchestration decoupled from the spend manager and
storage implementation. A future adapter can validate that:

- the token exists
- the token is not expired, used, or revoked
- the linked spend decision was allowed
- payment amount, currency, agent, merchant, and task match the authorized spend

After a successful rail execution and ledger write, the token is marked used.

## Ledger Model

Successful payments write one immutable ledger transaction with balanced entries.
The initial accounting shape is:

- debit `AgentSpendExpense`
- credit `UserWalletCash`

SQLite stores:

- `ledger_accounts`
- `ledger_transactions`
- `ledger_entries`

The ledger enforces basic immutability with SQLite triggers that reject updates
and deletes for transactions and entries. Balance is enforced in Rust before
entries are inserted.

## First-Milestone Limitations

- Failed rail attempts return a failed payment response and are persisted in the
  local payment-attempt table, but Hubu does not yet expose a separate failed
  payment journal view.
- Payment responses are cached in memory for idempotency during a process run
  and hydrated from SQLite payment-attempt records on restart. Successful money
  movement is also recorded in the SQLite ledger.
- Real rail adapters still need rail-specific confirmation and reconciliation
  flows.
