//! Provider contracts, target configuration, and adapter-facing types.

pub mod contract;
pub mod flux_contract;
pub mod http_kernel;
pub mod pack;
pub mod pack_async;
pub mod pack_sync;
pub mod provider_contracts;
pub mod registry;
pub mod targets;
pub mod verified_packs;

#[cfg(test)]
mod conformance;
