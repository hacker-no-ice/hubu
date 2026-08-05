# Gongbu

Gongbu is the execution plane for Hubu-authorized work. The repository currently
contains the v1 persistence and normalized-artifact foundations; execution
orchestration, provider adapters, and the v1 HTTP API are intentionally not yet
implemented.

## V1 boundary

`Execution` is the persisted aggregate root and is unique by
`(account_id, operation_key)`. Provider attempts, artifact metadata, and receipts
are persisted under that aggregate. Raw Hubu tokens and provider credentials are
not persisted.

Gongbu owns normalized artifact bytes. The local backend stores them beneath an
operator-supplied artifact root and persists only generated, storage-neutral
keys. The artifact service accepts PNG and JPEG, validates count and decoded-size
limits, and never exposes absolute filesystem paths.

The retained Hubu v4 client is a low-level protocol client for future durable
workflow work. No current production path claims, settles, releases, or invokes a
provider.

## Planned service surface

HUB-23 will add the authoritative v1 routes:

- `POST /v1/executions`
- `GET /v1/executions/{execution_id}`
- `POST /v1/executions/{execution_id}/cancel`
- `GET /v1/executions/{execution_id}/artifacts`
- `GET /v1/artifacts/{artifact_id}`

There is no persisted quote resource. Provider invocation, durable workflow,
remote artifact fetching, SVG support, retention, and cloud storage remain out
of scope for the current foundation.

## Operator target configuration

Set `GONGBU_PROVIDER_CONFIG` to an operator-owned JSON file. It is loaded and
validated once at startup, so changing a target requires a restart. Callers
must request one exact `workload_type + provider + adapter + model`; Gongbu
never chooses, orders, optimizes, or falls back between targets.

```json
{
  "provider_configs": [{
    "provider_config_version": "example-image-2026-08-05",
    "workload_type": "image_generation",
    "provider": "example",
    "adapter": "fixture",
    "model": "image-v1",
    "enabled": true
  }]
}
```

The immutable version and resolved selector are stored on each `Execution`.
Duplicate selectors or versions, unknown fields, an unavailable adapter, and a
missing provider secret fail closed. Endpoints, credentials, headers, account
identifiers, timeouts, and retry settings remain operator configuration and are
not accepted from callers.

## Development

```sh
cargo test --workspace
```
