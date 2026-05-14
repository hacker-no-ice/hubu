use hubu_common::PaymentIntent;
use hubu_core::{BudgetManager, PolicyDecision, PolicyEngine};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthorizePaymentRequest {
    pub intent: PaymentIntent,
    pub budget_limit: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizePaymentResponse {
    pub approved: bool,
    pub remaining_budget: u128,
}

#[derive(Clone, Debug, Default)]
pub struct McpAdapter {
    policy_engine: PolicyEngine,
}

impl McpAdapter {
    pub fn authorize_payment(
        &self,
        request: AuthorizePaymentRequest,
    ) -> anyhow::Result<AuthorizePaymentResponse> {
        let mut budget = BudgetManager::new(request.budget_limit);
        let decision = self.policy_engine.authorize(&request.intent, &mut budget)?;

        Ok(AuthorizePaymentResponse {
            approved: decision == PolicyDecision::Approved,
            remaining_budget: budget.remaining(),
        })
    }
}
