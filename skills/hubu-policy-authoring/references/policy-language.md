# Hubu policy language and attachment

Use this reference while drafting, validating, reviewing, or attaching a Hubu
policy.

## YAML shape

```yaml
id: image_generation_guardrails
version: "2026-08-31.1"
default_effect: needs_approval
rules:
  - id: deny_amount_over_cap
    effect: deny
    reason: a single request must not exceed $5.00
    when:
      op: gt
      field: amount
      value:
        money_cents: 500

  - id: deny_unapproved_provider
    effect: deny
    reason: only approved image providers may be used
    when:
      op: not
      condition:
        op: in
        field: provider
        values:
          - string: provider:google:gemini-developer
          - string: provider:black-forest-labs:flux

  - id: deny_unapproved_executor
    effect: deny
    reason: provider work must run through the approved executor
    when:
      op: not
      condition:
        op: in
        field: executor
        values:
          - string: executor:gongbu:image

  - id: deny_unapproved_capability
    effect: deny
    reason: this policy is limited to image generation
    when:
      op: not
      condition:
        op: in
        field: capability
        values:
          - string: capability:image:generate

  - id: deny_unapproved_billing_merchant
    effect: deny
    reason: only approved billing merchants may charge the account
    when:
      op: not
      condition:
        op: in
        field: billing_merchant
        values:
          - string: merchant:google
          - string: merchant:black-forest-labs

  - id: allow_within_single_request_cap
    effect: allow
    reason: requests within the $5.00 cap are pre-approved when no guardrail denies them
    when:
      op: lte
      field: amount
      value:
        money_cents: 500
```

This pattern makes the amount, provider, executor, capability, and billing
merchant independently editable. The final amount rule supplies the positive
allow path; any matching guardrail still denies because deny has higher
precedence. `neq` does not match a missing field, so use `not` + `in` rather
than `neq` when absence must fail closed and the field is not guaranteed by the
request contract.

The ids above are examples from Hubu's current built-in catalog, not a license
to guess an id. Resolve the canonical tuple for the target environment.

## Supported policy fields and values

| Field | Value encoding | Notes |
| --- | --- | --- |
| `amount` | `{money_cents: INTEGER}` | The only field supporting `gt`, `gte`, `lt`, and `lte` |
| `currency` | `{currency: usd}` | Hubu currently supports USD |
| `agent_id` | `{agent_id: agt_...}` | Use a registered public agent id |
| `provider` | `{string: provider:...}` | Stable provider id from the trusted scope catalog |
| `executor` | `{string: executor:...}` | Stable execution-plane adapter id |
| `capability` | `{string: capability:...}` | Provider-independent requested outcome |
| `billing_merchant` | `{string: merchant:...}` | Party expected to charge the account |
| `category` | `{string: VALUE}` | Optional caller-supplied category |
| `merchant` | `{string: VALUE}` | Legacy only; do not use for new execution policies |

Supported condition operators are `all`, `any`, `not`, `eq`, `neq`, `gt`,
`gte`, `lt`, `lte`, `in`, and `exists`. `all` and `any` use `conditions`; `not`
uses one `condition`; `in` uses `values`; comparison operators use one
`value`. Empty `all` or `any` groups and unknown fields are invalid.

Effects are `allow`, `needs_approval`, and `deny`. All matching effects merge
with `deny > needs_approval > allow > default_effect`.

## Attachment scopes

Hubu stores the policy resource separately from its assignment:

- User default: applies when the spending agent has no override.
- Agent override: applies only to the exact registered agent and takes
  precedence over the user default.

Validate and apply through the CLI:

```bash
hubu policy validate --path policies/POLICY.yaml

# User default
hubu policy apply --path policies/POLICY.yaml \
  --name "REVIEWABLE DISPLAY NAME"

# Exact agent override
hubu policy apply --path policies/POLICY.yaml \
  --name "REVIEWABLE DISPLAY NAME" \
  --agent-id agt_EXACT_AGENT_ID
```

When updating an existing resource, inspect it and pin the write:

```bash
hubu policy show --policy-id pol_EXACT_POLICY_ID
hubu policy apply --path policies/POLICY.yaml \
  --key STABLE_DECLARATIVE_KEY \
  --expected-revision CURRENT_REVISION \
  --expected-hash CURRENT_SHA256
```

Equivalent unified MCP operations are:

- `hubu_show_policy`, `hubu_export_policy`, `hubu_policy_history`, and
  `hubu_policy_diff` for read-only inspection;
- `hubu_apply_policy` for reconciliation and assignment. It requires
  `policy_yaml`; optional fields are `declarative_key`, `display_name`,
  `agent_id`, `expected_revision`, and `expected_hash`.

Applying without `agent_id` targets the user default. Applying with `agent_id`
targets that exact agent override. Applying identical canonical content is
idempotent; changed content appends an immutable revision. A stale expected
revision or hash must stop the workflow for fresh inspection rather than be
silently omitted.

## Review checklist

- Does the attachment scope name the intended user default or exact agent?
- Are cumulative limits represented as budgets rather than policy rules?
- Are amounts non-negative integer USD cents with boundary behavior stated?
- Are typed ids taken from a canonical scope instead of inferred from display
  names?
- Does every independently governed constraint have its own rule and reason?
- Could two separate allow rules accidentally broaden access through OR-like
  effect merging?
- Do missing typed fields fail closed where the policy requires them?
- Does `needs_approval` or another deliberate fallback cover unmatched work?
- Were current revision and hash pinned for a replacement?
- After apply, does `show` report the intended assignment and returned revision?
