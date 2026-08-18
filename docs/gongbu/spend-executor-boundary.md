# Spend executor boundary

The canonical Hubu protocol is the in-repository
[Hubu spend executor contract](../spend-executor-contract.md). Gongbu's
production Hubu activities implement that v4.1 HTTP contract for resolution, claim,
inspection, settlement, and release. Sharing a repository and product version
does not turn this wire boundary into an in-process or shared-database call.

Hubu owns authorization, budget reservation, claims, settlement, release, and
their audit state. Gongbu owns operator-controlled provider configuration,
provider credentials, provider execution, normalized artifacts, cost
calculation, and settlement evidence.

The caller submits only `spend_auth_token_id` plus execution intent and target.
Gongbu resolves Hubu's read-only authorization snapshot before admission. Hubu
is authoritative for account, agent, `operation_key`, optional `task_id`,
`reason`, amount, currency, workload profile, expiry, status, and typed scope.
Gongbu independently derives the operator-controlled target, typed scope, and
catalog price and requires exact agreement before it persists or schedules.
Resolution never claims; the durable workflow claims only after persistence.
Any preview remains optional UX for obtaining the right authorization amount;
admission always recomputes from the active operator catalog and never trusts a
preview or caller-supplied price.
The legacy v1 `hubu_token_reference` name remains input-only compatibility:
when both names are present they must be equal.

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
