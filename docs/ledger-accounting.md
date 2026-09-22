# Ledger accounting

The `hubu-ledger` domain owns accounting identities, exact amounts, balanced
entries, account semantics and immutable source evidence. Wallet payments and
externally billed provider expenses use **one** `ledger_transactions` /
`ledger_entries` store. A budget is linked control context, not the identity or
owner of a ledger. No duplicate provider expense table exists.

`TransactionRecord` and `TransactionEntry` are the canonical typed model.
Transaction IDs do not depend on a budget. Agent/account links remain available
without a budget; `GovernedSpendContext` adds budget/version, decision and
operation links when Hubu governs a spend. All governed provider and wallet
spends carry that context. Source kind distinguishes wallet payments, external
provider expenses and adjustments. `ledger_transaction_metadata` holds immutable
source evidence and control context keyed by the existing transaction ID.

## Account semantics and precision

| Source | Debit | Credit | Meaning |
| --- | --- | --- | --- |
| Wallet payment | Agent spend expense | User wallet cash | Confirmed wallet payment |
| External provider | Agent spend expense | Externally billed clearing | Provider billed outside Hubu |
| Provider correction | Original accounts, reversed for a negative delta | Original accounts | Append-only correction of external billing evidence |

External clearing does not assert that Hubu holds cash, owes a provider payable,
or processed a payment. Corrections do not invoke a payment rail. The current
ledger correction command **rejects wallet originals**: wallet refunds or
additional charges require their own confirmed payment workflow.

Canonical entries retain exact coefficient strings, scale (0–18) and currency.
Read them using integer/decimal arithmetic, never floating point or SQLite REAL.
Provider entries and corrections use scale 18; legacy wallet entries retain their
exact scale-2 cents. Zero-cost confirmed provider receipts create balanced
zero-value lines. Balance checks use checked integer arithmetic; account owner
and currency must match the transaction's entries.

Budget charges remain separate metadata. USD 0.001 creates expense lines of
0.001, consumes one budget cent, and records a 0.009 rounding difference. That
rounding is a budget-control quantity, not provider expense. For each governed
posting or adjustment:

```text
budget_charge_delta_cents * 10^16 = signed expense delta at scale 18 + rounding_delta_amount
```

The original exact receipt, provider request ID, pricing snapshot, artifact
reference and human reconciliation evidence remain attached to provider
transactions. Wallet source evidence includes payment, token and rail references.
Missing historical evidence is explicit instead of inferred.

## Settlement and corrections

Provider settlement writes its canonical transaction, two entries, metadata,
receipt, token use, claim finalization, hold settlement and budget consumption in
one immediate SQLite transaction. A failure rolls everything back. Identical
receipt replay returns the original settlement across response loss, concurrency
and restart. Changed receipts conflict. Vendor-billed reconciliation uses the
same transaction and may record already-incurred cost above the authorization or
budget limit. Release and vendor-not-billed outcomes create no expense. Uncertain
claims remain frozen and pending reconciliation.

The application supplies authoritative governed context to wallet payment
posting. Evidence and entries commit together; wallet IDs and account semantics
are preserved. The existing wallet payment/budget lifecycle is still a sequence
of writes; [HUB-210](https://linear.app/hubu/issue/HUB-210) owns making that complete
lifecycle atomic. This change does not claim that gap is solved.

`hubu_core::ledger::LedgerService::adjust` owns the trusted human-authorized
provider correction command. It accepts owner, original and expected previous
transaction IDs, stable correction key, corrected total exact cost, reason and
evidence. There is no public HTTP/CLI/MCP correction endpoint. The caller holds
the budget-manager lock; the ledger service obtains its associated shared
repository, commits accounting and budget changes, and publishes validated
committed state through BudgetManager before releasing the repository lock.
BudgetManager owns its state and publication boundary, not the accounting API.

Corrections append links to both original and immediate predecessor. Existing
entries, receipts and metadata cannot be rewritten. Exact replay returns the
same correction; changed replay or a stale predecessor conflicts. The expense
delta is corrected total minus previous effective total. Budget delta is the
difference between the rounded totals, not the rounded delta. Corrections cannot
make this expense negative; positive corrections can make remaining budget
negative, even on a revoked or expired budget. Frozen holds and limits remain
unchanged. Receipt replay after a correction returns the original receipt and
current budget, without restoring the original charge.

## Deployed wallet migration and legacy coverage

Schema upgrade is transactional. Existing wallet transaction, entry and account
IDs, original cents, external references and timestamps survive unchanged.
Exact amount columns are materialized from authentic cents at scale 2. Provider
entries have NULL in the legacy `amount_cents` projection; the old API never
silently rounds them. If migration fails, the original tables, rows and immutable
triggers remain intact. Permanent update/delete protection applies to canonical
transactions, entries and metadata.

Provider backfill uses matching settled claim, used token, decision, settled hold
and receipt evidence. It creates one transaction under the stable settlement
source key without changing consumed budget. A per-record savepoint prevents a
skipped evidence error from leaving partial postings; actual SQL/write failures
abort the migration.

Historical wallet postings are **not copied or reposted**. Backfill appends
metadata only when one consistent payment-attempt/token/decision/hold chain proves
owner, agent/account, budget/version and amount. Both debit and credit must have
the correct amount, currency, account ownership and expense/cash semantics.
Incomplete or inconsistent linkage is recorded in `ledger_legacy_gaps`; the
original owner-scoped transaction remains readable without fabricated context.
Backfill is idempotent, and never changes balances. Gap observations are retained;
operators must check current metadata before treating an old observation as still
unresolved. Restore authentic evidence through a reviewed recovery process; never
edit posted amounts or invent evidence to make totals match.

## Typed owner/agent/budget inspection

`LedgerService::transactions_for_budget(repository, BudgetLedgerQuery)` accepts
owner, agent and budget IDs. It authorizes through the registered agent-account
and budget relationship, even when the budget has no ledger rows. Unknown or
foreign owner/agent combinations fail before transactions are returned.

A single SQLite snapshot returns wallet, provider and adjustment records linked
to that exact agent budget, consumed cents, summed signed budget charges,
unaccounted consumption, pending-hold count and the count of owner transactions
without budget context. Unlinked owner transactions are not guessed into a
budget. An unused owned budget returns an empty list and zero consumed/covered
amounts. Missing legacy source evidence remains visible on canonical records.

For example, a wallet expense of USD 10.00 and a provider expense of USD 0.001
produce two canonical transactions, exact expense of USD 10.001, budget charges
of 1001 cents and rounding of USD 0.009. Append-only provider corrections appear
in the same query with signed budget deltas.

The old `hubu_wallet::LedgerTransaction` / `LedgerEntry` types are explicitly
cent-only compatibility projections re-exported from the ledger crate. Existing
`/ledger` behavior remains wallet-only. [HUB-34](https://linear.app/hubu/issue/HUB-34)
owns exposing the full ledger and owner/agent/budget filters through API, CLI and
MCP. The typed internal query is implemented here; those public filters are not.

Implementation: [ledger domain](../crates/hubu-ledger/src/domain.rs),
[ledger facade](../crates/hubu-core/src/ledger.rs),
[governance adapter](../crates/hubu-core/src/persistence/accounting.rs), and
[wallet payment adapter](../crates/hubu-wallet/src/payment.rs).
