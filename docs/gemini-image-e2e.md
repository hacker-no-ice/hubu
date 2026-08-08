# Opt-in Google Gemini image E2E

This live check is deliberately excluded from ordinary test runs and CI. It uses
the production `google` / `gemini_image` HTTP adapter and never substitutes a
fixture or another provider.

Create an operator target file with one enabled Gemini image target. The model
must be enabled for the configured Google project and location. Store a
short-lived OAuth bearer token in macOS Keychain under `secret_service` and
`secret_account`; do not put it in JSON or shell history.

```json
{"provider_configs":[{
  "provider_config_version":"google-gemini-image-local-v1",
  "workload_type":"image_generation",
  "provider":"google",
  "adapter":"gemini_image",
  "model":"OPERATOR_APPROVED_MODEL_VERSION",
  "secret_service":"gongbu.google",
  "secret_account":"local-e2e",
  "gemini_image":{
    "endpoint":"https://us-central1-aiplatform.googleapis.com",
    "api_version":"v1",
    "project":"YOUR_SENSITIVE_PROJECT_ID",
    "location":"us-central1",
    "timeout_ms":120000,
    "max_retries":0,
    "approved_artifact_hosts":["storage.googleapis.com"],
    "headers":{}
  },
  "enabled":true
}]}
```

Create a schema-v2 pricing catalog with one rule for each Gemini resolution you
allow. The rates below are illustrative USD minor units (cents), not Google's
published prices; replace them with the exact rates for your account and model.
The `model` must exactly match the operator target above.

```json
{
  "schema_version": 2,
  "catalog_version": "gemini-image-prices-2026-08-07",
  "rules": [
    {
      "rule_id": "gemini-image-1k",
      "provider": "google",
      "model": "OPERATOR_APPROVED_MODEL_VERSION",
      "currency": "USD",
      "selector": { "image_size": "1k" },
      "components": [
        {
          "unit": "image",
          "rate_numerator_minor": 4,
          "rate_denominator": 1
        }
      ]
    },
    {
      "rule_id": "gemini-image-2k",
      "provider": "google",
      "model": "OPERATOR_APPROVED_MODEL_VERSION",
      "currency": "USD",
      "selector": { "image_size": "2k" },
      "components": [
        {
          "unit": "image",
          "rate_numerator_minor": 8,
          "rate_denominator": 1
        }
      ]
    },
    {
      "rule_id": "gemini-image-4k",
      "provider": "google",
      "model": "OPERATOR_APPROVED_MODEL_VERSION",
      "currency": "USD",
      "selector": { "image_size": "4k" },
      "components": [
        {
          "unit": "image",
          "rate_numerator_minor": 16,
          "rate_denominator": 1
        }
      ]
    }
  ]
}
```

Select the tier before invocation with the normalized lowercase value `1k`,
`2k`, or `4k`. The adapter verifies that the request selection matches the
frozen pricing selector and maps it to Gemini's vendor representation. It never
uses returned artifact dimensions to choose a price.

For example, run the single ignored test at 4K with a USD-minor-unit ceiling no
lower than the selected 4K rule:

```sh
GONGBU_PROVIDER_CONFIG=/absolute/path/provider-targets.json \
GONGBU_PRICING_CATALOG=/absolute/path/pricing.json \
GONGBU_LIVE_GEMINI_MAX_MINOR=16 \
GONGBU_LIVE_GEMINI_IMAGE_SIZE=4k \
GONGBU_LIVE_GEMINI_CONFIRM=I_ACCEPT_GOOGLE_CHARGES \
GONGBU_LIVE_GEMINI_PROMPT='Draw one small blue circle on white.' \
cargo test -p gongbu-api provider::gemini_image::tests::live_gemini_e2e_requires_explicit_spend_guard_and_never_uses_fixture -- --ignored --exact
```

With the illustrative `16`-cent 4K rule above,
`GONGBU_LIVE_GEMINI_MAX_MINOR` must be at least `16`. Set the ceiling from your
real catalog rate. Omitting `GONGBU_LIVE_GEMINI_IMAGE_SIZE` is compatible only
with a legacy flat schema-v1 rule; selector-qualified schema-v2 rules require it.

The check fails closed when configuration, credentials, the exact confirmation,
or the spend bound is absent. It performs one generation request with no retry.
