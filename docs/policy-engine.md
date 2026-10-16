# Policy Engine

Hubu's policy engine is a deterministic rules engine for agent spending. It
takes a structured `SpendRequest`, evaluates it against a human-authored
`Policy`, and returns an auditable `Evaluation`.

The v1 engine evaluates one policy at a time. Mandate aggregation and
multi-policy evaluation are outside this contract.

## Declarative Resources

Authored YAML is reconciled into an owner-scoped policy resource. The authored
`id` is the default immutable declarative key; the server creates a stable
opaque `pol_` public id and a mutable display name. The authored `version`
remains part of the rule payload for evaluation compatibility, while the server
assigns an independent monotonic revision number and SHA-256 payload hash.

`hubu policy apply` is idempotent. Reapplying identical canonical content does
not append a revision. Changed content appends an immutable revision and moves
the current pointer in the same SQLite transaction. Optional
`--expected-revision` and `--expected-hash` arguments provide compare-and-set;
stale or invalid requests do not alter the current policy or its assignment.

Assignments are separate references. A user-default assignment is the fallback
and an agent override takes precedence. `show`, `export`, `history`, and `diff`
are available through the CLI, HTTP API, and MCP adapter. History includes the
actor, source, timestamp, old/new hashes, and affected assignments.

On first startup after upgrade, embedded legacy assignment rows are migrated
transactionally. Identical owner/id payloads share a resource. If the same
legacy id had divergent content in separate scopes, Hubu creates a
hash-suffixed migration key so each scope keeps its exact effective rules.
Legacy authored ids that do not satisfy the new safe key syntax are mapped to a
deterministic `legacy-<hash>` key; their authored id remains unchanged inside
the immutable policy payload.

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
    amount: "45.00".parse()?, // exact, in the request currency
    currency: Currency::Usd,
    agent_id: AgentId::new(),
    execution_scope: Some(canonical_execution_scope),
    merchant: None,
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
                value: PolicyValue::Money("50.00".parse()?),
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
amount      -> money (exact decimal, request currency)
currency    -> currency
agent_id    -> agent_id
merchant    -> string
provider    -> string (stable provider id)
executor    -> string (stable executor id)
capability  -> string (stable capability id)
billing_merchant -> string (stable billing merchant id)
category    -> string
```

`merchant` is retained only for legacy policies. New execution policies should
target the four typed scope fields; see the
[trusted execution scope](spend-lifecycle.md#trusted-execution-scope).

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
// Invalid: amount expects Money, not String.
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
    amount: "45.00".parse()?,
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
                    value: PolicyValue::Money("50.00".parse()?),
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

Policies are authored as YAML. The corresponding shape is:

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
        money: "500.00"

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
            money: "50.00"
```

### Money values

- Write amounts as quoted decimal strings with at most 6 decimal places, such
  as `money: "50.00"` or `money: "0.005"`. Unquoted numbers (`money: 0.005`),
  more decimal places, signs and exponents are rejected when the policy is
  written, so a YAML float can never round a limit.
- An amount value compares with the request amount in the request's own
  currency (`usd` or `usdc`); budgets are per currency. Scope a cap to one
  currency with a `currency` condition.
- `POST /policies` returns `warnings`, without rejecting the policy, when an
  amount value is finer than one cent, because USD wallet payments move whole
  cents and can never match it.
- The legacy `money_cents: 5000` form still loads and means the same as
  `money: "50.00"`; both compare in the same policy. Stored policy revisions are
  never rewritten.

See [policies/README.md](../policies/README.md) for the full YAML rules.

Policies can be loaded directly from YAML:

```rust
use hubu_core::policy::Policy;

let policy = Policy::from_yaml_file("policies/base.yaml")?;
```

Loading performs three steps:

```txt
read file -> parse yaml -> validate policy
```

If the YAML parses but the policy is invalid, loading returns a validation
error before the policy can be used for evaluation.
