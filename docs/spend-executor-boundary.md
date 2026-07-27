# Spend Executor Boundary

Gongbu implements Hubu's `hubu-spend-executor-v4` contract.

The canonical contract, ownership boundary, request shapes, response shapes,
and safety rules live in Hubu:

- [Hubu spend executor contract](https://github.com/hacker-no-ice/hubu/blob/main/docs/spend-executor-contract.md)

Keep this document limited to Gongbu-specific implementation notes so it does
not drift from Hubu's control-plane contract.

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

## Gongbu Image Jobs

The dry-run endpoint intentionally performs no real vendor work. The
`POST /mock-executor/dry-run` endpoint exists only to exercise the Hubu
validate, settle, and release lifecycle.

The image job endpoint implements the first real Gongbu work domain for
`gongbu.image`:

1. `GET /image-jobs/guidance` exposes configured provider/model, required Hubu
   merchant and amount, and non-secret readiness.
2. `POST /image-jobs` accepts a prompt, Hubu spend auth token, expected spend
   scope, and optional provider/model confirmation.
3. Gongbu validates provider/model and exclusively claims the Hubu spend scope
   before calling the provider.
4. The local mock adapter writes a deterministic SVG artifact.
5. The Gemini `generateContent` adapter sends the API key server-side via
   `x-goog-api-key`, asks for `IMAGE` response modality, extracts inline image
   bytes, and writes a local artifact.
6. Gongbu settles only after artifact writes succeed, and releases after
   claimed pre-work failures where no irreversible provider work occurred.

The request's immutable, platform-provided `operation_key` is reused for claim,
inspection, settlement or release, and every retry. Ambiguous claim responses
are retried with the same scope. Ambiguous finalization responses first trigger
an authoritative claim lookup and then the same idempotent finalization
request. Gongbu does not derive artifact names from spend authorization tokens
or persist those tokens alongside artifacts.

Provider endpoints must be HTTPS for remote vendors. Plain HTTP is accepted
only for loopback test endpoints, and URL userinfo such as
`localhost@vendor.example` is rejected.
