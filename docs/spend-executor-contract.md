# Spend Executor Contract

> The proposed task-scoped mandate extension is deferred. This v4 contract is
> normative: one authorization funds one potentially billable executor call.
> Multi-call tasks use multiple independent v4 operations.

This contract lets an external work service use Hubu for spend control without
turning Hubu into the service that performs the work:

```text
Hubu controls spend; executors do work.
```

Gongbu can implement this contract for model calls, image generation, or other
vendor-backed work. Hubu does not receive vendor credentials, provider payloads,
prompts, or artifact contents. It does retain compact provider and artifact
references needed for settlement auditability.

## Protocol Version

The current version is `hubu-spend-executor-v4.1`. Agents and executors can
discover its machine-readable guidance from either public route:

```http
GET /spend/executor/guidance
GET /.well-known/hubu-spend-executor.json
```

V4 retains V3's immutable, platform-provided `operation_key` from authorization
through claim and finalization. Hubu stores workflow state under
`(agent_id, operation_key)`. Retrying with the same key and scope returns that
same workflow. Two agents owned by the same user may use the same operation
key; one agent may not reuse an operation key for different work.
V2's separate `executor_execution_id` is no longer part of the public contract.
V4 adds durable provider receipts and actual-cost settlement to V3's exclusive
claims, owner-scoped lookup, and human-gated reconciliation. Settlement consumes
the actual vendor cost and releases the remainder of the authorized maximum.
`POST /spend/executor/validate` remains available for scope inspection, but
validation alone does not authorize irreversible work.

## Multiple Invocations In One Agent Task

The agent platform orchestrates tasks that need more than one model or provider
invocation. It creates a separate v4 spend operation for every call that may
produce a distinct vendor charge:

```text
agent task
├── operation A: authorize -> claim -> invoke -> settle or release
├── operation B: authorize -> claim -> invoke -> settle or release
└── operation C: authorize -> claim -> invoke -> settle or release
```

Each operation has a unique, durable `operation_key` and an immutable invocation
scope consisting of its merchant, maximum amount, workload profile, and
operation purpose. An HTTP retry of the same provider invocation reuses that
operation. A retry or alternate model call that may create another charge uses
a new operation key and requires a new authorization.

The platform may retain a shared task identifier to correlate operations,
artifacts, and presentation. Hubu does not treat that correlation identifier as
a pooled allocation: v4 enforces each operation maximum and the authoritative
agent budget, not a separate task-level aggregate ceiling.

The deferred [multi-spend mandate design](multi-spend-mandate-protocol.md) may
be revisited if dogfooding demonstrates a need for one authorization covering
several charges under a hard shared maximum.

## Boundary

Hubu is responsible for:

- agent and owner identity
- policy evaluation and spend authorization tokens
- authoritative workflow state keyed by agent and operation
- one agent-budget hold per spend decision
- exclusive executor claims and claim leases
- actual-cost budget settlement and release of unused authorization
- durable provider request, price/model, and artifact references
- audit events for spend state transitions
- durable provider references and evidence for human reconciliation decisions

Inside Hubu, `hubu-core::app::ExecutorClaimService` owns claim creation,
owner-scoped lookup, reconciliation queue selection, executor settlement or
release, and human reconciliation orchestration. The HTTP API remains the
transport and authentication boundary, while the SQLite repository atomically
commits receipt, claim, token, hold, and budget-balance transitions.

Agent platforms or orchestrators are responsible for:

- supplying one stable, namespaced `operation_key` for each logical operation
- reusing that key for every authorization retry
- keeping operation identity outside the language model's conversational memory

Executors are responsible for:

- storing vendor API keys and other execution secrets outside Hubu
- resolving the immutable `operation_key` from Hubu and carrying it into the
  durable claim/finalization requests
- claiming Hubu authorization before irreversible work
- calling vendors or tools
- reporting the actual vendor cost with a provider request ID, price/model
  snapshot, and artifact reference
- settling after billable work or releasing before billable work

## Timing Profiles

The authorization request selects a `workload_profile`. Hubu snapshots that
profile into the spend decision, so the executor cannot exchange a short job for
a longer claim lease. The default profile gives authorization five minutes to
start and gives a successful claim a fifteen-minute execution lease.

Operators can load different profiles with `HUBU_SPEND_TIMING_CONFIG` pointing
to a YAML file:

```yaml
default_profile: interactive
profiles:
  interactive:
    authorization_ttl_seconds: 300
    claim_ttl_seconds: 900
  image_generation:
    authorization_ttl_seconds: 300
    claim_ttl_seconds: 3600
  batch:
    authorization_ttl_seconds: 900
    claim_ttl_seconds: 14400
```

## Operation Key Generation and Storage

The agent platform or orchestrator—not the language model—owns the operation
key lifecycle. It should use a durable platform operation ID with a namespace,
for example `codex:tool-call:01J...`. If the platform has no suitable ID, its
adapter should generate and persist an opaque key before the first attempt.
Operation keys are compared case-sensitively after trimming whitespace.

Hubu is the authoritative store for workflow state under the agent-scoped key.
Replaying authorization with the same agent, operation key, and scope recovers
the decision, token, hold, and current workflow state. Claim and finalization
use that same key.

Hubu journals immutable, monotonically numbered authorization attempts beneath
the stable key. A changed scope is admitted only when every prior attempt has an
explicit terminal denial and no token, pending approval, hold, claim, dispatch,
payment, or settlement side effect exists. Admission uses an immediate SQLite
transaction, so process-local locking is not the concurrency authority. Exact
scope replay can recover any historical attempt, including a denial that
precedes a later allowed attempt.

Do not derive the operation key from mutable request fields, generate it inside
a retry loop, or ask the model to invent or remember it. A reusable skill can
teach the protocol, but enforcement belongs in the platform adapter or SDK and
Hubu's database constraints.

The current Hubu MCP transport accepts this key as an explicit input but does
not yet derive it from trusted platform invocation metadata. That adapter is a
separate integration layer; this contract defines the server-side invariant it
must satisfy.

Hubu rejects unknown profiles and non-positive durations during startup or
authorization. The effective timing configuration is published in executor
guidance.

## Flow

1. An agent asks Hubu to authorize a scoped executor action:

   ```http
   POST /spend/authorize
   ```

   ```json
   {
     "operation_key": "codex:tool-call:01JABC123",
     "account_id": "aga_example",
     "amount_cents": 500,
     "task_id": "linear:HUB-73",
     "execution_scope": {
       "schema_version": 1,
       "provider": "provider:google:gemini-developer",
       "executor": "executor:gongbu:image",
       "capability": "capability:image:generate",
       "billing_merchant": "merchant:google"
     },
     "reason": "hubu-logo-demo",
     "workload_profile": "image_generation"
   }
   ```

   Hubu returns the original `operation_key`, `auth_token_id`, decision, and
   frozen agent-budget hold. Retrying the same operation key and scope returns
   those same records. After a side-effect-free terminal denial, a corrected
   scope may append a new revision under the same key. Once any revision is
   pending approval, allowed, or side-effect-capable, changed scope is rejected.
   Another agent may independently use the same operation key.

   Every response includes `revision`, `idempotent_replay`, `attempt_history`,
   and `retry_guidance`. Retry guidance has one machine-readable action:
   `reuse_operation_key`, `replay_exactly`, or `create_new_operation`.

   `operation_key`, `task_id`, and `reason` are independent. The first is the
   trusted financial/idempotency identity, the second is an optional trusted
   external correlation, and the third is human-readable authorization and
   audit context. For compatibility, an absent `task_id` maps `reason` into the
   stored task ID; explicit null means no task correlation.

   ```json
   {
     "operation_key": "codex:tool-call:01JABC123",
     "decision": "allow",
     "auth_token_id": "00000000-0000-4000-8000-000000000123"
   }
   ```

2. The agent sends only `spend_auth_token_id` plus execution intent and target
   selection to the executor. It does not repeat account, operation key, money,
   typed scope, task ID, reason, workload profile, or expiry.

3. Before persistence, the executor performs a read-only resolution:

   ```http
   POST /spend/executor/resolve
   ```

   ```json
   {"spend_auth_token_id":"00000000-0000-4000-8000-000000000123"}
   ```

   Hubu returns the authoritative authorization snapshot without claiming it.
   The executor independently derives its operator-controlled target, typed
   scope, and catalog price and requires exact identity, price, currency,
   workload, and scope agreement before persistence. An existing execution may
   replay locally by the same token and immutable intent after a claim or
   restart.

4. After persistence and before irreversible work, the durable executor claims
   the authorization:

   ```http
   POST /spend/executor/claim
   ```

   ```json
   {
     "operation_key": "codex:tool-call:01JABC123",
     "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
     "account_id": "aga_example",
     "amount_cents": 500,
     "execution_scope": {
       "schema_version": 1,
       "provider": {"id":"provider:google:gemini-developer","display_name":"Google Gemini Developer API"},
       "executor": {"id":"executor:gongbu:image","display_name":"Gongbu image executor"},
       "capability": {"id":"capability:image:generate","display_name":"Generate image"},
       "billing_merchant": {"id":"merchant:google","display_name":"Google"}
     }
   }
   ```

   Hubu accepts only if the token is unexpired, unused, unrevoked, unclaimed,
   matches the authorized operation and scope, and has a frozen agent-budget
   hold. Hubu resolves task ID and reason from the stored authorization rather
   than trusting executor input. A legacy executor may send `task_id` as a
   compatibility assertion, but a mismatch is rejected. A retry with the same
   operation key returns the existing claim,
   including its terminal state if it has already been settled or released.

   The version-1 typed scope and legacy migration behavior are specified in
   [Trusted execution scope](execution-scope.md). New callers omit `merchant`.

   Claiming moves the hold to `claimed` and extends its expiry to
   `claim_expires_at`. The claim may remain active after the original
   authorization expires.

5. The executor performs work using its own credentials.

6. After irreversible billable work succeeds, the executor settles:

   ```http
   POST /spend/executor/settle
   ```

   ```json
   {
     "agent_id": "agt_example",
     "operation_key": "codex:tool-call:01JABC123",
     "receipt": {
       "actual_vendor_cost_cents": 400,
       "provider_request_id": "provider-request-abc123",
       "price_model_snapshot": {
         "provider": "example-image-provider",
         "model": "image-model-v1",
         "unit_price_cents": 400,
         "pricing_unit": "image",
         "currency": "usd"
       },
       "artifact_reference": "artifact://hubu-logo.png"
     }
   }
   ```

   Hubu persists the receipt, marks the claim settled and token used, consumes
   the 400-cent actual cost, and releases the 100-cent remainder in one SQLite
   write transaction. Actual cost cannot be negative or exceed the 500-cent
   authorized maximum. Hubu resolves the claim from the agent and operation key,
   so an identical retry returns the original `settlement_id` and receipt even
   if the caller lost the response. A retry with changed receipt data is
   rejected, and budget is not consumed twice.

7. If no irreversible billable work occurred, the executor releases without a
   receipt:

   ```http
   POST /spend/executor/release
   ```

   ```json
   {
     "agent_id": "agt_example",
     "operation_key": "codex:tool-call:01JABC123"
   }
   ```

   Hubu atomically marks the claim released and token revoked while returning
   the held amount to the remaining budget. An identical release retry returns
   the stored terminal state without returning the budget twice.

## Claim Response

```json
{
  "operation_key": "codex:tool-call:01JABC123",
  "claim_id": "uuid",
  "workload_profile": "image_generation",
  "status": "claimed",
  "claimed_at": "2026-07-20T12:04:00Z",
  "claim_expires_at": "2026-07-20T13:04:00Z",
  "finalized_at": null,
  "settlement_id": null,
  "reconciliation_required": false,
  "reconciliation_outcome": null,
  "provider_reference": null,
  "evidence": null,
  "reconciled_at": null,
  "reconciled_by_user_id": null,
  "spend": {
    "operation_key": "codex:tool-call:01JABC123",
    "spend_auth_token_id": "uuid",
    "decision_id": "uuid",
    "account_id": "aga_...",
    "agent_id": "agt_...",
    "amount_cents": 500,
    "currency": "usd",
    "merchant": null,
    "execution_scope": {
      "schema_version": 1,
      "provider": {"id":"provider:google:gemini-developer","display_name":"Google Gemini Developer API"},
      "executor": {"id":"executor:gongbu:image","display_name":"Gongbu image executor"},
      "capability": {"id":"capability:image:generate","display_name":"Generate image"},
      "billing_merchant": {"id":"merchant:google","display_name":"Google"}
    },
    "task_id": "hubu-logo-demo",
    "reason": "Generate the Project Hubu logo",
    "workload_profile": "image_generation",
    "status": "claimed",
    "expires_at": "2026-07-20T12:05:00Z",
    "budget_hold": {
      "hold_id": "uuid",
      "budget_id": "bgt_...",
      "status": "claimed",
      "amount_cents": 500,
      "consumed_amount_cents": 0,
      "frozen_amount_cents": 500,
      "remaining_amount_cents": 0
    }
  }
}
```

`spend.expires_at` is the original authorization deadline;
`claim_expires_at` is the separate execution lease.

The authenticated owner can inspect any claim, including terminal claims:

```http
GET /spend/executor/claim?claim_id=CLAIM_ID
```

The response includes `reconciliation_required`, terminal settlement details,
and any stored reconciliation outcome, provider reference, evidence, resolving
user, and timestamp.

## Settlement Response

```json
{
  "operation_key": "codex:tool-call:01JABC123",
  "settlement_id": "uuid",
  "claim_id": "uuid",
  "status": "settled",
  "receipt": {
    "authorized_max_cents": 500,
    "actual_vendor_cost_cents": 400,
    "released_amount_cents": 100,
    "currency": "usd",
    "provider_request_id": "provider-request-abc123",
    "price_model_snapshot": {
      "provider": "example-image-provider",
      "model": "image-model-v1",
      "unit_price_cents": 400,
      "pricing_unit": "image",
      "currency": "usd"
    },
    "artifact_reference": "artifact://hubu-logo.png",
    "created_at": "2026-07-20T12:05:00Z"
  }
}
```

## Transactional Finalization

Hubu captures one `settlement_started_at` value after parsing and authenticating
the request. Settle and release each obtain an immediate SQLite write lock,
check the claim against that timestamp, and atomically update:

- the executor claim to `settled` with a stable `settlement_id`, or `released`
- the spend authorization token to `used`, or `revoked`
- the claimed budget hold to `settled`, or `released`
- the immutable provider receipt for settlement
- the budget balance from frozen to actual cost consumed plus the unused
  remainder returned, or the full hold returned for release

This removes the internal race where token and claim state could commit before
the hold and balance. It also serializes settle against release so the first
terminal transaction wins. If any update fails, SQLite rolls all five changes
back. A claim that was active when the transaction began can complete even if
wall-clock time passes its lease during the transaction; a claim at or past
expiry when the transaction begins is rejected for reconciliation.

## Expired Claims

Hubu does not automatically release a claimed hold when its lease expires. The
vendor may have completed work while the executor failed before settlement, so
automatic release could make billed work disappear from governed consumption.
An expired claim remains frozen and normal settle/release requests reject it as
requiring reconciliation. The authenticated owner can list only their expired,
still-claimed work:

```http
GET /spend/executor/reconciliation
```

After checking provider billing, a human chooses one terminal outcome. If the
vendor billed, the human settles the hold:

```http
POST /spend/executor/settle
X-Hubu-Reconciliation-Capability: HUMAN_CAPABILITY
```

```json
{
  "claim_id": "00000000-0000-4000-8000-000000000456",
  "provider_reference": "vendor-request-abc123",
  "evidence": "Provider usage export confirms the completed billed request.",
  "receipt": {
    "actual_vendor_cost_cents": 400,
    "provider_request_id": "vendor-request-abc123",
    "price_model_snapshot": {
      "provider": "example-image-provider",
      "model": "image-model-v1",
      "unit_price_cents": 400,
      "pricing_unit": "image",
      "currency": "usd"
    },
    "artifact_reference": "artifact://hubu-logo.png"
  }
}
```

If the vendor did not bill, the human releases the hold with the same request
shape:

```http
POST /spend/executor/release
X-Hubu-Reconciliation-Capability: HUMAN_CAPABILITY
```

The existing settle/release endpoints therefore accept one of two exclusive
request shapes: the normal executor shape with `agent_id` plus the immutable
`operation_key`, or the human reconciliation shape with `claim_id`,
`provider_reference`, and `evidence`. Mixing the shapes is rejected. The
vendor-billed shape also requires the provider receipt; vendor-did-not-bill
rejects one. Reconciliation requires non-empty evidence fields, accepts only an
expired claim owned by the active user, and atomically updates the receipt,
claim, token, hold, and budget balance. A matching retry returns the stored
outcome; a retry with different evidence or receipt is rejected. Reconciliation
records the outcome, provider reference, evidence, resolving user, and
timestamp. Evidence must not contain vendor credentials or sensitive provider
payloads.

The reconciliation capability is loaded separately from
`HUBU_RECONCILIATION_TOKEN` or `HUBU_RECONCILIATION_TOKEN_FILE`. It must not
equal or be distributed with the normal Hubu bearer token. Executors receive
only the normal bearer, while human-facing CLI/MCP administration receives the
reconciliation capability. The server validates both credentials before
entering the reconciliation transaction.

The CLI is the direct operator surface:

```sh
hubu spend claim --claim-id CLAIM_ID
hubu spend reconcile list
hubu spend reconcile billed --claim-id CLAIM_ID \
  --provider-reference VENDOR_REFERENCE \
  --evidence "Provider usage export confirms billing" \
  --actual-vendor-cost-cents 400 \
  --provider-request-id VENDOR_REQUEST_ID \
  --provider example-image-provider \
  --model image-model-v1 \
  --unit-price-cents 400 \
  --pricing-unit image \
  --artifact-reference artifact://hubu-logo.png
hubu spend reconcile not-billed --claim-id CLAIM_ID \
  --provider-reference VENDOR_REFERENCE \
  --evidence "Provider billing search found no charge"
```

MCP exposes the same lookup and queue as read-only tools. Its billed and
not-billed resolution tools are protected administrative tools: Hubu advertises
`prompt_before_call`, and the MCP adapter refuses them unless it is configured
to trust a client-side human approval gate. Even after that gate, the server
requires the distinct reconciliation capability, so direct executor HTTP calls
with only the normal bearer are rejected.

## Safety Rules

- Executors must claim before irreversible work; validation alone is insufficient.
- Agent platforms must supply one stable operation key and reuse it for
  authorization, claim, finalization, and every retry.
- Executors must settle after irreversible billable work succeeds.
- Settlement must report actual vendor cost and immutable provider receipt
  metadata; actual cost cannot exceed the authorized maximum.
- Executors must release only when no irreversible billable work occurred.
- Agents and executors may retry any stage after an ambiguous response; Hubu
  returns stored workflow state for the same operation key and rejects changed
  spend scope.
- Executors must not resolve expired claims. A human must review provider
  billing and choose the reconciliation outcome.
- Never distribute the human reconciliation capability to an executor.
- Hubu never stores executor vendor secrets through this contract.
