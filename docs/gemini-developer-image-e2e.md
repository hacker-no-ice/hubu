# Opt-in Gemini Developer API image E2E

This test is ignored by default and must never run in ordinary tests or CI. It
makes exactly one potentially billable request to the operator-selected
`google` / `gemini_developer_image` / model target, with no retry or fallback.

Store a Google AI Studio API key in macOS Keychain (not in JSON or shell
history):

```sh
security add-generic-password -U -s gongbu.google-ai-studio -a local-e2e -w
```

Create an operator-owned target file:

```json
{"provider_configs":[{
  "provider_config_version":"google-gemini-developer-local-v1",
  "workload_type":"image_generation",
  "provider":"google",
  "adapter":"gemini_developer_image",
  "model":"gemini-3.1-flash-image",
  "secret_service":"gongbu.google-ai-studio",
  "secret_account":"local-e2e",
  "gemini_developer_image":{
    "endpoint":"https://generativelanguage.googleapis.com",
    "api_version":"v1beta",
    "timeout_ms":120000,
    "max_retries":0,
    "headers":{}
  },
  "enabled":true
}]}
```

Create an operator-owned schema-v2 pricing catalog with an exact `google` +
`gemini-3.1-flash-image` image rule for a resolution supported by that
model. This example selects 4K:

```json
{
  "schema_version": 2,
  "catalog_version": "google-gemini-developer-local-v2",
  "rules": [
    {
      "rule_id": "google-gemini-3.1-flash-image-4k",
      "provider": "google",
      "model": "gemini-3.1-flash-image",
      "currency": "USD",
      "selector": { "image_size": "4k" },
      "components": [{
        "unit": "image",
        "rate_numerator_minor": 151,
        "rate_denominator": 10
      }]
    }
  ]
}
```

The component rate is the frozen amount Gongbu will authorize and settle per
image, expressed as exact USD minor units. The example value is a local test
ceiling, not a statement of Google's current price; verify and update it before
a live run. Add separate selector-qualified rules for any other enabled tiers.

Then explicitly confirm the charge and set the USD-minor-unit ceiling at least
as high as the catalog rule:

```sh
export GONGBU_PROVIDER_CONFIG=/absolute/path/provider-targets.json
export GONGBU_PRICING_CATALOG=/absolute/path/pricing.json
export GONGBU_LIVE_GEMINI_DEVELOPER_MAX_MINOR=16
export GONGBU_LIVE_GEMINI_DEVELOPER_IMAGE_SIZE=4k
export GONGBU_LIVE_GEMINI_DEVELOPER_CONFIRM=I_ACCEPT_GOOGLE_CHARGES
export GONGBU_LIVE_GEMINI_DEVELOPER_PROMPT='Draw one small blue circle on white.'
export GONGBU_LIVE_GEMINI_DEVELOPER_OUTPUT=/absolute/path/gemini-live-output.png

cargo test -p gongbu-api provider::gemini_developer_image::tests::live_developer_api_e2e_requires_explicit_spend_guard -- --ignored --exact
```

The adapter reads the selected Keychain secret and sends it only in the
`x-goog-api-key` header to
`https://generativelanguage.googleapis.com/v1beta/interactions`.

`GONGBU_LIVE_GEMINI_DEVELOPER_IMAGE_SIZE` must use normalized lowercase `1k`,
`2k`, or `4k` and must match a catalog selector. The adapter validates that
selection against the frozen snapshot and transmits the corresponding vendor
value (`1K`, `2K`, or `4K`) before generation.

On success, the test validates the returned image and writes its exact bytes to
`GONGBU_LIVE_GEMINI_DEVELOPER_OUTPUT`. The path must be absolute, its parent
directory must already exist, and the test refuses to overwrite an existing
file. This output file is the manual run's inspectable evidence; it is not added
to Gongbu's database or managed artifact store.
