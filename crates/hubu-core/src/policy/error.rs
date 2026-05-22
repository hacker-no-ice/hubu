use crate::policy::condition::{Field, ValueKind};

#[derive(Debug, thiserror::Error)]
pub enum PolicyValidationError {
    #[error("policy id cannot be empty")]
    EmptyPolicyId,

    #[error("policy version cannot be empty")]
    EmptyPolicyVersion,

    #[error("rule id cannot be empty")]
    EmptyRuleId,

    #[error("rule `{rule_id}` reason cannot be empty")]
    EmptyRuleReason { rule_id: String },

    #[error("rule `{rule_id}` has an empty `{operator}` condition group")]
    EmptyConditionGroup {
        rule_id: String,
        operator: &'static str,
    },

    #[error("rule `{rule_id}` uses field `{field}` with {actual} value, but expected {expected}")]
    FieldValueMismatch {
        rule_id: String,
        field: Field,
        expected: ValueKind,
        actual: ValueKind,
    },

    #[error(
        "rule `{rule_id}` uses ordered operator `{operator}` on non-orderable field `{field}`"
    )]
    UnorderableField {
        rule_id: String,
        field: Field,
        operator: &'static str,
    },
}
