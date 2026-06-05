# Gongbu

Gongbu is the external work executor for Hubu-authorized spend.

Hubu controls policy, spend authorization tokens, frozen budget holds, and
settlement audit state. Gongbu owns execution concerns: vendor credentials,
provider calls, artifacts, retries, and the decision to settle or release a
Hubu hold after work is attempted.

## PR 1 Scaffold

This repository currently contains the first minimal service slice:

- HTTP server with `GET /health`
- environment-based config
- typed Hubu spend executor client for:
  - `POST /spend/executor/validate`
  - `POST /spend/executor/settle`
  - `POST /spend/executor/release`
- `POST /mock-executor/dry-run` to validate and close Hubu spend state without
  calling a real vendor
- `GET /image-jobs/guidance` for non-secret provider readiness and required
  Hubu spend scope
- `POST /image-jobs` for Hubu-authorized image generation through either the
  local mock provider or Gemini `generateContent`

Run locally:

```sh
cargo run --bin gongbu-server
```

Useful environment:

```text
GONGBU_BIND_ADDR=127.0.0.1:8790
HUBU_BASE_URL=http://127.0.0.1:8787
```

Dry-run success request:

```json
{
  "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
  "agent_id": "agt_example",
  "amount_cents": 500,
  "merchant": "gongbu.image",
  "task_id": "hubu-logo-demo",
  "outcome": "success"
}
```

Use `"outcome": "pre_work_failure"` to release the hold after validation.

## Image Jobs

Default local mock provider config:

```text
GONGBU_IMAGE_PROVIDER_ADAPTER=mock
GONGBU_IMAGE_PROVIDER_NAME=local-mock
GONGBU_IMAGE_PROVIDER_MODEL=mock-image-v1
GONGBU_IMAGE_PROVIDER_MERCHANT=gongbu.image
GONGBU_IMAGE_PROVIDER_PRICE_CENTS=500
GONGBU_IMAGE_OUTPUT_DIR=target/gongbu-image-outputs
```

Gemini/Nano Banana style config:

```sh
export GONGBU_SECRET_PROVIDER=gcp-secret-manager
export GONGBU_IMAGE_PROVIDER_API_KEY_SECRET=projects/PROJECT_ID/secrets/gongbu-gemini-api-key/versions/latest
export GONGBU_IMAGE_PROVIDER_ADAPTER=gemini-generate-content
export GONGBU_IMAGE_PROVIDER_NAME=google-gemini
export GONGBU_IMAGE_PROVIDER_MODEL=gemini-2.5-flash-image
export GONGBU_IMAGE_PROVIDER_ENDPOINT=https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent
export GONGBU_IMAGE_PROVIDER_PRICE_CENTS=500
export GONGBU_IMAGE_PROVIDER_MERCHANT=gongbu.image
```

`gcp-secret-manager` expects Gongbu to run with a Google Cloud runtime identity
that can read only the configured secret. Gongbu fetches the secret once at
startup using the metadata server access token flow, then keeps the API key in
memory. Agents should not share Gongbu's runtime identity, filesystem, process
environment, or cloud credentials.

For local-only development, `GONGBU_SECRET_PROVIDER=env-dev` reads
`GONGBU_IMAGE_PROVIDER_API_KEY`. This is intentionally not the safe production
mode because any agent with access to the same shell or filesystem may be able
to inspect it.

Before reserving spend, call `GET /image-jobs/guidance`. It returns provider
and spend requirements plus readiness booleans, but not the provider endpoint
or API key.

Image job request after Hubu authorizes spend:

```json
{
  "prompt": "Create a crisp logo for Project Hubu",
  "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
  "agent_id": "agt_example",
  "amount_cents": 500,
  "merchant": "gongbu.image",
  "task_id": "hubu-logo-demo",
  "provider": "google-gemini",
  "model": "gemini-2.5-flash-image"
}
```

Gongbu validates the Hubu authorization before any provider call, writes local
Gemini image artifacts before settlement, settles Hubu spend only after that
write succeeds, and releases the Hubu hold if a pre-work provider/configuration
failure prevents billable work.

## Secret Handling

Secret provider modes:

- `none`: no provider API key is loaded. This is suitable for the local mock
  provider.
- `gcp-secret-manager`: recommended for real Gemini runs. Requires
  `GONGBU_IMAGE_PROVIDER_API_KEY_SECRET`.
- `env-dev`: development fallback only. Requires `GONGBU_IMAGE_PROVIDER_API_KEY`.

The safe deployment model is separate identity, not just separate storage:
Gongbu's service account gets Secret Manager access; agents do not. Hubu never
receives the provider key.
