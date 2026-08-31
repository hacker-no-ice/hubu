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
- durable execution and provider-attempt records;
- Temporal workflows and activities;
- exact integer cost calculation, frozen pricing snapshots, and settlement
  evidence;
- normalized artifacts; and
- execution recovery.

The components communicate over
[`hubu-spend-executor-v4.3`](spend-executor-contract.md).

## Admission and execution flow

The canonical caller submits a Hubu spend-authorization token plus execution
intent and an operator-configured target to `POST /v2/executions`.

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
7. Creates a `ProviderAttempt` before irreversible provider transmission.
8. Normalizes artifacts and preserves exact provider cost, currency, decimal
   scale, and the complete frozen pricing snapshot.
9. Settles confirmed billable work, routes a cost above the authorized maximum
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

## Retry and reconciliation

Execution identity, its persisted account and agent snapshot, operation key,
provider-attempt identity, Hubu claim, and
Temporal workflow ID remain stable across recovery. A restart resubmits
nonterminal executions to the same workflow identity rather than creating a
second provider call.

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

## Provider targets and pricing

Provider selection is an operator decision, not a caller override. A production
target binds:

- workload type;
- provider, adapter, and model;
- typed execution scope;
- credential reference;
- pricing model and currency;
- maximum authorized spend; and
- whether live provider execution is explicitly enabled.

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

Workflow inputs contain execution identifiers and non-secret business data.
Activities resolve credentials at execution time so secrets do not enter
Temporal history.

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
