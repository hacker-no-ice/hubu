# Gongbu execution plane

Gongbu turns a Hubu spend authorization into durable provider work and safe
artifacts. It runs as a separate process with its own database, credentials,
provider configuration, Temporal worker, artifacts, readiness, and recovery.

Hubu remains the control plane. Sharing a repository and product release does
not turn the boundary into an in-process call or authorize either component to
open the other's database.

## Responsibilities

Hubu owns:

- policy and budget decisions;
- spend authorization and expiry;
- executor claims, settlement, release, and reconciliation; and
- financial audit state.

Gongbu owns:

- operator-controlled provider targets and pricing;
- provider credentials and calls;
- durable execution and provider-attempt records;
- Temporal workflows and activities;
- cost calculation and settlement evidence;
- normalized artifacts; and
- execution recovery.

The components communicate over
[`hubu-spend-executor-v4.3`](spend-executor-contract.md).

## Admission and execution flow

The canonical caller submits a Hubu spend-authorization token plus execution
intent and an operator-configured target to `POST /v2/executions`.

Gongbu then:

1. Resolves Hubu's read-only authorization snapshot.
2. Derives the provider, adapter, model, execution scope, and price from its
   operator-controlled catalog.
3. Requires exact agreement on the account, agent, operation key, amount,
   currency, lease profile, expiry, and typed execution scope.
4. Persists the `Execution` aggregate before scheduling work.
5. Starts the stable Temporal workflow
   `gongbu-execution-{execution_id}` on the `gongbu-executions` task queue.
6. Claims the Hubu authorization from the durable workflow.
7. Creates a `ProviderAttempt` before irreversible provider transmission.
8. Normalizes artifacts and calculates actual provider cost.
9. Settles confirmed billable work or releases confirmed non-billable work.

Resolving authorization never claims it. Preview APIs are optional UX and are
never authority for admission or price. Gongbu recomputes from its active
catalog immediately before persistence.

## Retry and reconciliation

Execution identity, operation key, provider-attempt identity, Hubu claim, and
Temporal workflow ID remain stable across recovery. A restart resubmits
nonterminal executions to the same workflow identity rather than creating a
second provider call.

An ambiguous provider or settlement outcome becomes
`reconciliation_required`. Gongbu does not blindly retry the provider call or
release Hubu's hold merely because a response was lost. Finalization uses a
persisted provider receipt and remains idempotent under repeated delivery.

## Provider targets and pricing

Provider selection is an operator decision, not a caller override. A production
target binds:

- workload type;
- provider, adapter, and model;
- typed execution scope;
- credential reference;
- pricing model and currency;
- maximum authorized spend; and
- whether live provider execution is explicitly enabled.

Admission fails closed when target selection is unknown or ambiguous, price or
scope differs from Hubu authorization, a required credential is unavailable,
or the live-spend gate is incomplete.

Provider credentials belong to Gongbu's runtime identity. They are never
accepted in execution requests, stored in repository records, included in
fixtures, returned by APIs, written to Temporal payloads, or emitted in logs and
errors.

## Temporal ownership

`gongbu-server` always owns its Temporal worker. It supports two service modes:

- `managed_local`: Gongbu starts and stops one pinned local Temporal child and
  retains its data across ordinary restart.
- external: Gongbu connects to an independently operated Temporal service and
  never assumes lifecycle authority over it.

Gongbu readiness requires the selected Temporal service and a polling worker.
Losing either closes new execution admission while preserving inspection and
recovery state.

Workflow inputs contain execution identifiers and non-secret business data.
Activities resolve credentials at execution time so secrets do not enter
Temporal history.

## Artifacts

Providers never choose final storage keys or write directly into the configured
artifact root. All bytes pass through Gongbu's normalized artifact service,
which validates supported media, computes stable metadata and hashes, and
persists storage-neutral artifact identities.

API and MCP responses expose safe artifact IDs, media type, size, and digest.
They never expose an absolute filesystem path or internal storage key.

## Service surface

The persistent server exposes:

- liveness, readiness, and version metadata;
- versioned execution creation and inspection;
- artifact listing and retrieval; and
- authenticated operator diagnostics.

Agents normally reach this surface through
[`hubu-unified-mcp`](unified-mcp.md). The router forwards Gongbu calls using
only the Gongbu endpoint, bearer credential, and account claim.

For local startup, shutdown, backup, and troubleshooting, use
[Gongbu server operations](operations/gongbu-server.md). For deterministic and
live-provider test modes, use the [sandbox](operations/gongbu-sandbox.md) and
[live provider testing](operations/live-provider-testing.md) guides.

The implementation lives in [`crates/gongbu-api`](../crates/gongbu-api).
