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

Then attach the policy to the current human user:

```sh
hubu policy validate --path policies/openai-allowlist.yaml
hubu policy add --path policies/openai-allowlist.yaml
```

Policies are for single-request guardrails such as merchant allowlists,
per-request amount caps, and approval rules. Use budgets for cumulative limits
such as daily or monthly spend.
