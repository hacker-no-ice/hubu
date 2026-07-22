# Spend Executor Contract

This contract lets an external work service use Hubu for spend control without
turning Hubu into the service that performs the work:

```text
Hubu controls spend; executors do work.
```

Gongbu can implement this contract for model calls, image generation, or other
vendor-backed work. Hubu does not receive vendor credentials, provider payloads,
prompts, or execution artifacts.

## Protocol Version

The current version is `hubu-spend-executor-v3`. Agents and executors can
discover its machine-readable guidance from either public route:

```http
GET /spend/executor/guidance
GET /.well-known/hubu-spend-executor.json
```

V3 uses one immutable, platform-provided `operation_key` from authorization
through claim and finalization. Hubu stores workflow state under
`(agent_id, operation_key)`. Retrying with the same key and scope returns that
same workflow. Two agents owned by the same user may use the same operation
key; one agent may not reuse an operation key for different work.
V2's separate
`executor_execution_id` is no longer part of the public contract.
V3 also keeps the exclusive, durable execution claim introduced in V2 and adds
owner-scoped claim lookup plus human-gated reconciliation for expired,
uncertain claims. `POST /spend/executor/validate` remains available for scope
inspection, but validation alone does not authorize irreversible work.

## Boundary

Hubu is responsible for:

- agent and owner identity
- policy evaluation and spend authorization tokens
- authoritative workflow state keyed by agent and operation
- one agent-budget hold per spend decision
- exclusive executor claims and claim leases
- budget settlement or release
- audit events for spend state transitions
- durable provider references and evidence for human reconciliation decisions

Agent platforms or orchestrators are responsible for:

- supplying one stable, namespaced `operation_key` for each logical operation
- reusing that key for every authorization retry
- keeping operation identity outside the language model's conversational memory

Executors are responsible for:

- storing vendor API keys and other execution secrets outside Hubu
- carrying the immutable `operation_key` into executor requests
- claiming Hubu authorization before irreversible work
- calling vendors or tools
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
     "merchant": "gongbu.image",
     "reason": "hubu-logo-demo",
     "workload_profile": "image_generation"
   }
   ```

   Hubu returns the original `operation_key`, `auth_token_id`, decision, and
   frozen agent-budget hold. Retrying the same operation key and scope returns
   those same records; reusing the operation key with different scope for that
   agent is rejected. Another agent may independently use the same operation
   key.

   ```json
   {
     "operation_key": "codex:tool-call:01JABC123",
     "decision": "allow",
     "auth_token_id": "00000000-0000-4000-8000-000000000123"
   }
   ```

2. The agent sends the work request, `operation_key`, and
   `spend_auth_token_id` to the executor.

3. Before irreversible work, the executor claims the authorization:

   ```http
   POST /spend/executor/claim
   ```

   ```json
   {
     "operation_key": "codex:tool-call:01JABC123",
     "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
     "account_id": "aga_example",
     "amount_cents": 500,
     "merchant": "gongbu.image",
     "task_id": "hubu-logo-demo"
   }
   ```

   Hubu accepts only if the token is unexpired, unused, unrevoked, unclaimed,
   matches the authorized operation and scope, and has a frozen agent-budget
   hold. A retry with the same operation key returns the existing claim,
   including its terminal state if it has already been settled or released.

   Claiming moves the hold to `claimed` and extends its expiry to
   `claim_expires_at`. The claim may remain active after the original
   authorization expires.

4. The executor performs work using its own credentials.

5. After irreversible billable work succeeds, the executor settles:

   ```http
   POST /spend/executor/settle
   ```

   ```json
   {
     "agent_id": "agt_example",
     "operation_key": "codex:tool-call:01JABC123"
   }
   ```

   Hubu marks the claim settled and token used and consumes the claimed hold in
   one SQLite write transaction. Hubu resolves the claim from the agent and
   operation key, so an identical retry returns the original `settlement_id`
   even if the caller lost the claim response. Budget is not consumed twice.

6. If no irreversible billable work occurred, the executor releases using the
   same finalize request shape:

   ```http
   POST /spend/executor/release
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
    "merchant": "gongbu.image",
    "task_id": "hubu-logo-demo",
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

## Transactional Finalization

Hubu captures one `settlement_started_at` value after parsing and authenticating
the request. Settle and release each obtain an immediate SQLite write lock,
check the claim against that timestamp, and atomically update:

- the executor claim to `settled` with a stable `settlement_id`, or `released`
- the spend authorization token to `used`, or `revoked`
- the claimed budget hold to `settled`, or `released`
- the budget balance from frozen to consumed, or back to remaining

This removes the internal race where token and claim state could commit before
the hold and balance. It also serializes settle against release so the first
terminal transaction wins. If any update fails, SQLite rolls all four changes
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
```

```json
{
  "claim_id": "00000000-0000-4000-8000-000000000456",
  "provider_reference": "vendor-request-abc123",
  "evidence": "Provider usage export confirms the completed billed request."
}
```

If the vendor did not bill, the human releases the hold with the same request
shape:

```http
POST /spend/executor/release
```

The existing settle/release endpoints therefore accept one of two exclusive
request shapes: the normal executor shape with `agent_id` plus the immutable
`operation_key`, or the human reconciliation shape with `claim_id`,
`provider_reference`, and `evidence`. Mixing the shapes is rejected. The
reconciliation shape requires non-empty evidence fields, accepts only an
expired claim owned by the active user, and atomically updates the claim, token,
hold, and budget balance. A matching retry returns the stored outcome; a retry
with different evidence is rejected. Reconciliation records the outcome,
provider reference, evidence, resolving user, and timestamp. Evidence must not
contain vendor credentials or sensitive provider payloads.

The CLI is the direct operator surface:

```sh
hubu spend claim --claim-id CLAIM_ID
hubu spend reconcile list
hubu spend reconcile billed --claim-id CLAIM_ID \
  --provider-reference VENDOR_REFERENCE \
  --evidence "Provider usage export confirms billing"
hubu spend reconcile not-billed --claim-id CLAIM_ID \
  --provider-reference VENDOR_REFERENCE \
  --evidence "Provider billing search found no charge"
```

MCP exposes the same lookup and queue as read-only tools. Its billed and
not-billed resolution tools are protected administrative tools: Hubu advertises
`prompt_before_call`, and the MCP adapter refuses them unless it is configured
to trust a client-side human approval gate.

## Safety Rules

- Executors must claim before irreversible work; validation alone is insufficient.
- Agent platforms must supply one stable operation key and reuse it for
  authorization, claim, finalization, and every retry.
- Executors must settle after irreversible billable work succeeds.
- Executors must release only when no irreversible billable work occurred.
- Agents and executors may retry any stage after an ambiguous response; Hubu
  returns stored workflow state for the same operation key and rejects changed
  spend scope.
- Executors must not resolve expired claims. A human must review provider
  billing and choose the reconciliation outcome.
- Hubu never stores executor vendor secrets through this contract.
