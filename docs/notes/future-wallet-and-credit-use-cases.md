# Future Wallet And Credit Use Cases

This note captures future Hubu use cases discussed after the spend executor
contract pivot. It is intentionally roadmap-level: the current first demo should
keep Hubu as the spend control plane and let Gongbu perform model/vendor work.

## Three Spend Execution Modes

Hubu can support multiple post-authorization execution modes without turning
every mode into the same subsystem.

### 1. Accounted External Execution

Hubu authorizes and reserves budget, but an external executor performs the work
and pays or is billed by the vendor directly.

Example:

```text
Agent -> Hubu: authorize $5 for merchant=gongbu.image
Agent -> Gongbu: generate image with spend_auth_token_id
Gongbu -> Hubu: validate token and frozen hold
Gongbu -> Vendor: call image model with Gongbu-held API key
Vendor -> Gongbu/operator account: bill directly
Gongbu -> Hubu: settle or release spend
```

This is the right mode for the Project Hubu logo demo. Hubu tracks policy,
budget, and spend state, but does not move money or call the model vendor.

### 2. Wallet Payment

Hubu authorizes and directly moves money or value through a payment rail.

Examples:

- pay an invoice
- reimburse a human
- pay a contractor or marketplace seller
- buy a domain, SaaS seat, dataset, or stock asset
- order food delivery for the owner
- issue a limited virtual card
- send a stablecoin payout

In these cases, the spend request is itself a payment or purchase. Hubu Wallet
is the executor because the primary action is moving money.

### 3. Credit Pool

Hubu authorizes a money spend to buy prepaid credits, then separately tracks
allocation and consumption of those credits.

Example:

```text
Agent -> Hubu: authorize $50 to buy provider credits
Hubu Wallet -> Vendor/payment rail: purchase or top up credits
Hubu -> Credit pool: record acquired provider credits

Later:
Agent -> Hubu: reserve credits for a task
Executor/vendor: consume credits
Hubu -> Credit pool: settle consumed credits or release unused reservation
```

This mode is useful for API credits, cloud credits, data-provider credits, ad
credits, print/shipping balances, or internal team quotas.

## Why A Money Ledger Alone Is Not Enough For Credits

A double-entry money ledger is still necessary for the actual cash movement that
funds credits, but it is not enough to model credit inventory cleanly.

Money ledger answers:

```text
What money moved?
```

Credit inventory answers:

```text
What prepaid entitlement remains, and who consumed it?
```

Credit pools often need first-class state because:

- credits may be vendor-specific and non-transferable
- credits may expire
- credits may be denominated in tokens, images, seats, requests, or USD-like
  units
- paid amount and granted credits may differ because of promotions or discounts
- usage may reserve, consume, release, expire, or adjust credits asynchronously
- refunds and vendor adjustments may occur in credit units rather than dollars
- teams may want per-agent or per-task allocation without moving money each time

## Suggested Domain Split

Keep the concepts separate:

```text
hubu-wallet
  actual money movement, payment rails, double-entry ledger

hubu-core
  policy, spend authorization, budget holds, executor contract

hubu-credit (future)
  prepaid entitlement inventory, reservations, consumption, expiry, adjustment
```

`hubu-credit` should probably depend only on `hubu-common` at first. It can link
to wallet payments or ledger transactions by ID, but should not need to depend
on `hubu-wallet`.

## Candidate Credit Model

Future credit records may include:

- `CreditPool`
  - owner
  - provider or merchant
  - unit type, such as `usd_credit`, `token`, `image`, `seat`, or `request`
  - acquired units
  - current balance
  - optional expiration
  - linked wallet payment or funding event

- `CreditReservation`
  - pool
  - agent or explicit pool member
  - reserved units
  - expiration
  - status: frozen, consumed, released, expired

- `CreditConsumption`
  - reservation or pool
  - consumed units
  - executor/vendor reference
  - task metadata

- `CreditAdjustment`
  - manual correction, vendor refund, promo credit, expiration, or reconciliation

The lifecycle can mirror budget holds:

```text
reserve -> consume
reserve -> release
reserve -> expire
```

## Product Boundary

For v1, keep the rule crisp:

```text
Hubu authorizes and accounts for spend.
External executors perform work.
Hubu Wallet moves money only when the requested action is actually a payment.
Credit pools track prepaid non-cash entitlements separately from money movement.
```
