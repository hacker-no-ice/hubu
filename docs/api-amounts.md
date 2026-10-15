# Amounts in the Hubu HTTP API

Hubu accounts every budget, hold, spend and settlement in **micro-units**:
units of 10⁻⁶ of the budget's currency (`usd` or `usdc`). Budgets are per
currency, and spend only counts against a budget in its own currency. This
page describes how amounts travel over HTTP. See
[ledger-accounting.md](ledger-accounting.md) for the accounting rules and
[ledger-history.md](ledger-history.md) for the history endpoints.

## Sending amounts

Send amounts as exact **decimal strings** with at most 6 decimal places, never
as JSON numbers:

| Route | Exact field | Deprecated field |
| --- | --- | --- |
| `POST /spend/authorize`, `POST /spend` | `amount` | `amount_cents` |
| `POST /budgets` | `amount` | `amount_cents` |
| `POST /budgets/{budget_id}/versions` | `amount_limit` | `amount_limit_cents` |
| `POST /user/spending-target` | `amount` | `amount_cents` |
| `POST /policies` (starter policy) | `daily_limit` | `daily_limit_cents` |

```json
{"account_id": "aga_…", "amount": "0.005", "currency": "usdc", "reason": "x402 call"}
```

Rules:

- `"5"`, `"5.1"`, `"0.005"` and `"0.000001"` are valid. More than 6 decimal
  places, signs, exponents, empty parts and values beyond the supported range
  are rejected with a message saying how to fix the value.
- Send the exact field **or** the deprecated field, never both.
- `currency` defaults to `usd` on spend and budget creation.

## Reading amounts

Responses carry an exact money object next to each legacy `*_cents` field:

```json
"consumed_amount": {"units": "3100", "scale": 6, "currency": "usdc", "display": "0.0031"},
"consumed_amount_cents": 1
```

- `units` is an integer string of 10^-`scale` of `currency`. Read it with
  integer or decimal arithmetic, never floating point.
- `display` is the exact adaptive form for people: 2 to 6 decimal places,
  trailing zeros trimmed, never rounded.
- The `*_cents` fields are **conservative projections** kept for one release.
  Amounts that use budget (consumed, frozen, holds, charges, overruns) round
  up; amounts that permit spend (limits, remaining, released) round down.
  A legacy reader therefore never sees more available budget than exists.

Budgets, budget versions, holds, spending targets and their warnings,
approval reviews, executor settlement receipts, authorization records,
ledger coverage and the watch projection all carry money objects. The
executor contract's own amount fields move to explicit units with contract
v4.5.

## Deprecation

A request that sends an amount **only** through a deprecated `*_cents` field
still succeeds, and its response carries:

```text
Deprecation: true
Link: <https://github.com/hacker-no-ice/hubu/blob/main/docs/api-amounts.md>; rel="deprecation"
```

Move to the exact fields; the `*_cents` request and response fields will be
removed after the deprecation window.
