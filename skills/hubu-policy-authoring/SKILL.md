---
name: hubu-policy-authoring
description: Author, review, validate, and attach extensible Hubu spending policies from an operator's constraints. Use for per-request caps, approval or deny rules, typed provider/executor/capability/billing-merchant allowlists, and user-default or agent-specific policy assignment; use Hubu budgets instead for cumulative period limits.
---

# Hubu Policy Authoring

Translate the operator's intent into a policy whose behavior is apparent from
its rules and whose independently governed constraints can change without
rewriting unrelated rules.

Read [references/policy-language.md](references/policy-language.md) before
drafting YAML or choosing apply arguments. It contains the supported schema,
safe composition pattern, attachment commands, and review checklist.

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

Run local validation before proposing attachment:

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

## Attach only after explicit review

Policy attachment changes live spending authority. Obtain the operator's
explicit approval of the reviewed YAML and assignment scope immediately before
calling an apply operation. Validation and inspection are read-only and do not
need that approval.

Prefer compare-and-set when replacing an existing policy: inspect its current
revision and hash, then supply them during apply. Use the authored id as the
declarative key unless the operator needs a different stable owner-scoped key.
Omitting `--agent-id` assigns the user default; passing it creates or replaces
that agent's override.

After apply, report the returned policy id, declarative key, revision, payload
hash, and assignment scope. Show the policy again and verify that the intended
assignment is present. Never treat a successful YAML parse as proof that the
correct live scope was attached.
