//! Targets Gongbu must not send new paid work to, whatever the operator's
//! provider configuration says.
//!
//! The Gemini Interactions image paths are token-metered, and none of the four
//! verified Gemini packs (`google-gemini-developer-image`, its `-edit`
//! variant, and their v2 revisions) sends an image-count, output-token or
//! monetary cap. Gongbu cannot verify a provider-enforced cost ceiling for
//! them, so it refuses them at admission and again immediately before the
//! first provider submission (HUB-388). Records already created stay readable,
//! and reconciliation of an earlier submission never resubmits, so neither is
//! affected.

use super::targets::TargetKey;

/// Failure code recorded when the workflow refuses a blocked target.
pub const UNBOUNDED_COST_CODE: &str = "target_cost_unbounded";

/// Provider and adapter shared by every Gemini Interactions image pack, for
/// both generation and editing.
const BLOCKED_ADAPTERS: &[(&str, &str)] = &[("google", "gemini_developer_image")];

/// Whether Gongbu must refuse new work for this target before any budget
/// claim or provider request.
pub fn blocks_new_work(provider: &str, adapter: &str) -> bool {
    BLOCKED_ADAPTERS
        .iter()
        .any(|(blocked_provider, blocked_adapter)| {
            provider == *blocked_provider && adapter == *blocked_adapter
        })
}

/// [`blocks_new_work`] for a validated target key.
pub fn blocks_target(key: &TargetKey) -> bool {
    blocks_new_work(&key.provider, &key.adapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::verified_packs::verified_packs;

    #[test]
    fn every_verified_gemini_pack_is_blocked_and_every_bfl_pack_is_not() {
        let mut gemini = 0;
        let mut bfl = 0;
        for pack in verified_packs() {
            let blocked = blocks_new_work(pack.provider(), pack.adapter_id());
            for model in pack.models() {
                assert_eq!(blocks_target(&pack.target_key(model)), blocked);
            }
            match pack.provider() {
                "google" => {
                    gemini += 1;
                    assert!(blocked, "{} must be blocked", pack.pack_id());
                }
                _ => {
                    bfl += 1;
                    assert!(!blocked, "{} must stay open", pack.pack_id());
                }
            }
        }
        assert_eq!(gemini, 4);
        assert_eq!(bfl, 16);
    }
}
