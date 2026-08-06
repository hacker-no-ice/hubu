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

Create a pricing catalog with a matching `google` + model image rule, then run
the single ignored test with a USD-minor-unit ceiling no lower than that frozen
rule:

```sh
GONGBU_PROVIDER_CONFIG=/absolute/path/provider-targets.json \
GONGBU_PRICING_CATALOG=/absolute/path/pricing.json \
GONGBU_LIVE_GEMINI_MAX_MINOR=10 \
GONGBU_LIVE_GEMINI_CONFIRM=I_ACCEPT_GOOGLE_CHARGES \
GONGBU_LIVE_GEMINI_PROMPT='Draw one small blue circle on white.' \
cargo test -p gongbu-api provider::gemini_image::tests::live_gemini_e2e_requires_explicit_spend_guard_and_never_uses_fixture -- --ignored --exact
```

The check fails closed when configuration, credentials, the exact confirmation,
or the spend bound is absent. It performs one generation request with no retry.
