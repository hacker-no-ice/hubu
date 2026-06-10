use serde::{Deserialize, Serialize};

use hubu_common::ids::UserId;

use crate::policy::condition::Condition;
use crate::spend::model::SpendRequest;

/// A human-authored policy version.
///
/// The engine evaluates all rules and falls back to `default_effect` when no
/// rule matches.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub id: String,
    pub version: String,
    #[serde(default = "UserId::new")]
    pub owner_user_id: UserId,
    pub rules: Vec<Rule>,
    pub default_effect: Effect,
}

/// One declarative policy rule.
///
/// If `when` evaluates to true, `effect` contributes to the final decision and
/// `reason` appears in the evaluation trace.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub effect: Effect,
    pub when: Condition,
    pub reason: String,
}

/// The result of evaluating one rule against one spend request.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuleResult {
    pub rule_id: String,
    pub matched: bool,
    pub effect: Option<Effect>,
    pub reason: Option<String>,
}

/// The auditable output of policy evaluation.
///
/// `decision` is the final merged effect. `rule_results` contains every rule,
/// matched or not, so callers can explain how the decision was reached.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Evaluation {
    pub policy_id: String,
    pub policy_version: String,
    pub decision: Effect,
    pub reasons: Vec<String>,
    pub rule_results: Vec<RuleResult>,
}

/// A rule effect and final decision value.
///
/// When multiple matching rules produce effects, they are merged with this
/// precedence: deny > needs_approval > allow.
#[derive(Debug, Copy, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Deny,
    NeedsApproval,
}

impl Rule {
    pub fn evaluate(&self, request: &SpendRequest) -> RuleResult {
        let matched = self.when.eval(request);

        RuleResult {
            rule_id: self.id.clone(),
            matched,
            effect: matched.then_some(self.effect),
            reason: matched.then(|| self.reason.clone()),
        }
    }
}
