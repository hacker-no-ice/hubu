# Amounts in the Hubu HTTP API

Hubu accounts every budget, hold, spend and settlement in **micro-units**:
units of 10⁻⁶ of the budget's currency (`usd` or `usdc`). Budgets are per
currency. See [ledger-accounting.md](ledger-accounting.md) for accounting rules
and [ledger-history.md](ledger-history.md) for history endpoints.

## Sending amounts

Send exact decimal strings with at most 6 decimal places, never JSON numbers:

| Route | Field |
| --- | --- |
| `POST /spend/authorize`, `POST /spend` | `amount` |
| `POST /budgets` | `amount` |
| `POST /budgets/{budget_id}/versions` | `amount_limit` |
| `POST /user/spending-target` | `amount` |
| `POST /policies` (starter policy) | `daily_limit` |

```json
{"account_id": "aga_…", "amount": "0.005", "currency": "usdc", "reason": "x402 call"}
```

Values such as `"5"`, `"5.1"`, `"0.005"` and `"0.000001"` are valid. More than
6 decimal places, signs, exponents, empty parts, zero, and values beyond the
supported range are rejected. Currency defaults to `usd` on spend and budget
creation. Wallet payments require whole US-dollar cents; provider authorizations
support sub-cent amounts.

The deprecation window has ended. Legacy `amount_cents`, `amount_limit_cents`
and `daily_limit_cents` inputs are rejected, including when an exact field is
also present. Responses no longer emit a `Deprecation` header.

## Reading amounts

Responses use exact money objects:

```json
{"consumed_amount": {"units": "3100", "scale": 6, "currency": "usdc", "display": "0.0031"}}
```

`units` is an integer string of 10^-`scale` of `currency`. Use integer or decimal
arithmetic. `display` is the exact adaptive form for people, with 2 to 6 decimal
places and no rounding. General HTTP, CLI JSON and MCP responses no longer carry
legacy cent projections. Native artifact and gallery receipts carry
`budget_charge` as the same exact money object. Existing on-disk gallery
manifests remain readable. CLI `--round cents` remains an explicit display option.

## Persistence and compatibility

Fresh databases use the final constrained µ schema directly. On the first
open of an existing v0.2.4/v0.2.5 µ profile, Hubu and Gongbu atomically rebuild
their dual-written µ tables,
copying authoritative µ values unchanged and dropping legacy budget `*_cents`
and executor `*_minor` columns. Database constraints require integer µ values,
scale 6, positive budget limits and holds, and nonnegative consumed/frozen
amounts. Remaining budget may be negative after a confirmed overrun. Invalid
rows abort the rebuild; the original tables remain intact. No live profile
migration is needed during development or validation. Pre-µ profiles are
rejected before writes and must first upgrade through v0.2.4 or v0.2.5; the
older cents-to-µ migration chain is retired.

Stored policy `MoneyCents` payloads remain supported so hashed revisions do not
change. Legacy ledger metadata remains readable. Executor v4.4 wire
compatibility is retained until its separate retirement (HUB-299), including
its authorization `amount_cents` assertion and projection. See
[spend-executor-contract.md](spend-executor-contract.md).
