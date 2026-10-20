# Provider pack schema v1

A provider pack describes one provider integration as data: a single TOML file
that a reviewed Gongbu engine runs. Adding a provider, a model or a price
becomes a pack change instead of Rust code and a release.

> **Status.** Schema v1 is defined, Gongbu validates and loads packs, and the
> `sync_fixed` engine runs them. The async engine, the verified FLUX and Gemini
> packs, and the `hubu provider` commands that activate packs in a stack arrive
> in later releases. You can't activate a pack in a stack yet.

## What a pack declares and what engines own

A pack declares *what* a provider is. An engine decides *how* calls are made
safely, and a pack can't change that.

| Owned by the engine (a pack can't change it) | Declared by the pack |
| --- | --- |
| Zero mutation retries; ambiguous outcomes go to reconciliation | Endpoint, API version, allowed HTTPS hosts |
| The proven-before-transmission rule for releasing a claim | Auth header and credential alias |
| Durable checkpoints and read-only resume of polling | Agent-settable parameters with types and bounds; fixed fields |
| Settling only from the frozen pricing snapshot | Pricing rules, `pricing_version`, `pricing_reviewed_on` |
| Credentials sent only to `transport.allowed_hosts` | Response paths: provider request ID, usage, reported cost, artifacts |
| Artifact media type, size and host checks | Artifact media types and download hosts |

Gongbu rejects a pack that sets an engine-owned field anywhere in the file,
and the error names the field: `retries`, `max_retries`,
`generation_retries`, `retry`, `fallback`, `reconciliation`, `recovery`,
`settlement`, `credential_hosts` or `idempotency_header`. For example:

```text
transport.max_retries: engine-owned field is not allowed in a pack; Gongbu never retries a provider mutation
```

## Archetypes

| `archetype` | Hold | Settle | Fits |
| --- | --- | --- | --- |
| `sync_fixed` | Exact price from the request | Same frozen price | One request returns the result (Gemini images, search APIs) |
| `async_poll` | Frozen price × units | Frozen price plus the durable provider job ID | Submit a job, then poll (FLUX) |
| `metered` | Worst case, such as `max_output_tokens` × output rate | Reported usage, never above the hold | Model calls billed by usage |

## Example

```toml
schema_version = 1
pack_id = "sandbox-image"
pack_version = "2026-10-19.1"
provider = "sandbox-image"
display_name = "Sandbox image provider"
billing_merchant = "merchant:sandbox-image"
capability = "capability:image:generate"
workload_type = "image_generation"
archetype = "sync_fixed"
pricing_trust = "verified"

[transport]
endpoint = "https://api.sandbox-image.test"
api_version = "v1"
allowed_hosts = ["api.sandbox-image.test"]
auth_header = "x-api-key"
credential = "sandbox_image"
timeout_ms = 30000

[[parameters]]
name = "prompt"
type = "string"
required = true
max_length = 4000

[[parameters]]
name = "image_count"
type = "integer"
field = "n"
min = 1
max = 4
default = 1
quantity = "image_count"

[[parameters]]
name = "quality"
type = "enum"
values = ["draft", "final"]
default = "draft"
selector = "quality"

[fixed]
response_format = "url"

[response]
provider_request_id = "/id"
usage = { images = "/data" }
artifact_items = "/data"
artifact_url = "/url"

[artifacts]
media_types = ["image/png"]
hosts = ["cdn.sandbox-image.test"]

[[models]]
model = "image-v1"
provider_config_version = "sandbox-image-v1-2026-10-19"
pricing_version = "sandbox-image-v1-usd-2026-10-19-v1"
pricing_reviewed_on = "2026-10-19"

[[models.pricing_rules]]
rule_id = "sandbox-image-v1-draft-2026-10-19"
currency = "USD"
selector = { quality = "draft" }
components = [
  { unit = "request", rate_numerator_minor = 1, rate_denominator = 10 },
  { unit = "image", rate_numerator_minor = 15, rate_denominator = 10 },
]

[[models.pricing_rules]]
rule_id = "sandbox-image-v1-final-2026-10-19"
currency = "USD"
selector = { quality = "final" }
components = [
  { unit = "request", rate_numerator_minor = 1, rate_denominator = 10 },
  { unit = "image", rate_numerator_minor = 6, rate_denominator = 1 },
]
```

With this pack, `{"prompt": "a red circle", "image_count": 2, "quality":
"final"}` is held at exactly 0.1¢ + 2 × 6¢ = 12.1¢. The provider receives
`{"prompt": "a red circle", "n": 2, "quality": "final", "response_format":
"url"}`.

## Fields

### Identity

| Field | Rules |
| --- | --- |
| `schema_version` | Required. `1`. |
| `pack_id` | Required lowercase slug (`a`–`z`, `0`–`9`, `.`, `_`, `-`). One pack per provider integration. |
| `pack_version` | Required slug. Change it with every content change. |
| `provider` | Required slug. Becomes the provider ID of every target and pricing rule. |
| `display_name` | Required, 1–128 characters. |
| `billing_merchant` | Required, `merchant:<name>`. |
| `capability` | Required, `capability:<name>`, for example `capability:image:generate`. |
| `workload_type` | Required slug, for example `image_generation`. |
| `archetype` | Required: `sync_fixed`, `async_poll` or `metered`. |
| `adapter` | Optional slug recorded as the target's adapter ID. Defaults to the archetype. |
| `pricing_trust` | Required: `verified` (shipped in the repository with reviewed pricing) or `unverified` (written by a user; live use will require an explicit acknowledgement). |

### `[transport]`

| Field | Rules |
| --- | --- |
| `endpoint` | Required HTTPS origin with no port, path or query. |
| `allowed_hosts` | Required. Bare host names that receive the credential, including the endpoint host. |
| `api_version` | Required slug. |
| `auth_header` | Required HTTP header name that carries the credential. |
| `credential` | Required credential alias. The stack profile binds it to a Keychain item; the secret never appears in a pack. |
| `timeout_ms` | Required overall deadline, within Gongbu's provider deadline range. |
| `headers` | Optional non-secret headers. `authorization`, `cookie` and the auth header are rejected. |
| `auth_scheme` | Optional: `raw` (default, the secret as is) or `bearer` (`Bearer <secret>`). |

### `[request]`

Optional. Engines send one JSON `POST` to `path` on the endpoint; without
`[request]`, the path is `/`.

| Field | Rules |
| --- | --- |
| `path` | Required absolute path, such as `/v1beta/interactions`, with no query, fragment or `..`. |
| `body` | Optional body template. Without it, the body is `[fixed]` plus each parameter under its `field`. |

In a body template, a string that is exactly `{{name}}` becomes the typed
value of parameter `name`, and `{{model}}` becomes the model ID.
`{{name|upper}}` and `{{name|lower}}` change a string's case. A key whose
placeholder has no value (an optional parameter with no default) is left out.
A placeholder must be the whole string, so values are never spliced into
text. With a template, `[fixed]` and `field` aren't used, and every parameter
must appear in the template unless it is a constant (an integer with
`min = max`, or an enum with one value):

```toml
[request]
path = "/v1beta/interactions"

[request.body]
model = "{{model}}"
input = [{ type = "text", text = "{{prompt}}" }]
response_format = { type = "image", image_size = "{{image_size|upper}}" }
```

### `[[parameters]]`

Parameters are the only values an agent can set. Input that names an
undeclared parameter, or breaks a parameter's bounds, is rejected before any
spend is reserved.

| Field | Rules |
| --- | --- |
| `name` | Required lowercase identifier the agent uses. `model` is reserved for the selected target's model ID. |
| `type` | Required: `string`, `integer`, `boolean` or `enum`. |
| `field` | Optional provider request field. Defaults to `name`. |
| `required` / `default` | A required parameter has no default. A default must satisfy the bounds. |
| `max_length` | Required on `string` parameters. |
| `min` / `max` | `integer` only. |
| `values` | `enum` only: unique, lowercase selector values. |
| `quantity` | `integer` only: the priced quantity it sets (`image_count`, `input_tokens`, `max_output_tokens`, `results`, `pages`, `seconds`). Needs `min >= 0` and a `max`, so every hold is bounded. |
| `selector` | `enum` only: the pricing selector key it sets. |

A priced parameter (`quantity` or `selector`) must be required or have a
default. `[fixed]` fields are sent with every request and can't collide with a
parameter's field.

### `[response]`

Every value is a response path: a [JSON Pointer](https://www.rfc-editor.org/rfc/rfc6901)
into the provider's JSON body whose segments may filter an array.
`/steps[type=model_output]/content[type=image]` keeps the elements of `steps`
whose `type` is `model_output`, then the elements of their `content` whose
`type` is `image`.

| Field | Rules |
| --- | --- |
| `request_id_headers` | Optional response headers that carry the provider request ID, checked in order. |
| `provider_request_id` | Optional body path to the request ID, used when no header carries it. Set it, `request_id_headers`, or both. |
| `usage` | Map of usage quantity (`images`, `input_tokens`, `output_tokens`, `results`, `pages`, `seconds`) to a path at an integer, at an array whose length counts, or at a filtered selection whose size counts. |
| `reported_cost` | Optional. A decimal amount in major currency units, such as `"0.012"`. |
| `artifact_items` | Required path to the artifacts: an array, or a filtered selection. |
| `artifact_url` / `artifact_base64` | Exactly one: a path inside each item to a download URL or to inline base64 bytes. |

Usage a pack doesn't map settles at the requested quantity, because a
fixed-price call delivers what it was asked for. Map a usage path when the
provider can return less, such as fewer images than requested; settlement
then charges only what arrived.

Every declared usage path must be present and have the expected shape. An
unfiltered path must select one nonnegative integer or one array. A path whose
last segment filters an array counts its matches, including zero matches;
missing fields and non-array filter inputs still fail validation. Malformed
or missing declared usage sends the response to reconciliation with its
provider request ID, instead of treating it as zero or one unit.

A response the mapping can't read arrives after the provider accepted the
work, so the engine sends it to reconciliation and never releases the claim.

## How the `sync_fixed` engine classifies outcomes

| Outcome | Claim |
| --- | --- |
| Input fails the declared parameters, or a retry is requested | Released; nothing was sent |
| The request couldn't be sent | Released |
| HTTP 4xx | Released as `provider_rejected` |
| Timeout or lost response after sending | Reconciliation (`timeout_unknown_outcome`) |
| HTTP 5xx or any other non-2xx | Reconciliation (`provider_failure`) |
| 2xx the mapping can't read, an artifact off the allowlist, too large, or of the wrong type | Reconciliation, keeping the request ID |
| 2xx that maps cleanly | Settled from the frozen price |

### `[artifacts]`

| Field | Rules |
| --- | --- |
| `media_types` | Required subset of `image/png` and `image/jpeg`. Gongbu checks the bytes, not the provider's label. |
| `hosts` | Download hosts for URL artifacts. They never receive the credential, so a host that is also in `transport.allowed_hosts` is rejected. |

### `[[models]]`

| Field | Rules |
| --- | --- |
| `model` | Required unique slug. |
| `provider_config_version` | Required unique slug, frozen into every execution. |
| `pricing_version` | Required slug. |
| `pricing_reviewed_on` | `YYYY-MM-DD`. Required for verified packs. |
| `pricing_rules` | Required. Same rule format as [`providers.toml` pricing rules](configuration/local-stack/v1/providers-toml.md#pricing_rules), without `provider` and `model`, which come from the pack. |

Every combination of selector parameter values must be priced by exactly one
rule, and every priced unit must be set by a bounded parameter. Every explicit
`quantity` binding must have a corresponding unit in every model's pricing
rules, so it is retained in the frozen snapshot. A valid agent input can
therefore never reach an unpriced or unbounded request.

For a flat per-request bundle, use ordinary bounded integer parameters without
`quantity` bindings. If a bundled quantity must also be frozen and checked at
settlement, give its component a zero rate and put the bundle price in the
`request` component.

## Integrity

Gongbu computes a content digest over the validated pack; reformatting the
file doesn't change it. Each model's target revision records the pack ID,
version, digest and archetype. A target only binds to the exact pack it was
frozen with, so editing a pack is a new revision, never a silent change to
executions already in flight.

Binding also requires the runtime catalog's normalized rules for each pack
target to match the pinned pack's rules exactly. The pack digest covers each
model's `pricing_version`; a combined catalog can retain its own aggregate
version without changing those per-model rules.

## Related

- [Spend executor contract](spend-executor-contract.md)
- [Gongbu execution](gongbu-execution.md)
- [`providers.toml` reference](configuration/local-stack/v1/providers-toml.md)
