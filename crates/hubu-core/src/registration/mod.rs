//! Agent registration flow.
//!
//! Registration connects the stable identity of an agent to the exact runnable
//! version it is using, gives that identity an account, and records a session
//! for the current MCP connection.
//!
//! The flow is intentionally split into small, deterministic steps:
//!
//! 1. Validate the request shape.
//! 2. Resolve or create [`AgentIdentity`](hubu_common::models::identity::AgentIdentity) by identity fingerprint.
//! 3. Resolve or create [`AgentVersion`](hubu_common::models::identity::AgentVersion) inside that agent's version lineage.
//! 4. Resolve or create the agent's single [`AgentAccount`](hubu_common::models::account::AgentAccount).
//! 5. Create a fresh [`AgentSession`](hubu_common::models::session::AgentSession) for this connection.
//!
//! Fingerprint scope matters:
//!
//! ```text
//! identity_fingerprint: globally identifies one logical agent
//! version_fingerprint: identifies one version only within one agent lineage
//! ```
//!
//! That means two agents can use identical code/model/runtime config while
//! still owning separate version records. This keeps audit history and ownership
//! boundaries simple for the in-memory prototype.
//!
//! See `docs/registration-flow.md` for a fuller guide.
//!
//! # Example
//!
//! ```rust,ignore
//! use hubu_core::registration::{RegisterAgentRequest, RegistrationManager};
//!
//! let mut manager = RegistrationManager::new();
//! let response = manager.register_agent(RegisterAgentRequest {
//!     // fill identity, version, and MCP client fields
//! })?;
//!
//! assert_eq!(response.version.agent_id, response.agent.id);
//! # Ok::<(), hubu_core::registration::RegistrationError>(())
//! ```

mod error;
mod manager;
mod model;

pub use error::*;
pub use manager::*;
pub use model::*;
