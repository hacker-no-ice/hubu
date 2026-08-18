# Spend executor boundary

The canonical Hubu protocol is the in-repository
[Hubu spend executor contract](../spend-executor-contract.md). Gongbu's
production Hubu activities implement that v4.2 HTTP contract for resolution, claim,
inspection, settlement, and release. Sharing a repository and product version
does not turn this wire boundary into an in-process or shared-database call.

Hubu owns authorization, budget reservation, claims, settlement, release, and
their audit state. Gongbu owns operator-controlled provider configuration,
provider credentials, provider execution, normalized artifacts, cost
calculation, and settlement evidence.

The canonical v2 caller submits only `spend_auth_token_id` plus execution intent
and target to `POST /v2/executions`.
Gongbu resolves Hubu's read-only authorization snapshot before admission. Hubu
is authoritative for account, agent, `operation_key`, optional `task_id`,
`reason`, amount, currency, workload profile, expiry, status, and typed scope.
Gongbu independently derives the operator-controlled target, typed scope, and
catalog price and requires exact agreement before it persists or schedules.
Resolution never claims; the durable workflow claims only after persistence.
Any preview remains optional UX for obtaining the right authorization amount;
admission always recomputes from the active operator catalog and never trusts a
preview or caller-supplied price.
The original v1 `hubu_authorization_id` and `hubu_token_reference` names are
historical aliases of the same spend-auth token ID and must be equal after
trimming. Neither is a decision ID. The v1 `operation_key`, null
`hubu_claim_id`, `authorization`, and optional `execution_scope` remain
input-only compatibility assertions and must exactly match Hubu's resolved
snapshot and Gongbu's derived price and scope. Any mismatch fails before Hubu
resolution, persistence, scheduling, or provider work. V1 creation remains
available through all `0.1.x` releases and is removed in `0.2.0`; new callers
must use v2 and omit every legacy field.

Future execution work must use the persisted `Execution` aggregate. It must
create a `ProviderAttempt` before irreversible provider transmission and use a
stable persisted receipt for finalization. An ambiguous provider or settlement
outcome requires reconciliation; it must not trigger a blind retry or release.

Provider secrets belong to Gongbu's runtime identity. They must not be accepted
from callers, persisted in repository records, included in fixtures, exposed in
API responses, or emitted in logs and errors. The production secret-loading
mechanism is the operator-owned Keychain configuration described in
[Local Keychain secrets](local-keychain-secrets.md).

Artifact bytes are written only through Gongbu's normalized artifact service.
Providers do not choose final storage keys or write the configured artifact root
directly, and receipts use stable artifact identities rather than filesystem
locations.
