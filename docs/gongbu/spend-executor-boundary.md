# Spend executor boundary

The canonical Hubu protocol is the in-repository
[Hubu spend executor contract](../spend-executor-contract.md). Gongbu's
production Hubu activities implement that v4 HTTP contract for claim,
inspection, settlement, and release. Sharing a repository and product version
does not turn this wire boundary into an in-process or shared-database call.

Hubu owns authorization, budget reservation, claims, settlement, release, and
their audit state. Gongbu owns operator-controlled provider configuration,
provider credentials, provider execution, normalized artifacts, cost
calculation, and settlement evidence.

Hubu's stored authorization snapshot is authoritative for `operation_key`, the
optional external `task_id`, and the descriptive `reason`. Current Gongbu
compatibility requests carry the persisted execution operation key required by
the v4 claim route, but omit `task_id`; Hubu resolves and returns the authorized
task correlation and reason. Gongbu must not derive task identity from its
operation key or accept a new caller-controlled duplicate. The planned
token-resolution flow will reduce this further to the token plus execution
intent, with Gongbu resolving the full Hubu snapshot before admission.

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
