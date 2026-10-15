use hubu_common::ids::AgentId;
use hubu_common::money::{Amount, Currency};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

use crate::policy::error::PolicyValidationError;
use crate::spend::model::SpendRequest;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
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

#[derive(Debug, Copy, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Amount,
    Currency,
    AgentId,
    Merchant,
    Provider,
    Executor,
    Capability,
    BillingMerchant,
    Category,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ValueKind {
    String,
    Money,
    Currency,
    AgentId,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyValue {
    String(String),
    /// An exact budget-currency amount, authored as a quoted decimal string
    /// (`money: "0.005"`).
    Money(#[serde(with = "money_string")] Amount),
    /// Legacy whole-cent amount (`money_cents: 10` ≡ `money: "0.10"`). Kept
    /// as its own variant so stored, hashed policy revisions re-serialize
    /// unchanged; it compares as [`ValueKind::Money`].
    MoneyCents(i64),
    Currency(Currency),
    AgentId(AgentId),
}

impl Condition {
    pub fn describe(&self) -> String {
        match self {
            Condition::All { conditions } => describe_group("all", conditions),
            Condition::Any { conditions } => describe_group("any", conditions),
            Condition::Not { condition } => format!("not ({})", condition.describe()),
            Condition::Eq { field, value } => describe_comparison(*field, "equals", value),
            Condition::Neq { field, value } => describe_comparison(*field, "does not equal", value),
            Condition::Gt { field, value } => describe_comparison(*field, "is greater than", value),
            Condition::Gte { field, value } => describe_comparison(*field, "is at least", value),
            Condition::Lt { field, value } => describe_comparison(*field, "is less than", value),
            Condition::Lte { field, value } => describe_comparison(*field, "is at most", value),
            Condition::In { field, values } => format!(
                "{} is one of {}",
                field_name(*field),
                values.iter().map(value_name).collect::<Vec<_>>().join(", ")
            ),
            Condition::Exists { field } => format!("{} is supplied", field_name(*field)),
        }
    }

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

    /// Every money value this condition compares the request amount with.
    pub fn amount_values(&self) -> Vec<&PolicyValue> {
        match self {
            Condition::All { conditions } | Condition::Any { conditions } => conditions
                .iter()
                .flat_map(Condition::amount_values)
                .collect(),
            Condition::Not { condition } => condition.amount_values(),
            Condition::Eq { field, value }
            | Condition::Neq { field, value }
            | Condition::Gt { field, value }
            | Condition::Gte { field, value }
            | Condition::Lt { field, value }
            | Condition::Lte { field, value } => {
                if *field == Field::Amount {
                    vec![value]
                } else {
                    Vec::new()
                }
            }
            Condition::In { field, values } => {
                if *field == Field::Amount {
                    values.iter().collect()
                } else {
                    Vec::new()
                }
            }
            Condition::Exists { .. } => Vec::new(),
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

fn describe_group(operator: &str, conditions: &[Condition]) -> String {
    format!(
        "{operator} of ({})",
        conditions
            .iter()
            .map(Condition::describe)
            .collect::<Vec<_>>()
            .join("; ")
    )
}

fn describe_comparison(field: Field, operator: &str, value: &PolicyValue) -> String {
    format!("{} {operator} {}", field_name(field), value_name(value))
}

fn field_name(field: Field) -> &'static str {
    match field {
        Field::Amount => "amount",
        Field::Currency => "currency",
        Field::AgentId => "agent_id",
        Field::Merchant => "merchant",
        Field::Provider => "provider",
        Field::Executor => "executor",
        Field::Capability => "capability",
        Field::BillingMerchant => "billing_merchant",
        Field::Category => "category",
    }
}

fn value_name(value: &PolicyValue) -> String {
    match value {
        PolicyValue::String(value) => format!("`{value}`"),
        PolicyValue::Money(amount) => format!("USD {amount}"),
        PolicyValue::MoneyCents(cents) => match Amount::from_legacy_cents(*cents) {
            Ok(amount) => format!("USD {amount}"),
            Err(_) => format!("{cents} cents"),
        },
        PolicyValue::Currency(value) => value.to_string(),
        PolicyValue::AgentId(value) => value.to_string(),
    }
}

impl Field {
    pub fn value_from(&self, request: &SpendRequest) -> Option<PolicyValue> {
        match self {
            Field::Amount => Some(PolicyValue::Money(request.amount)),
            Field::Currency => Some(PolicyValue::Currency(request.currency)),
            Field::AgentId => Some(PolicyValue::AgentId(request.agent_id.clone())),
            Field::Merchant => request.merchant.clone().map(PolicyValue::String),
            Field::Provider => request
                .execution_scope
                .as_ref()
                .map(|scope| PolicyValue::String(scope.provider.id.clone())),
            Field::Executor => request
                .execution_scope
                .as_ref()
                .map(|scope| PolicyValue::String(scope.executor.id.clone())),
            Field::Capability => request
                .execution_scope
                .as_ref()
                .map(|scope| PolicyValue::String(scope.capability.id.clone())),
            Field::BillingMerchant => request
                .execution_scope
                .as_ref()
                .map(|scope| PolicyValue::String(scope.billing_merchant.id.clone())),
            Field::Category => request.category.clone().map(PolicyValue::String),
        }
    }

    pub fn value_kind(&self) -> ValueKind {
        match self {
            Field::Amount => ValueKind::Money,
            Field::Currency => ValueKind::Currency,
            Field::AgentId => ValueKind::AgentId,
            Field::Merchant
            | Field::Provider
            | Field::Executor
            | Field::Capability
            | Field::BillingMerchant
            | Field::Category => ValueKind::String,
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
            PolicyValue::Money(_) | PolicyValue::MoneyCents(_) => ValueKind::Money,
            PolicyValue::Currency(_) => ValueKind::Currency,
            PolicyValue::AgentId(_) => ValueKind::AgentId,
        }
    }

    /// The budget-currency amount of a money value, or `None` for other
    /// kinds and for legacy cents beyond the accounting-scale range.
    pub fn money(&self) -> Option<Amount> {
        match self {
            PolicyValue::Money(amount) => Some(*amount),
            PolicyValue::MoneyCents(cents) => Amount::from_legacy_cents(*cents).ok(),
            _ => None,
        }
    }

    /// Exact accounting-scale units for comparison. Widening to `i128`
    /// keeps legacy cent values beyond the `Amount` range distinct and
    /// ordered.
    fn money_units(&self) -> Option<i128> {
        match self {
            PolicyValue::Money(amount) => Some(i128::from(amount.units())),
            PolicyValue::MoneyCents(cents) => Some(i128::from(*cents) * 10_000),
            _ => None,
        }
    }

    fn eq_value(&self, other: &PolicyValue) -> bool {
        assert_value_kind_matches(self, other);
        match (self.money_units(), other.money_units()) {
            (Some(left), Some(right)) => left == right,
            _ => self == other,
        }
    }

    fn money_pair(&self, other: &PolicyValue, operator: &str) -> (i128, i128) {
        match (self.money_units(), other.money_units()) {
            (Some(left), Some(right)) => (left, right),
            _ => panic!(
                "policy value mismatch: cannot compare {} {operator} {}",
                self.kind(),
                other.kind()
            ),
        }
    }

    fn gt_value(&self, other: &PolicyValue) -> bool {
        let (left, right) = self.money_pair(other, ">");
        left > right
    }

    fn gte_value(&self, other: &PolicyValue) -> bool {
        let (left, right) = self.money_pair(other, ">=");
        left >= right
    }

    fn lt_value(&self, other: &PolicyValue) -> bool {
        let (left, right) = self.money_pair(other, "<");
        left < right
    }

    fn lte_value(&self, other: &PolicyValue) -> bool {
        let (left, right) = self.money_pair(other, "<=");
        left <= right
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
            Field::Provider => f.write_str("provider"),
            Field::Executor => f.write_str("executor"),
            Field::Capability => f.write_str("capability"),
            Field::BillingMerchant => f.write_str("billing_merchant"),
            Field::Category => f.write_str("category"),
        }
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueKind::String => f.write_str("string"),
            ValueKind::Money => f.write_str("money"),
            ValueKind::Currency => f.write_str("currency"),
            ValueKind::AgentId => f.write_str("agent_id"),
        }
    }
}

/// Serde for policy money: exact decimal strings only. Unquoted numbers are
/// rejected because YAML/JSON parsers may read them as binary floats.
mod money_string {
    use super::*;

    pub fn serialize<S: Serializer>(amount: &Amount, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(amount)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Amount, D::Error> {
        deserializer.deserialize_any(MoneyVisitor)
    }

    struct MoneyVisitor;

    const QUOTE_HINT: &str =
        "quote decimal amounts so they stay exact, for example `money: \"0.005\"`";

    impl de::Visitor<'_> for MoneyVisitor {
        type Value = Amount;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a quoted decimal amount such as \"0.005\"")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Amount, E> {
            value.parse().map_err(E::custom)
        }

        fn visit_f64<E: de::Error>(self, value: f64) -> Result<Amount, E> {
            Err(E::custom(format_args!(
                "unquoted number {value} for `money`; {QUOTE_HINT}"
            )))
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<Amount, E> {
            Err(E::custom(format_args!(
                "unquoted number {value} for `money`; {QUOTE_HINT}"
            )))
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<Amount, E> {
            Err(E::custom(format_args!(
                "unquoted number {value} for `money`; {QUOTE_HINT}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_money::cents;
    use hubu_common::execution_scope::{
        ExecutionScope, ScopeIdentity, EXECUTION_SCOPE_SCHEMA_VERSION,
    };
    use hubu_common::ids::{AgentAccountId, UserId};

    fn spend_request() -> SpendRequest {
        SpendRequest {
            amount: cents(4_500),
            currency: Currency::Usd,
            owner_user_id: test_user_id(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            merchant: Some("Acme Cafe".to_string()),
            execution_scope: None,
            category: Some("meals".to_string()),
            task_id: None,
            reason: "test spend".to_string(),
            lease_profile: "default".to_string(),
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
    fn evaluates_each_typed_execution_scope_field() {
        let mut request = spend_request();
        let identity = |id: &str| ScopeIdentity {
            id: id.into(),
            display_name: id.into(),
        };
        request.execution_scope = Some(ExecutionScope {
            schema_version: EXECUTION_SCOPE_SCHEMA_VERSION,
            provider: identity("provider:google:gemini-developer"),
            executor: identity("executor:gongbu:image"),
            capability: identity("capability:image:generate"),
            billing_merchant: identity("merchant:google"),
        });
        for (field, expected) in [
            (Field::Provider, "provider:google:gemini-developer"),
            (Field::Executor, "executor:gongbu:image"),
            (Field::Capability, "capability:image:generate"),
            (Field::BillingMerchant, "merchant:google"),
        ] {
            assert!(Condition::Eq {
                field,
                value: PolicyValue::String(expected.into())
            }
            .eval(&request));
        }
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

    fn money(value: &str) -> PolicyValue {
        PolicyValue::Money(value.parse().unwrap())
    }

    #[test]
    fn money_and_legacy_cents_compare_as_one_kind() {
        let request = spend_request(); // 4_500 cents = USD 45.00
        for (value, lte, gt, eq) in [
            (money("45"), true, false, true),
            (PolicyValue::MoneyCents(4_500), true, false, true),
            (money("44.999999"), false, true, false),
            (money("45.000001"), true, false, false),
        ] {
            let lte_holds = Condition::Lte {
                field: Field::Amount,
                value: value.clone(),
            }
            .eval(&request);
            let gt_holds = Condition::Gt {
                field: Field::Amount,
                value: value.clone(),
            }
            .eval(&request);
            let eq_holds = Condition::Eq {
                field: Field::Amount,
                value: value.clone(),
            }
            .eval(&request);
            assert_eq!((lte_holds, gt_holds, eq_holds), (lte, gt, eq), "{value:?}");
        }
        assert!(Condition::In {
            field: Field::Amount,
            values: vec![money("1"), PolicyValue::MoneyCents(4_500)],
        }
        .eval(&request));
        assert_eq!(money("0.10").kind(), PolicyValue::MoneyCents(10).kind());
        assert_eq!(money("0.10").money(), PolicyValue::MoneyCents(10).money());
    }

    #[test]
    fn mixed_money_and_legacy_conditions_validate() {
        let condition = Condition::All {
            conditions: vec![
                Condition::Lte {
                    field: Field::Amount,
                    value: money("0.005"),
                },
                Condition::Gt {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(0),
                },
            ],
        };
        condition.validate("mixed").unwrap();
    }

    #[test]
    fn legacy_amounts_beyond_the_accounting_range_stay_exact_and_ordered() {
        // The largest request sits between two legacy caps that are both
        // beyond the accounting range; they must not collapse into one value.
        let mut request = spend_request();
        request.amount = Amount::from_units(i64::MAX);
        let below = PolicyValue::MoneyCents(922_337_203_685_477);
        let above = PolicyValue::MoneyCents(922_337_203_685_478);
        assert_eq!(above.money(), None);
        for (cap, lte) in [(below, false), (above, true)] {
            assert_eq!(
                Condition::Lte {
                    field: Field::Amount,
                    value: cap.clone(),
                }
                .eval(&request),
                lte,
                "{cap:?}"
            );
            assert_eq!(
                Condition::Gt {
                    field: Field::Amount,
                    value: cap,
                }
                .eval(&request),
                !lte
            );
        }
    }

    #[test]
    fn describes_money_in_budget_currency() {
        assert_eq!(
            Condition::Lte {
                field: Field::Amount,
                value: money("0.005"),
            }
            .describe(),
            "amount is at most USD 0.005"
        );
        assert_eq!(
            Condition::Gt {
                field: Field::Amount,
                value: PolicyValue::MoneyCents(10),
            }
            .describe(),
            "amount is greater than USD 0.10"
        );
    }

    #[test]
    fn money_serializes_as_an_exact_string_and_legacy_cents_unchanged() {
        assert_eq!(
            serde_json::to_value(money("0.005")).unwrap(),
            serde_json::json!({"money": "0.005"})
        );
        assert_eq!(
            serde_json::to_value(PolicyValue::MoneyCents(10)).unwrap(),
            serde_json::json!({"money_cents": 10})
        );
        let parsed: PolicyValue =
            serde_json::from_value(serde_json::json!({"money": "0.0050"})).unwrap();
        assert_eq!(parsed, money("0.005"));
    }

    #[test]
    fn money_rejects_unquoted_numbers_and_invalid_decimals() {
        for (value, expected) in [
            (serde_json::json!({"money": 0.005}), "quote decimal amounts"),
            (serde_json::json!({"money": 5}), "quote decimal amounts"),
            (
                serde_json::json!({"money": "0.0050000"}),
                "at most 6 decimal places",
            ),
            (
                serde_json::json!({"money": "-0.01"}),
                "must not carry a sign",
            ),
            (
                serde_json::json!({"money": "1e-3"}),
                "exponents are not accepted",
            ),
            (
                serde_json::json!({"money": "9223372036854.775808"}),
                "too large",
            ),
        ] {
            let error = serde_json::from_value::<PolicyValue>(value.clone()).unwrap_err();
            assert!(error.to_string().contains(expected), "{value}: {error}");
        }
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
