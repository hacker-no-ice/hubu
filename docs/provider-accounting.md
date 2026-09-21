# Provider spend accounting

Hubu accounts for governed provider spending even when the provider bills the
user directly. The provider subledger is owned by Hubu core, in Hubu's database.
Gongbu and other executors submit the same settlement receipt contract; they do
not write this journal or access its database.

## Posting semantics

A confirmed settlement creates one immutable `provider_accounting_journal` row.
Each row represents a balanced pair exposed by the read-only
`provider_accounting_lines` SQL view:

```text
debit  provider_spend_expense
credit externally_billed_clearing
```

The clearing account records externally billed expense evidence. It does not
assert that Hubu holds cash, owes a provider payable, or processed a payment.
Wallet payments retain their existing debit-expense/credit-wallet-cash postings
in `ledger_transactions` and `ledger_entries`. They are not copied into the
provider journal. Wallet payment/budget atomicity is tracked separately in
[HUB-210](https://linear.app/hubu/issue/HUB-210).

The journal stores owner, agent and account, logical budget and authorization
version, operation, claim, settlement and provider receipt identities. It retains
the original receipt, pricing snapshot, artifact reference, and human
reconciliation evidence. Provider and billing merchant come from the authorized
execution scope when available; receipt provider and legacy merchant labels are
fallback evidence. Purpose comes from the persisted spend reason. Missing fields
are explicitly listed in `missing_evidence`; no merchant identity is invented.

Exact cost remains an integer coefficient, decimal scale and currency. Journal
expense deltas and rounding deltas are signed decimal **strings at scale 18**;
readers must use integer/decimal arithmetic, never floating point or SQLite REAL.
The line view exposes absolute amounts and reverses debit/credit for negative
adjustments. Zero-cost confirmed receipts retain a balanced zero-value pair.

Budget charges are separate cents values. For example, USD 0.001 creates expense
lines of 0.001, consumes one budget cent, and records a 0.009 rounding difference.
That rounding difference is a budget-control quantity, not additional provider
expense. The invariant for each original posting or adjustment is:

```text
budget_charge_delta_cents * 10^16 = expense_delta_amount + rounding_delta_amount
```

## Atomic settlement and recovery

One immediate SQLite transaction persists receipt, journal pair, token use,
claim finalization, hold settlement and consumed/remaining budget. A posting
failure rolls the entire transition back. An identical receipt retry returns the
existing settlement, including after response loss or process restart; a changed
receipt conflicts. Vendor-billed human reconciliation uses the same transaction
and may record incurred cost above the authorization or budget limit.

Release and vendor-not-billed reconciliation produce no provider expense.
Uncertain claims keep their hold frozen and stay pending reconciliation. Journal
absence for such a claim means **unresolved**, not evidence that the cost was zero.
Updates and deletes of journal rows are rejected by SQLite triggers. Representing
both accounting lines with one row makes partial line insertion impossible.

## Corrections

The supported core command is
`BudgetManager::adjust_provider_accounting(ProviderAccountingAdjustment)`.
There is no executor, HTTP, CLI or MCP correction endpoint in this change.
A trusted caller must authorize the human correction and supply the owner,
original entry, expected latest entry, stable correction operation key, corrected
total exact cost, reason and evidence.

The facade acquires its shared repository, appends the adjustment and changes the
budget in one transaction, validates the resulting budget state, then publishes
that committed state to its cache before releasing the lock. Storage mutation is
crate-private. An identical correction retry returns the original adjustment and
refreshes current budget state; changed input under the same key conflicts. A
stale predecessor conflicts, including concurrent corrections. Every adjustment
links to both the original entry and its immediate predecessor. Original journal
rows and settlement receipts never change.

The expense delta is corrected total minus the previous effective total. The
budget delta is the difference between the two independently rounded total
charges; it is **not** a separately rounded expense delta. A refund can reduce
this settlement to zero but cannot make its cost negative. A positive correction
accounts for already-incurred cost even on an exhausted, expired or revoked
budget, potentially making remaining budget negative. Frozen holds and budget
limits are unchanged. Overflow or inconsistent budget state rejects the entire
correction. Receipt retries after corrections return the immutable original
receipt and the current adjusted budget; they do not restore the original charge.

## Upgrade and legacy evidence

Opening Hubu's governance repository installs this schema and runs an idempotent
backfill in an immediate transaction. For each settled executor claim without a
journal, a matching receipt, hold and spend decision provide the original
identities and exact cost. The entry uses `settlement:<settlement_id>` as its
stable identity, preserves receipt time and evidence, and sets `legacy_backfill`.
Backfill **never changes consumed budget**, rewrites a receipt, or re-posts an
existing wallet entry. Legacy cents receipts have already migrated to exact
scale-2 cost through the executor receipt migration.

Missing or inconsistent linkage creates a `provider_accounting_legacy_gaps` row
keyed by claim ID with a reason, instead of fabricating a posting. Missing
optional provider/merchant/purpose fields remain explicitly incomplete on the
backfilled record. Genuine SQL/I/O errors fail startup so migration can retry
safely. Gap rows preserve the original migration observation; an operator must
inspect current journal coverage before interpreting an old gap as still open.
Do not repair history with UPDATE/DELETE or invent source evidence to make totals
match. Restore authentic missing evidence through a separately reviewed recovery
procedure; rerunning startup can then create the previously missing entry.

## Budget reconciliation and inspection

`SqliteGovernanceRepository::reconcile_provider_budget` reads provider charges
and budget balance in one SQLite snapshot. It returns consumed cents, summed
provider charge deltas (originals plus adjustments), and their difference as
`other_or_unaccounted_consumption_cents`. A provider-only, fully evidenced budget
reconciles to zero difference. Mixed wallet/provider usage and missing legacy
provider evidence require inspecting the other sources; the difference must not
be presented as fabricated provider expense or complete coverage.

This internal helper requires an owner-scoped provider entry for the budget.
It does not report budgets with no provider entries. Owner-authorized history
must enumerate those budgets separately and expose their missing coverage.
[HUB-34](https://linear.app/hubu/issue/HUB-34) owns that history and the unified
API/CLI/MCP ledger-list presentation. The existing `/ledger` wallet endpoint does
not yet show the provider subledger.

Implementation: [provider journal](../crates/hubu-core/src/persistence/accounting.rs),
[finalization transaction](../crates/hubu-core/src/persistence.rs),
[budget facade](../crates/hubu-core/src/budget/manager.rs), and
[wallet ledger](../crates/hubu-wallet/src/ledger.rs).
