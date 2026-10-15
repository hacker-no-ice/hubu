# Governance lifecycle evidence

Hubu's governance lifecycle contract separates an initial policy evaluation,
automatic admission, human intervention, authorization and financial resolution.
It provides typed facts and append-only storage in Hubu's existing governance
database. Gongbu retains its own execution database and provider authority.

This is the storage foundation delivered by HUB-291. Production capture,
aggregate queries, HTTP and unified MCP reads are separate follow-up work.
Installing this schema does **not** establish complete capture or make an empty
event table evidence that no requests were denied.

## Facts and identity

Facts distinguish policy `allow`, `deny` and `needs_approval`, automatic budget
decisions, actual escalation to a human, confirmed human resolution,
authorization grants, execution outcomes, settlement, safe release and
reconciliation entry/resolution. A policy allow or a human action alone does not
prove an authorization was granted or provider work succeeded. An internal
evaluation-pending placeholder is not a human escalation.

A confirmed human resolution requires the matching durable closure of the
immutable approval request: accepted approval is `allowed`, and accepted
decline is `denied`. An approval attempt that fails its downstream budget check
and leaves the request pending is not a confirmed resolution. Human resolution
and authorization grant remain separate recorded facts and counts; recording a
resolution never synthesizes a grant fact.

Each fact has an opaque public identity and an internal identity linking its
owner, agent, account, logical operation and immutable request revision. Replays
and repeated observations reuse the same semantic event identity. A conflicting
payload for that identity is an error; a corrected request is a separate
revision and never rewrites the original fact.

The store validates owner/agent/account relationships before accepting facts.
Private operation identity is for internal correlation and must not be
serialized into public API or MCP responses.

## Time, snapshots and eventual queries

The event's occurrence time records when the transition happened. Recording
time and a monotonic recorded sequence identify when it became visible in
Hubu's evidence store. Unknown historical times or evidence remain unavailable
rather than being inferred from the current configuration.

One recorded watermark bounds a snapshot. Later-recorded events stay outside
that snapshot even when their occurrence timestamp is older. Future query
implementations must use the same boundary for rows, whole-filter totals and
backlog.

Period counts use occurrence time. Pending approval and unresolved
reconciliation are backlog as of the snapshot and include older work. Recorded
sequence is a visibility boundary, not proof of causal order: a late observation
of an earlier open state must not undo a known later resolution.

Count immutable request revisions as attempts and distinct logical operations
separately. Lifecycle counts overlap: one attempt can be escalated, approved and
later enter reconciliation. Multiple matching deny rules do not multiply the
denied-attempt count. Later success or resolution does not erase earlier facts.

## Evidence and financial meaning

Structured evidence can retain the policy/rule or budget revision and safe
evaluated values used at the decision, with explicit unavailable fields. Budget
amounts use the shared exact `Amount` representation at accounting scale 6;
provider costs retain their separate exact receipt representation. Never
reconstruct an old available balance from today's budget.

A failed execution does not establish whether the provider billed. Confirmed
safe release, uncertain billing, and reconciled billed/no-bill outcomes are
distinct facts. Evidence capture and reads do not settle, release, retry or
change the underlying spend decision.

Public projections must allowlist fields and trusted identities. Do not expose
private operation keys, tokens, approval capabilities, credentials, prompts,
raw provider payloads or artifact URLs that grant access. Typed internal
evidence is not itself a public serialization contract.

## Persistence and historical coverage

Schema initialization is additive and idempotent. It does not rewrite old
immutable request/policy JSON, alter replay fingerprints, or backfill invented
transitions. Append/read primitives support future writer integration in the
same transaction as the authoritative transition, so an evidence append rolls
back with its parent write.

Coverage markers distinguish the installed schema from enabled capture for each
stream. Production writers must explicitly establish capture coverage when
they are integrated. Record completeness and explanation completeness are
separate: a confirmed denial may be countable while its historical threshold is
unknown. Missing record coverage never implies zero activity.

Implementation: [typed lifecycle contract](../crates/hubu-core/src/spend/lifecycle.rs)
and [lifecycle persistence](../crates/hubu-core/src/persistence/lifecycle.rs).
Existing public reads remain described by the
[ledger and authorization history contract](ledger-history.md).
