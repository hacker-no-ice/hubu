use crate::policy::error::PolicyValidationError;
use crate::policy::model::{Effect, Evaluation, Policy, Rule, RuleResult};
use crate::spend::model::SpendRequest;

/// Evaluate a spend request against a policy and return the final decision trace.
///
/// This validates the policy first. Invalid policy returns
/// [`PolicyValidationError`] and the request is not evaluated.
///
/// Every rule is evaluated so the returned [`Evaluation`] can explain both the
/// final decision and the matching rule reasons.
pub fn evaluate_policy(
    request: &SpendRequest,
    policy: &Policy,
) -> Result<Evaluation, PolicyValidationError> {
    validate_policy(policy)?;

    let rule_results = evaluate_rules(request, &policy.rules);
    let decision = final_decision(policy.default_effect, &rule_results);
    let reasons = rule_results
        .iter()
        .filter_map(|result| result.reason.clone())
        .collect();

    Ok(Evaluation {
        policy_id: policy.id.clone(),
        policy_version: policy.version.clone(),
        decision,
        reasons,
        rule_results,
    })
}

/// Validate a policy before it is used for spend evaluation.
///
/// This first pass catches shape and type errors that would otherwise make rule
/// evaluation ambiguous, such as comparing `amount` to a string or using `gt`
/// on `category`.
pub fn validate_policy(policy: &Policy) -> Result<(), PolicyValidationError> {
    if policy.id.trim().is_empty() {
        return Err(PolicyValidationError::EmptyPolicyId);
    }

    if policy.version.trim().is_empty() {
        return Err(PolicyValidationError::EmptyPolicyVersion);
    }

    for rule in &policy.rules {
        if rule.id.trim().is_empty() {
            return Err(PolicyValidationError::EmptyRuleId);
        }

        if rule.reason.trim().is_empty() {
            return Err(PolicyValidationError::EmptyRuleReason {
                rule_id: rule.id.clone(),
            });
        }

        rule.when.validate(&rule.id)?;
    }

    Ok(())
}

/// Evaluate a spend request against a slice of rules and return every rule result.
fn evaluate_rules(request: &SpendRequest, rules: &[Rule]) -> Vec<RuleResult> {
    rules.iter().map(|rule| rule.evaluate(request)).collect()
}

/// Merge rule evaluation results.
///
/// If no rule matched, return the policy default. Otherwise merge by fixed
/// precedence: deny > needs_approval > allow.
fn final_decision(default: Effect, results: &[RuleResult]) -> Effect {
    let mut decision: Option<Effect> = None;

    for result in results {
        let Some(effect) = result.effect else {
            continue;
        };
        decision = Some(match decision {
            None => effect,
            Some(current) => merge_effects(current, effect),
        });
    }

    decision.unwrap_or(default)
}

fn merge_effects(current: Effect, effect: Effect) -> Effect {
    match (current, effect) {
        // 1. Highest Priority, Deny wins everything
        (_, Effect::Deny) | (Effect::Deny, _) => Effect::Deny,
        // 2. Medium Priority, Needs Approval wins Allow
        (_, Effect::NeedsApproval) | (Effect::NeedsApproval, _) => Effect::NeedsApproval,
        // 3. Lowest Priority, If both are Allow, it stays Allow
        (Effect::Allow, Effect::Allow) => Effect::Allow,
    }
}

#[cfg(test)]
mod tests {
    use hubu_common::ids::{AgentAccountId, AgentId, UserId};

    use super::*;
    use crate::policy::condition::{Condition, Field, PolicyValue, ValueKind};
    use hubu_common::money::Currency;

    fn spend_request(amount_cents: i64, category: Option<&str>) -> SpendRequest {
        SpendRequest {
            amount_cents,
            currency: Currency::Usd,
            owner_user_id: test_user_id(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            merchant: Some("Acme Cafe".to_string()),
            category: category.map(str::to_string),
            task_id: None,
        }
    }

    fn rule(id: &str, effect: Effect, when: Condition) -> Rule {
        Rule {
            id: id.to_string(),
            effect,
            when,
            reason: format!("{id} matched"),
        }
    }

    fn policy(default_effect: Effect, rules: Vec<Rule>) -> Policy {
        Policy {
            id: "base_spending_policy".to_string(),
            version: "2026-05-22.1".to_string(),
            owner_user_id: test_user_id(),
            rules,
            default_effect,
        }
    }

    fn test_user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    #[test]
    fn returns_default_effect_when_no_rules_match() {
        let request = spend_request(2_500, Some("meals"));
        let policy = policy(
            Effect::NeedsApproval,
            vec![rule(
                "deny_large_purchase",
                Effect::Deny,
                Condition::Gt {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(50_000),
                },
            )],
        );

        let evaluation = evaluate_policy(&request, &policy).expect("policy should be valid");

        assert_eq!(evaluation.decision, Effect::NeedsApproval);
        assert_eq!(evaluation.policy_id, "base_spending_policy");
        assert_eq!(evaluation.policy_version, "2026-05-22.1");
        assert!(evaluation.reasons.is_empty());
        assert_eq!(evaluation.rule_results.len(), 1);
    }

    #[test]
    fn deny_wins_over_allow() {
        let request = spend_request(60_000, Some("meals"));
        let policy = policy(
            Effect::NeedsApproval,
            vec![
                rule(
                    "allow_meals",
                    Effect::Allow,
                    Condition::Eq {
                        field: Field::Category,
                        value: PolicyValue::String("meals".to_string()),
                    },
                ),
                rule(
                    "deny_large_purchase",
                    Effect::Deny,
                    Condition::Gt {
                        field: Field::Amount,
                        value: PolicyValue::MoneyCents(50_000),
                    },
                ),
            ],
        );

        let evaluation = evaluate_policy(&request, &policy).expect("policy should be valid");

        assert_eq!(evaluation.decision, Effect::Deny);
        assert_eq!(
            evaluation.reasons,
            vec![
                "allow_meals matched".to_string(),
                "deny_large_purchase matched".to_string()
            ]
        );
    }

    #[test]
    fn needs_approval_wins_over_allow() {
        let request = spend_request(8_000, Some("meals"));
        let policy = policy(
            Effect::Deny,
            vec![
                rule(
                    "allow_meals",
                    Effect::Allow,
                    Condition::Eq {
                        field: Field::Category,
                        value: PolicyValue::String("meals".to_string()),
                    },
                ),
                rule(
                    "approve_meals_over_75",
                    Effect::NeedsApproval,
                    Condition::Gt {
                        field: Field::Amount,
                        value: PolicyValue::MoneyCents(7_500),
                    },
                ),
            ],
        );

        let evaluation = evaluate_policy(&request, &policy).expect("policy should be valid");

        assert_eq!(evaluation.decision, Effect::NeedsApproval);
    }

    #[test]
    fn rule_result_contains_effect_and_reason_only_when_matched() {
        let request = spend_request(2_500, Some("meals"));
        let rules = vec![
            rule(
                "allow_small_meals",
                Effect::Allow,
                Condition::Lte {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(5_000),
                },
            ),
            rule(
                "deny_large_purchase",
                Effect::Deny,
                Condition::Gt {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(50_000),
                },
            ),
        ];

        let results = evaluate_rules(&request, &rules);

        assert_eq!(results.len(), 2);
        assert!(results[0].matched);
        assert_eq!(results[0].effect, Some(Effect::Allow));
        assert_eq!(
            results[0].reason,
            Some("allow_small_meals matched".to_string())
        );
        assert!(!results[1].matched);
        assert_eq!(results[1].effect, None);
        assert_eq!(results[1].reason, None);
    }

    #[test]
    fn validation_rejects_field_value_mismatch() {
        let policy = policy(
            Effect::NeedsApproval,
            vec![rule(
                "invalid_amount_type",
                Effect::Deny,
                Condition::Gt {
                    field: Field::Amount,
                    value: PolicyValue::String("50000".to_string()),
                },
            )],
        );

        let error = validate_policy(&policy).expect_err("policy should be invalid");

        match error {
            PolicyValidationError::FieldValueMismatch {
                rule_id,
                field,
                expected,
                actual,
            } => {
                assert_eq!(rule_id, "invalid_amount_type");
                assert_eq!(field, Field::Amount);
                assert_eq!(expected, ValueKind::MoneyCents);
                assert_eq!(actual, ValueKind::String);
            }
            other => panic!("unexpected validation error: {other:?}"),
        }
    }

    #[test]
    fn validation_rejects_ordered_operator_on_unorderable_field() {
        let policy = policy(
            Effect::NeedsApproval,
            vec![rule(
                "invalid_category_ordering",
                Effect::Deny,
                Condition::Gt {
                    field: Field::Category,
                    value: PolicyValue::String("meals".to_string()),
                },
            )],
        );

        let error = validate_policy(&policy).expect_err("policy should be invalid");

        match error {
            PolicyValidationError::UnorderableField {
                rule_id,
                field,
                operator,
            } => {
                assert_eq!(rule_id, "invalid_category_ordering");
                assert_eq!(field, Field::Category);
                assert_eq!(operator, "gt");
            }
            other => panic!("unexpected validation error: {other:?}"),
        }
    }

    #[test]
    fn evaluate_policy_returns_validation_error_before_evaluating_invalid_policy() {
        let request = spend_request(2_500, Some("meals"));
        let policy = policy(
            Effect::NeedsApproval,
            vec![rule(
                "invalid_amount_type",
                Effect::Deny,
                Condition::Gt {
                    field: Field::Amount,
                    value: PolicyValue::String("50000".to_string()),
                },
            )],
        );

        assert!(matches!(
            evaluate_policy(&request, &policy),
            Err(PolicyValidationError::FieldValueMismatch { .. })
        ));
    }
}
