# Spend Executor Contract

This document defines the Hubu spend executor contract. The contract lets an
external work service use Hubu for spend control without turning Hubu into the
service that performs the work.

The core rule is:

```text
Hubu controls spend; executors do work.
```

Gongbu can implement this contract for model calls, image generation, or other
vendor-backed work. Other services can implement the same contract without
Hubu knowing their vendor APIs, credentials, retries, or artifact formats.

## Boundary

Hubu is responsible for:

- agent and owner identity
- policy evaluation
- spend authorization tokens
- budget holds
- token validation against authorized scope
- budget settlement or release
- audit events for spend state transitions

Executors are responsible for:

- storing vendor API keys and other execution secrets outside Hubu
- accepting work requests from agents
- validating Hubu spend authorization before irreversible work
- calling vendors or tools
- storing or returning work artifacts
- settling or releasing the Hubu budget hold

Hubu must not require model prompts, vendor API keys, provider-specific payloads,
or execution artifacts in this protocol.

## Protocol Version

Current version:

```text
hubu-spend-executor-v1
```

Agents and executors can discover the contract from either route:

```http
GET /spend/executor/guidance
GET /.well-known/hubu-spend-executor.json
```

## Flow

1. An agent asks Hubu to authorize spend for a scoped executor action:

   ```http
   POST /spend/authorize
   ```

   Example body:

   ```json
   {
     "agent_id": "agt_example",
     "amount_cents": 500,
     "merchant": "gongbu.image",
     "reason": "hubu-logo-demo"
   }
   ```

   Hubu evaluates policy, issues a `spend_auth_token_id`, and freezes budget in
   a `budget_hold`.

2. The agent sends the work request and `spend_auth_token_id` to an executor.
   The executor keeps vendor-specific secrets server-side.

3. Before irreversible work, the executor validates the authorization:

   ```http
   POST /spend/executor/validate
   ```

   Example body:

   ```json
   {
     "spend_auth_token_id": "00000000-0000-4000-8000-000000000123",
     "agent_id": "agt_example",
     "amount_cents": 500,
     "merchant": "gongbu.image",
     "task_id": "hubu-logo-demo"
   }
   ```

   Hubu accepts only if the token is unexpired, unused, unrevoked, matches the
   authorized agent/account, amount, merchant, and task, and has a frozen budget
   hold.

4. The executor performs work with its own credentials.

5. If irreversible billable work succeeded, the executor settles:

   ```http
   POST /spend/executor/settle
   ```

   Hubu marks the auth token used and consumes the frozen budget hold.

6. If work will not be performed and no irreversible billable work happened, the
   executor releases:

   ```http
   POST /spend/executor/release
   ```

   Hubu returns the frozen amount to remaining budget. Future validation of the
   same token rejects the non-frozen hold.

## Request Fields

Executor validate, settle, and release currently share one request shape:

```json
{
  "spend_auth_token_id": "uuid",
  "agent_id": "agt_...",
  "account_id": null,
  "amount_cents": 500,
  "merchant": "gongbu.image",
  "task_id": "hubu-logo-demo"
}
```

Fields:

- `spend_auth_token_id`: required Hubu spend authorization token.
- `amount_cents`: required minor-unit amount. Currency is USD in v1.
- `agent_id` or `account_id`: exactly one is required.
- `merchant`: optional, but must match the authorization when present there.
- `task_id`: optional executor task scope. In the current Hubu demo API this
  matches the `/spend/authorize` `reason` field.

## Response Fields

Validation returns the matched spend scope and budget hold state:

```json
{
  "spend_auth_token_id": "uuid",
  "decision_id": "uuid",
  "account_id": "acct_...",
  "agent_id": "agt_...",
  "amount_cents": 500,
  "currency": "USD",
  "merchant": "gongbu.image",
  "task_id": "hubu-logo-demo",
  "expires_at": "2026-06-05T12:00:00Z",
  "budget_hold": {
    "hold_id": "uuid",
    "budget_id": "uuid",
    "status": "frozen",
    "amount_cents": 500,
    "consumed_amount_cents": 0,
    "frozen_amount_cents": 500,
    "remaining_amount_cents": 0
  }
}
```

Settlement wraps the same spend response with a Hubu settlement id:

```json
{
  "settlement_id": "uuid",
  "spend": {}
}
```

The settlement id is Hubu's local idempotency/audit handle for this v1 demo
surface. It is not a vendor payment id and does not imply Hubu has called the
vendor.

## Safety Rules

- Executors must validate before irreversible work.
- Executors must settle after irreversible billable work succeeds.
- Executors must release only when no irreversible billable work happened.
- Executors must not reuse a token after settlement or release.
- Hubu rejects validation when the budget hold is no longer frozen.
- Hubu never stores executor vendor secrets through this contract.

