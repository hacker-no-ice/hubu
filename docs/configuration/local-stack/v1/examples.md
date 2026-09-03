# Complete local stack examples

Use a separate profile directory for each outcome. `hubu stack init` writes the
complete topology and source files for that outcome; do not copy a hand-written
`stack.toml` between modes.

## Sandbox: complete stack without live spend

Sandbox is the recommended first profile and a zero-edit outcome. It runs the
real Hubu, Gongbu, and Temporal boundaries but replaces the external provider
edge with a deterministic, non-billable fixture:

```sh
hubu stack init --mode sandbox --install-temporal \
  --profile /absolute/path/to/hubu-sandbox
hubu stack doctor --profile /absolute/path/to/hubu-sandbox
```

Initialization writes the complete managed topology, fixture target, and
synthetic frozen pricing. The generated profile contains no provider
credential, live-spend ceiling, or live-spend acknowledgement. When doctor
reports `ready_to_render`, the profile is ready for `hubu stack start` without
an operator edit.

## Hubu-only: governance without an execution plane

Hubu-only is also a zero-edit outcome:

```sh
hubu stack init --mode hubu-only --profile /absolute/path/to/hubu-only
hubu stack doctor --profile /absolute/path/to/hubu-only
```

The generated `stack.toml` deliberately omits Gongbu and Temporal. Its
`providers.toml` is:

```toml
schema_version = 1
mode = "disabled"
```

Doctor reports Gongbu, Temporal, and provider execution as intentionally
absent rather than unhealthy. This profile supports registration, policy,
authorization, and budget administration without adopting provider execution.

## Live: Gemini Developer API and FLUX.2

Use live mode only in its own profile after the sandbox profile is healthy.
With the four Hubu binaries and the Temporal CLI discoverable, initialization
writes the complete managed `stack.toml`; the operator edits only the provider
references and selections shown below:

```sh
hubu stack init --mode local-stack --install-temporal \
  --profile /absolute/path/to/hubu-live
```

This example enables both shipped Gemini Developer API image contracts and the
FLUX.2 Pro contract. Live execution can incur provider charges after this
profile is validated, rendered, activated, started, and selected for a governed
request.

### Store two provider credentials in macOS Keychain

Create the Google AI Studio API key and BFL API key with **Keychain Access**.
The two items must use different lookup coordinates. In Hubu configuration:

- `service` maps to the Keychain Access **Where** field;
- `account` maps to the Keychain Access **Account** field; and
- a matching **Name** alone is insufficient because Gongbu looks up the exact
  **Where** and **Account** pair.

You can verify only that each item exists without printing either credential.
Replace the example coordinates with the same non-secret **Where** and
**Account** values used below:

```sh
security find-generic-password \
  -s 'operator.google.gemini' -a 'gemini-image' >/dev/null 2>&1 \
  && echo 'Gemini credential reference exists'
security find-generic-password \
  -s 'operator.bfl.flux' -a 'flux-image' >/dev/null 2>&1 \
  && echo 'FLUX credential reference exists'
```

Do not add `-w`: that option prints the credential value. Hubu and Gongbu must
run as the macOS user allowed to access these Keychain items.

### Edit `credentials.toml`

`credentials.toml` contains lookup coordinates only, never credential values:

```toml
schema_version = 1

[opaque.google_gemini]
service = "operator.google.gemini"
account = "gemini-image"

[opaque.bfl_flux]
service = "operator.bfl.flux"
account = "flux-image"
```

The opaque table names are local aliases. Both Gemini contracts intentionally
share `google_gemini`; FLUX uses the separate `bfl_flux` alias and Keychain
item.

### Edit `providers.toml`

All fields shown here are required for this three-contract live catalog. The
catalog version is an immutable operator-owned label for this exact composite;
use a new label if any selected contract or frozen price changes.

```toml
schema_version = 1
mode = "live"
catalog_version = "operator-gemini-flux-2026-09-03-v1"

# Positive USD cents approved as this profile's local spend ceiling.
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

Each versioned contract expands to an immutable target and its exact pricing;
do not reproduce these shipped integrations as raw `[[targets]]` or
`[[pricing_rules]]` tables:

| Contract | Target | Frozen USD-cent pricing per image |
| --- | --- | --- |
| Gemini 3.1 Flash Lite Image | `google` / `gemini_developer_image` / `gemini-3.1-flash-lite-image` | `1k`: `336/100` |
| Gemini 3.1 Flash Image | `google` / `gemini_developer_image` / `gemini-3.1-flash-image` | `1k`: `67/10`; `2k`: `101/10`; `4k`: `151/10` |
| FLUX.2 Pro | `flux` / `flux2_api` / `flux-2-pro` | `1k`: `3/1`; `2k`: `45/10`; `4k`: `75/10` |

These prices are frozen versioned configuration, not timeless provider price
claims. The [Gemini contract](../../../operations/gemini-provider-contract.md)
and [FLUX.2 contract](../../../operations/flux-provider-contract.md) document
their source dates, capabilities, dimensions, transport, and recovery policy.
If current provider terms no longer match, keep this profile inactive until a
new contract is shipped.

### Validate with doctor

`hubu stack doctor` is the authoritative validation path for the whole profile:

```sh
hubu stack doctor --profile /absolute/path/to/hubu-live
```

Follow its field-specific diagnostics until it reports `ready_to_render`.
Doctor verifies the source shape, credential-reference existence, contract and
target expansion, selector-complete rational pricing, production validation,
and live-spend gate without printing a credential or contacting a provider.
Use `--json` for the same authoritative result in automation. Do not replace
doctor with a manual live-profile checklist.

After doctor succeeds, render and review the generation before activation as
described in [Local stack quick start](../../../local-stack.md#apply-a-configuration-change).

## Keep sandbox and live profiles separate

Do not switch one profile back and forth between `sandbox` and `live`. Separate
profiles isolate provider credentials, spend acknowledgement, frozen catalogs,
generated generations, runtime state, databases, artifacts, and logs. Switching
is then an explicit profile selection instead of a risky source rewrite:

```sh
hubu stack select --profile /absolute/path/to/hubu-sandbox
hubu stack select --profile /absolute/path/to/hubu-live
```

This makes the non-billable default easy to recover, keeps live credentials out
of sandbox state, and makes the active spend boundary visible in the selected
profile path. Stop the active managed stack before selecting and starting the
other profile when their default local ports overlap.

Advanced raw-provider configuration, external Hubu/Gongbu/Temporal ownership,
and provider-specific recovery details remain available in the
[configuration decisions](decisions.md), [provider reference](providers-toml.md),
and [live provider operations](../../../operations/live-providers.md).
