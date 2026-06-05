# Gongbu

Gongbu is the execution plane for Hubu-authorized work.

Hubu controls policy, spend authorization, budget holds, settlement, release,
and audit state. Gongbu performs the work after Hubu approves spend. This keeps
vendor credentials, provider adapters, artifact generation, retries, and
execution-specific failures outside Hubu.

## Boundary

Hubu is responsible for:

- policy evaluation
- spend authorization tokens
- frozen budget holds
- executor validation
- spend settlement and release
- audit state

Gongbu is responsible for:

- storing execution secrets server-side
- accepting work requests from agents
- validating Hubu spend authorization before irreversible work
- calling model or image vendors
- writing or storing artifacts
- settling Hubu spend after successful billable work
- releasing Hubu holds when no irreversible billable work happened
- returning execution result metadata to agents

## Service Surface

Gongbu currently exposes:

- `GET /health`
- `POST /mock-executor/dry-run` for exercising the Hubu executor contract
- `GET /image-jobs/guidance` for non-secret image provider readiness
- `POST /image-jobs` for Hubu-authorized image generation

Image jobs support a local mock provider and a Gemini `generateContent` adapter.
Remote provider endpoints must use HTTPS. Plain HTTP is accepted only for
loopback test endpoints.

## Secret Handling

Gongbu is designed to run as a separate service with its own runtime identity.
Agents should not share Gongbu's filesystem, process environment, or cloud
credentials.

Secret provider modes:

- `none`: no provider API key is loaded, suitable for the local mock provider.
- `gcp-secret-manager`: recommended for real Gemini runs.
- `env-dev`: local-only fallback for development.

With `gcp-secret-manager`, Gongbu fetches the configured provider key at startup
using its Google Cloud runtime identity and keeps it in memory. Hubu never
receives provider keys.

## Local Development

```sh
cargo run --bin gongbu-server
```

```sh
cargo test --workspace
```
