# Demo Findings and Improvement Opportunities

These notes came up while adding the local CLI/demo surface. They are documented
here rather than folded into behavior changes.

## Findings

- The `hubu-api` binary existed but was an empty entry point, so the demo needed
  a thin local server adapter around the existing managers.
- The current policy engine can express an amount threshold, but it does not
  track calendar-day aggregate spend. The CLI keeps the requested
  `--daily-limit` flag for demo ergonomics and documents that it maps to a
  per-request limit today.
- Ledger records were writeable and transaction entries were readable by ID, but
  there was no public list operation for demo inspection. A read-only transaction
  listing method was added to support `hubu ledger list`.
- Registration, spend decisions, auth tokens, policies, and payments are all
  process-local in this demo path. The SQLite ledger also uses an in-memory
  database through the existing `SqliteLedger::in_memory` constructor.

## Improvement Opportunities

- Add persistent demo storage once the project is ready for repeatable demos
  across server restarts.
- Add first-class API DTOs near the core domain models if the HTTP surface grows
  beyond demo usage.
- Rename or extend the policy command when true daily budget aggregation exists,
  so CLI language and policy semantics match exactly.
- Add a production-grade HTTP server stack if Hubu's optional API becomes more
  than a local demo adapter.
- Add an explicit approval workflow if `needs_approval` should become an
  actionable state in live demos.
