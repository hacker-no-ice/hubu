# FLUX.2 provider contract

The shipped versioned contract `hubu.flux-2-pro.text-to-image/v1` is the
managed-stack recipe for the deliberately narrow FLUX capability. It renders one
immutable target and its matching pricing rules; it is not a general BFL
configuration template.

Start with [live provider operations](live-providers.md) for the shared
credential, governance, pricing, spend, retry, reconciliation, artifact, and
qualification boundaries. This page is the canonical description of FLUX
provider behavior: the contract and its BFL authentication, sizing,
asynchronous transport, artifact, cost, activation, and recovery differences.
Field definitions live in the
[`providers.toml` reference](../configuration/local-stack/v1/providers-toml.md).

This contract prepares a production-validated execution target, but this release does not
make a billable qualification call. Its catalog therefore always reports
`live_qualified = false` and `live_qualification = "not_performed"`. A later
live-qualification procedure must preserve the same spend and credential
boundaries.

## Frozen contract

| Field | Contract value |
| --- | --- |
| Provider / adapter | `flux` / `flux2_api` |
| Model | non-preview `flux-2-pro` |
| Request | text-to-image, exactly one image |
| Output formats | normalized `png` or `jpeg` |
| Generation retries / fallback | `0` / disabled |
| Polling | the same submitted operation, every 500 ms, under its original 270-second deadline |
| Recovery | durable async resume of that same operation; never a replacement submission |

The contract binds each normalized preset to exact dimensions and one exact
USD rational rate:

| Preset | Exact output | Frozen price in USD cents |
| --- | --- | --- |
| `1k` | `1024` × `1024` | `3 / 1` |
| `2k` | `1920` × `1088` (landscape) | `45 / 10` |
| `4k` | `2048` × `2048` | `75 / 10` |

Those values belong to pricing version
`bfl-flux-2-pro-usd-2026-08-28-v1`, reviewed on 2026-08-28. They are frozen
configuration evidence, not a timeless statement of BFL's current prices.
Before activation, compare the version with BFL's
[pricing documentation](https://docs.bfl.ai/quick_start/pricing) and
[pricing page](https://bfl.ai/pricing). If the provider's terms no longer
match, do not edit or override this contract; keep the stack disabled until a
new reviewed contract version is shipped.

The target follows BFL's documented non-preview
[FLUX.2 Pro model](https://docs.bfl.ai/flux_2/flux2_overview) and
[request contract](https://docs.bfl.ai/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bpro%5D).
Preview models, edits, batch requests, more than one image, arbitrary
dimensions, WebP, model or quality selection, routing, retries, and fallback
are outside this contract. Although the `flux-2-pro` request contract offers
JPEG, PNG, and WebP output, Gongbu's initial normalized FLUX subset is PNG and
JPEG. The same contract defines `safety_tolerance` as the integer range
`0..=5`; Gongbu rejects `6`, non-integers, and unsupported values before any
provider request.

Gongbu's production validator compares the rendered schema-v3 binding, target,
and pricing against this shipped contract before serving.

## Certified output dimensions

The `flux2_api` adapter pins the non-preview `flux-2-pro` model, and its
initial certified output profile is intentionally limited to the three presets
in the table above. These preset names belong to Hubu and Gongbu; BFL does not
name these exact dimension pairs `1k`, `2k`, and `4k`. The mapping is
deterministic and is not an automatic resolution-selection feature. Arbitrary
dimensions, partial width/height overrides, and overrides that conflict with
the selected preset are rejected during admission.

The profile enforces BFL's documented minimum of `64` × `64`, requires each
dimension to be a multiple of `16`, and caps output at the documented 4 MP
maximum represented by `2048` × `2048`. See BFL's
[official dimension guidance](https://help.bfl.ai/articles/8916739058-what-aspect-ratios-and-output-dimensions-are-supported).
Each enabled FLUX profile must contain one operator-verified,
selector-qualified price for every certified preset; a flat rule or partial
pricing set is not enough. Gongbu selects that rule, binds the exact
dimensions, and freezes the preset, dimensions, and pricing snapshot before it
resolves or claims Hubu authorization. Admission fails before persistence,
`ProviderAttempt` creation, or provider network activity if the rule or
dimension contract is missing or inconsistent.

The adapter transmits only BFL's top-level integer `width` and `height` request
fields documented by the
[`flux-2-pro` API](https://docs.bfl.ai/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bpro%5D).
It never forwards Gongbu's generic `image_size` selector, which is durable
pricing and replay evidence only. The durable normalized input and pricing
snapshot retain the selected preset and exact transmitted dimensions, so exact
replay reconstructs the frozen request after catalog rotation or process
restart instead of consulting the current catalog.

For schema-v2 executions whose snapshot predates the additive
`output_dimensions` field, recovery is limited to the same pinned FLUX target
and a supported frozen selector. Gongbu derives the certified pair on a cloned
request only when the persisted input selects that same preset and either omits
both explicit dimensions or already contains the exact pair. Partial,
conflicting, arbitrary, or unsupported legacy evidence still fails before
claim, `ProviderAttempt` creation, credential resolution, or provider activity;
the durable legacy record is not rewritten and the current catalog is not
consulted.

## Settled cost units

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
conversion or conservative cent ceiling a second time. Hubu's one checked
ceiling conversion is defined by the
[spend executor contract](../spend-executor-contract.md#precise-external-cost-and-budget-conversion).

## BFL account and key prerequisites

The operator owns the BFL account and key lifecycle. Follow BFL's
[account and API-key setup](https://docs.bfl.ai/quick_start/get_started), then
create and store the key yourself with the macOS **Keychain Access** app under
an operator-chosen service and account, following the shared
[credential boundary](live-providers.md#ownership-and-credential-boundary).
Hubu should never ask for, print, export, or persist its value.

`credentials.toml` stores only the non-secret lookup coordinates:

```toml
schema_version = 1

[opaque.bfl_flux2_pro]
service = "operator-owned BFL Keychain service"
account = "operator-owned BFL Keychain account"
```

Doctor checks item existence without retrieving its value.

## Select and review the contract binding

Use the normal managed `stack.toml`, then select the frozen contract in
`providers.toml`:

```toml
schema_version = 1
mode = "live"

# Example only. Replace this with the positive USD-cent ceiling you explicitly
# reviewed and are willing to authorize for this local profile.
maximum_spend_minor = 8
live_spend_acknowledgement = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"

[[contract_bindings]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux2_pro"
```

For a contract-only catalog, Hubu derives the immutable pricing version from
the contract. The only operator-specific provider input is the credential
reference; the spend ceiling and exact acknowledgement remain separate,
explicit live-spend choices. Do not reproduce the target, settings, dimensions,
or pricing as raw `[[targets]]` or `[[pricing_rules]]` entries.

Inspect and validate without contacting BFL using the shared catalog, doctor,
and render checks in
[configure governance, pricing, and spend](live-providers.md#configure-governance-pricing-and-spend).
For FLUX, `production_validated` means the rendered target, versioned
policies, and all three frozen pricing rules passed Gongbu's production
validator, and `live_qualified` is always `false` with `not_performed` in this
release. The other catalog facts are defined in
[readiness and live qualification](live-providers.md#readiness-and-live-qualification).

An unknown contract, missing credential reference, missing or changed pricing
version, missing poll/delivery/recovery policy, or unsupported option fails
source or production validation before Hubu claim, `ProviderAttempt` creation,
or provider work. The contract binding has no fields for changing the model,
format set, dimensions, retries, fallback, or policy versions.

Review the generated plan and `maximum_spend_minor` before activation. Hubu
authorization, policy, budget, and human approval still apply per request; the
profile ceiling does not replace any of them. Ordinary demo and CI paths remain
fixture-only and non-billable.

## FLUX transport, artifact, and recovery details

BFL's current
[integration guide](https://docs.bfl.ai/api_integration/integration_guidelines)
requires clients to poll the URL returned by a generation request and notes
that artifact delivery regions can change. Gongbu keeps those two network
policies separate.

### Submission and polling checkpoint

FLUX submission and polling are separate durable activities. The generation
POST is isolated in the patch-protected `submit_provider` activity and is never
retried as provider generation work. A successful submit must be followed
immediately by one atomic Gongbu SQLite checkpoint containing only the safe
request ID when present, operation ID, normalized polling hostname, and the
absolute adapter deadline, before any long polling begins. Before origin
validation can terminate the execution, the same checkpoint also stores a
versioned sanitized recovery record: normalized scheme/host/explicit port,
fixed endpoint shape, query-key names, URL fingerprint, exact validation
reason, and polling-policy version. It never stores the polling URL, arbitrary
query values, userinfo, fragments, headers, credentials, provider bodies,
signed artifact URLs, or storage paths.

`poll_provider_operation` reconstructs the status request from frozen runtime
configuration and that checkpoint, then issues only GET requests for the same
operation. Worker restart and activity recovery reuse the checkpoint, the same
`ProviderAttempt`, and its deadline rather than submitting again or granting a
fresh timeout budget.

A provider-returned polling URL may receive `x-key` only when it is an HTTPS URL
on `api.bfl.ai` or exactly `api.<region-or-shard>.bfl.ai`, where the variable
portion is one safe ASCII DNS label. BFL documents that clients must use the
returned polling URL; live provider evidence shows those URLs can use a
one-label shard such as `api.us7.bfl.ai`. This is a narrow polling namespace,
not a `*.bfl.ai` credential wildcard. User information, explicit ports,
fragments, redirects, extra labels, IDNA labels, suffix confusion, lookalikes,
and all other origins are rejected before the credentialed request is sent.

If a generation request may have reached BFL but its operation ID cannot be
durably established, the workflow reconciles. It does not infer safety from a
missing checkpoint, retry the POST, or release the Hubu claim. Once the
checkpoint exists, subsequent polling ambiguity retains that same safe
operation evidence for recovery or reconciliation. Execution detail tells the
agent not to resubmit, to recover first, and that artifact retrieval is
time-sensitive.

### Provider status and failure classification

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
reconciliation. Before an operation is accepted, a definitive rejection
releases the authorization; after acceptance, the same HTTP ambiguity
reconciles. BFL's documented HTTP failures map `402` to insufficient credit,
`403` to permission failure, and `429` to rate limiting; Gongbu also classifies
`401` defensively as authentication failure. Raw provider bodies are not
retained. Only compact, validated, non-secret request and operation identifiers
may survive as reconciliation evidence.

### Rejected polling origins and reinspect

If a returned polling origin is rejected after submission, the execution detail
contains only the URL fingerprint, normalized origin fields, fixed path shape,
query-key names, validation reason, policy version, operation/correlation IDs,
and frozen provider-binding reference. Treat this as urgent: do not resubmit.
Update policy only after verifying a provider endpoint, then send an explicit
`reinspect` reconciliation action. It may reopen only that same ambiguous
attempt and enter the GET-only poll-existing path, and only if execution detail
still reports that action while the original absolute polling deadline leaves
enough time for a status GET; the generation POST remains unreachable. After
that recovery window, execution detail directs the operator to provider
support; Gongbu rejects a stale reinspect instead of reopening polling or
creating a fresh timeout budget. BFL result URLs expire after 10 minutes, so
artifact preservation precedes diagnosis.

### Artifact delivery

Artifact URLs follow a different, credential-free path. The artifact policy
accepts only an HTTPS `delivery.<region>.bfl.ai` host with exactly one safe
dot-separated region label, matching BFL's current
[region-varying delivery guidance](https://docs.bfl.ai/api_integration/integration_guidelines).
It deliberately preserves the documented dot-separated host family rather than
broadening it to an unsupported hyphenated variant, and it rejects redirects
and URL ambiguity. Artifact downloads never receive the BFL `x-key` credential.

Because BFL's
[quick start](https://docs.bfl.ai/quick_start/generating_images) describes these
as short-lived signed URLs, the adapter downloads them immediately within the
invocation's byte and time limits. The complete signed URL is treated as
ephemeral: it is neither returned, logged, nor persisted. Gongbu decodes and
validates the bounded response as PNG or JPEG before the downloaded bytes enter
normalized artifact storage.

Live recovery validated a provider-returned `api.us7.bfl.ai` operation as
`Ready`, and its signed artifact used `delivery.us7.bfl.ai`. These are
intentionally different trust classes: `x-key` is sent only to the narrow API
polling family, while the delivery host is fetched without it.

### Redaction attestation

The managed-FLUX redaction-attestation contract additionally has one
authenticated, bodyless read-only attestation endpoint at
`GET /v1/executions/{id}/redaction-attestation`. It accepts only the exact
successful frozen FLUX tuple and clean one-authorization-snapshot,
one-claim-reference, one-attempt, one-artifact, one-receipt path. Gongbu
revalidates the stored artifact bytes, resolves the currently registered key
only after all fixed checks pass, and exact-matches it against named
Gongbu-owned projections. A match fails closed. The response is an allowlisted
set of booleans, counts, money facts, content digest, and canonical component
hashes; it exposes no IDs, timestamps, coordinate, secret-derived hash, provider
body, URL, or storage location and performs no provider work.

### Live canary and acceptance fixture

The credentialed live canary remains opt-in because it incurs a provider
charge. With explicit approval, use the existing unified MCP governed-execution
flow for one 1k PNG and verify one submit, at least one poll, one immediate
artifact fetch, and normalized `gongbu_get_artifact` retrieval. Record the
execution and operation IDs and transport counters; never run this canary from
CI or as part of an unapproved release check.

The feature-gated local Temporal acceptance adapter reports the selected
1k/2k/4k fixture price so the full offline stack can assert exact settlement at
each fixture size. It is not a production provider adapter and does not alter
this provider contract's frozen BFL pricing. Do not adjust production pricing
or host policy to make a predecessor caveat disappear.

### Credential rotation

Follow the shared
[credential rotation procedure](live-providers.md#ownership-and-credential-boundary):
keep the selected Keychain `service` and `account` stable while any FLUX
execution can still resume. Never make a replacement provider call merely to
test recovery.
