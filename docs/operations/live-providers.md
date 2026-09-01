# Live provider operations

This is the external operator entry point for billable Gongbu provider work.
Hubu currently supports two documented live integrations: the Gemini Developer
API image adapter and the managed FLUX.2 profile. Both use the same governance,
credential, pricing, spend, retry, reconciliation, artifact, and qualification
boundaries described here. Provider-specific sections contain only the
authentication, transport, sizing, artifact, and recovery differences.

Live provider execution can incur charges. Ordinary tests, demos, CI, catalog
inspection, doctor, render, and production validation must remain non-billable
and must not contact a provider.

| Integration | Gongbu identity | Provider-specific behavior |
| --- | --- | --- |
| Gemini Developer API | `google` / `gemini_developer_image` | AI Studio API key, synchronous response, one operator-owned output file |
| FLUX.2 Pro | `flux` / `flux2_api` through `hubu.flux-2-pro.text-to-image/v1` | Managed target and prices, asynchronous submit and polling, BFL artifact delivery and durable resume |

Other adapter IDs can remain available for operator-managed configurations,
but they are not additional supported integrations in this guide.

## Ownership and credential boundary

Hubu owns registration, policy, authorization, budget holds, approval, and
settlement. Gongbu owns provider configuration, credentials, requests,
provider-attempt evidence, Temporal recovery, receipts, and artifacts. Their
processes, databases, credentials, artifacts, and failure domains remain
separate even in one managed local stack.

Gongbu resolves provider credentials from the logged-in operator's macOS
Keychain. Configuration contains only an opaque local alias whose `service` and
`account` locate the Keychain item. Never place secret values in JSON, TOML,
environment variables, command arguments, SQLite, Temporal payloads, fixtures,
logs, source control, documentation, or support messages.

Create or rotate the secret with Keychain Access so its value never appears on
a command line. Run Gongbu as the macOS user allowed to access the item. A
missing item or denied access fails before an authorization claim,
`ProviderAttempt` creation, or provider traffic. Keep the lookup coordinates
stable while submitted work can still resume, and restart Gongbu after rotating
the value.

## Configure governance, pricing, and spend

Use `mode = "live"` only after the topology is healthy in sandbox mode. Every
live profile requires:

- one exact active, execution-enabled provider target or supported contract;
- an opaque credential reference;
- immutable target and catalog version labels;
- schema-v2 pricing with exactly one matching rule for every enabled request
  selector and every billable component;
- a positive `maximum_spend_minor` no greater than the operator-approved local
  ceiling; and
- the literal `I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND` acknowledgement.

The profile ceiling is an operational upper boundary, not a replacement for
Hubu policy, budget, or per-request human approval. Verify the selected model,
availability, billing units, and current account price against authoritative
provider documentation before activation. Documentation prices and examples
are versioned structure or frozen evidence, never timeless current prices.

Run these non-provider checks before activation:

```sh
hubu stack catalog --profile /absolute/path/to/profile --json
hubu stack doctor --profile /absolute/path/to/profile --json
hubu stack render --profile /absolute/path/to/profile
```

Review the generated plan, immutable versions, sanitized target catalog,
pricing selectors, spend ceiling, and affected components. Activate and start
only the reviewed generation.

## Submission, retry, and reconciliation

Each approved operation creates at most one billable generation submission.
Set provider mutation retries to zero and do not configure fallback. A timeout,
lost response, worker interruption, or ambiguous transport error does not prove
that the provider rejected or did not bill the request.

- A failure proven to precede transmission may release the Hubu claim.
- A durably recorded asynchronous provider operation may resume read-only
  polling for that same operation under its original deadline.
- If transmission may have happened without a durable operation checkpoint,
  preserve the execution and claim for reconciliation. Do not submit another
  operation, mint a replacement operation key, or release the claim.
- Settle only from the exact frozen pricing snapshot and durable provider
  evidence. Over-limit or inconsistent cost remains reconciliation-required.

Provider idempotency is usable only when the selected provider documents the
semantics and Gongbu's adapter contract supports them. It never authorizes a
blind retry of an ambiguous mutation.

## Artifact boundary

Create and validate every operator-owned output directory or file parent before
provider transmission. Gongbu accepts only the artifact media types, dimensions,
and HTTPS hosts allowed by the selected adapter contract. Provider credentials
are sent only to the provider API transport and never to an artifact download
host. Gongbu stores normalized artifacts under its own artifact root and exposes
them through authenticated execution-bound reads; Hubu stores no artifact bytes.

Inspect content type, dimensions, digest, and the persisted receipt before
settlement or qualification. Never treat a provider URL, signed URL, local
storage path, or raw provider body as safe public evidence.

## Readiness and live qualification

The sanitized provider catalog reports independent facts for configuration,
credential-reference presence, production validation, and live qualification.
Catalog, doctor, render, and production validation make no provider call and
cannot establish live qualification.

Ignored live tests are focused operator tools, not ordinary test-suite steps.
Run one only with explicit human authorization, an exact confirmation string,
a strict minor-unit ceiling, current pricing, a minimal prompt, and an
operator-owned output path. Never run live provider tests in CI.

## Gemini Developer API

The `google` / `gemini_developer_image` adapter reads an AI Studio API key from
Keychain and sends it only in the `x-goog-api-key` header to the configured
Google API endpoint. The request returns synchronously; the focused test writes
one validated image to an absolute operator-owned path and refuses to overwrite
an existing file.

After completing the shared checks above, an explicitly authorized operator can
run the single ignored test with absolute paths:

```sh
GONGBU_PROVIDER_CONFIG=/absolute/path/provider-targets.json \
GONGBU_PRICING_CATALOG=/absolute/path/pricing.json \
GONGBU_LIVE_GEMINI_DEVELOPER_MAX_MINOR=OPERATOR_CEILING \
GONGBU_LIVE_GEMINI_DEVELOPER_IMAGE_SIZE=4k \
GONGBU_LIVE_GEMINI_DEVELOPER_CONFIRM=I_ACCEPT_GOOGLE_CHARGES \
GONGBU_LIVE_GEMINI_DEVELOPER_PROMPT='Draw one small blue circle on white.' \
GONGBU_LIVE_GEMINI_DEVELOPER_OUTPUT=/absolute/path/gemini-live-output.png \
cargo test -p gongbu-api \
  provider::gemini_developer_image::tests::live_developer_api_e2e_requires_explicit_spend_guard \
  -- --ignored --exact
```

Normalized image sizes are `1k`, `2k`, and `4k`. The adapter verifies the
selected size against the frozen schema-v2 pricing selector before calling the
provider and never derives the authorized price from returned artifact
dimensions. A synchronous timeout or ambiguous response has no pollable Gongbu
checkpoint, so reconcile rather than rerun.

## FLUX.2 Pro

The supported FLUX integration uses the managed
`hubu.flux-2-pro.text-to-image/v1` contract. It freezes the BFL target, certified
dimensions, dated rational prices, zero generation retries, no fallback,
polling, artifact-delivery, and durable-recovery policies. The operator supplies
only an opaque Keychain credential alias and the two explicit spend choices.

FLUX submission is asynchronous. Gongbu sends one generation POST, checkpoints
the safe request and operation identifiers, validated polling host, and original
deadline, then performs bounded read-only polling. Restart resumes that same
operation and never submits a replacement generation.

The shipped managed profile is production-validated but not live-qualified;
its catalog reports `live_qualified = false` and
`live_qualification = "not_performed"`. Ordinary demos and CI remain
fixture-only. For BFL authentication, frozen dimensions and prices, settled
cost conversion, delivery-host policy, activation, and recovery details, read
the [managed FLUX.2 profile](managed-flux-profile.md).
