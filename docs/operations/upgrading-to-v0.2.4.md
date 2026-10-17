# Upgrading to v0.2.4: exact amounts

v0.2.4 moves every budget, hold, settlement and Gongbu quote from whole cents
to micro-units: 10⁻⁶ of the budget currency. A USD 0.001 call now consumes
exactly 0.001 of its budget instead of a whole cent, so 1,000 such calls fit a
USD 1.00 budget. USDC is a budget currency, and budgets are per currency.

The upgrade is a **one-way storage migration** and an **executor contract
bump**. Read this page before you upgrade a stack that holds budget data.

## Before you upgrade

1. Stop the stack: `hubu stack stop`.
2. Take cold backups of the Hubu and Gongbu state directories. Hubu also writes
   its own backup during the migration (see below); Gongbu does not.
3. Finish or note in-flight work. Claimed executions survive the upgrade and
   settle afterwards, but a clean stop is simpler to reason about.

## What happens on first start

**Hubu** migrates its database once, in one transaction:

- Before it changes anything, it copies the database to
  `<database>.pre-accounting-scale.bak` (for a managed stack,
  `state/hubu/hubu.sqlite3.pre-accounting-scale.bak`) and logs the path.
- It adds micro-unit columns to budget versions, balances, holds, spending
  targets and executor settlement receipts, and backfills them exactly from
  the cent columns (cents × 10⁴).
- It refuses to run if any stored amount exceeds USD 9,223,372,036,854.77,
  and it rolls back if any backfilled value or budget invariant does not
  match.
- Stored ledger postings, spend decisions and policy revisions are never
  rewritten. Old cent values are read exactly as cents × 10⁴.

**Gongbu** migrates its database the same way. It adds `authorized_units`,
`amount_units`, `settlement_units` and `units_scale` next to the `*_minor`
columns, backfills them in one verified transaction, and refuses amounts
beyond the micro-unit range. Pricing snapshots frozen before the upgrade keep
their whole-cent estimate byte for byte, so a settlement retry sends the same
payload as before.

Reopening either database after the migration changes nothing.

## What changes for clients

- **HTTP API.** Requests take exact decimal strings (`"amount": "0.005"`).
  Responses add money objects (`{"units": "5000", "scale": 6, "currency":
  "usd", "display": "0.005"}`) beside every `*_cents` field. The `*_cents`
  fields are now **deprecated, conservative projections**: amounts that use
  budget round up, and amounts that permit spend round down. A request that
  sends only deprecated `*_cents` fields still works, and its response carries
  `Deprecation: true` with a `Link` to [Amounts in the Hubu HTTP
  API](../api-amounts.md). The fields will be removed in a later release.
- **Policies.** Write `money: "0.005"`. The legacy `money_cents` form still
  loads. See [money values](../policy-engine.md#money-values).
- **CLI.** `--amount` takes up to 6 decimal places, amounts print exactly
  (`$0.035`), and `--round cents` shows rounded cents marked `≈`. See
  the [CLI reference](../cli.md).
- **Executors.** Every binary now reports and requires executor contract
  `hubu-spend-executor-v4.5`, where validate and claim send an exact
  `amount` with an explicit `scale`. Hubu keeps accepting v4.4 cent requests
  (no `contract_version`) during the transition. Managed stacks regenerate
  Gongbu's `expected_executor_contract`; a hand-written Gongbu configuration
  must change it to `hubu-spend-executor-v4.5`. See [v4.5 exact
  amounts](../spend-executor-contract.md#v45-exact-amounts).
- **Gongbu prices.** A fractional-cent catalog price is quoted, reserved and
  settled exactly (a 4.5-cent image is 0.045, not 5 cents). Gongbu still
  admits the price rounded up to a cent for callers that authorize whole
  cents, and releases the remainder.

## Historical consumption is not recomputed

Budgets keep the consumption they recorded before the upgrade. A pre-upgrade
USD 0.001 call that consumed a whole cent still shows as one cent consumed;
Hubu does not recompute historical charges from the exact receipts. The ledger
keeps each exact provider cost and its rounding delta, so the difference stays
visible in [ledger history](../ledger-history.md).

## Downgrading

Do not downgrade in place. v0.2.4 keeps writing the cent columns as
conservative projections, so an older binary reading a migrated database would
see no more available budget than v0.2.4 does. But it cannot represent
sub-cent amounts, v4.5 executor requests or USDC budgets, and the downgrade
path is not qualified. To go back, stop the stack and restore the cold backups
(or Hubu's `.pre-accounting-scale.bak`) taken before the upgrade. Anything
recorded after the upgrade is lost.
