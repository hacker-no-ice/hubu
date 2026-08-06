//! Gongbu API domain modules.

pub mod artifact;
pub mod config;
pub mod execution;
pub mod http;
pub mod hubu;
pub mod provider;

// Preserve the original public module paths while callers migrate at their own pace.
pub use artifact as artifacts;
pub use config::redaction;
pub use config::secrets;
pub use execution as persistence;
pub use http as http_api;
pub use provider::contract as provider_contract;
pub use provider::targets as provider_targets;
