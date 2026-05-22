# Policy Engine

Hubu's policy engine is a deterministic rules engine for agent spending. It
takes a structured `SpendRequest`, evaluates it against a human-authored
`Policy`, and returns an auditable `Evaluation`.

The current v1 engine supports one policy at a time. Mandates and multi-policy
evaluation can be layered on top later by reusing the same `Rule` and
`Condition` model.

## Core Flow

```txt
SpendRequest + Policy
        |
        v
validate_policy(policy)
        |
        v
evaluate every rule condition
        |
        v
collect RuleResult trace
        |
        v
merge matched effects by precedence
        |
        v
Evaluation
```

Validation happens before condition evaluation. If validation rejects the
policy, the request is not evaluated. If a type mismatch somehow reaches
condition evaluation anyway, the evaluator panics because that means an invalid
policy bypassed validation.

## Data Model

`SpendRequest` is the request the agent wants to spend:

```rust
SpendRequest {
    amount_cents: 4_500,
    currency: Currency::Usd,
    agent_id: AgentId::new(),
    merchant: Some("Acme Cafe".to_string()),
    category: Some("meals".to_string()),
}
```

`Policy` is the human-authored rule set:

```rust
Policy {
    id: "base_spending_policy".to_string(),
    version: "2026-05-22.1".to_string(),
    default_effect: Effect::NeedsApproval,
    rules: vec![/* rules */],
}
```

`Evaluation` is the final decision plus the trace:

```rust
Evaluation {
    policy_id,
    policy_version,
    decision,
    reasons,
    rule_results,
}
```

## Effects

Each matching rule contributes one `Effect`:

```txt
allow
needs_approval
deny
```

Effects are merged by strict precedence:

```txt
deny > needs_approval > allow > policy default
```

That means:

- any matching `deny` rule makes the final decision `deny`
- otherwise, any matching `needs_approval` rule makes the final decision
  `needs_approval`
- otherwise, any matching `allow` rule makes the final decision `allow`
- if no rules match, the policy's `default_effect` is used

For spending, `needs_approval` is the recommended default.

## Rules

A `Rule` has an id, effect, condition, and human-facing reason:

```rust
Rule {
    id: "allow_small_meals".to_string(),
    effect: Effect::Allow,
    reason: "Meals under $50 are pre-approved".to_string(),
    when: Condition::All {
        conditions: vec![
            Condition::Eq {
                field: Field::Category,
                value: PolicyValue::String("meals".to_string()),
            },
            Condition::Lte {
                field: Field::Amount,
                value: PolicyValue::MoneyCents(5_000),
            },
        ],
    },
}
```

When a rule matches, its effect and reason appear in `RuleResult`. If a rule
does not match, the result records `matched: false` and leaves `effect` and
`reason` empty.

## Conditions

Conditions are a typed expression tree. Supported operators:

```txt
all
any
not
eq
neq
gt
gte
lt
lte
in
exists
```

Supported fields:

```txt
amount      -> money_cents
currency    -> currency
agent_id    -> agent_id
merchant    -> string
category    -> string
```

Ordered comparisons are currently only valid for `amount`.

## Validation

`validate_policy` rejects malformed policies before evaluation. It checks:

- policy id is present
- policy version is present
- rule id is present
- rule reason is present
- `all` / `any` groups are not empty
- condition values match their fields
- ordered operators are only used on orderable fields

Examples of invalid policy conditions:

```rust
// Invalid: amount expects MoneyCents, not String.
Condition::Gt {
    field: Field::Amount,
    value: PolicyValue::String("50000".to_string()),
}

// Invalid: category is not orderable.
Condition::Gt {
    field: Field::Category,
    value: PolicyValue::String("meals".to_string()),
}
```

## Evaluation Example

```rust
use hubu_common::ids::AgentId;
use hubu_core::policy::{
    evaluate_policy, Condition, Currency, Effect, Field, Policy, PolicyValue, Rule, SpendRequest,
};

let request = SpendRequest {
    amount_cents: 4_500,
    currency: Currency::Usd,
    agent_id: AgentId::new(),
    merchant: Some("Acme Cafe".to_string()),
    category: Some("meals".to_string()),
};

let policy = Policy {
    id: "base_spending_policy".to_string(),
    version: "2026-05-22.1".to_string(),
    default_effect: Effect::NeedsApproval,
    rules: vec![Rule {
        id: "allow_small_meals".to_string(),
        effect: Effect::Allow,
        reason: "Meals under $50 are pre-approved".to_string(),
        when: Condition::All {
            conditions: vec![
                Condition::Eq {
                    field: Field::Category,
                    value: PolicyValue::String("meals".to_string()),
                },
                Condition::Lte {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(5_000),
                },
            ],
        },
    }],
};

let evaluation = evaluate_policy(&request, &policy).expect("policy should be valid");

assert_eq!(evaluation.decision, Effect::Allow);
assert_eq!(evaluation.reasons, vec!["Meals under $50 are pre-approved"]);
```

## Human-Authored Policy Shape

The Rust model is designed for future YAML/TOML/JSON policy files. A YAML policy
would look roughly like this:

```yaml
id: base_spending_policy
version: 2026-05-22.1
default_effect: needs_approval
rules:
  - id: deny_large_purchase
    effect: deny
    reason: Purchase exceeds the $500 single-transaction limit
    when:
      op: gt
      field: amount
      value:
        money_cents: 50000

  - id: allow_small_meals
    effect: allow
    reason: Meals under $50 are pre-approved
    when:
      op: all
      conditions:
        - op: eq
          field: category
          value:
            string: meals
        - op: lte
          field: amount
          value:
            money_cents: 5000
```

## Future Extensions

The v1 API intentionally keeps the core small. Natural next steps:

- return policy id/version on each individual `RuleResult`
- validate duplicate rule ids
- add policy loading from YAML/TOML
- add mandate context with the same rule/condition model
- add multi-policy evaluation by evaluating each policy and merging all matched
  effects with the same precedence
