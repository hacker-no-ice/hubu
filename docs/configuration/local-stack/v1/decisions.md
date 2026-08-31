# Local stack configuration decision guides

The field reference describes valid shapes. These guides explain the operational decisions behind fields that are syntactically simple but carry non-obvious lifecycle, security, or billing consequences.

## Managed versus external ownership

Use `managed` when this profile owns the local process lifecycle. Use `external` when another operator, supervisor, or installation owns it.

| Question | Managed | External |
| --- | --- | --- |
| Does `hubu stack start` launch it? | Yes, after dependencies pass | No |
| Does `hubu stack stop` signal it? | Yes, gracefully | No |
| Are local binary and state paths required? | Yes | No |
| Does Hubu still check endpoint health and compatibility? | Yes | Yes |
| Does external ownership permit remote endpoints in schema v1? | Not applicable | No; Hubu and Gongbu endpoints remain explicit loopback origins |

The common local choice is managed Hubu and managed Gongbu. External ownership is advanced configuration, not a way to skip required compatibility or authentication.

Hubu and Gongbu stay separate in either mode. Ownership never authorizes one component to open the other's database, read its credentials, or manage its artifacts.

## Managed-local versus external Temporal

Choose `managed_local` when managed Gongbu should start a local Temporal service using an installed Temporal CLI. Choose `external` when a Temporal deployment already exists and another operator owns it.

For `managed_local`:

- select an existing CLI with `temporal.binary_path`;
- copy its exact version into `temporal.expected_cli_version`;
- dedicate `temporal.data_path`;
- select distinct RPC and UI ports;
- keep `namespace` and `task_queue` aligned with Gongbu.

For `external`:

- provide `temporal.address` instead of managed-local binary, data, and port fields;
- use a namespace that already exists;
- ensure the external operator provides availability and lifecycle management;
- remember that Gongbu still owns its worker.

`temporal.ui_url` is display-only. It does not replace the RPC address.

## Account and agent attribution

The stack does not configure an execution account or agent. Gongbu's caller
capability authenticates the installation/service and carries no principal
claim. For every new execution, Hubu spend authorization is authoritative for
account and agent attribution and Gongbu persists that snapshot.

Registering or funding another agent after startup changes Hubu governance
state only. It requires no stack render, activation, stop, restart, or Gongbu
configuration change.

## Managed credentials versus explicit references

Choose by ownership, not by storage technology:

| Situation | Source profile | Provisioner/resolver | User chooses a location? |
| --- | --- | --- | --- |
| Managed Hubu | Omit the three Hubu file fields | Final `hubu-server` creates or reuses private capabilities during start | No |
| Managed Gongbu default | Omit caller file and both reserved opaque tables | Gongbu-owned bootstrap runs after Hubu readiness and before Gongbu serve | No |
| External Hubu | Supply three absolute file references | External operator provisions; Hubu CLI and unified MCP consume | Yes |
| External Gongbu | Supply the caller file reference | External operator provisions; unified MCP consumes | Yes |
| Managed Gongbu explicit override | Supply both reserved opaque references and caller file | Gongbu's normal secret backend plus operator-provisioned client side | Yes |
| Provider credential | Supply `[opaque.<key>]` and reference its key from a target | Gongbu's provider-secret backend | Yes, as opaque metadata |

The managed profile contract does not expose the internal credential paths.
Current local storage is file-backed so existing clients can consume the
handoff, but it may change without adding file locations to operator source.
Never convert a file or opaque mechanism by copying secret bytes into TOML. The
renderer checks reference shape and ownership contracts but does not read
secret values.

## Disabled, sandbox, and live provider modes

Choose `disabled` for installation, topology validation, documentation exploration, or any environment that must not perform provider work.

Disabled mode must omit all live-only fields. Commented examples are not active TOML, but keeping speculative future values out of the initial profile reduces confusion and accidental activation risk.

Choose `sandbox` through `hubu stack init --mode sandbox` for a complete local
ecosystem with real Hubu/Gongbu/Temporal communication and the built-in
deterministic provider fixture. The renderer owns its fixture catalog and
internal ceiling; sandbox rejects provider credentials and the live-spend
acknowledgement.

Choose `live` only after:

1. the local stack validates in the intended topology;
2. provider credentials exist behind Gongbu-owned references;
3. the exact provider, adapter, model, endpoint, and API version are verified;
4. frozen pricing represents every billable component;
5. a conservative profile spend ceiling is approved;
6. the operator enters the exact live-spend acknowledgement;
7. the rendered plan is reviewed before activation.

## Immutable configuration versions

`catalog_version` and `provider_config_version` are operator-owned labels for immutable content.

Use a new label whenever the content changes. Examples of meaningful changes include:

- provider or model selection;
- credential coordinate;
- endpoint, region, API version, timeout, or retry settings;
- activation or execution gate;
- price, unit, selector, or component composition.

Do not use a mutable label such as `latest` and then replace its meaning. Gongbu persists configuration digests and pricing snapshots so execution history remains attributable to the exact reviewed inputs.

## Supported profile versus generic target

Use `[[supported_profiles]]` when its named, versioned contract exactly matches
the desired capability. The initial contract,
`hubu.flux-2-pro.text-to-image/v1`, freezes the FLUX provider, adapter, model,
three dimension presets, PNG/JPEG output, zero generation retries, no fallback,
poll and artifact policies, durable recovery policy, and its dated rational USD
pricing. The operator supplies only an opaque credential reference and the two
explicit spend choices.

Use raw `[[targets]]` and `[[pricing_rules]]` for other providers such as
Gemini. A composite FLUX-plus-Gemini catalog requires a new immutable
`catalog_version`, distinct credential aliases, and unambiguous target and
pricing keys. Raw entries cannot override or duplicate a supported contract.
Both providers stay inside Gongbu's execution boundary with separate targets,
credentials, attempts, and artifacts; Hubu's governance database remains a
separate process and failure domain.

### Readiness is not qualification

The supported-provider catalog reports four independent facts:

- configuration resolved the exact contract;
- the credential reference is present for the local process identity;
- the rendered contract passed Gongbu's production validator; and
- live qualification was performed.

The last fact is deliberately false with `not_performed` for the initial FLUX
profile. Catalog, doctor, render, and production validation never call BFL or
claim that a provider transaction succeeded. See the
[managed FLUX.2 runbook](../../../operations/managed-flux-profile.md).

## Provider targets and adapter settings

A provider target key consists of:

```text
workload_type / provider / adapter / model
```

These fields answer different questions:

- `workload_type`: what kind of governed work is requested;
- `provider`: which provider namespace owns the target;
- `adapter`: which Gongbu implementation constructs and interprets requests;
- `model`: which exact provider model receives the work;
- `settings.type`: which adapter-specific configuration schema is used.

`adapter` and `settings.type` should agree, but they are not substitutes. The former is part of the target identity; the latter selects the tagged settings payload validated by Gongbu.

Endpoint and API-version values come from the integration contract and provider documentation. Do not copy a console URL, documentation URL, or marketing model name unless it is the exact adapter input.

## Active versus execution-enabled

`active` selects one immutable revision for a target key. `execution_enabled` independently permits new work through that revision.

| Active | Execution enabled | New work |
| --- | --- | --- |
| `false` | Either | Not selectable |
| `true` | `false` | Selected revision remains blocked |
| `true` | `true` | Eligible when pricing, credentials, authorization, and readiness also pass |

Keeping both gates explicit supports retaining history and disabling execution without mutating an immutable revision.

## Pricing, authorization, holds, and settlement

These amounts are related but not interchangeable:

1. **Catalog rate** — the operator-verified exact rational price for each billable unit.
2. **Estimated amount** — Gongbu evaluates request quantities against the matching rule and rounds conservatively to currency minor units.
3. **Requested Hubu authorization** — the governed amount requested before provider transmission.
4. **Budget hold** — Hubu reserves the authorized amount so concurrent work cannot spend it twice.
5. **Profile maximum spend** — `maximum_spend_minor` prevents this Gongbu configuration from exceeding its explicit ceiling.
6. **Exact provider cost** — Gongbu preserves the provider's integer amount,
   decimal scale, currency, and the complete pricing snapshot frozen before
   provider work.
7. **Final settlement** — Hubu converts the final exact cost to cents with one
   checked ceiling operation. A normal executor settlement cannot exceed the
   authorization; an overrun or ambiguous outcome retains its evidence for
   human reconciliation rather than guessing that no spend occurred.

```text
exact catalog × request quantity
              │ conservative rounding
              ▼
       estimated minor units
              │ bounded by profile and Hubu policy/budget
              ▼
       authorization + budget hold
              │ provider execution + exact receipt
              ▼
 checked ceiling to budget cents
              │
              ▼
        settlement or reconciliation
```

`maximum_spend_minor` does not create a Hubu budget, and a sufficient Hubu budget does not waive the profile ceiling. Both checks must pass.

Do not convert a provider decimal through floating point. For an exact amount
`amount × 10^-scale` major currency units, USD budget consumption is
`ceil(amount / 10^(scale - 2))` cents when `scale` is greater than 2, with
checked multiplication for smaller scales. For example, USD 0.000001 consumes
one cent. Gongbu preserves the original exact tuple even though Hubu accounts
for the conservative cent amount.

After the claim lease expires, a human may confirm a legitimate billed overrun.
Hubu then records and consumes the full conservative charge, records the amount
above the authorization, and releases none of the hold. This retrospective
accounting may make the budget's remaining balance negative and exhausts it for
later reservations.

## Converting provider prices

Convert the provider's published currency amount into integer minor units without losing the fraction.

For USD $0.067 per image:

```text
$0.067 = 6.7 cents = 67 / 10 cents
```

```toml
{ unit = "image", rate_numerator_minor = 67, rate_denominator = 10 }
```

For USD $2.50 per million input tokens:

```text
$2.50 = 250 cents for 1,000,000 tokens
```

```toml
{ unit = "input_token", rate_numerator_minor = 250, rate_denominator = 1000000 }
```

If the provider bills images plus input and output tokens, include three components. Do not omit a component because it is usually small.

## Changing an active profile

Source edits never silently replace the active generation. The safe update sequence is:

```sh
hubu stack doctor --profile /absolute/path/to/profile
hubu stack render --profile /absolute/path/to/profile
# review generation ID, changed source files, and affected components
hubu stack stop --profile /absolute/path/to/profile
hubu stack activate --generation GENERATION_ID --profile /absolute/path/to/profile
hubu stack start --profile /absolute/path/to/profile
```

Keep previous credentials and operator-owned source available until the new generation is verified. Rollback requires restoring the exact source and compatible binary provenance recorded for the target generation.
