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

Before token issuance, Gongbu's authenticated
[authorization scope preview](authorization-scope.md) derives the exact Hubu
request from the operator-owned account, agent identity, provider target,
pricing catalog, and Hubu timing guidance. On execution submission Gongbu
recomputes that scope and asks Hubu to validate the token before persistence or
workflow scheduling. The later workflow preflight and claim remain defense in
depth before provider transmission.

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
