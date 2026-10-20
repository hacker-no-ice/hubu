//! Provider contracts, target configuration, and adapter-facing types.

pub mod contract;
pub mod flux2_api;
pub mod gemini_developer_image;
pub mod http_kernel;
pub mod pack;
pub mod pack_sync;
pub mod provider_contracts;
pub mod registry;
pub mod targets;

#[cfg(test)]
mod conformance;
