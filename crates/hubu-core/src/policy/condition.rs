use hubu_common::ids::AgentId;
use hubu_common::money::Currency;
use serde::Deserialize;
use std::fmt;

use crate::policy::error::PolicyValidationError;
use crate::spend::model::SpendRequest;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Condition {
    All {
        conditions: Vec<Condition>,
    },
    Any {
        conditions: Vec<Condition>,
    },
    Not {
        condition: Box<Condition>,
    },
    Eq {
        field: Field,
        value: PolicyValue,
    },
    Neq {
        field: Field,
        value: PolicyValue,
    },
    Gt {
        field: Field,
        value: PolicyValue,
    },
    Gte {
        field: Field,
        value: PolicyValue,
    },
    Lt {
        field: Field,
        value: PolicyValue,
    },
    Lte {
        field: Field,
        value: PolicyValue,
    },
    In {
        field: Field,
        values: Vec<PolicyValue>,
    },
    Exists {
        field: Field,
    },
}

#[derive(Debug, Copy, Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Amount,
    Currency,
    AgentId,
    Merchant,
    Category,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ValueKind {
    String,
    MoneyCents,
    Currency,
    AgentId,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyValue {
    String(String),
    MoneyCents(i64),
    Currency(Currency),
    AgentId(AgentId),
}

impl Condition {
    pub fn eval(&self, request: &SpendRequest) -> bool {
        match self {
            Condition::All { conditions } => {
                conditions.iter().all(|condition| condition.eval(request))
            }
            Condition::Any { conditions } => {
                conditions.iter().any(|condition| condition.eval(request))
            }
            Condition::Not { condition } => !condition.eval(request),
            Condition::Eq { field, value } => field
                .value_from(request)
                .is_some_and(|actual| actual.eq_value(value)),
            Condition::Neq { field, value } => field
                .value_from(request)
                .is_some_and(|actual| !actual.eq_value(value)),
            Condition::Gt { field, value } => field
                .value_from(request)
                .is_some_and(|actual| actual.gt_value(value)),
            Condition::Gte { field, value } => field
                .value_from(request)
                .is_some_and(|actual| actual.gte_value(value)),
            Condition::Lt { field, value } => field
                .value_from(request)
                .is_some_and(|actual| actual.lt_value(value)),
            Condition::Lte { field, value } => field
                .value_from(request)
                .is_some_and(|actual| actual.lte_value(value)),
            Condition::In { field, values } => field
                .value_from(request)
                .is_some_and(|actual| values.iter().any(|value| actual.eq_value(value))),
            Condition::Exists { field } => field.value_from(request).is_some(),
        }
    }

    pub fn validate(&self, rule_id: &str) -> Result<(), PolicyValidationError> {
        match self {
            Condition::All { conditions } => validate_group(rule_id, "all", conditions),
            Condition::Any { conditions } => validate_group(rule_id, "any", conditions),
            Condition::Not { condition } => condition.validate(rule_id),
            Condition::Eq { field, value } | Condition::Neq { field, value } => {
                validate_field_value(rule_id, *field, value)
            }
            Condition::Gt { field, value } => {
                validate_ordered_field_value(rule_id, "gt", *field, value)
            }
            Condition::Gte { field, value } => {
                validate_ordered_field_value(rule_id, "gte", *field, value)
            }
            Condition::Lt { field, value } => {
                validate_ordered_field_value(rule_id, "lt", *field, value)
            }
            Condition::Lte { field, value } => {
                validate_ordered_field_value(rule_id, "lte", *field, value)
            }
            Condition::In { field, values } => {
                for value in values {
                    validate_field_value(rule_id, *field, value)?;
                }
                Ok(())
            }
            Condition::Exists { .. } => Ok(()),
        }
    }
}

impl Field {
    pub fn value_from(&self, request: &SpendRequest) -> Option<PolicyValue> {
        match self {
            Field::Amount => Some(PolicyValue::MoneyCents(request.amount_cents)),
            Field::Currency => Some(PolicyValue::Currency(request.currency)),
            Field::AgentId => Some(PolicyValue::AgentId(request.agent_id.clone())),
            Field::Merchant => request.merchant.clone().map(PolicyValue::String),
            Field::Category => request.category.clone().map(PolicyValue::String),
        }
    }

    pub fn value_kind(&self) -> ValueKind {
        match self {
            Field::Amount => ValueKind::MoneyCents,
            Field::Currency => ValueKind::Currency,
            Field::AgentId => ValueKind::AgentId,
            Field::Merchant | Field::Category => ValueKind::String,
        }
    }

    pub fn is_orderable(&self) -> bool {
        matches!(self, Field::Amount)
    }
}

impl PolicyValue {
    pub fn kind(&self) -> ValueKind {
        match self {
            PolicyValue::String(_) => ValueKind::String,
            PolicyValue::MoneyCents(_) => ValueKind::MoneyCents,
            PolicyValue::Currency(_) => ValueKind::Currency,
            PolicyValue::AgentId(_) => ValueKind::AgentId,
        }
    }

    fn eq_value(&self, other: &PolicyValue) -> bool {
        assert_value_kind_matches(self, other);
        self == other
    }

    fn gt_value(&self, other: &PolicyValue) -> bool {
        match (self, other) {
            (PolicyValue::MoneyCents(left), PolicyValue::MoneyCents(right)) => left > right,
            _ => panic!(
                "policy value mismatch: cannot compare {} > {}",
                self.kind(),
                other.kind()
            ),
        }
    }

    fn gte_value(&self, other: &PolicyValue) -> bool {
        match (self, other) {
            (PolicyValue::MoneyCents(left), PolicyValue::MoneyCents(right)) => left >= right,
            _ => panic!(
                "policy value mismatch: cannot compare {} >= {}",
                self.kind(),
                other.kind()
            ),
        }
    }

    fn lt_value(&self, other: &PolicyValue) -> bool {
        match (self, other) {
            (PolicyValue::MoneyCents(left), PolicyValue::MoneyCents(right)) => left < right,
            _ => panic!(
                "policy value mismatch: cannot compare {} < {}",
                self.kind(),
                other.kind()
            ),
        }
    }

    fn lte_value(&self, other: &PolicyValue) -> bool {
        match (self, other) {
            (PolicyValue::MoneyCents(left), PolicyValue::MoneyCents(right)) => left <= right,
            _ => panic!(
                "policy value mismatch: cannot compare {} <= {}",
                self.kind(),
                other.kind()
            ),
        }
    }
}

fn validate_group(
    rule_id: &str,
    operator: &'static str,
    conditions: &[Condition],
) -> Result<(), PolicyValidationError> {
    if conditions.is_empty() {
        return Err(PolicyValidationError::EmptyConditionGroup {
            rule_id: rule_id.to_string(),
            operator,
        });
    }

    for condition in conditions {
        condition.validate(rule_id)?;
    }

    Ok(())
}

fn validate_field_value(
    rule_id: &str,
    field: Field,
    value: &PolicyValue,
) -> Result<(), PolicyValidationError> {
    let expected = field.value_kind();
    let actual = value.kind();

    if expected != actual {
        return Err(PolicyValidationError::FieldValueMismatch {
            rule_id: rule_id.to_string(),
            field,
            expected,
            actual,
        });
    }

    Ok(())
}

fn validate_ordered_field_value(
    rule_id: &str,
    operator: &'static str,
    field: Field,
    value: &PolicyValue,
) -> Result<(), PolicyValidationError> {
    if !field.is_orderable() {
        return Err(PolicyValidationError::UnorderableField {
            rule_id: rule_id.to_string(),
            field,
            operator,
        });
    }

    validate_field_value(rule_id, field, value)
}

fn assert_value_kind_matches(left: &PolicyValue, right: &PolicyValue) {
    assert_eq!(
        left.kind(),
        right.kind(),
        "policy value mismatch: cannot compare {} with {}",
        left.kind(),
        right.kind()
    );
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Field::Amount => f.write_str("amount"),
            Field::Currency => f.write_str("currency"),
            Field::AgentId => f.write_str("agent_id"),
            Field::Merchant => f.write_str("merchant"),
            Field::Category => f.write_str("category"),
        }
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueKind::String => f.write_str("string"),
            ValueKind::MoneyCents => f.write_str("money_cents"),
            ValueKind::Currency => f.write_str("currency"),
            ValueKind::AgentId => f.write_str("agent_id"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubu_common::ids::{AgentAccountId, UserId};

    fn spend_request() -> SpendRequest {
        SpendRequest {
            amount_cents: 4_500,
            currency: Currency::Usd,
            owner_user_id: test_user_id(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            merchant: Some("Acme Cafe".to_string()),
            category: Some("meals".to_string()),
            task_id: None,
        }
    }

    fn test_user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    #[test]
    fn evaluates_amount_comparisons() {
        let request = spend_request();

        assert!(Condition::Lte {
            field: Field::Amount,
            value: PolicyValue::MoneyCents(5_000),
        }
        .eval(&request));

        assert!(!Condition::Gt {
            field: Field::Amount,
            value: PolicyValue::MoneyCents(5_000),
        }
        .eval(&request));
    }

    #[test]
    fn evaluates_boolean_groups() {
        let request = spend_request();

        let condition = Condition::All {
            conditions: vec![
                Condition::Eq {
                    field: Field::Category,
                    value: PolicyValue::String("meals".to_string()),
                },
                Condition::Not {
                    condition: Box::new(Condition::Gt {
                        field: Field::Amount,
                        value: PolicyValue::MoneyCents(5_000),
                    }),
                },
            ],
        };

        assert!(condition.eval(&request));
    }

    #[test]
    fn missing_optional_field_does_not_match() {
        let mut request = spend_request();
        request.merchant = None;

        assert!(!Condition::Eq {
            field: Field::Merchant,
            value: PolicyValue::String("Acme Cafe".to_string()),
        }
        .eval(&request));

        assert!(!Condition::Exists {
            field: Field::Merchant,
        }
        .eval(&request));
    }

    #[test]
    #[should_panic(expected = "policy value mismatch")]
    fn type_mismatch_panics_during_ordered_comparison() {
        let request = spend_request();

        Condition::Gt {
            field: Field::Amount,
            value: PolicyValue::String("5000".to_string()),
        }
        .eval(&request);
    }
}
