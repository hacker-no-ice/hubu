---
name: hubu-policy-authoring
description: Draft, review, and validate extensible Hubu spending policies from an operator's constraints, then hand the file to the owner to apply. Use for per-request caps, approval or deny rules, typed provider/executor/capability/billing-merchant allowlists, and user-default or agent-specific policy assignment; use Hubu budgets instead for cumulative period limits.
---

# Hubu Policy Authoring

Translate the operator's intent into a policy whose behavior is apparent from
its rules and whose independently governed constraints can change without
rewriting unrelated rules.

Read [references/policy-language.md](references/policy-language.md) before
drafting YAML or choosing the apply command for the owner. It contains the
supported schema, safe composition pattern, attachment commands, and review
checklist.

You draft and validate; the owner applies. Applying a policy or changing a
budget is an owner action confirmed with Touch ID, and no agent tool applies
one. Never try to apply a policy or change a budget yourself.

## Establish the intended governance

Determine these items from the request and available Hubu state:

- whether the policy is the user's default or an override for one registered
  agent;
- the per-request effects: `allow`, `needs_approval`, and `deny`;
- the maximum amount in integer USD cents, if any;
- each governed identity separately: provider, executor, capability, and
  billing merchant; and
- whether any limit is cumulative. A daily, monthly, or other period total is a
  budget, not a policy rule. Do not disguise a cumulative limit as a
  per-request amount condition.

Ask only about material ambiguities. In particular, do not infer one typed
identity from another: the service performing work, the execution adapter, the
requested outcome, and the party charging the account are distinct. Resolve
stable ids from Hubu's trusted execution-scope catalog or a known canonical
execution scope; do not turn display names into guessed ids. New policies use
the typed fields and not the legacy `merchant` field.

## Compose for independent change

Default to `needs_approval` unless the operator explicitly chooses another
fallback.

Hubu evaluates every rule and merges matching effects as:

```text
deny > needs_approval > allow > default_effect
```

This is not first-match evaluation. Separate positive allow rules behave like
OR, so they do not jointly enforce an amount cap and several allowlists.

For a pre-approved path governed by multiple constraints:

1. Represent each independently governed boundary as its own deny guardrail,
   such as amount over the cap, provider outside its allowlist, executor outside
   its allowlist, capability outside its allowlist, or billing merchant outside
   its allowlist.
2. Add a narrow allow rule for the intended path. The independent deny rules
   continue to win when any boundary is violated.
3. Keep one concern per rule and give it a stable id and a reason that explains
   the operator's intent. Group conditions only when one business decision
   genuinely depends on that conjunction or disjunction.

Do not add a catch-all allow merely to avoid approval. Preserve operator
choices instead of broadening them, and fail closed when a required typed field
is absent. A negated allowlist guardrail naturally denies an absent field
because `in` is false and `not` makes the guardrail match.

## Draft, validate, and review

Create or edit the YAML in the operator's chosen location. Otherwise use a
descriptive file under `policies/`. Use a stable authored `id`; change
`version` when the authored rule payload changes.

Run local validation before handing the file over:

```bash
hubu policy validate --path policies/POLICY.yaml
```

Present a compact review containing:

- assignment scope (`user_default` or the exact agent id);
- default effect;
- each rule id, condition, effect, and reason;
- money values in both cents and dollars;
- the resolved provider, executor, capability, and billing-merchant ids; and
- any cumulative constraint that must be implemented as a separate budget.

Call out precedence-sensitive behavior and test representative requests
mentally or with existing policy evaluation facilities: a fully compliant
request, each individual constraint violation, a missing typed field, the exact
amount boundary, and one cent above it.

## Hand the draft to the owner

End by telling the owner where the file is and how to apply it. Give both
routes, with the exact path and scope filled in:

- **Portal:** open the Policies page, choose the policy for that scope (or
  **New policy**), click **Load file…** and pick the file, then **Validate**.
  The editor shows what the draft changes before **Save**, which asks the
  owner to confirm on their Mac.
- **CLI:** the exact command, for example

  ```bash
  hubu policy apply --path policies/POLICY.yaml --agent-id agt_EXACT_AGENT_ID
  ```

  Omit `--agent-id` for the user default; pass it for that agent's override.
  The command prints the diff against the policy currently applied to that
  scope before the Touch ID dialog appears.

When replacing an existing policy, you may add
`--expected-revision CURRENT_REVISION --expected-hash CURRENT_SHA256` from
`hubu_show_policy` so a concurrent change stops the apply. Use the authored id
as the declarative key unless the operator needs a different stable key.

For a cumulative limit, hand over the exact budget command instead of a
policy file, for example:

```bash
hubu budget update --budget-id bud_EXACT_BUDGET_ID --amount 20.00 --reason "December design work"
```

After the owner says it is applied, you may read the policy back with
`hubu_show_policy` and confirm the intended assignment and revision. Never
treat a successful YAML parse as proof that the correct live scope was
attached.
