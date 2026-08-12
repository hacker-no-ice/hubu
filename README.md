# Gongbu

Gongbu is the execution plane for Hubu-authorized work. The repository currently
contains the v1 persistence, normalized-artifact foundations, authenticated
Execution and Artifact HTTP contract, and the first single-provider durable
execution workflow, plus operator-selected Google Gemini and Ideogram image
adapters.

The separate `gongbu-mcp` crate exposes the authenticated HTTP contract to local
agent platforms over MCP stdio. See [docs/mcp.md](docs/mcp.md) for operator
configuration, tool examples, and its opt-in integration test.

## V1 boundary

`Execution` is the persisted aggregate root and is unique by
`(account_id, operation_key)`. Provider attempts, artifact metadata, and receipts
are persisted under that aggregate. Raw Hubu tokens and provider credentials are
not persisted.

Gongbu owns normalized artifact bytes. The local backend stores them beneath an
operator-supplied artifact root and persists only generated, storage-neutral
keys. The artifact service accepts PNG and JPEG, validates count and decoded-size
limits, and never exposes absolute filesystem paths.

The workflow drives one persisted execution through preflight, claim, one
durably recorded provider attempt, normalized artifact persistence, and Hubu
settlement or safe release. Ambiguous post-boundary outcomes enter
`reconciliation_required`; they are never blindly retried or released. Temporal
retains the workflow and performs only bounded same-identity Hubu observation
or finalization after durable timers. The default recovery delays are 30, 120,
and 600 seconds and may be replaced at workflow start with
`GONGBU_RECONCILIATION_DELAYS_SECONDS` (a comma-separated positive list). After
the schedule is exhausted, the workflow remains alive for authenticated
operator signals and the recovery path never invokes the provider.
Artifact validation and durable publication occur while the execution remains
`executing`; it transitions directly to `settling` only after durability is
confirmed. The former `persisting` state was never merged, stored by a shipped
schema, or exposed by a shipped API, so v1 removes it without compatibility
handling or a cleanup migration.

The runnable application composition in `application::serve` starts Temporal
before accepting HTTP requests, retains the returned worker thread for process
lifetime, and passes its `scheduler` to `http::Api::new`. Each accepted or
replayed pending execution starts the stable
workflow ID `gongbu-execution-{execution_id}` on `gongbu-executions`; Temporal's
use-existing conflict policy makes duplicate scheduling safe. Interrupted
activities are redelivered, while persisted transmission markers prevent
provider or Hubu side effects from being repeated.

The composition boundary requires an explicit authenticated-principal verifier
and durable activity implementations. It never installs a caller-selected
identity or fixture provider implicitly. An executable host supplies the Gemini
adapter to `PersistedExecutionRunner` and then calls `application::serve`.
`application::gemini_execution_runner`,
`application::gemini_developer_execution_runner`, and
`application::ideogram_execution_runner` provide the production compositions:
each connects its selected provider target and Keychain credential to provider
activities, routes returned bytes through `ArtifactService`, and uses the same
durable workflow for settlement. No provider or fixture fallback is installed.

## Service surface

The authoritative v1 routes are:

- `POST /v1/executions`
- `GET /v1/executions/{execution_id}`
- `GET /v1/executions/{execution_id}/artifacts`
- `POST /v1/executions/{execution_id}/reconciliation`
- `GET /v1/artifacts/{artifact_id}`

The reconciliation route accepts an idempotent `action_id`, an action of
`reinspect`, `settle`, or `release`, and an evidence object. It only signals the
stable Temporal workflow. Settlement or release proceeds only when persisted
execution evidence proves that finalization is safe.

Transport adapters validate authentication before constructing the trusted
account principal; request bodies cannot override it. Accepted v1 executions
run to a terminal outcome; public and in-flight cancellation are deferred to
HUB-37. Remote artifact fetching, SVG support, retention, cloud storage,
multi-provider execution, and operator reconciliation remain out of scope.

## Operator target configuration

Set `GONGBU_PROVIDER_CONFIG` to an operator-owned JSON file. It is loaded and
validated once at startup, so changing a target requires a restart. Callers
must request one exact `workload_type + provider + adapter + model`; Gongbu
never chooses, orders, optimizes, or falls back between targets.

```json
{
  "schema_version": 2,
  "provider_configs": [{
    "provider_config_version": "example-image-2026-08-05",
    "workload_type": "image_generation",
    "provider": "example",
    "adapter": "fixture",
    "model": "image-v1",
    "secret_service": "gongbu.example",
    "secret_account": "local",
    "active": true,
    "execution_enabled": true,
    "settings": { "type": "fixture" }
  }]
}
```

Schema v2 permits multiple immutable revisions for one selector. Exactly one
may be `active` for new executions; inactive revisions remain available to
finish frozen work unless `execution_enabled` is set to `false` as an emergency
stop. Adapter settings are tagged by `type`, and each execution stores the
selected revision plus its canonical SHA-256 configuration digest.

Existing schema-v1 files (no `schema_version`, legacy adapter-specific setting
field, and optional `enabled`) are migrated while loading. `enabled` maps to
both v2 gates, and serializing the validated catalog emits schema v2. Reusing a
version with changed content, multiple active revisions, unknown fields, an
unavailable adapter, and a missing provider secret fail closed. Endpoints,
credentials, headers, account identifiers, deadlines, and retry settings remain
operator configuration and are not accepted from callers.

Database rows created before configuration digests existed retain their exact
target key and revision and use a narrowly scoped `legacy-unresolved` lookup so
in-flight work survives the upgrade. Every newly accepted execution requires
and stores a canonical digest; the legacy path cannot be selected by new work.

See [Local Keychain secrets](docs/local-keychain-secrets.md) for setup and
manual credential replacement. The deliberately opt-in live check is documented
in [Gemini image E2E](docs/gemini-image-e2e.md); CI uses fixtures only. The
separate Google AI Studio adapter setup is documented in
[Gemini Developer API image E2E](docs/gemini-developer-image-e2e.md).
Ideogram adapter is likewise fixture-backed in CI and has no implicit live-call
or alternate-provider path.

## Versioned pricing contract

`PricingCatalog` loads an operator-managed JSON catalog, validates and
canonicalizes its rules, computes a SHA-256 digest, and freezes the result in
memory. A bundled mock catalog is available for local development; production
startup code should load the operator path once and retain that catalog for the
process lifetime.

Catalog schema v2 permits resolution-qualified image rules selected from the
normalized request `image_size` (`1k`, `2k`, or `4k`) and compound input/output
token components. Rates are exact `rate_numerator_minor / rate_denominator`
values, so per-million-token prices use a denominator of `1000000` without
floating point. The frozen v2 snapshot records the selector, every exact rate
and bounded quantity, and the reduced exact aggregate estimate.

For example, a 4K image rule priced at 16 USD cents per image is:

```json
{
  "rule_id": "gemini-image-4k",
  "provider": "google",
  "model": "OPERATOR_APPROVED_MODEL_VERSION",
  "currency": "USD",
  "selector": { "image_size": "4k" },
  "components": [{
    "unit": "image",
    "rate_numerator_minor": 16,
    "rate_denominator": 1
  }]
}
```

This rule belongs inside a catalog with `"schema_version": 2`. The normalized
request must contain `"image_size": "4k"`; missing or unsupported values fail
before provider invocation. See [Gemini image E2E](docs/gemini-image-e2e.md) for
a complete three-tier catalog and live invocation example.

Authorization uses the ceiling of that exact estimate at the integer currency
minor-unit boundary. Settlement recomputes all components exactly and performs
one round-half-up operation on the aggregate; components and intermediates are
never rounded. Existing schema-v1 flat image catalogs and persisted v1 snapshots
remain accepted and replay with their original integer semantics. Persistence
rejects malformed snapshots, target mismatches, insufficient authorization, and
receipts above either the authorization or frozen estimate.

The provider boundary exposes normalized request, usage, outcome, capability,
retry, redaction, and opaque idempotency-key contracts. Retries require an
adapter-declared vendor idempotency guarantee. Provider-reported amounts remain
evidence only; deterministic settlement uses normalized usage and the frozen
snapshot. No routing, failover, quote resource, scheduled activation, or billing
export is implemented here.

## API module map

`gongbu-api` is organized by domain:

- `execution` owns the persisted execution aggregate, lifecycle-facing repository
  operations, provider attempts, receipts, and persistence errors.
- `provider` owns shared adapter and pricing contracts plus operator-selected
  provider targets.
- `artifact` owns normalized artifact validation and storage.
- `hubu` owns the Hubu client integration and its private outbound transport.
- `http` owns routes, request/response DTOs, authentication, and HTTP error mapping.
- `config` owns secret-provider wiring and sensitive-value redaction.

Dependencies point inward from `http` and composition code to the domain modules.
`artifact` may use `execution` persistence, provider target configuration may use
`config` secret resolution, and persistence may use provider pricing contracts and
redaction. Provider contracts, execution, and artifacts must not depend on HTTP
routes or DTOs; provider-specific adapters must depend on the shared provider
contracts. The crate root contains only domain declarations and compatibility
re-exports that preserve the original public module paths.

## Development

Use the configurable [Gongbu sandbox](docs/sandbox.md) for deterministic local
runs, bounded live-provider checks, Hubu compatibility, and guarded dogfood.
Hubu and provider modes are selected independently and validated before any
execution boundary becomes ready.

The official Temporal Rust SDK compiles its protobuf definitions during the
build, so install `protoc` first (`brew install protobuf` on macOS or
`apt-get install protobuf-compiler` on Debian/Ubuntu).

```sh
cargo test --workspace
```
