# Gongbu

Gongbu is the execution plane for Hubu-authorized work. The repository currently
contains the v1 persistence, normalized-artifact foundations, and authenticated
Execution and Artifact HTTP contract. Execution orchestration and provider
adapters are intentionally not yet implemented.

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

## Service surface

The authoritative v1 routes are:

- `POST /v1/executions`
- `GET /v1/executions/{execution_id}`
- `GET /v1/executions/{execution_id}/artifacts`
- `GET /v1/artifacts/{artifact_id}`

Transport adapters validate authentication before constructing the trusted
account principal; request bodies cannot override it. There is no cancellation
route or persisted quote resource. Provider invocation, durable workflow, remote
artifact fetching, SVG support, retention, and cloud storage remain out of scope
for the current foundation.

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

## Versioned pricing contract

`PricingCatalog` loads an operator-managed JSON catalog, validates and
canonicalizes its rules, computes a SHA-256 digest, and freezes the result in
memory. A bundled mock catalog is available for local development; production
startup code should load the operator path once and retain that catalog for the
process lifetime.

Each execution persists a typed pricing snapshot containing the provider and
model, catalog version and digest, rule ID, unit amount, bounded quantity,
conservative estimate, and currency. Persistence rejects malformed snapshots,
target mismatches, insufficient authorization, and receipts above either the
authorization or frozen estimate.

The provider boundary exposes normalized request, usage, outcome, capability,
retry, redaction, and opaque idempotency-key contracts. Retries require an
adapter-declared vendor idempotency guarantee. Provider-reported amounts remain
evidence only; deterministic settlement uses normalized usage and the frozen
snapshot. No routing, failover, quote resource, scheduled activation, or billing
export is implemented here.

## Development

```sh
cargo test --workspace
```
