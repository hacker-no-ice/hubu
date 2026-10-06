# Gongbu execution plane

Gongbu turns a Hubu spend authorization into durable provider work and safe
artifacts. It runs as a separate process with its own database, credentials,
provider configuration, Temporal worker, artifacts, readiness, and recovery.

Hubu remains the control plane. Sharing a repository and product release does
not turn the boundary into an in-process call or authorize either component to
open the other's database.

## Responsibilities

Hubu owns:

- policy and budget decisions;
- spend authorization and expiry;
- executor claims, settlement, release, and reconciliation; and
- financial audit state.

Gongbu owns:

- operator-controlled provider targets and pricing;
- provider credentials and calls;
- durable execution, provider-attempt, and safe asynchronous-operation records;
- Temporal workflows and activities;
- exact integer cost calculation, frozen pricing snapshots, and settlement
  evidence;
- normalized artifacts; and
- execution recovery.

The components communicate over
[`hubu-spend-executor-v4.4`](spend-executor-contract.md).

## Admission and execution flow

The canonical caller submits a Hubu spend-authorization token, execution
intent, and an opaque `target_id` discovered from `GET /v2/execution-targets`
to `POST /v2/executions`. Gongbu resolves the internal target tuple from its
operator-approved catalog.

For a new execution, Gongbu then:

1. Derives the provider, adapter, model, execution scope, normalized provider
   input, and price from its operator-controlled catalog. Provider-specific
   billable dimensions and the matching selector-qualified pricing rule are
   frozen at this boundary.
2. Resolves Hubu's read-only authorization snapshot, which is authoritative for
   account and agent attribution.
3. Requires Hubu's authorization to be available, unexpired, and backed by a
   frozen hold equal to the authorized amount; that amount and currency to equal
   the catalog price; and Hubu's typed execution scope to equal the scope
   Gongbu derived for the target. The `POST /v2/executions` request carries
   only the schema version, `spend_auth_token_id`, input, input schema
   version, and target ID; any other field, including an amount, currency,
   scope, claim ID, or `operation_key`, is rejected. Account, agent, lease
   profile, and expiry come only from Hubu.
4. Persists the `Execution` aggregate and immutable Hubu authorization snapshot
   before scheduling work.
5. Starts the stable Temporal workflow
   `gongbu-execution-{execution_id}` on the `gongbu-executions` task queue.
6. Claims the Hubu authorization from the durable workflow.
7. Creates exactly one `ProviderAttempt` before irreversible provider
   transmission.
8. For new asynchronous histories, uses Temporal's patch-protected
   `submit_provider` activity to submit once. After a successful FLUX submit,
   Gongbu checkpoints the safe provider request ID, operation ID, validated BFL
   polling host, and original absolute deadline in Gongbu SQLite before any
   long polling begins.
9. Uses `poll_provider_operation` to read that checkpoint and poll the existing
   operation. Activity or worker recovery performs status GETs for the same
   operation under the same deadline; it never sends a second generation POST.
   Before every poll or artifact fetch, Gongbu durably increments the matching
   provider-attempt counter; a failed counter write prevents the transport
   call. Synchronous adapters retain their existing one-activity behavior.
10. Normalizes artifacts and preserves exact provider cost, currency, decimal
   scale, and the complete frozen pricing snapshot.
11. Settles confirmed billable work, routes a cost above the authorized maximum
   to reconciliation with its evidence intact, or releases confirmed
   non-billable work.

Resolving authorization never claims it. Preview APIs are optional UX and are
never authority for admission or price. Gongbu recomputes from its active
catalog immediately before persistence.

An exact replay is different: Gongbu first looks up a persisted execution by
the opaque spend-auth token ID, validates the immutable execution request, and
returns or reschedules that local record without resolving Hubu again. This
keeps replay available after the token has been claimed or settled. A changed
immutable request conflicts, and an ambiguous legacy token reference fails
closed.

Diagnostic admission failures remain HTTP 400 `invalid_request` errors and may
add one bounded `reason_code`/`fields` pair. `target_not_selectable` identifies
`target_id`; alternatively,
`pricing_selector_not_matched` identifies `input.image_size`. The field names
identify contract locations only: Gongbu never echoes their values. Other
validation failures retain the generic error without diagnostic fields.

For either allowlisted diagnostic, Gongbu emits one
`gongbu_admission_rejected` JSON event on the first occurrence of that route
version and reason in each process. The event contains the static
`create_execution` route, route version, HTTP status, error code, reason code,
and field names. It never copies a request body, value, identifier, target
value, raw error, or unknown diagnostic into that event.

## Recovery and reconciliation

Execution identity (Gongbu's `hubu-decision:<decision_id>` operation identity),
its persisted account and agent snapshot,
provider-attempt identity, Hubu claim, and Temporal workflow ID remain stable
across recovery. New asynchronous workflow histories cross a Temporal patch
before running `submit_provider` and then `poll_provider_operation`; histories
that predate the patch retain their deterministic synchronous activity path.
Restarting or replaying the new path reuses the same `ProviderAttempt` and safe
SQLite operation checkpoint instead of creating a second provider call or
financial mutation.

A failure proven to occur before transmission remains non-billable and may
release the authorization. Once submission may have crossed the provider
boundary, Gongbu must establish the operation checkpoint before it can safely
continue. An interruption immediately before that checkpoint is ambiguous even
if the generation POST may have succeeded, so Gongbu records compact safe
reconciliation evidence and neither resubmits nor releases the claim. An
interruption immediately after the checkpoint resumes status GET polling for
the same operation and never resets the original absolute deadline.

An ambiguous provider or settlement outcome becomes
`reconciliation_required`. Gongbu does not blindly retry the provider call or
release Hubu's hold merely because a response was lost. Gongbu claims with
the execution's persisted Hubu authorization token and settles or releases by
the Hubu `claim_id`, following the v4.4 executor request identity rules in the
[spend executor contract](spend-executor-contract.md), including replaying the identical claim to recover a lost `claim_id`.
Gongbu never receives or sends Hubu's private operation key: it identifies each
new execution by Hubu's `decision_id` (`hubu-decision:<id>`), which also seeds
the vendor idempotency key. Executions persisted before v4.4 keep the identity
they were created with. With the persisted provider receipt, finalization
remains idempotent under repeated delivery.

The same rule applies when an exact vendor charge rounds conservatively above
the authorization. Gongbu persists the exact integer amount, scale, currency,
provider identifiers, and full frozen pricing snapshot before finalization,
and on Hubu's normal-settlement rejection routes the execution to
reconciliation with that evidence intact instead of repeating provider work or
discarding the legitimate bill. The human resolution path is defined in
[expired claims](spend-executor-contract.md#expired-claims).

## Execution timing

Execution responses include an additive, agent-safe `timing` projection. Gongbu
derives `execution_total_ms` from its durable execution boundaries and
`provider_interaction_ms` from the provider-attempt transmission and completion
boundaries that it owns. When both are available, `non_provider_ms` is their
checked difference. A missing, malformed, or non-monotonic boundary produces a
null duration instead of an estimate.

The projection contains elapsed durations only. It does not expose raw
provider-attempt identifiers or timestamps, and callers must not infer provider
time from how long an external observer sees the execution in `executing`.

Execution responses also expose
`provider_transport: { schema_version: 1, poll_count,
artifact_fetch_count }`. The counters are cumulative, restart-durable entries
into Gongbu-owned transport boundaries, not router polling estimates. A
pretransmission or terminal attempt cannot advance them.

## Provider targets, discovery, and pricing

Provider availability is an operator decision; selection among the available
targets is a per-request caller decision. `GET /v2/execution-targets` projects
only active, execution-enabled targets with an opaque stable `target_id`, safe
provider/model labels, the Hubu authorization scope, supported image-size
selectors, and exact configured price components. It never returns adapter
settings, credential references, endpoints, headers, configuration revisions,
or configuration digests.

The ID is stable across credential, endpoint, and provider-configuration
revision rotation for the same internal target key. A changed model or adapter
is a different logical target and therefore receives a new ID. Public callers
select only that ID and runtime inputs such as `image_size`; internal immutable
target keys remain persisted for durable replay.

`POST /v1/executions` is retired. The v1 GET execution, status, artifact, and
redaction-attestation routes remain observation surfaces.

A production target binds:

- workload type;
- provider, adapter, and model;
- typed execution scope;
- credential reference;
- pricing model and currency;
- maximum authorized spend; and
- whether live provider execution is explicitly enabled.

Agents cannot register or synthesize targets through discovery or execution.
Admission fails closed when target selection is unknown or ambiguous, price or
scope differs from Hubu authorization, a required credential is unavailable,
or the live-spend gate is incomplete.

Pricing and provider amounts never pass through floating point. Gongbu keeps
the exact rational catalog calculation and any exact provider-reported decimal
as checked integers. Hubu's single per-operation ceiling conversion to budget
cents is defined in
[precise external cost and budget conversion](spend-executor-contract.md#precise-external-cost-and-budget-conversion).

Provider credentials belong to Gongbu's runtime identity. They are never
accepted in execution requests, stored in repository records, included in
fixtures, returned by APIs, written to Temporal payloads, or emitted in logs and
errors.

Provider-specific behavior is owned by the provider contract pages. The
[FLUX.2 provider contract](operations/flux-provider-contract.md) covers the
managed `hubu.flux-2-pro.text-to-image/v1` target, certified output dimensions,
settled cost units, asynchronous transport and artifact delivery, and the
managed-FLUX `GET /v1/executions/{id}/redaction-attestation` endpoint; the
[Gemini provider contract](operations/gemini-provider-contract.md) covers the
synchronous Gemini Developer API targets.

## Temporal ownership

`gongbu-server` always owns its Temporal worker. It supports two service modes:

- `managed_local`: Gongbu starts and stops one pinned local Temporal child and
  retains its data across ordinary restart.
- external: Gongbu connects to an independently operated Temporal service and
  never assumes lifecycle authority over it.

Gongbu readiness requires the selected Temporal service and a polling worker.
Losing either closes new execution admission while preserving inspection and
recovery state.

The patch-protected provider activities exchange only the `execution_id` and a
small phase enum through Temporal. Durable normalized input, provider-attempt
identity, and the asynchronous operation checkpoint remain in Gongbu SQLite;
activities reload them at execution time. The checkpoint allowlists only the
safe request ID, operation ID, validated polling hostname, and original
absolute deadline. Credentials, raw provider bodies, complete polling URLs,
signed artifact URLs, and storage paths have no representation in Temporal
payloads or the operation checkpoint. Activities resolve credentials and
reconstruct provider requests at execution time.

## Artifacts

Providers never choose final storage keys or write directly into the configured
artifact root. All bytes pass through Gongbu's normalized artifact service,
which validates supported media, computes stable metadata and hashes, and
persists storage-neutral artifact identities.

API and MCP responses expose safe artifact IDs, media type, size, and digest.
They never expose an absolute filesystem path or internal storage key.

## Service surface

The persistent server exposes:

- liveness, readiness, and version metadata;
- versioned execution creation and inspection;
- artifact listing and retrieval; and
- authenticated operator diagnostics.

Agents normally reach this surface through
[`hubu-unified-mcp`](unified-mcp.md). The router forwards Gongbu calls using
only the Gongbu endpoint and installation-scoped bearer credential. The
capability carries no account or agent claim. One installation caller can
retrieve known executions and their artifacts across the owner's agents, but
there is no owner-wide browse/list promise: access remains by known execution
or artifact ID. This local trust model does not provide strong multi-user or
per-agent isolation.

For local startup, shutdown, backup, and troubleshooting, use
[Gongbu server operations](operations/gongbu-server.md). For deterministic
execution use the [sandbox](operations/gongbu-sandbox.md); for billable targets,
use the shared [live provider operations](operations/live-providers.md) guide.

The implementation lives in [`crates/gongbu-api`](../crates/gongbu-api).

On repository open, Gongbu independently migrates legacy v4.3 minor-unit
attempt and receipt amounts to exact amounts with scale 2, keeping a
lost-response retry immutable and idempotent; neither Hubu nor Gongbu opens or
migrates the other's state. The exact mapping is defined in
[persistence migration and v4 compatibility](spend-executor-contract.md#persistence-migration-and-v4-compatibility).
