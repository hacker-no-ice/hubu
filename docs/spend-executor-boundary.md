# Spend Executor Boundary

Gongbu implements Hubu's `hubu-spend-executor-v1` contract.

## Hubu Owns

- policy evaluation
- spend authorization tokens
- frozen budget holds
- executor validation
- spend settlement and release
- spend audit state

## Gongbu Owns

- server-side vendor API keys
- agent work request intake
- Hubu spend validation before irreversible work
- model or image vendor calls
- artifact writing and storage
- settlement after successful billable work
- release when no irreversible billable work happened
- execution result metadata returned to agents

## Secret Handling

Production Gongbu should load vendor API keys from an external secret manager
at startup. The current implementation supports `GONGBU_SECRET_PROVIDER`:

- `none`: no provider key, used by the local mock provider.
- `gcp-secret-manager`: fetches
  `GONGBU_IMAGE_PROVIDER_API_KEY_SECRET` through the Google Cloud metadata
  server and Secret Manager REST API.
- `env-dev`: local-only fallback that reads `GONGBU_IMAGE_PROVIDER_API_KEY`.

The safety boundary depends on IAM: Gongbu's runtime service account may read
the configured secret, while agents must not share that identity or have shell,
filesystem, environment, or cloud-credential access to the Gongbu process.

## Current Demo Slice

The dry-run endpoint intentionally performs no real vendor work. The
`POST /mock-executor/dry-run` endpoint proves Gongbu can:

1. receive agent scope and a Hubu spend authorization token
2. call `POST /spend/executor/validate`
3. simulate either successful work or pre-work failure
4. call `POST /spend/executor/settle` or `POST /spend/executor/release`
5. return agent-visible lifecycle metadata

The image job endpoint extends that contract for `gongbu.image`:

1. `GET /image-jobs/guidance` exposes configured provider/model, required Hubu
   merchant and amount, and non-secret readiness.
2. `POST /image-jobs` accepts a prompt, Hubu spend auth token, expected spend
   scope, and optional provider/model confirmation.
3. Gongbu validates provider/model and Hubu spend scope before calling the
   provider.
4. The local mock adapter writes a deterministic SVG artifact.
5. The Gemini `generateContent` adapter sends the API key server-side via
   `x-goog-api-key`, asks for `IMAGE` response modality, extracts inline image
   bytes, and writes a local artifact.
6. Gongbu settles only after artifact writes succeed, and releases after
   validated pre-work failures where no irreversible provider work occurred.

Provider endpoints must be HTTPS for remote vendors. Plain HTTP is accepted
only for loopback test endpoints, and URL userinfo such as
`localhost@vendor.example` is rejected.
