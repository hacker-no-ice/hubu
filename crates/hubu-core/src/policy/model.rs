use hubu_common::ids::AgentId;
use serde::{Deserialize, Serialize};

use crate::policy::condition::Condition;

#[derive(Debug, Clone)]
pub struct SpendRequest {
    pub amount_cents: i64, // in minor unit
    pub currency: Currency,
    pub agent_id: AgentId,
    pub merchant: Option<String>,
    pub category: Option<String>,
}

// we only read in policy from file so, only derive deserialize
#[derive(Debug, Clone, Deserialize)]
pub struct Policy {
    pub id: String,
    pub version: String,
    pub rules: Vec<Rule>,
    pub default_effect: Effect,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    pub effect: Effect,
    pub when: Condition,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleResult {
    pub rule_id: String,
    pub matched: bool,
    pub effect: Option<Effect>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evaluation {
    pub policy_id: String,
    pub policy_version: String,
    pub decision: Effect,
    pub reasons: Vec<String>,
    pub rule_results: Vec<RuleResult>,
}

#[derive(Debug, Copy, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Deny,
    NeedsApproval,
}

#[derive(Debug, Copy, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Usd,
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
