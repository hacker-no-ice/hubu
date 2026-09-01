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
[`hubu-spend-executor-v4.3`](spend-executor-contract.md).

## Admission and execution flow

The canonical caller submits a Hubu spend-authorization token plus execution
intent and either a `target_id` discovered from `GET /v2/execution-targets` or
the legacy explicit target tuple to `POST /v2/executions`.

For a new execution, Gongbu then:

1. Derives the provider, adapter, model, execution scope, normalized provider
   input, and price from its operator-controlled catalog. Provider-specific
   billable dimensions and the matching selector-qualified pricing rule are
   frozen at this boundary.
2. Resolves Hubu's read-only authorization snapshot, which is authoritative for
   account and agent attribution.
3. Requires exact agreement on the operation key, amount, currency, lease
   profile, expiry, and typed execution scope, and accepts account and agent
   only from Hubu.
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
`workload_type`, `provider`, `adapter`, and `model`; alternatively,
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

Execution identity, its persisted account and agent snapshot, operation key,
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
release Hubu's hold merely because a response was lost. Finalization uses the
persisted execution agent and provider receipt and remains idempotent under
repeated delivery.

The same rule applies when an exact vendor charge rounds conservatively above
the authorization. Gongbu persists the exact integer amount, scale, currency,
provider identifiers, and full frozen pricing snapshot before finalization.
Hubu's normal-settlement rejection leaves the hold claimed; Gongbu retains the
exact provider-attempt cost, frozen snapshot, and evidence and routes the
execution to reconciliation instead of repeating provider work or discarding
the legitimate bill. Hubu permits the human billed resolution after the claim
lease expires.

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
revision rotation for the same workload/provider/adapter/model key. A changed
model or adapter is a different logical target and therefore receives a new
ID. New callers select that ID and runtime inputs such as `image_size`; the
legacy raw tuple remains accepted for compatibility but cannot be combined
with `target_id` in one request.

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
as checked integers. Hubu converts the final exact cost to budget cents once by
ceiling, so any positive fractional-cent charge consumes at least one cent.
Each operation keeps its own receipt and conversion; two independent provider
settlements are never pooled before rounding.

Provider credentials belong to Gongbu's runtime identity. They are never
accepted in execution requests, stored in repository records, included in
fixtures, returned by APIs, written to Temporal payloads, or emitted in logs and
errors.

The guarded HUB-172 workflow additionally has one authenticated, bodyless
read-only attestation endpoint at
`GET /v1/executions/{id}/redaction-attestation`. It accepts only the exact
successful frozen FLUX tuple and clean one-authorization-snapshot,
one-claim-reference, one-attempt, one-artifact, one-receipt path. Gongbu
revalidates the stored artifact bytes, resolves the currently registered key
only after all fixed checks pass, and exact-matches it against named
Gongbu-owned projections. A match fails closed. The response is an allowlisted
set of booleans, counts, money facts, content digest, and canonical component
hashes; it exposes no IDs, timestamps, coordinate, secret-derived hash, provider
body, URL, or storage location and performs no provider work.

The managed-stack contract `hubu.flux-2-pro.text-to-image/v1` binds the exact
FLUX target, certified preset dimensions, three dated rational USD prices,
poll/artifact/recovery policies, zero generation retries, and no fallback.
Gongbu's production validator compares the rendered schema-v3 binding, target,
and pricing against that shipped contract before serving. The sanitized catalog
reports validation and Keychain-reference presence separately from live
qualification; validation never calls BFL. See the
[managed FLUX.2 profile runbook](operations/managed-flux-profile.md).

### FLUX settled cost units

Black Forest Labs defines settled generation cost as the top-level numeric
`cost` field in its
[Get Result response](https://docs.bfl.ai/api-reference/utility/get-result),
not as `result.cost`. A missing or null top-level field means the response did
not provide settled-cost evidence; an undocumented nested-only value is ignored.
A malformed, negative, overflowing, or excessively precise top-level value
fails closed into reconciliation with the provider request and operation
identifiers preserved.

BFL reports this value in credits and defines
[one credit as exactly USD 0.01](https://docs.bfl.ai/quick_start/pricing). The
`flux2_api` adapter parses the JSON number's decimal lexeme exactly and applies
that conversion once as a decimal scale offset: a source coefficient with
credit scale `s` becomes the same coefficient with USD scale `s + 2`. For
example, `1.0001` credits becomes
`{amount: 10001, scale: 6, currency: "USD"}`, or USD 0.010001. Retaining the
coefficient and provider precision makes the source value and fixed conversion
reviewable without binary floating point.

The converted exact USD value is persisted on the provider attempt and receipt.
Normal settlement, restart, replay, and reconciliation reuse that value and the
receipt's already-derived budget-cent amount; they never apply the credit
conversion or conservative cent ceiling a second time.

### FLUX asynchronous transport and artifact delivery

BFL's current
[integration guide](https://docs.bfl.ai/api_integration/integration_guidelines)
requires clients to poll the URL returned by a generation request and notes
that artifact delivery regions can change. Gongbu keeps those two network
policies separate. A provider-returned polling URL may receive `x-key` only
when it is an HTTPS URL on exactly `api.bfl.ai`, `api.eu.bfl.ai`, or
`api.us.bfl.ai`. User information, explicit ports, fragments, redirects, and
all other origins are rejected before the credentialed request is sent.

The generation POST is isolated in the patch-protected `submit_provider`
activity and is never retried as provider generation work. A successful submit
must be followed immediately by one atomic Gongbu SQLite checkpoint containing
only the validated request ID when present, operation ID, polling hostname, and
the absolute adapter deadline. The polling URL itself is not persisted.
`poll_provider_operation` reconstructs the status request from frozen runtime
configuration and that checkpoint, then issues only GET requests for the same
operation. Worker restart and activity recovery reuse the checkpoint and its
deadline rather than submitting again or granting a fresh timeout budget.

The [Get Result OpenAPI](https://docs.bfl.ai/api-reference/utility/get-result)
enumerates `Pending`, `Reasoning`, `Generating`, `Ready`, `Request Moderated`,
`Content Moderated`, `Task not found`, and `Error`. Gongbu treats the first
three as pollable, `Ready` as success, and every other state as immediately
terminal; it never keeps polling a terminal response until timeout. Moderation
and provider `Error` outcomes release the authorization because BFL's current
[moderation guidance](https://help.bfl.ai/articles/4212278032-my-prompt-is-getting-moderated)
says moderated requests are not charged and only `Ready` consumes credits.
`Task not found`, malformed results, transport ambiguity after submission, and
other outcomes that cannot prove whether work was accepted go to
reconciliation. Before an operation is accepted, a
definitive rejection releases the authorization; after acceptance, the same
HTTP ambiguity reconciles. BFL's documented HTTP failures map `402` to
insufficient credit, `403` to permission failure, and `429` to rate limiting;
Gongbu also classifies `401` defensively as authentication failure. Raw provider
bodies are not retained. Only compact, validated, non-secret request and
operation identifiers may survive as reconciliation evidence.

If a generation request may have reached BFL but its operation ID cannot be
durably established, the workflow also reconciles. It does not infer safety
from a missing checkpoint, retry the POST, or release the Hubu claim. Once the
checkpoint exists, subsequent polling ambiguity retains that same safe
operation evidence for recovery or reconciliation.

Artifact URLs follow a different, credential-free path. Gongbu accepts only
HTTPS `delivery.<region>.bfl.ai` hosts with exactly one safe region label,
rejects redirects and URL ambiguity, and never forwards `x-key`. Because BFL's
[quick start](https://docs.bfl.ai/quick_start/generating_images) describes these
as short-lived signed URLs, the adapter downloads them immediately within the
invocation's byte and time limits. The signed URL is neither returned nor
persisted. Gongbu decodes and validates the bounded response as PNG or JPEG
before the downloaded bytes enter normalized artifact storage.
Although the
[`flux-2-pro` request contract](https://docs.bfl.ai/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bpro%5D)
offers JPEG, PNG, and WebP output, Gongbu's initial normalized FLUX subset is
PNG and JPEG. The same contract defines `safety_tolerance` as the integer range
`0..=5`; Gongbu rejects `6`, non-integers, and unsupported values before any
provider request.

### Certified FLUX.2 output dimensions

The `flux2_api` adapter pins the non-preview `flux-2-pro` model described in the
[official FLUX.2 overview](https://docs.bfl.ai/flux_2/flux2_overview). Its
initial certified output profile is intentionally limited:

| Normalized preset | Exact BFL width and height |
| --- | --- |
| `1k` | `1024` × `1024` |
| `2k` | `1920` × `1088` (landscape) |
| `4k` | `2048` × `2048` |

These preset names belong to Hubu and Gongbu; BFL does not name these exact
dimension pairs `1k`, `2k`, and `4k`. The mapping is deterministic and is not an
automatic resolution-selection feature. Arbitrary dimensions, partial
width/height overrides, and overrides that conflict with the selected preset
are rejected during admission.

The profile enforces BFL's documented minimum of `64` × `64`, requires each
dimension to be a multiple of `16`, and caps output at the documented 4 MP
maximum represented by `2048` × `2048`. See BFL's
[official dimension guidance](https://help.bfl.ai/articles/8916739058-what-aspect-ratios-and-output-dimensions-are-supported).
Each enabled FLUX profile must contain one operator-verified,
selector-qualified price for every certified preset. Gongbu selects that rule,
binds the exact dimensions, and freezes the preset, dimensions, and pricing
snapshot before it resolves or claims Hubu authorization. Admission fails
before persistence, `ProviderAttempt` creation, or provider network activity if
the rule or dimension contract is missing or inconsistent.

The adapter transmits only BFL's top-level integer `width` and `height` request
fields documented by the
[`flux-2-pro` API](https://docs.bfl.ai/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bpro%5D).
It never forwards Gongbu's generic `image_size` selector. The durable normalized
input and pricing snapshot retain the selected preset and exact transmitted
dimensions, so exact replay reconstructs the frozen request after catalog
rotation or process restart instead of consulting the current catalog.

For pre-HUB-168 schema-v2 executions whose snapshot predates the additive
`output_dimensions` field, recovery is limited to the same pinned FLUX target
and a supported frozen selector. Gongbu derives the certified pair on a cloned
request only when the persisted input selects that same preset and either omits
both explicit dimensions or already contains the exact pair. Partial,
conflicting, arbitrary, or unsupported legacy evidence still fails before
claim, `ProviderAttempt` creation, credential resolution, or provider activity;
the durable legacy record is not rewritten and the current catalog is not
consulted.

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
[Gongbu server operations](operations/gongbu-server.md). For deterministic and
live-provider test modes, use the [sandbox](operations/gongbu-sandbox.md) and
[live provider testing](operations/live-provider-testing.md) guides.

The implementation lives in [`crates/gongbu-api`](../crates/gongbu-api).

On repository open, Gongbu independently migrates legacy v4.3 minor-unit
attempt and receipt amounts to exact amount with scale 2 and their stored
currency. For an in-flight legacy receipt, it also reconstructs the exact v4.3
wire evidence: the receipt ID remains the provider-request reference and the
reduced price/model projection is derived from the unchanged frozen execution
snapshot. New receipts instead persist the provider-reported reference and the
complete snapshot. This keeps a lost-response retry immutable and idempotent.
Hubu performs its corresponding migration only in the Hubu database; neither
process opens or migrates the other's state. See
[persistence migration and v4 compatibility](spend-executor-contract.md#persistence-migration-and-v4-compatibility).
