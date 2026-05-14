use hubu_common::PaymentIntent;
use hubu_core::{BudgetManager, PolicyDecision, PolicyEngine};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatePaymentRequest {
    pub intent: PaymentIntent,
    pub budget_limit: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreatePaymentResponse {
    pub approved: bool,
    pub remaining_budget: u128,
}

#[derive(Clone, Debug, Default)]
pub struct ApiService {
    policy_engine: PolicyEngine,
}

impl ApiService {
    pub fn create_payment(
        &self,
        request: CreatePaymentRequest,
    ) -> anyhow::Result<CreatePaymentResponse> {
        let mut budget = BudgetManager::new(request.budget_limit);
        let decision = self.policy_engine.authorize(&request.intent, &mut budget)?;

        Ok(CreatePaymentResponse {
            approved: decision == PolicyDecision::Approved,
            remaining_budget: budget.remaining(),
        })
    }
}
