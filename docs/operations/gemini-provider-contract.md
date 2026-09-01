# Gemini Developer API provider contract

Hubu ships one deliberately narrow Gemini Developer API contract beside the
FLUX contract. Operators select the contract and provide only an opaque
Keychain credential reference; the renderer supplies the immutable target,
transport, capability, policy, and price.

| Frozen field | Value |
| --- | --- |
| Contract | `hubu.gemini-3.1-flash-lite-image.text-to-image/v1` |
| Target | `google` / `gemini_developer_image` / `gemini-3.1-flash-lite-image` |
| API | `https://generativelanguage.googleapis.com/` / `v1beta` |
| Capability | one 1024×1024 (`1k`) PNG or JPEG image |
| Price review | 2026-09-01 |
| Standard image-output price | USD $0.0336, represented exactly as `336/100` cents per image |
| Retry and fallback | zero generation retries; no fallback |
| Transport | synchronous inline response; no polling |

The stable model, 1K-only output capability, and standard output price come
from Google's [model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite-image)
and [Developer API pricing](https://ai.google.dev/gemini-api/docs/pricing).
Changing any frozen fact requires a new immutable contract and pricing version.

## Configure

Use a distinct credential alias and an explicit composite catalog version when
Gemini and FLUX are enabled together:

```toml
schema_version = 1
mode = "live"
catalog_version = "operator-gemini-flux-2026-09-01-v1"
maximum_spend_minor = 25
live_spend_acknowledgement = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"

[[contract_bindings]]
contract = "hubu.gemini-3.1-flash-lite-image.text-to-image/v1"
credential = "google_gemini"

[[contract_bindings]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux"
```

The two aliases must resolve to independent opaque credential coordinates.
Readiness checks report configuration presence, credential-reference presence,
production validation, and live qualification independently and do not call a
provider.

## Execute and recover

Discover the target with `GET /v2/execution-targets` or
`gongbu_list_execution_targets`, then submit only its opaque `target_id` plus
the normalized 1K request. The default `ProviderAdapter` lifecycle performs one
synchronous submission. A successful response completes immediately; an
ambiguous timeout has no pollable checkpoint and requires reconciliation, not
automatic resubmission.

FLUX uses the same authorization, pricing, claim, settlement, replay, artifact,
and redaction lifecycle but overrides submission and polling. See the
[FLUX provider contract](flux-provider-contract.md) for that asynchronous
transport.

Ordinary tests and demos remain fixture-only. Do not perform paid qualification
without a separate, explicit operator-authorized procedure.
