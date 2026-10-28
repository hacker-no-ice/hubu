# Gemini Developer API provider contract

Hubu ships two deliberately narrow Gemini Developer API text-to-image
contracts beside the FLUX contract, and two image-edit contracts on the same
models (see [image editing](#image-editing)). Operators select either or both contracts and provide only an
opaque Keychain credential reference; the renderer supplies each immutable
target, transport, capability, policy, and price.

Start with [live provider operations](live-providers.md) for the shared
credential, governance, pricing, spend, retry, reconciliation, artifact, and
qualification procedure. This page is the canonical description of Gemini
provider behavior; field definitions live in the
[`providers.toml` reference](../configuration/local-stack/v1/providers-toml.md).

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
| Implementation | verified [provider pack](../provider-packs.md) `packs/verified/google-gemini-developer-image.toml` on the `sync_fixed` engine |

Since v0.2.5 both targets run from one verified provider pack instead of a
dedicated Rust adapter. Contract IDs, `provider_config_version` values,
pricing versions, prices and behavior are unchanged; the target settings now
name the pack and its content digest, so their revision digest changed.
Upgrade only with no unfinished Gemini work; see
[upgrading to v0.2.5](upgrading-to-v0.2.5.md).

The stable models, resolution limits, and standard output prices come from
Google's [Lite model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite-image),
[non-Lite model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-image),
[image generation guide](https://ai.google.dev/gemini-api/docs/image-generation),
and [Developer API pricing](https://ai.google.dev/gemini-api/docs/pricing).
Changing any frozen fact requires a new immutable contract and pricing version.

## Image editing

| Frozen field | Value |
| --- | --- |
| Contracts | `hubu.gemini-3.1-flash-lite-image.image-edit/v1`; `hubu.gemini-3.1-flash-image.image-edit/v1` |
| Targets | workload `image_edit`, `google` / `gemini_developer_image`, same two models |
| Hubu scope capability | `capability:image:edit` |
| Input | a prompt, `image_size`, and `input_images`: 1 to 4 artifact IDs of images this account's earlier governed executions delivered |
| Output | one image, at the text-to-image sizes and prices of the same model |
| Input image price | each input image is billed as 1,120 input tokens, the Gemini 3 default for an image: USD $0.00056 (`56/1000` cents) on Flash Image, $0.00028 (`28/1000` cents) on Flash Lite Image |
| Price review | 2026-10-21 |
| Implementation | verified pack `packs/verified/google-gemini-developer-image-edit.toml` on the `sync_fixed` engine |

A 1K Flash edit of one draft is held at exactly 6.756 cents: 6.7 cents for the
output image plus 0.056 cents for the input image. The text prompt's input
tokens stay unpriced, as for text-to-image. Gongbu sends each input image
inline, after loading it from the account's own artifacts and checking its
recorded digest; see [editing an image](../gongbu-execution.md#editing-an-image).
The edit contracts' prices are their own rules (`workload_type = "image_edit"`),
so binding them never changes a text-to-image price.

Bind an edit contract beside the text-to-image one; it uses the same Google
credential alias:

```toml
[[contract_bindings]]
contract = "hubu.gemini-3.1-flash-image.image-edit/v1"
credential = "google_gemini"
```

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

Credential coordinates must remain independent across providers. Readiness
checks do not call a provider; see
[readiness and live qualification](live-providers.md#readiness-and-live-qualification).

## Execute and recover

Discover the target with `GET /v2/execution-targets` or
`gongbu_list_execution_targets`, then submit only its opaque `target_id` plus
a normalized request. Use the Lite target only for `1k`; use the non-Lite target
for `1k`, `2k`, or `4k`. The adapter verifies the selected size against the
frozen schema-v2 pricing selector before calling the provider and never
derives the authorized price from returned artifact dimensions.

The adapter reads an AI Studio API key from Keychain and sends it only in the
`x-goog-api-key` header to the configured Google API endpoint. The default
`ProviderAdapter` lifecycle performs one synchronous submission. A successful response completes immediately; an
ambiguous timeout has no pollable checkpoint and requires reconciliation, not
automatic resubmission.

FLUX uses the same authorization, pricing, claim, settlement, replay, artifact,
and redaction lifecycle but overrides submission and polling. See the
[FLUX provider contract](flux-provider-contract.md) for that asynchronous
transport.

Ordinary tests and demos remain fixture-only. Do not perform paid qualification
without a separate, explicit operator-authorized procedure; the focused live
test is documented in
[live provider operations](live-providers.md#gemini-developer-api).
