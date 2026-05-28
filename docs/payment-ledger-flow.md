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

That keeps wallet/payment orchestration decoupled from the current in-memory
spend implementation. A future adapter can validate that:

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

- Failed rail attempts return a failed payment response but do not yet persist a
  failed-payment journal row.
- Payment records are idempotent in memory; the durable system of record in this
  slice is the SQLite ledger for successful money movement.
- Real rail adapters still need rail-specific confirmation and reconciliation
  flows.
