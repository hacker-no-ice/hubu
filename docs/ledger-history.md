# Ledger and spend history

Hubu exposes one owner-scoped history model through HTTP, the CLI, and unified
MCP. These are read operations: they never authorize, claim, settle, release,
correct a posting, or execute a provider request. Provider settlement writes the
[canonical ledger](ledger-accounting.md) before history reads it.

## Inspect a selected agent's budget

```sh
hubu ledger list --agent-id agt_EXACT_AGENT_ID --budget-id bgt_EXACT_BUDGET_ID --limit 20
hubu ledger list --agent-id agt_EXACT_AGENT_ID --budget-id bgt_EXACT_BUDGET_ID --limit 20 --cursor CURSOR_FROM_PREVIOUS_PAGE
```

The equivalent endpoint is `GET /ledger/transactions` with `agent_id`,
`budget_id`, `limit`, and optional `cursor` query parameters. The MCP tool
`hubu_list_ledger` takes the same parameter names. Omit filters for the complete
owner ledger, or select `agent_id` and/or `account_id`. A budget selection
requires its agent explicitly; foreign IDs and mismatched agent/account/budget
pairs fail even when there are no transactions. No caller-supplied owner selector
is accepted.

Each response has `schema_version: "hubu-history-v1"`, `transactions`, `coverage`,
and `next_cursor`. Each transaction retains its canonical ID and exact debit and
credit entries, source kind, public agent/account and budget/version IDs when
supported by authentic ownership evidence, public workflow and settlement links,
and correction predecessor/original IDs. Costs use a coefficient **string**,
scale and currency: `{"amount":"1","scale":3,"currency":"USD"}` means USD
0.001. Do not convert these values through floating point.

`effective_cost` is the original total on an original posting and the **corrected
total** on an adjustment (`cost_semantics` identifies which). Do not sum original
and corrected totals. Entries express the actual signed expense change through
account and debit/credit direction. `budget_charge_delta_cents` is a separate
signed control charge; `rounding_delta_amount` is its difference from the signed
expense at scale 18. A USD 0.001 provider expense consumes one budget cent and
has a rounding difference of USD 0.009.

For a selected budget, `coverage` reports full-budget consumed cents, recorded
signed charges, their difference (`unaccounted_consumption_cents`), pending-hold
count, currency, and an owner-wide count of transactions lacking trustworthy
budget context. These totals come from the **whole budget**, not the current
page. Budget attribution requires the decision’s actual hold to match both the
budget and version; inconsistent historical links remain unassigned. Expired
unclaimed holds are projected as expired and excluded from the pending count.
Claimed holds remain pending until finalization, including expired claims that
require reconciliation; history reads never mutate the stored hold. A valid unused budget has an empty list and zero totals. Owner-wide and
agent-wide reads return `coverage: null` rather than adding unlike currencies or
claiming aggregate reconciliation. Unknown historical attribution remains
unassigned. Historical wallet reconciliation is explicitly incomplete; wallet
atomicity and remaining legacy wallet validation are deferred in HUB-210.

## Discover authorization workflows

```sh
hubu spend history --agent-id agt_EXACT_AGENT_ID --status settled --limit 20
hubu spend history --account-id aga_EXACT_ACCOUNT_ID
hubu spend show --workflow-id PUBLIC_DECISION_UUID
```

`GET /spend/workflows` accepts `agent_id`, `account_id`, `status`, `limit`, and
`cursor`. Unified MCP exposes `hubu_list_spend_workflows` with those same fields.
The list returns `schema_version`, `workflows`, and `next_cursor`. Discovery
selects the latest authorization revision for each independent agent-scoped
operation. Two provider operations remain two workflows even if they came from
one higher-level task; a safe `task_reference` correlates their common task ID.
Denied-request history is intentionally excluded.

Statuses are `needs_approval`, `authorized`, `claimed`, `settled`, `released`,
`expired`, `reconciliation_required`, and `unknown`. Missing historical authorization
outcome evidence is shown as `unknown`, never inferred as allowed. `decision`
reflects the latest authorization outcome, including human approval; the
immutable initial policy evaluation is separately `policy_decision`.

A workflow shows the public decision identity, authorized maximum, currency,
trusted provider/billing identity when known, hold and budget version, claim
lease timestamps and terminal status, receipt with exact provider cost and
rounded budget charge, linked ledger transaction IDs, and reconciliation outcome
and evidence references. Release does not invent a receipt or expense posting.

`GET /spend/workflows/show?workflow_id=PUBLIC_DECISION_UUID` and MCP
`hubu_get_spend_workflow` inspect a known public workflow. Trusted HTTP/CLI callers
may alternatively look up `agent_id` plus their existing private `operation_key`:

```sh
hubu spend show --agent-id agt_EXACT_AGENT_ID --operation-key EXISTING_PRIVATE_KEY
```

This read does not generate an operation key. MCP accepts only `workflow_id` for
lookup; it neither accepts nor returns private operation keys. Query values use
standard URL percent encoding. Do not put trusted private-key lookup URLs in
shared screenshots or shell evidence; use the public workflow ID for sharing.

## Pagination, evidence and compatibility

Both list endpoints order newest first by creation timestamp and immutable ID.
The default page size is 50; the maximum is 100. Return `next_cursor` unchanged
with the same filters. Cursors bind the owner, resource kind and query filters,
and carry a first-page upper bound; changing filters requires a new first page.
Newer postings do not displace subsequent ledger pages. Each request uses one
SQLite snapshot for postings, workflows, receipts and balances. Pages are not a
persistent historical snapshot: workflow statuses, newly backfilled older
records and budget totals can change between requests. Restart from the first
page to refresh. Store the schema version and cursor with dogfood evidence.

Public projections never serialize raw stored metadata, operation keys,
authorization tokens, provider credentials, pricing JSON or capability-bearing
artifact URLs. Arbitrary source, purpose, provider-request, artifact, pricing and
reconciliation strings become stable SHA-256 evidence references with
`redacted: true`; they support correlation without becoming a credential or a
fetchable artifact URL. Provider/billing display identities come only from the
trusted scope catalog. Missing-evidence labels are allowlisted. This API does
not offer raw-secret retrieval or artifact download.

`GET /ledger` remains the original wallet-only compatibility response. The
existing CLI `hubu ledger list` and MCP `hubu_list_ledger` now use the canonical
history response so they also show external-provider settlement. Consumers of
the old CLI table or old MCP wallet shape must adopt `hubu-history-v1` JSON.

Implementation: [public projections](../crates/hubu-api/src/history.rs),
[consistent owner snapshot](../crates/hubu-core/src/persistence/history.rs),
[CLI](../crates/hubu-cli/src/main.rs), and
[unified MCP adapter](../crates/hubu-unified-mcp/src/hubu/routing.rs).
