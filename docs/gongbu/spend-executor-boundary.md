# Spend executor boundary

The canonical Hubu protocol remains the
[Hubu spend executor contract](https://github.com/hacker-no-ice/hubu/blob/main/docs/spend-executor-contract.md).
Gongbu retains a low-level v4 client, but no current service or orchestration
path invokes it.

Hubu owns authorization, budget reservation, claims, settlement, release, and
their audit state. Gongbu owns operator-controlled provider configuration,
provider credentials, provider execution, normalized artifacts, cost
calculation, and settlement evidence.

Future execution work must use the persisted `Execution` aggregate. It must
create a `ProviderAttempt` before irreversible provider transmission and use a
stable persisted receipt for finalization. An ambiguous provider or settlement
outcome requires reconciliation; it must not trigger a blind retry or release.

Provider secrets belong to Gongbu's runtime identity. They must not be accepted
from callers, persisted in repository records, included in fixtures, exposed in
API responses, or emitted in logs and errors. The production secret-loading
mechanism will be introduced with the authoritative provider/security work.

Artifact bytes are written only through Gongbu's normalized artifact service.
Providers do not choose final storage keys or write the configured artifact root
directly, and receipts use stable artifact identities rather than filesystem
locations.
