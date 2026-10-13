# Hubu Policies

This folder is the default place to keep local Hubu spending policies.

Create an editable policy from the starter template:

```sh
hubu policy new-template
```

By default, this writes `policies/policy.yaml`. You can choose a different path:

```sh
hubu policy new-template --path policies/openai-allowlist.yaml
```

Then declaratively apply the policy as the current human user's default:

```sh
hubu policy validate --path policies/openai-allowlist.yaml
hubu policy apply --path policies/openai-allowlist.yaml --name "OpenAI allowlist"
```

The first apply creates a stable `pol_...` id. Subsequent applies reconcile by
the immutable owner-scoped key (the YAML `id` by default), append a revision
only when canonical content changes, and atomically update the assignment.
`hubu policy add` remains a compatibility alias.

Inspect or export without reading SQLite:

```sh
hubu policy show --policy-id pol_...
hubu policy export --policy-id pol_...
hubu policy history --policy-id pol_...
hubu policy diff --policy-id pol_... --from-revision 1
```

## YAML compatibility

Hubu parses policy files with `serde-saphyr`, a pure-Rust YAML 1.2 parser.
Existing starter and test policies are migration fixtures. Policy, rule, and
condition objects reject unknown fields; required fields and semantic
validation remain enforced after parsing.

- Write enum values as nested maps, for example `value:` then
  `money_cents: 500` on the next indented line. `hubu policy export` emits
  this form. Older exports that used YAML tags (`value: !money_cents 500`)
  still load.
- A document must be a single YAML document. Duplicate keys, tab
  indentation, and unknown fields are rejected.
- String fields such as `id` and `version` must not be `null`, `Null`, or
  `~`; quote them if you need that literal text. Words such as `no`, `yes`,
  or `on` stay strings.
- Integer values accept `_` digit separators, so `money_cents: 10_000` means
  10000. Floating-point and quoted numbers are rejected for integer fields.
- Anchors and aliases are allowed, but expansion is bounded.

Callers may rely on Hubu's top-level errors (`failed to parse policy yaml` and
`invalid policy`), but the parser's detailed source diagnostic is not a stable
public contract and can change across dependency updates.

Policies are for single-request guardrails such as merchant allowlists,
per-request amount caps, and approval rules. Use budgets for cumulative limits
such as daily or monthly spend.
