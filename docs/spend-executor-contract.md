# Spend Executor Contract

> This v4 contract maps one authorization to one potentially billable executor
> call. Multi-call tasks use multiple independent v4 operations.

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

The current version is `hubu-spend-executor-v4.3`. Agents and executors can
discover its machine-readable guidance from either public route:

```http
GET /spend/executor/guidance
GET /.well-known/hubu-spend-executor.json
```

V4.3 renames Hubu's `workload_profile` field to `lease_profile`, makes the
authorization TTL global, and limits each lease profile to its claim TTL.
Gongbu workload types remain independent execution-plane identities and no
longer have to equal a Hubu lease profile. The breaking payload and guidance
shape makes v4.3 intentionally startup-incompatible with v4.2.

V4.2 introduced the read-only `POST /spend/executor/resolve` capability used
for token-only executor admission.

The unified MCP continuation binding added for HUB-126 does not change the v4.3
Hubu-to-executor wire shape. It makes the existing `auth_token_id` /
`spend_auth_token_id` the agent-visible continuation identifier for one private
normalized operation. The agent never supplies or receives `operation_key` on
Gongbu tools. Gongbu learns it only from authenticated Hubu resolution and
retains it internally for claim, idempotency, settlement, and recovery.

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
scope consisting of its merchant, maximum amount, lease profile, and
operation purpose. An HTTP retry of the same provider invocation reuses that
operation. A retry or alternate model call that may create another charge uses
a new operation key and requires a new authorization.

The platform may retain a shared task identifier to correlate operations,
artifacts, and presentation. Hubu does not treat that correlation identifier as
a pooled allocation: v4 enforces each operation maximum and the authoritative
agent budget, not a separate task-level aggregate ceiling.

One authorization covering several charges under a shared maximum is outside
v4 and requires a new protocol version.

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

## Lease Profiles

The authorization request selects a `lease_profile`. Hubu snapshots that
profile into the spend decision, so the executor cannot exchange a short job for
a longer claim lease. The global authorization TTL controls how quickly any
authorized operation must start; the selected profile controls how long the
claimed execution may run.

Operators configure one authorization start window and a bounded set of lease
profiles with `HUBU_LEASE_CONFIG` pointing to a YAML file:

```yaml
authorization_ttl_seconds: 300
default_lease_profile: interactive
lease_profiles:
  interactive:
    claim_ttl_seconds: 900
  long_running:
    claim_ttl_seconds: 3600
  batch:
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

The unified Hubu MCP transport derives this key from supported trusted harness
call metadata, persists the normalized identity in its own SQLite registry, and
rejects identity collisions before backend access. Direct HTTP and diagnostic
CLI clients still supply an explicit key; this contract defines the server-side
invariant every adapter must satisfy.

Hubu rejects unknown profiles and non-positive durations during startup or
authorization. The effective lease configuration is published in executor
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
     "task_id": "release:artwork",
     "execution_scope": {
       "schema_version": 1,
       "provider": "provider:google:gemini-developer",
       "executor": "executor:gongbu:image",
       "capability": "capability:image:generate",
       "billing_merchant": "merchant:google"
     },
     "reason": "hubu-logo-demo",
     "lease_profile": "long_running"
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

   When policy returns `needs_approval`, the response also includes a durable
   `approval` object:

   ```json
   {
     "decision": "needs_approval",
     "auth_token_id": null,
     "approval": {
       "approval_request_id": "<spend decision id>",
       "status": "pending",
       "review": {
         "operation_key": "codex:tool-call:01JABC123",
         "account_id": "aga_example",
         "agent_id": "agt_example",
         "amount_cents": 500,
         "currency": "usd",
         "lease_profile": "long_running",
         "reason": "hubu-logo-demo",
         "policy_summary": "policy defaulted to needs_approval because no automatic-allow rule matched"
       }
     }
   }
   ```

   No token, hold, claim, payment, or provider work exists while approval is
   pending. The client shows the complete `approval.review` object to the human
   and waits for an explicit decision. It may recover the durable state with:

   ```http
   GET /spend/approval?approval_request_id=<spend decision id>
   ```

   The owner-authenticated client then submits exactly one decision:

   ```http
   POST /spend/approval/resolve
   X-Hubu-Approval-Capability: <owner-only capability>
   ```

   ```json
   {
     "approval_request_id": "<spend decision id>",
     "decision": "approve"
   }
   ```

   The normal API bearer is insufficient for this route; the server also verifies
   the owner-only approval capability, which must not be shared with executors.
   `decision` is `approve` or `deny`. Approval reserves the original immutable
   maximum and returns the normal authorization token; it never invokes the
   provider. Denial is terminal for that immutable request. Repeating the same
   resolution or exact authorization request returns the stored result, while
   a conflicting resolution is rejected.

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
   selection to canonical `POST /v2/executions` with `schema_version: 2`. It
   does not repeat account, operation key, money, typed scope, task ID, reason,
   lease profile, or expiry.

   Original v1 clients send `hubu_authorization_id` and
   `hubu_token_reference` as equal historical aliases of the same spend-auth
   token ID; neither field contains Hubu's decision ID. V1 also carries
   `operation_key`, a null `hubu_claim_id`, `authorization`, and optional
   `execution_scope` as compatibility assertions. Unequal token aliases fail
   before Hubu resolution, and every other mismatch fails before persistence or
   scheduling. V1 is supported through all `0.1.x` releases and removed in
   `0.2.0`.

   On the unified MCP surface, the router first requires the continuation ID to
   match exactly one allowed normalized operation and binds the first canonical
   execution intent to it. It rejects changed intent or any model-authored
   operation identity, endpoint, credential, retry control, trusted `task_id`,
   or protected lifecycle state before the Gongbu request. The public operation
   handle remains visible for correlation but is not authority and is not an
   execution-creation input.

3. Before persistence, the executor performs a read-only resolution:

   ```http
   POST /spend/executor/resolve
   ```

   ```json
   {"spend_auth_token_id":"00000000-0000-4000-8000-000000000123"}
   ```

   Hubu returns the authoritative authorization snapshot without claiming it.
   For a new execution, its account and agent are the authoritative attribution;
   the executor caller credential contributes no execution identity. The
   executor independently derives its operator-controlled target, typed scope,
   and catalog price and requires exact operation, price, currency, workload,
   and scope agreement before persisting that attribution snapshot.

   Before this resolution, the executor checks for a persisted execution by the
   same token. An exact immutable request replays locally even after the token
   was claimed or settled, without resolving Hubu again. A changed request
   conflicts. The persisted execution agent is used for claim settlement or
   release.

   Gongbu's HTTP response still contains `operation_key` on this private
   backend contract. The unified MCP router verifies it against its bound
   normalized operation, persists Gongbu's execution ID and lifecycle state,
   and removes the key recursively from all agent-facing content, structured
   content, errors, failure text, and status projections. A returned execution
   ID conflict fails closed. Exact replay can only recover the same Gongbu
   execution; payload-similarity inference and ambiguous provider retry remain
   out of scope.

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
   [trusted execution scope](spend-lifecycle.md#trusted-execution-scope). New
   callers omit `merchant`.

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
  "lease_profile": "long_running",
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
    "lease_profile": "long_running",
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
