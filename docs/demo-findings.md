# Demo Findings and Improvement Opportunities

These notes came up while adding the local CLI/demo surface. They are documented
here rather than folded into behavior changes.

## Findings

- The `hubu-api` binary existed but was an empty entry point, so the demo needed
  a thin local server adapter around the existing managers.
- The current policy engine can express an amount threshold, but it does not
  track calendar-day aggregate spend. The CLI now loads YAML policies through
  `hubu policy add --path`, so the demo wording can describe the rule as a
  per-request amount threshold directly.
- Ledger records were writeable and transaction entries were readable by ID, but
  there was no public list operation for demo inspection. A read-only transaction
  listing method was added to support `hubu ledger list`.
- The demo API now owns an in-process budget manager. Human-scoped budgets are
  reserved before payment; successful payments settle the hold into consumed
  balance, while failed payments release the hold back to available balance.
- Registration, spend decisions, auth tokens, policies, and payments are all
  process-local in this demo path. Budget state and the SQLite ledger are also
  in memory.

## Improvement Opportunities

- Add persistent demo storage once the project is ready for repeatable demos
  across server restarts.
- Add first-class API DTOs near the core domain models if the HTTP surface grows
  beyond demo usage.
- Add agent-scoped budget and ledger reads once the API exposes enough account
  metadata to filter those views cleanly.
- Add a production-grade HTTP server stack if Hubu's optional API becomes more
  than a local demo adapter.
- Add an explicit approval workflow if `needs_approval` should become an
  actionable state in live demos.
