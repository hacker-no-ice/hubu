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

Hubu policy files use the YAML 1.1 syntax supported by `serde_yaml_ng`. Existing
starter and test policies are migration fixtures. Policy, rule, and condition
objects reject unknown fields; required fields and semantic validation remain
enforced after parsing.

Callers may rely on Hubu's top-level errors (`failed to parse policy yaml` and
`invalid policy`), but the parser's detailed source diagnostic is not a stable
public contract and can change across dependency updates.

Policies are for single-request guardrails such as merchant allowlists,
per-request amount caps, and approval rules. Use budgets for cumulative limits
such as daily or monthly spend.
