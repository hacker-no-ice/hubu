use hubu_common::PaymentIntent;
use hubu_core::{BudgetManager, PolicyEngine};

fn main() -> anyhow::Result<()> {
    let intent = PaymentIntent::new("alice", "bob", 1_000_000, "USDC");
    let mut budget = BudgetManager::new(2_000_000);
    let decision = PolicyEngine::default().authorize(&intent, &mut budget)?;

    println!("decision: {decision:?}");
    println!("remaining budget: {}", budget.remaining());

    Ok(())
}
