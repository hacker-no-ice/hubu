# Live provider testing

Live Gongbu provider tests are ignored by default and must never run in ordinary
tests or CI. Each test makes one potentially billable generation submission
with no resubmission or fallback. An asynchronous adapter may perform bounded
read-only status polling for that same operation under its original deadline.
Use short-lived credentials, operator-verified pricing, a strict minor-unit
ceiling, and the adapter's exact confirmation string.

The preferred end-to-end path is the guarded
[Gongbu sandbox](gongbu-sandbox.md). The ignored adapter tests below are useful
for focused provider integration checks.

The managed `hubu.flux-2-pro.text-to-image/v1` contract is configured through
the [managed FLUX.2 profile runbook](managed-flux-profile.md). Its catalog,
doctor, render, and production-validation paths make no BFL call and report
`live_qualified = false` / `not_performed`. HUB-171 does not add a live BFL
test; ordinary demos and CI stay fixture-only and non-billable. A future live
qualification must remain explicitly operator-triggered and spend-capped.

## Credential handling

Gongbu reads provider credentials from the logged-in operator's macOS Keychain.
Provider configuration contains only Keychain service and account identifiers.
Never place credential values in JSON, environment variables, SQLite, command
history, test fixtures, logs, or source control.

Create or update secrets through Keychain Access so the value never appears on
a command line. A provider target refers to the item as:

```json
{
  "secret_service": "gongbu.google",
  "secret_account": "local-e2e"
}
```

A missing item or denied Keychain access fails before claim or provider work.
Restart Gongbu after rotating a credential.

## Common gates

Before either live test:

1. Create an operator-owned target file for one exact provider, adapter, and
   model.
2. Set `max_retries` to zero.
3. Create a schema-v2 pricing catalog with a selector-qualified rule for every
   enabled image size.
4. Verify model availability and current account pricing with the provider.
5. Create the output directory before any provider work.
6. Set the test ceiling no lower than the selected catalog amount and no higher
   than the amount the operator is willing to spend.
7. Use a minimal prompt and one image.

Documentation examples must not be treated as current provider prices. The
operator-owned catalog is the frozen authorization and settlement input.

## Vertex AI Gemini image adapter

The `google` / `gemini_image` adapter uses a short-lived OAuth bearer token and
an operator-configured Vertex AI project, location, model, and approved artifact
hosts.

Run its single ignored test with absolute operator-owned paths:

```sh
GONGBU_PROVIDER_CONFIG=/absolute/path/provider-targets.json \
GONGBU_PRICING_CATALOG=/absolute/path/pricing.json \
GONGBU_LIVE_GEMINI_OUTPUT_DIR=/absolute/path/gemini-output \
GONGBU_LIVE_GEMINI_MAX_MINOR=OPERATOR_CEILING \
GONGBU_LIVE_GEMINI_IMAGE_SIZE=4k \
GONGBU_LIVE_GEMINI_CONFIRM=I_ACCEPT_GOOGLE_CHARGES \
GONGBU_LIVE_GEMINI_PROMPT='Draw one small blue circle on white.' \
cargo test -p gongbu-api \
  provider::gemini_image::tests::live_gemini_e2e_requires_explicit_spend_guard_and_never_uses_fixture \
  -- --ignored --exact
```

The output directory must exist. The test validates the returned image and
writes one inspectable file. Choose a directory outside the repository unless
the artifact is intentionally part of the worktree.

## Gemini Developer API adapter

The `google` / `gemini_developer_image` adapter reads an AI Studio API key from
Keychain and sends it only in the `x-goog-api-key` header to the configured
Google API endpoint.

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

The output path must be absolute, its parent must exist, and the test refuses to
overwrite an existing file.

## Image-size pricing

Normalized image sizes are `1k`, `2k`, and `4k`. The adapter verifies the
request selection against the frozen schema-v2 pricing selector before calling
the provider. It never chooses a price from returned artifact dimensions.

The tests fail closed when target configuration, credentials, exact
confirmation, output preflight, price selection, or spend ceiling is absent or
inconsistent. After a timeout or ambiguous response, inspect provider and local
evidence before any new billable action; a lost response does not prove the
request was unbilled. Resume read-only polling only when Gongbu durably recorded
the existing operation. Without that checkpoint, reconcile rather than rerun
the generation test or release its claim.
