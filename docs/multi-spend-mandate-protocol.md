# Multi-Spend Mandate Protocol

Status: **deferred; not planned for the current implementation**

Provisional protocol version: `hubu-spend-executor-v5-draft`

## Implementation Decision

Hubu and Gongbu will not implement this draft for the current dogfood phase.
The normative integration remains
[`hubu-spend-executor-v4`](spend-executor-contract.md): one spend authorization
funds exactly one potentially billable provider invocation.

When an agent task needs several model or provider calls, the agent platform
issues several independent v4 spend requests. Each request has its own stable
`operation_key`, immutable invocation scope, maximum cost, claim lease, and
settlement or release. A shared task identifier may correlate those operations
for audit and presentation, but it does not create a task-level allocation or
hard aggregate cap.

This draft is retained as design research for a future requirement to authorize
multiple charges under one strict shared ceiling. Its routes, records,
capabilities, and accounting model are not current implementation targets.

This protocol extends the single-spend executor contract with one task-scoped
authorization that can fund several separately reserved and settled provider
calls:

```text
mandate
├── attempt A: reserve -> execute -> settle or release
├── attempt B: reserve -> execute -> settle or release
└── close mandate -> release unused authorization
```

The existing [`hubu-spend-executor-v4`](spend-executor-contract.md) contract
remains normative for all current executor spend. The material below describes
the deferred design and may change if mandate work resumes.

## Goals

- Authorize one maximum amount for an agent task.
- Permit multiple provider calls without allowing their total cost to exceed
  that maximum.
- Keep Hubu authoritative for available, reserved, and consumed money.
- Keep Gongbu stateless with respect to authoritative balances.
- Preserve one atomic reserve/finalize lifecycle per potentially billable call.
- Let Hubu and Gongbu recover safely after ambiguous responses or process
  restarts.
- Keep provider credentials, prompts, outputs, and artifact contents outside
  Hubu.

## Non-Goals For The First Implementation

- Concurrent active attempts under one mandate.
- Automatic provider or model selection by Hubu.
- Any artifact storage or normalization owned by Hubu.
- Charging host-managed tools, including Codex built-in ImageGen, against the
  external vendor mandate.
- Refinement rounds, nested mandates, mandate increases, or mandate transfers.
- Automatic resolution of provider calls whose billing outcome is uncertain.

The object model and idempotency rules should not prevent these capabilities
from being added later.

## Roles And Ownership

### Agent Platform

The agent platform:

- supplies and durably retains one stable `task_key` for the logical task
- requests the mandate from Hubu
- sends the Hubu-issued mandate capability and work request to Gongbu
- presents returned artifact references to the user

The language model must not invent or remember `task_key`.

### Hubu

Hubu:

- evaluates whether the task may receive the requested maximum
- freezes that maximum from one active agent budget
- owns mandate and attempt state
- atomically enforces all accounting invariants
- issues attempt reservations before irreversible work
- settles actual cost or releases unused reservations
- closes the mandate and returns unused authorization
- stores compact receipt, provider, and artifact references for audit
- requires human reconciliation for expired attempts with uncertain billing

Hubu does not plan or execute model calls.

### Gongbu

Gongbu:

- accepts a mandate-scoped work request
- chooses providers and models from a static, category-based routing
  configuration in the MVP
- computes a defensible maximum cost before each call
- supplies and durably retains one stable `attempt_key` per potentially
  billable provider attempt
- reserves with Hubu immediately before irreversible work
- calls providers with Gongbu-owned credentials
- owns the artifact store and persists normalized provider results
- settles actual vendor cost or releases before any billable work occurs
- recovers attempt state from Hubu after ambiguous Hubu responses
- invokes Hubu's mandate-close callback when it will schedule no more attempts

Gongbu may cache mandate state for planning, but only a successful reservation
authorizes a provider call.

### Initial Routing

Gongbu does not need intelligent model selection for the MVP. It loads a static
operator-managed file that groups eligible provider/model adapters by work
category, for example `logo_brand`, `product_image`, or `image_edit`. The work
request names or resolves to a category, and Gongbu executes the configured
providers for that category within the mandate.

The routing file, category taxonomy, and selection order belong to Gongbu.
Hubu sees only the governed merchant, workload, attempt scope, cost bound, and
receipt metadata.

## Resource Model

### Mandate

A mandate is a task-scoped sub-allocation of an agent budget.

```json
{
  "protocol_version": "hubu-spend-executor-v5-draft",
  "mandate_id": "spm_example",
  "agent_id": "agt_example",
  "account_id": "aga_example",
  "task_key": "codex:task:01JABC123",
  "merchant": "gongbu.image",
  "purpose": "generate Hubu logo candidates",
  "workload_profile": "image_generation",
  "authorized_amount_cents": 200,
  "currency": "usd",
  "status": "open",
  "available_amount_cents": 200,
  "reserved_amount_cents": 0,
  "consumed_amount_cents": 0,
  "released_amount_cents": 0,
  "expires_at": "2026-07-27T20:00:00Z",
  "constraints": {
    "max_attempts": 4,
    "allowed_executor": "gongbu"
  }
}
```

The first implementation supports these mandate states:

- `open`: accepts a new attempt reservation
- `closing`: rejects new reservations while active or uncertain attempts finish
- `completed`: terminal; unused authorization was returned
- `cancelled`: terminal; unused authorization was returned
- `expired`: no new reservations; unresolved attempts may require reconciliation

`exhausted` is a derived condition when `available_amount_cents` is zero, not a
required lifecycle state. An open exhausted mandate may still receive
settlements or releases.

### Attempt

An attempt represents exactly one potentially billable provider invocation.

```json
{
  "mandate_id": "spm_example",
  "attempt_id": "spa_example",
  "attempt_key": "gongbu:attempt:01JDEF456",
  "executor": "gongbu",
  "provider": "example-image-provider",
  "model": "image-model-v1",
  "reserved_amount_cents": 30,
  "currency": "usd",
  "status": "claimed",
  "claimed_at": "2026-07-27T19:05:00Z",
  "claim_expires_at": "2026-07-27T19:20:00Z",
  "finalized_at": null
}
```

Attempt states:

- `claimed`: cost is reserved and the provider work lease is active
- `settled`: terminal; actual vendor cost was consumed
- `released`: terminal; no billable work occurred
- `reconciliation_required`: provider billing is uncertain and a human decision
  is required

An attempt must never move from one terminal state to another.

The reserve operation atomically reserves money and creates the executor claim.
The mandate must still be open and unexpired when this happens. As in v4, the
claim has its own execution lease: a successfully claimed attempt may settle or
release after the mandate authorization window ends, provided finalization
starts before `claim_expires_at`. Once that lease expires, uncertain work
requires reconciliation.

## Accounting Invariants

For every mandate:

```text
authorized_amount
  = available_amount + reserved_amount + consumed_amount + released_amount
```

All amounts are non-negative integers in the mandate currency.
`released_amount` is authorization returned to the underlying agent budget
when the mandate becomes terminal. Releasing an individual attempt does not
increase this field; it returns that attempt's reservation to mandate
availability.

For an open mandate:

```text
sum(active attempt reservations) = reserved_amount
sum(settled attempt actual costs) = consumed_amount
```

The mandate maximum is frozen from the underlying agent budget when the mandate
is created. Child attempt reservation only moves value inside that allocation;
it does not freeze additional agent-budget money.

At mandate creation:

```text
agent budget remaining -= authorized maximum
agent budget frozen    += authorized maximum
```

At attempt reservation:

```text
mandate available -= reserved maximum
mandate reserved  += reserved maximum
```

At attempt settlement:

```text
mandate reserved  -= reserved maximum
mandate consumed  += actual cost
mandate available += reserved maximum - actual cost

agent budget frozen   -= actual cost
agent budget consumed += actual cost
```

The remainder stays frozen for later attempts. At terminal mandate closure:

```text
agent budget frozen   -= mandate available
agent budget remaining += mandate available
mandate released += mandate available
mandate available = 0
```

Closing must not release money reserved by active or uncertain attempts.

All transitions that touch attempt, mandate, budget hold, balance, receipt, or
token records must commit in one database transaction.

## Identity And Idempotency

The platform owns:

```text
(agent_id, task_key)
```

This pair identifies one mandate request. Repeating an identical request
returns the stored mandate. Reusing the pair with a different amount, currency,
merchant, purpose, workload profile, or constraints is rejected.

The MVP requires a stable platform-supplied key but does not yet define how each
agent platform derives it from trusted invocation metadata. This is a known
integration gap. Adapters must persist the key outside model conversation state
and reuse it across retries; the stable cross-platform derivation convention can
be specified later.

Gongbu owns:

```text
(mandate_id, attempt_key)
```

This pair identifies one potentially billable provider invocation. Repeating an
identical reservation returns the stored attempt. Reusing the pair with a
different reservation scope is rejected.

An HTTP retry for the same provider invocation reuses its attempt key. A retry
that may create a second provider charge uses a new attempt key and requires a
new reservation.

Gongbu should derive a vendor idempotency key from stable server-side attempt
identity when the provider supports idempotency. It must not contain Hubu
credentials or other secrets.

## Mandate Capability

An allowed mandate response includes one Hubu-issued bearer token that wraps
the mandate's immutable execution scope, including mandate identity, agent,
account, merchant, currency, authorized maximum, executor, mandate expiry, and
capability expiry. The agent platform passes this single `mandate_token` to
Gongbu rather than asking Gongbu to reconstruct the scope from separate
untrusted fields.

Gongbu presents the token as a bearer capability when it inspects the mandate,
reserves or finalizes attempts, and requests closure. Hubu verifies the token
and rechecks current durable mandate state on every mutation. The token is not a
balance snapshot: possession never allows Gongbu to bypass current available
amount, lifecycle, attempt-count, or claim-lease checks.

Mandate expiry and capability expiry are distinct:

- `expires_at` is the authorization deadline for creating new attempt claims.
- `capability_expires_at` is the bearer-token deadline and must be late enough
  to finalize and recover every claim Hubu can issue.
- Hubu defines a positive `finalization_recovery_grace_seconds` and must enforce
  `claim_expires_at + recovery grace <= capability_expires_at` when reserving
  and claiming an attempt.
- After `expires_at`, the capability may inspect the mandate, settle or release
  an already claimed attempt before its claim lease expires, and request
  closure. It cannot create another attempt.
- After `claim_expires_at`, the capability remains valid through the recovery
  grace period for inspection, closure, and idempotent replay of a finalization
  that Hubu already committed. A replay may return stored terminal state but
  must not newly settle or release an attempt whose claim lease expired.

This lets Gongbu finalize work authorized immediately before mandate expiry
and recover an ambiguous Hubu response without turning the bearer token into an
unbounded credential.

The token format and signing profile can follow Hubu's existing spend-token
approach. It must be opaque to the language model, must not contain provider
credentials, and must not be logged or stored with artifacts.

## Protocol Flow

Route names below are provisional. Implementations should depend on the
semantics and response fields rather than treating the draft paths as stable.

### 1. Create Or Recover A Mandate

```http
POST /spend/mandates
```

```json
{
  "protocol_version": "hubu-spend-executor-v5-draft",
  "task_key": "codex:task:01JABC123",
  "account_id": "aga_example",
  "authorized_amount_cents": 200,
  "currency": "usd",
  "merchant": "gongbu.image",
  "purpose": "generate Hubu logo candidates",
  "workload_profile": "image_generation",
  "constraints": {
    "max_attempts": 4,
    "allowed_executor": "gongbu"
  }
}
```

Hubu evaluates policy and atomically creates the mandate plus its underlying
budget allocation. An allow response returns the complete mandate state.
Policy denial or insufficient agent budget creates no mandate allocation.

For the MVP, creation freezes the full authorized maximum from the agent budget.
More flexible allocation can be explored later without weakening child-attempt
reservation.

The allow response envelope includes the capability and Hubu callback links:

```json
{
  "mandate": {
    "mandate_id": "spm_example",
    "status": "open",
    "authorized_amount_cents": 200,
    "available_amount_cents": 200,
    "reserved_amount_cents": 0,
    "consumed_amount_cents": 0,
    "released_amount_cents": 0,
    "currency": "usd"
  },
  "mandate_token": "opaque-hubu-bearer-token",
  "capability_expires_at": "2026-07-27T20:20:00Z",
  "finalization_recovery_grace_seconds": 300,
  "links": {
    "self": "/spend/mandates/spm_example",
    "reserve_attempt": "/spend/mandates/spm_example/attempts/reserve",
    "close": "/spend/mandates/spm_example/close"
  }
}
```

### 2. Inspect A Mandate

```http
GET /spend/mandates/{mandate_id}
```

The owner and authorized executor can recover the authoritative mandate state.
The response includes attempts or a stable link through which they can be
listed.

### 3. Reserve And Claim One Attempt

```http
POST /spend/mandates/{mandate_id}/attempts/reserve
```

```json
{
  "attempt_key": "gongbu:attempt:01JDEF456",
  "executor": "gongbu",
  "provider": "example-image-provider",
  "model": "image-model-v1",
  "maximum_cost_cents": 30,
  "currency": "usd",
  "pricing_basis": {
    "unit_price_cents": 15,
    "pricing_unit": "image",
    "maximum_units": 2
  }
}
```

Hubu accepts only when:

- the mandate exists and is `open`
- owner, agent, executor, currency, merchant, and workload scope match
- `maximum_cost_cents` is positive
- the attempt limit is not exceeded
- sufficient mandate amount is available
- no other active attempt exists in the sequential MVP

A successful response atomically reserves the amount, claims the attempt,
extends that attempt into its own execution lease, and returns the claimed
attempt plus the updated mandate balance. Receipt of this response is the
authorization boundary for irreversible provider work.

The `pricing_basis` is audit metadata. Hubu need not independently price the
provider request in the MVP. It is a pre-call estimate and upper bound because
the model output and final billed usage may not yet be known. Gongbu must cap
the controllable inputs and outputs so the reserved maximum bounds the possible
provider charge.

### 4. Execute Provider Work

Gongbu preflights artifact storage before the provider call, then invokes the
provider using its own credentials and the stable vendor idempotency key when
supported.

Gongbu must not make the call when reserve returns a denial, conflict, expired
state, or ambiguous failure. After an ambiguous Hubu response, it first
retrieves the attempt by identity.

### 5a. Settle Billable Work

```http
POST /spend/mandates/{mandate_id}/attempts/{attempt_key}/settle
```

```json
{
  "receipt": {
    "actual_vendor_cost_cents": 24,
    "currency": "usd",
    "provider_request_id": "provider-request-abc123",
    "price_model_snapshot": {
      "provider": "example-image-provider",
      "model": "image-model-v1",
      "unit_price_cents": 12,
      "pricing_unit": "image",
      "billed_units": 2,
      "currency": "usd"
    },
    "artifact_reference": "artifact://hubu-logo/candidate-a.png"
  }
}
```

The actual cost must satisfy:

```text
0 <= actual_vendor_cost <= reserved maximum
```

Hubu atomically stores the receipt, settles the attempt, updates the mandate,
and updates the underlying agent-budget balance. An identical retry returns the
stored settlement. A changed receipt is rejected.

Artifact persistence should complete before normal settlement. If the provider
billed but artifact persistence failed, Gongbu must still settle the charge and
use an audit-safe failure reference rather than release the reservation.

### 5b. Release Unbilled Work

```http
POST /spend/mandates/{mandate_id}/attempts/{attempt_key}/release
```

Release is permitted only when no irreversible billable provider work occurred.
Hubu atomically returns the entire reservation to mandate availability. An
identical retry returns the stored release.

### 6. Close The Mandate

```http
POST /spend/mandates/{mandate_id}/close
```

```json
{
  "task_key": "codex:task:01JABC123",
  "outcome": "completed"
}
```

The MVP accepts `completed` or `cancelled`.

When Gongbu has completed its configured routing plan and will schedule no more
attempts, it invokes the Hubu-provided `links.close` callback with the mandate
bearer token. This is a Gongbu-to-Hubu completion callback, not a webhook from
Hubu to Gongbu. The agent platform may also request cancellation through Hubu
when the user abandons the task.

If no attempt is active or awaiting reconciliation, Hubu returns unused
authorization to the agent budget and makes the mandate terminal. If an attempt
is active or uncertain, Hubu changes the mandate to `closing`, rejects new
reservations, and completes closure only after every attempt is terminal.

The response includes:

```json
{
  "mandate_id": "spm_example",
  "status": "completed",
  "authorized_amount_cents": 200,
  "consumed_amount_cents": 42,
  "released_amount_cents": 158,
  "currency": "usd"
}
```

An identical close retry returns the stored terminal result.

## Cost Bounding

Strict budget control requires an upper bound before a provider call. Gongbu
must derive `maximum_cost_cents` as a conservative pre-call estimate from
provider configuration such as:

- model and version
- output count
- dimensions and quality
- input/output units where applicable
- explicitly bounded retry behavior
- taxes or platform fees when they are part of the vendor charge

If Gongbu cannot conservatively bound a provider call, that call is not
eligible for strict-budget execution under this protocol.

For output-metered models, Gongbu cannot predict the exact result. It must use
a provider-supported output limit or another contractual maximum in the bound.
If neither exists, the provider is unsuitable for this strict MVP.

Hubu rejects settlement above the reserved maximum. The draft does not define
an automatic tolerance or post-hoc mandate overage.

## Retry And Failure Rules

| Situation | Required action |
| --- | --- |
| Hubu reserve response is lost | Look up the same `attempt_key`; do not call the provider until reservation is confirmed |
| Provider rejects before billable work | Release the attempt |
| Provider returns a normal billable result | Persist artifact, then settle |
| Provider billed but artifact persistence fails | Settle with an audit-safe failed-artifact reference |
| Provider timeout with idempotent status lookup | Recover provider status, then settle or release |
| Provider timeout with uncertain billing | Do not retry blindly; allow the attempt to require reconciliation |
| Hubu settlement response is lost | Inspect the attempt, then retry identical settlement with the same attempt key and receipt; the capability recovery grace permits inspection and replay of a stored settlement |
| Gongbu process restarts | Recover mandate and attempt state from Hubu before scheduling more work |

Retries performed internally by a provider SDK must be included in the reserved
maximum or disabled. A retry that can create a separate charge is a separate
attempt.

## Expiry And Reconciliation

An expired mandate accepts no new reservations.

An expired attempt that may have crossed the provider billability boundary is
not automatically released. It becomes `reconciliation_required`, and its
amount remains reserved. A human with the separate reconciliation capability
reviews provider evidence and records one outcome:

- `vendor_billed`: settle with an actual-cost receipt
- `vendor_did_not_bill`: release with provider reference and evidence

After all attempts are terminal, an expired or closing mandate returns unused
authorization and becomes terminal. Gongbu must never receive the human
reconciliation capability.

The first end-to-end MVP may expose reconciliation through existing operator
surfaces rather than implementing every v5-specific UI, but it must not
automatically treat ambiguous billing as unbilled.

## Artifact And Host-Managed Work

Gongbu owns the artifact store, downloads provider outputs, normalizes them,
persists their contents and metadata, and returns stable artifact references.
Hubu stores only compact references needed to link governed spend with produced
work; it never stores or serves the normalized artifact.

Host-managed tools such as Codex built-in ImageGen do not consume the external
vendor mandate because Hubu cannot observe or settle their product-managed
cost. A combined artifact manifest may record them as:

```json
{
  "provider": "codex_builtin",
  "billing_mode": "host_managed",
  "hubu_cost_cents": null,
  "artifact_reference": "artifact://host-managed/candidate.png"
}
```

Such an artifact has no mandate attempt. Hubu must not record a fabricated
zero-cost vendor settlement for it.

## MVP Interoperability Profile

Hubu and Gongbu can implement in parallel against this minimum profile:

- one open mandate per `(agent_id, task_key)`
- USD integer-cent accounting
- one active attempt at a time
- at most four attempts per mandate
- one provider invocation per attempt
- reserve atomically creates a claim with a separate execution lease
- reserve, settle, release, inspect, and close operations
- actual cost no greater than reserved maximum
- stable platform task keys and Gongbu attempt keys
- one Hubu-issued mandate bearer capability passed from the platform to Gongbu
- capability lifetime covers every claim lease Hubu can issue plus a positive
  finalization recovery grace period
- durable idempotency across process restart
- compact provider receipt and artifact reference
- no automatic release of uncertain provider work
- v4 single-spend behavior remains unchanged

Anything outside this list is optional until the draft advances.

## Parallel Implementation Boundary

Hubu can proceed independently with:

- mandate, balance, and attempt domain records
- database schema and atomic transitions
- owner/executor authorization
- idempotency and scope mismatch handling
- HTTP guidance and provisional routes
- expiry and reconciliation state
- concurrency and restart tests

Gongbu can proceed independently with:

- mandate-aware job and artifact manifest shapes
- Gongbu-owned normalized artifact storage
- static category-based provider routing configuration
- stable attempt-key generation and persistence
- provider adapter cost-bound functions
- provider idempotency mapping
- artifact destination preflight and persistence
- reserve-before-call and settle/release client workflow
- ambiguous timeout recovery
- two-provider sequential demo orchestration

The shared compatibility fixtures should cover:

1. create or recover a `$2.00` mandate
2. reserve `$0.30`, settle `$0.24`
3. reserve `$0.25`, release it
4. reserve and settle a second provider
5. reject a reservation above current availability
6. close and report authorized, consumed, and released totals
7. replay every request without changing balances
8. reject every reused identity whose scope or receipt changed

## Known Gaps And Later Extensions

- Define a stable cross-platform derivation convention for `task_key`.
- Specify the exact signed bearer-token claims, format, rotation, and
  revocation behavior.
- Decide whether mandate closure completes synchronously or reports `closing`
  and requires later inspection.
- Define sub-cent fee accumulation without weakening integer-cent enforcement.
- Enable and limit concurrent active attempts after the sequential MVP.
- Treat mandate amendment as a conditional state transition. The MVP does not
  support amendment; later work can define which lifecycle, policy, and balance
  conditions allow it.
- Explore partial or incremental mandate funding to improve budget utilization.
  The MVP deliberately freezes the full maximum at creation.

These questions do not change the MVP requirement that every billable call has
its own prior reservation and that Hubu is the authoritative balance owner.
