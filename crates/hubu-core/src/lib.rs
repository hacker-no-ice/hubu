use hubu_common::{Amount, PaymentIntent};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct BudgetManager {
    remaining: Amount,
}

impl BudgetManager {
    pub fn new(limit: Amount) -> Self {
        Self { remaining: limit }
    }

    pub fn remaining(&self) -> Amount {
        self.remaining
    }

    pub fn reserve(&mut self, amount: Amount) -> Result<(), PolicyError> {
        if amount > self.remaining {
            return Err(PolicyError::BudgetExceeded {
                requested: amount,
                remaining: self.remaining,
            });
        }

        self.remaining -= amount;
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct PolicyEngine;

impl PolicyEngine {
    pub fn authorize(
        &self,
        intent: &PaymentIntent,
        budget: &mut BudgetManager,
    ) -> Result<PolicyDecision, PolicyError> {
        if intent.amount == 0 {
            return Err(PolicyError::InvalidAmount);
        }

        budget.reserve(intent.amount)?;
        Ok(PolicyDecision::Approved)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyDecision {
    Approved,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PolicyError {
    #[error("payment amount must be greater than zero")]
    InvalidAmount,
    #[error("budget exceeded: requested {requested}, remaining {remaining}")]
    BudgetExceeded {
        requested: Amount,
        remaining: Amount,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorizes_payment_when_budget_is_available() {
        let intent = PaymentIntent::new("alice", "bob", 10, "USDC");
        let mut budget = BudgetManager::new(25);

        let decision = PolicyEngine::default().authorize(&intent, &mut budget);

        assert_eq!(decision, Ok(PolicyDecision::Approved));
        assert_eq!(budget.remaining(), 15);
    }
}
