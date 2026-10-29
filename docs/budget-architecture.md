# Budget administration boundary

`hubu_core::budget::BudgetManager` is the public service for supported single-budget
creation, total-limit updates, and revocation. Public models, typed commands,
results, and errors live under `hubu_core::budget`. HTTP handlers authenticate,
check ownership, parse public IDs, map DTOs, and present errors. CLI and unified MCP
continue to use those HTTP contracts.

The manager owns a [private coordinator](../crates/hubu-core/src/budget/coordinator.rs)
and [private state](../crates/hubu-core/src/budget/state.rs). The coordinator shares
the server's governance repository handle. Transports do not prepare repository
commands, acquire its lock for administration, or apply records to memory.
`BudgetUpdateService` has been removed. The version-append repository trait and
its request/result types are crate-private; raw budget saves exist only in
core test fixtures. Existing spend and hold persistence interfaces retain their
separate responsibilities.

`BudgetManager::new` and `from_records` can construct/query state, but cannot
perform administration until `with_repository` binds the shared repository.
Unconfigured create, update, and revoke calls fail without modifying memory.
Pure state tests use explicitly named, crate-private test helpers.

## Commit before publication

Each command holds the governance repository lock and uses a SQLite
`BEGIN IMMEDIATE` transaction. Creation reads the durable overlap constraints
inside that transaction, then writes the logical record, immutable revision 1,
current-version pointer, and initial balance together. Revocation reads the
current durable snapshot and updates only the logical administrative fields;
it cannot overwrite a newer version or balance with a stale cached value.

Updates retain revision CAS, immutable history, actor/source/reason provenance,
and exact retry fingerprints. Exact retries resolve the requested historical
edge before stale-head or availability rejection, returning the original applied
successor alongside the current head, even after later updates or revocation.

The transaction produces validated budget state, including complete history and
current holds. Only after commit succeeds does the coordinator publish this state
to the manager, while still holding the repository lock. Publication is infallible.
No separate post-commit reads can combine snapshots from different transactions.
Write errors, validation errors, and commit failures leave private memory unchanged.
This currently hydrates all budget records per admin command; it favors a simple
consistent snapshot over optimizing infrequent administration.

Create and revoke retain their existing retry contracts: create has no new
idempotency key and a repeated overlapping creation is rejected; repeated revoke
reports already revoked. Their committed state survives restart. Limit updates
continue to support exact historical replay.

## Lock and transaction order

For any flow requiring both state and persistence, acquire the budget-manager
mutex **before** the shared governance-repository mutex, then enter the SQLite
transaction. Release the transaction before publishing to memory and release the
repository mutex before the manager mutex. Never acquire the manager while holding
the repository lock, and never call a manager administration method from a scope
that already holds the repository lock: the mutex is not reentrant.

The server attaches the same `Arc<Mutex<SqliteGovernanceRepository>>` at startup.
Administration acquires its repository guard inside the coordinator. Existing
spend approval, executor claim/finalization, direct payment, and hold-expiry
workflows retain their budget-before-governance order and behavior. Outstanding
holds preserve their authorizing version and can finalize after revocation.
This change introduces no lifecycle states and no Hubu/Gongbu dependency or
storage-boundary changes.

See the [interactive budget diagram](../architecture/index.html) and
[public facade](../crates/hubu-core/src/budget/manager.rs) for ownership and source.
