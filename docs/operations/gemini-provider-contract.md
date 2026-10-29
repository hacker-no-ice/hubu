# Gemini Developer API provider contract

Hubu ships deliberately narrow Gemini Developer API contracts beside the FLUX
contracts: the v1 text-to-image and [image-edit](#image-editing) contracts for
Gemini 3.1 Flash Image and Flash Lite Image, and the
[v2 contracts](#v2-contracts-aspect-ratios-and-gemini-3-pro-image), which add
aspect ratios, Gemini Nano Banana 2.1 and Gemini 3 Pro Image. Operators
select contracts and provide
only an opaque Keychain credential reference; the renderer supplies each
immutable target, transport, capability, policy, and price.

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

## v2 contracts: aspect ratios and Gemini 3 Pro Image

The v2 contracts also add Gemini Nano Banana 2.1 (`gemini-nano-banana-2.1`).

| Frozen field | Value |
| --- | --- |
| Text-to-image contracts | `hubu.gemini-3.1-flash-lite-image.text-to-image/v2`; `hubu.gemini-3.1-flash-image.text-to-image/v2`; `hubu.gemini-nano-banana-2.1.text-to-image/v1`; `hubu.gemini-3-pro-image.text-to-image/v1` |
| Image-edit contracts | `hubu.gemini-3.1-flash-lite-image.image-edit/v2`; `hubu.gemini-3.1-flash-image.image-edit/v2`; `hubu.gemini-nano-banana-2.1.image-edit/v1`; `hubu.gemini-3-pro-image.image-edit/v1` |
| New targets | `google` / `gemini_developer_image` / `gemini-nano-banana-2.1` (Nano Banana 2.1); `google` / `gemini_developer_image` / `gemini-3-pro-image` (Nano Banana Pro) |
| Input | `image_size` (`1k`, `2k`, `4k`; Lite `1k` only) and `aspect_ratio`: `1:1`, `2:3`, `3:2`, `3:4`, `4:3`, `4:5`, `5:4`, `9:16`, `16:9` or `21:9`. Text-to-image defaults to `1:1`; edits must name a ratio |
| Certified sizes | Google's published size for each ratio at each tier, for example `16:9` is 1376×768 at 1K, 2752×1536 at 2K and 5504×3072 at 4K |
| Nano Banana 2.1 output price | USD $0.0336 (`336/100` cents) per 1K image; $0.0504 (`504/100` cents) per 2K image; $0.113 (`113/10` cents) per 4K image |
| Pro output price | USD $0.134 (`134/10` cents) per 1K or 2K image; $0.24 (`24/1` cents) per 4K image |
| Flash and Lite output prices | unchanged from v1 |
| Nano Banana 2.1 input image price (edits) | 1,120 input tokens at $1.50 per 1M: USD $0.00168 (`168/1000` cents) per input image |
| Pro input image price (edits) | 560 input tokens at $2.00 per 1M: USD $0.00112 (`112/1000` cents) per input image |
| Price review | 2026-10-28 |
| Implementation | verified packs `packs/verified/google-gemini-developer-image-v2.toml` and `packs/verified/google-gemini-developer-image-edit-v2.toml` |

Gemini bills per output image by size tier, so every aspect ratio in a tier
costs the same; the ratio only selects the certified output size. Gongbu
sends both fields in the request's `response_format`, freezes the size with
the price at admission, and rejects a ratio the target does not certify
before anything is authorized. Flash, Nano Banana 2.1 and Pro publish
identical sizes for these ratios. Google publishes no size table for Flash Lite Image, so Lite is
certified at the 1K sizes Flash publishes; the first live round confirms
them. The extra `1:4`, `4:1`, `1:8`, `8:1` ratios of Flash and Nano Banana
2.1, and Flash's 512px tier, are not offered yet.

Nano Banana 2.1 is stable, has no announced shutdown date, and is Google's
recommended replacement for Flash Image. It is offered beside Flash, not in
place of it: nothing moves a Flash binding or an agent to it. Google states
its 4K price as $0.113; at 3,780 output tokens × $30 per 1M the token
arithmetic is $0.1134. The rules use Google's stated per-image figures for
every model.

The v2 contracts are new revisions of the v1 targets: a profile binds either
the v1 or the v2 contract of a model, not both. Nano Banana 2.1 and Pro are
separate models, so each gets its own `v1` contracts, and a profile binds it only when the operator
names it.

### What a frozen price covers

A frozen price is the output images plus, for an edit, its input images. It
is not the whole Google bill. Google also bills the text prompt's input
tokens and the response's text and thinking output tokens (Pro: $2.00 and
$12.00 per 1M tokens; Nano Banana 2.1: $1.50 and $7.50, with `medium`
thinking by default; Flash: $0.50 and $3; Lite: $0.25 and $1.50); Gongbu
does not price them, as in v1, but records the reported
`usage.total_input_tokens` and `usage.total_output_tokens` as settlement
evidence. Batch, Flex and Priority rates do not apply: Gongbu uses the
standard synchronous API. The packs enable no Google Search grounding, which
Google bills separately.

Sources, reviewed 2026-10-28:
[Developer API pricing](https://ai.google.dev/gemini-api/docs/pricing#gemini-3-pro-image),
[image generation guide](https://ai.google.dev/gemini-api/docs/image-generation)
(model IDs, the Lite 1K limit, aspect ratios and sizes), the
[Nano Banana 2.1 model page](https://ai.google.dev/gemini-api/docs/models/gemini-nano-banana-2.1),
[deprecations](https://ai.google.dev/gemini-api/docs/deprecations) and the
[Interactions API reference](https://ai.google.dev/api/interactions-api)
(`response_format.aspect_ratio`, usage fields).

### Enable Gemini 3 Pro Image

Pro is never a default. Flash and Lite stay the inexpensive choices for
exploration, and nothing upgrades an agent to Pro:

1. Bind the Pro contract explicitly, beside the others, with the same Google
   credential alias:

   ```toml
   [[contract_bindings]]
   contract = "hubu.gemini-3-pro-image.text-to-image/v1"
   credential = "google_gemini"
   ```

2. Keep `maximum_spend_minor` at or above the largest price you allow; a 4K
   Pro image is 24 cents.
3. Let your Hubu policy decide Pro spend. The
   [`creative-tiers.yaml`](../../policies/creative-tiers.yaml) cookbook denies
   models it does not list, so Pro stays denied until you add
   `gemini-3-pro-image` to its approved models. Even then every Pro request
   needs a human: 2K and 4K are final renders, and a 13.4-cent 1K draft is
   above the cookbook's 10-cent draft allowance. An amount policy works the same way: a 1K Pro request
   authorizes exactly USD 0.134, which a policy that asks for approval above
   10 cents sends to a human before any token exists.
4. Agents discover the Pro target by model in `gongbu_list_execution_targets`
   and submit its `target_id`; a Flash target ID never runs on Pro.

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
