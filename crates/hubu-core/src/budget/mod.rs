//! Public budget models and the [`BudgetManager`] facade.
//!
//! Raw state and committed-record application are internal to core. Transports
//! must use supported commands instead of bypassing application coordination.
//!
//! ```compile_fail,E0603
//! use hubu_core::budget::state::BudgetState;
//! ```
//!
//! ```compile_fail,E0624
//! use hubu_core::budget::{BudgetManager, BudgetHold, BudgetBalance};
//! fn bypass(manager: &mut BudgetManager, hold: BudgetHold, balance: BudgetBalance) {
//!     manager.apply_persisted_finalization(hold, balance);
//! }
//! ```
//!
//! ```compile_fail,E0624
//! use hubu_core::budget::{BudgetManager, BudgetVersion, BudgetWithBalance};
//! fn bypass(manager: &mut BudgetManager, version: BudgetVersion, current: BudgetWithBalance) {
//!     manager.apply_persisted_budget_version_append(version, current);
//! }
//! ```

pub mod dto;
pub mod error;
pub mod manager;
pub mod model;
mod state;

pub use dto::*;
pub use error::*;
pub use manager::*;
pub use model::*;
