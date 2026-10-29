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
//! use hubu_core::budget::BudgetManager;
//! fn bypass(manager: &mut BudgetManager) {
//!     manager.apply_committed_state(todo!());
//! }
//! ```
//!
//! ```compile_fail,E0603
//! use hubu_core::budget::coordinator::BudgetCoordinator;
//! ```
//!
//! ```compile_fail,E0603
//! use hubu_core::persistence::AppendBudgetVersionRequest;
//! ```
//!
//! ```compile_fail,E0603
//! use hubu_core::persistence::AppendBudgetVersionResult;
//! ```
//!
//! ```compile_fail,E0603
//! use hubu_core::persistence::BudgetVersionRepository;
//! ```
//!
//! ```compile_fail,E0432
//! use hubu_core::app::BudgetUpdateService;
//! ```
//!
//! ```compile_fail,E0599
//! use hubu_core::{budget::{Budget, BudgetVersion, BudgetBalance}, persistence::SqliteGovernanceRepository};
//! fn bypass(repository: &mut SqliteGovernanceRepository, budget: &Budget, version: &BudgetVersion, balance: &BudgetBalance) {
//!     repository.save_budget_with_balance(budget, version, balance);
//! }
//! ```

mod commands;
pub(crate) mod coordinator;
pub mod dto;
pub mod error;
pub mod manager;
pub mod model;
pub(crate) mod state;
pub use commands::*;

pub use dto::*;
pub use error::*;
pub use manager::*;
pub use model::*;
