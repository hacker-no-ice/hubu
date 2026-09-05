# Gemini Developer API provider contract

Hubu ships two deliberately narrow Gemini Developer API contracts beside the
FLUX contract. Operators select either or both contracts and provide only an
opaque Keychain credential reference; the renderer supplies each immutable
target, transport, capability, policy, and price.

| Frozen field | Value |
| --- | --- |
| Contracts | `hubu.gemini-3.1-flash-lite-image.text-to-image/v1`; `hubu.gemini-3.1-flash-image.text-to-image/v1` |
| Targets | `google` / `gemini_developer_image` / `gemini-3.1-flash-lite-image`; `google` / `gemini_developer_image` / `gemini-3.1-flash-image` |
| API | `https://generativelanguage.googleapis.com/` / `v1beta` |
| Lite capability | one 1024×1024 (`1k`) PNG or JPEG image |
| Non-Lite capability | one 1024×1024 (`1k`), 2048×2048 (`2k`), or 4096×4096 (`4k`) PNG or JPEG image |
| Price review | 2026-09-01 |
| Lite standard price | USD $0.0336 (`336/100` cents) at 1K |
| Non-Lite standard prices | USD $0.067 (`67/10` cents) at 1K; $0.101 (`101/10` cents) at 2K; $0.151 (`151/10` cents) at 4K |
| Retry and fallback | zero generation retries; no fallback |
| Transport | synchronous inline response; no polling |

The stable models, resolution limits, and standard output prices come from
Google's [Lite model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite-image),
[non-Lite model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-image),
[image generation guide](https://ai.google.dev/gemini-api/docs/image-generation),
and [Developer API pricing](https://ai.google.dev/gemini-api/docs/pricing).
Changing any frozen fact requires a new immutable contract and pricing version.

## Configure

Use an explicit composite catalog version when multiple contracts are enabled.
Both Gemini contracts may use the same Google credential alias; FLUX must use
an independently isolated credential:

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
contract = "hubu.gemini-3.1-flash-image.text-to-image/v1"
credential = "google_gemini"

[[contract_bindings]]
contract = "hubu.flux-2-pro.text-to-image/v1"
credential = "bfl_flux"
```

Credential coordinates must remain independent across providers.
Readiness checks report configuration presence, credential-reference presence,
production validation, and live qualification independently and do not call a
provider.

## Execute and recover

Discover the target with `GET /v2/execution-targets` or
`gongbu_list_execution_targets`, then submit only its opaque `target_id` plus
a normalized request. Use the Lite target only for `1k`; use the non-Lite target
for `1k`, `2k`, or `4k`. The default `ProviderAdapter` lifecycle performs one
synchronous submission. A successful response completes immediately; an
ambiguous timeout has no pollable checkpoint and requires reconciliation, not
automatic resubmission.

FLUX uses the same authorization, pricing, claim, settlement, replay, artifact,
and redaction lifecycle but overrides submission and polling. See the
[FLUX provider contract](flux-provider-contract.md) for that asynchronous
transport.

Ordinary tests and demos remain fixture-only. Do not perform paid qualification
without a separate, explicit operator-authorized procedure.
