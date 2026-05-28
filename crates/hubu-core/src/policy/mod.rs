//! Deterministic spending policy evaluation.
//!
//! The policy engine evaluates a structured [`SpendRequest`] against a
//! human-authored [`Policy`] and returns an auditable [`Evaluation`].
//!
//! Evaluation is intentionally boring:
//!
//! 1. Validate the policy with [`validate_policy`].
//! 2. Evaluate every rule condition.
//! 3. Collect every [`RuleResult`] for auditability.
//! 4. Merge matched rule effects into one final decision.
//!
//! Effect precedence is fixed:
//!
//! ```text
//! deny > needs_approval > allow > policy default
//! ```
//!
//! Validation catches field/value mismatches before evaluation. If a mismatch
//! reaches condition evaluation anyway, the evaluator panics because invalid
//! policy bypassed validation.
//!
//! # Example
//!
//! ```rust,ignore
//! use hubu_common::ids::AgentId;
//! use hubu_core::policy::{
//!     evaluate_policy, Condition, Effect, Field, Policy, PolicyValue, Rule,
//! };
//! use hubu_core::spend::SpendRequest;
//! use hubu_common::money::Currency;
//!
//! let request = SpendRequest {
//!     amount_cents: 4_500,
//!     currency: Currency::Usd,
//!     agent_id: AgentId::new(),
//!     merchant: Some("Acme Cafe".to_string()),
//!     category: Some("meals".to_string()),
//! };
//!
//! let policy = Policy {
//!     id: "base_spending_policy".to_string(),
//!     version: "2026-05-22.1".to_string(),
//!     default_effect: Effect::NeedsApproval,
//!     rules: vec![Rule {
//!         id: "allow_small_meals".to_string(),
//!         effect: Effect::Allow,
//!         reason: "Meals under $50 are pre-approved".to_string(),
//!         when: Condition::Lte {
//!             field: Field::Amount,
//!             value: PolicyValue::MoneyCents(5_000),
//!         },
//!     }],
//! };
//!
//! let evaluation = evaluate_policy(&request, &policy)?;
//! assert_eq!(evaluation.decision, Effect::Allow);
//! # Ok::<(), hubu_core::policy::PolicyValidationError>(())
//! ```
//!
//! See `docs/policy-engine.md` for a fuller guide and YAML-shaped examples.

pub mod condition;
pub mod engine;
pub mod error;
pub mod loader;
pub mod model;

pub use condition::*;
pub use engine::*;
pub use error::*;
pub use model::*;
