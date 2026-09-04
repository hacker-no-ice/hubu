# FLUX.2 provider contract

The shipped versioned contract `hubu.flux-2-pro.text-to-image/v1` is the
managed-stack recipe for the deliberately narrow FLUX capability. It renders one
immutable target and its matching pricing rules; it is not a general BFL
configuration template.

Start with [live provider operations](live-providers.md) for the shared
credential, governance, pricing, spend, retry, reconciliation, artifact, and
qualification boundaries. This page covers only the FLUX contract and its BFL
authentication, sizing, asynchronous transport, artifact, cost, activation,
and recovery differences.

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
| `2k` | `1920` × `1088` | `45 / 10` |
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
are outside this contract.

## BFL account and key prerequisites

The operator owns the BFL account and key lifecycle. Follow BFL's
[account and API-key setup](https://docs.bfl.ai/quick_start/get_started), then
create and store the key yourself with the macOS **Keychain Access** app under
an operator-chosen service and account. Do not paste the key into Hubu, a
terminal command, environment variable, JSON, TOML, SQLite, documentation,
fixture, log, or support message. Hubu should never ask for, print, export, or
persist its value.

`credentials.toml` stores only the non-secret lookup coordinates:

```toml
schema_version = 1

[opaque.bfl_flux2_pro]
service = "operator-owned BFL Keychain service"
account = "operator-owned BFL Keychain account"
```

The Gongbu process must run as the macOS user allowed to access that Keychain
item. Doctor checks item existence without retrieving its value. A missing
item, denied access, or missing reference fails before authorization claim,
provider-attempt creation, or provider traffic.

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

Inspect and validate without contacting BFL:

```sh
hubu stack catalog --profile /absolute/path/to/profile --json
hubu stack doctor --profile /absolute/path/to/profile --json
hubu stack render --profile /absolute/path/to/profile
```

The catalog reports independent facts:

- `configured`: source validation resolved the exact provider contract;
- `credential_reference_present`: the referenced Keychain item exists for the
  local process identity; neither its value nor its coordinates are returned;
- `production_validated`: the rendered target, versioned policies, and all
  three frozen pricing rules passed Gongbu's production validator;
- `live_qualified`: always `false` with `not_performed` in this release.

Neither catalog inspection, doctor, render, nor production validation calls
BFL. The running Gongbu service exposes the same sanitized schema-v1 projection
through authenticated `GET /v1/provider-catalog`; agents use the read-only
`gongbu_get_provider_catalog` unified-MCP tool with an empty input. Neither
surface exposes Keychain coordinates or secret values.

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

FLUX submission and polling are separate durable activities. After a successful
submit, Gongbu checkpoints the safe request ID, operation ID, polling host,
sanitized polling-policy evidence, and original deadline before long polling.
The credentialed polling allowlist is exactly `api.bfl.ai`, `api.eu.bfl.ai`,
`api.us.bfl.ai`, and BFL's currently documented cluster origin
`api.us1.bfl.ai`; lookalikes and other cluster names remain denied. A restart resumes GET polling
for that same operation and `ProviderAttempt`; it does not submit another
generation. If transmission may have happened but the checkpoint did not
commit, preserve the execution for reconciliation instead of retrying or
releasing the claim.

For the HUB-200 incident, `api.us.bfl.ai` was a reconstructed documented
endpoint that successfully returned the stored operation as `Ready`; the exact
provider-returned polling URL had already been discarded and must not be
inferred from that recovery. The resulting signed artifact used
`delivery.us2.bfl.ai`. These are intentionally different trust classes:
`x-key` is sent only to a literal approved API origin, while an HTTPS
`delivery.<region>.bfl.ai` URL is fetched immediately without `x-key`, with the
complete signed URL treated as ephemeral and never logged or persisted.

If a returned polling origin is rejected after submission, the execution detail
contains only the URL fingerprint, normalized origin fields, fixed path shape,
query-key names, validation reason, policy version, operation/correlation IDs,
and frozen provider-binding reference. Treat this as urgent: do not resubmit.
Update policy only after verifying a provider endpoint, then send an explicit
`reinspect` reconciliation action to poll the same operation only if execution
detail still reports that action while the original absolute polling deadline
leaves enough time for a status GET. After that recovery window, contact
provider support; Gongbu rejects a stale reinspect instead of reopening polling
or creating a fresh timeout budget. BFL result URLs expire after 10 minutes,
so artifact preservation precedes diagnosis.

The credentialed live canary remains opt-in because it incurs a provider
charge. With explicit approval, use the existing unified MCP governed-execution
flow for one 1k PNG and verify one submit, at least one poll, one immediate
artifact fetch, and normalized `gongbu_get_artifact` retrieval. Record the
execution and operation IDs and transport counters; never run this canary from
CI or as part of an unapproved release check.

The artifact policy accepts only an HTTPS `delivery.<region>.bfl.ai` host with
exactly one safe dot-separated region label, matching BFL's current
[region-varying delivery guidance](https://docs.bfl.ai/api_integration/integration_guidelines).
The artifact policy deliberately preserves the documented dot-separated host
family rather than broadening it to an unsupported hyphenated variant. Artifact
downloads never receive the BFL `x-key` credential.

The feature-gated local Temporal acceptance adapter reports the selected
1k/2k/4k fixture price so the full offline stack can assert exact settlement at
each fixture size. It is not a production provider adapter and does not alter
this provider contract's frozen BFL pricing. Do not adjust production pricing
or host policy to make a predecessor caveat disappear.

Treat the selected Keychain `service` and `account` as part of the immutable
provider-target revision. Do not change those coordinates while an execution
can still resume: doing so would make its frozen target digest unavailable.
First let submitted work finish or preserve ambiguous work for reconciliation,
stop the stack, and verify no execution still depends on the old credential.
Then replace the value yourself in the same Keychain item, keeping its
coordinates stable, and rerun catalog, doctor, render, review, and activation.
Never make a replacement provider call merely to test recovery.
