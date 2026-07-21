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

The current version is `hubu-spend-executor-v2`. Agents and executors can
discover its machine-readable guidance from either public route:

```http
GET /spend/executor/guidance
GET /.well-known/hubu-spend-executor.json
```

V2 adds an exclusive, durable execution claim between authorization and vendor
work. `POST /spend/executor/validate` remains available for scope inspection,
but validation alone does not authorize irreversible work.

## Boundary

Hubu is responsible for:

- agent and owner identity
- policy evaluation and spend authorization tokens
- one agent-budget hold per spend decision
- exclusive executor claims and claim leases
- budget settlement or release
- audit events for spend state transitions

Executors are responsible for:

- storing vendor API keys and other execution secrets outside Hubu
- assigning one immutable `executor_execution_id` to each attempted job
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
     "account_id": "aga_example",
     "amount_cents": 500,
     "merchant": "gongbu.image",
     "reason": "hubu-logo-demo",
     "workload_profile": "image_generation"
   }
   ```

   Hubu returns `spend_auth_token_id`, `authorization_expires_at`, and a frozen
   agent-budget hold.

2. The agent sends the work request and `spend_auth_token_id` to the executor.

3. Before irreversible work, the executor claims the authorization:

   ```http
   POST /spend/executor/claim
   ```

   ```json
   {
     "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
     "account_id": "aga_example",
     "amount_cents": 500,
     "merchant": "gongbu.image",
     "task_id": "hubu-logo-demo",
     "executor_execution_id": "gongbu-job-123"
   }
   ```

   Hubu accepts only if the token is unexpired, unused, unrevoked, unclaimed,
   matches the authorized scope, and has a frozen agent-budget hold. A retry
   with the same execution ID returns the existing claim. A different execution
   ID cannot claim the token.

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
     "claim_id": "00000000-0000-4000-8000-000000000456",
     "executor_execution_id": "gongbu-job-123"
   }
   ```

   Hubu marks the claim settled and token used, then consumes the claimed hold.

6. If no irreversible billable work occurred, the executor releases using the
   same finalize request shape:

   ```http
   POST /spend/executor/release
   ```

   Hubu marks the claim released and token revoked, then returns the held amount
   to the remaining budget.

## Claim Response

```json
{
  "claim_id": "uuid",
  "executor_execution_id": "gongbu-job-123",
  "workload_profile": "image_generation",
  "status": "claimed",
  "claimed_at": "2026-07-20T12:04:00Z",
  "claim_expires_at": "2026-07-20T13:04:00Z",
  "spend": {
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

## Expired Claims

Hubu does not automatically release a claimed hold when its lease expires. The
vendor may have completed work while the executor failed before settlement, so
automatic release could make billed work disappear from governed consumption.
An expired claim remains frozen for a future reconciliation workflow and normal
settle/release requests reject it as requiring reconciliation.

## Safety Rules

- Executors must claim before irreversible work; validation alone is insufficient.
- Executors must use one stable execution ID for claim and finalization.
- Executors must settle after irreversible billable work succeeds.
- Executors must release only when no irreversible billable work occurred.
- Executors must not reuse a claim or token after finalization.
- Hubu never stores executor vendor secrets through this contract.
