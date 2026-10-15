# Governance lifecycle evidence

Hubu's governance lifecycle contract separates an initial policy evaluation,
automatic admission, human intervention, authorization and financial resolution.
It provides typed facts and append-only storage in Hubu's existing governance
database. Gongbu retains its own execution database and provider authority.

HUB-291 supplies the storage foundation. HUB-292 captures new policy evaluations
and automatic admission/authorization outcomes. HUB-293 captures genuine human
escalation and confirmed resolution. HUB-294 captures authoritative Hubu claim
and financial-resolution facts. HUB-295 provides shared owner-scoped queries;
HTTP and unified MCP reads follow separately.
Installing the schema alone does **not** establish complete capture or make an
empty event table evidence that no requests were denied.

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

## Time, snapshots and counts

The event's occurrence time records when the transition happened. Recording
time and a monotonic recorded sequence identify when it became visible in
Hubu's evidence store. Unknown historical times or evidence remain unavailable
rather than being inferred from the current configuration.

Where the authoritative transition has a durable timestamp, occurrence time
must match it: policy creation, terminal admission/human outcome acceptance,
authorization acceptance, claim start, finalization and reconciliation resolution
use their source records. Explicit voids use the matching void timestamp;
persisted expiry releases use the hold's release update time, rather than its
earlier expiry eligibility deadline. Human escalation uses decision creation;
financial reconciliation opens at the stored claim expiry. Technical admission
failures retain their observation time.

One recorded watermark bounds a snapshot. Later-recorded events stay outside
that snapshot even when their occurrence timestamp is older. Shared queries
use that boundary for rows, whole-filter totals, backlog and coverage.

Period counts use occurrence time. Pending approval and unresolved
reconciliation are backlog as of the snapshot and include older work. Recorded
sequence is a visibility boundary, not proof of causal order: a late observation
of an earlier open state must not undo a known later resolution.

Count immutable request revisions as attempts and distinct logical operations
separately. Lifecycle counts overlap: one attempt can be escalated, approved and
later enter reconciliation. Multiple matching deny rules do not multiply the
denied-attempt count. Later success or resolution does not erase earlier facts.

## Shared reads

Core `governance_list`, `governance_summary` and `governance_detail` use one
read model. The authenticated owner supplies scope; public agent/account
selectors are validated together, including when no records match. Fact-category
filters select lifecycle stages, rather than a mutable latest operation status.
Occurrence windows include their start and exclude their end.

A snapshot binds normalized filters to an immutable owned journal boundary.
Its captured-evidence time comes from that stored boundary, rather than a
client-supplied clock. An empty journal has no captured-time anchor. Reuse the
snapshot across summary, list and detail; keyset pages use parsed occurrence
instants with a stable tie-breaker. Full-filter totals never depend on page size.

Pending approval and open financial reconciliation fold all visible openings
and closures, including pre-period work. Their scope is the selected identities
at captured-evidence time; report-window and stage filters do not hide closures
or remove older unresolved work. Known closure dominates a late opening.

Counts distinguish records, immutable attempts and logical operations. Stage
counts overlap. Automatic denial reasons count the attempt once; human declines
remain separate. Requested amounts use checked wide sums per currency, including
sub-cent values. A blocked request total is not provider cost, budget consumption
or savings; unknown request amounts are reported separately.

Record coverage and explanation coverage remain separate. Enabled sources do
not imply support for every category or complete earlier history. The independent
observer stores grouped legacy-gap presence in the same append-only journal;
these observations never become enumerable lifecycle events or main totals.
Queries use the latest observation within their watermark. Absent observations
remain unknown, and broader/unattributed gaps cannot establish a narrower
account's activity. Inventory describes identity-scoped history, not a filtered
report period; its stored observation time is not a polling heartbeat.

Reads neither collect evidence nor mutate financial state or contact providers.
Public projections allowlist safe fields and leave money formatting to the
transport. HTTP and unified MCP expose this model in subsequent tasks.

## Automatic capture

The API carries the selected policy payload and its public resource, revision
and hash as one immutable snapshot. Core writers derive facts from that snapshot
and the typed admission branch; they do not accept arbitrary API-supplied facts
or recover a denial cause from prose. Direct core callers without a selected
resource retain explicit unavailable policy provenance.

Policy evaluation, terminal denial and automatic authorization each commit their
facts with the authoritative transition. Reservation evidence preserves the
budget version and balance before reservation and checks that snapshot against
SQLite inside the write transaction. A failed append rolls back its parent
transition. Exact replay does not regenerate facts, timestamps, explanations or
coverage from current configuration.

Each integrated writer declares its own owner capture coverage. Uninvolved
streams stay undeclared, and source activation does not establish support for
every possible fact. Policy and human history are not backfilled; financial
capture has its own source and eligibility rules below.

## Human capture

A genuine immutable `needs_approval` decision records escalation with its policy
fact. The earlier evaluation-pending row does not. Confirmed decline commits
with the denied outcome; confirmed approval and its separate authorization grant
commit with the token, reservation and allowed outcome. Failed budget acquisition
or evidence persistence leaves the approval pending without a confirmed outcome.

Human resolution retains the original policy result and captured policy
reference. Approval uses its own reservation-time budget snapshot. Exact replay,
repeated resolution and restart add no facts, and conflicting resolutions cannot
replace an accepted decision. Resolving legacy work does not manufacture an
uncaptured earlier escalation.

## Evidence and financial meaning

Structured evidence can retain the policy/rule or budget revision and safe
evaluated values used at the decision, with explicit unavailable fields. Budget
amounts use the shared exact `Amount` representation at accounting scale 6;
provider costs retain their separate exact receipt representation. Never
reconstruct an old available balance from today's budget.

Decisive rule IDs are a safe partial subset of uniquely identified, matched
rules with the winning stored initial policy effect. Known amount constraints
must agree with the stored condition snapshot, immutable policy comparison and
actual request amount. Direct comparisons and positive `all` conjuncts are
supported; `any`, `not`, ambiguous IDs and missing legacy condition snapshots
remain absent or unavailable instead of becoming a guessed threshold.

A failed execution does not establish whether the provider billed. Confirmed
safe release, uncertain billing, and reconciled billed/no-bill outcomes are
distinct facts. Evidence capture and reads do not settle, release, retry or
change the underlying spend decision.

Public projections must allowlist fields and trusted identities. Do not expose
private operation keys, tokens, approval capabilities, credentials, prompts,
raw provider payloads or artifact URLs that grant access. Typed internal
evidence is not itself a public serialization contract.

## Financial capture

Claim acceptance, settlement, confirmed release and eligible unclaimed expiry
record facts with their canonical financial transitions. Claim acceptance proves
that the executor acquired its lease; it does not prove provider work began.
A release does not by itself establish a provider failure or a billed/no-bill
reconciliation result.

Financial reconciliation opens at the stored expiry of an unresolved Hubu claim.
The writer verifies historical eligibility; claims finalized before expiry never
open this backlog. Confirmed reconciliation commits its matching open and close
with financial finalization, even if the collector has not observed expiry yet.
A late open cannot undo a known closure.

An independent collector observes expired claims on server startup and normally
every 30 seconds. It records evidence without changing financial state, and reads
do not trigger collection. Downtime, contention or write failures can delay
capture. Collection cost also grows with stored history and unresolved backlog;
the interval is not a latency guarantee. A provable existing expired claim may be observed after activation with
an older occurrence time; this does not establish complete earlier history or
invent its uncaptured original transitions.

The current executor contract does not supply durable provider-failure or
pre-expiry Gongbu uncertainty history. Those categories remain unsupported
coverage, rather than inferred facts. Gongbu recovery before expiry remains
separate from Hubu's expired financial-claim backlog.

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
Automatic writers live in [transactional capture](../crates/hubu-core/src/persistence/automatic_lifecycle.rs).
Human writers live in [confirmed resolution capture](../crates/hubu-core/src/persistence/human_lifecycle.rs).
Existing public reads remain described by the
[ledger and authorization history contract](ledger-history.md).

Financial writers live in [claim and reconciliation capture](../crates/hubu-core/src/persistence/financial_lifecycle.rs); the API owns the [independent observer](../crates/hubu-api/src/financial_observer.rs).

Shared reads: [query types](../crates/hubu-core/src/spend/governance_query.rs),
[query evaluator](../crates/hubu-core/src/persistence/governance_query.rs), and
[legacy coverage observation](../crates/hubu-core/src/persistence/lifecycle_coverage.rs).
