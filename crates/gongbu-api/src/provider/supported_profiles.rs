//! Exact managed-stack provider profiles and their sanitized catalog projection.

use super::{
    contract::{PricingCatalog, PricingRule, PricingUnit},
    targets::{AdapterSettings, ProviderTargetConfig, SupportedProfileBinding},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;

const PROFILE_DOCUMENT: &str = include_str!("../../../../contracts/provider-profiles-v1.json");
const PROFILE_DOCUMENT_SHA256: &str =
    "920d1f14c9da7273648d8cbfddca273092c6a94ffc49c0c2f04831681a4b3263";

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("supported provider profile document is invalid")]
    InvalidDocument,
    #[error("supported provider profile contract is unknown")]
    UnknownContract,
    #[error("supported provider profile binding does not match its frozen contract")]
    BindingMismatch,
    #[error("supported provider profile target does not match its frozen contract")]
    TargetMismatch,
    #[error("supported provider profile credential is not isolated from another provider")]
    CredentialIsolation,
    #[error("supported provider profile pricing does not match its frozen contract")]
    PricingMismatch,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    profiles: Vec<Profile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    contract: String,
    pricing_version: String,
    pricing_reviewed_on: String,
    target: Target,
    capability: Capability,
    policies: Policies,
    pricing_rules: Vec<PricingRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    provider_config_version: String,
    workload_type: String,
    provider: String,
    adapter: String,
    model: String,
    active: bool,
    execution_enabled: bool,
    settings: AdapterSettings,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    image_count: u32,
    output_formats: Vec<String>,
    presets: Vec<Preset>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    name: String,
    width: u32,
    height: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policies {
    generation_retries: u32,
    fallback: bool,
    poll: String,
    artifact_delivery: String,
    recovery: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CatalogProfile {
    pub contract: String,
    pub pricing_version: String,
    pub pricing_reviewed_on: String,
    pub target: CatalogTarget,
    pub capability: CatalogCapability,
    pub policies: CatalogPolicies,
    pub readiness: CatalogReadiness,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CatalogTarget {
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CatalogCapability {
    pub image_count: u32,
    pub output_formats: Vec<String>,
    pub presets: Vec<CatalogPreset>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CatalogPreset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub currency: String,
    pub rate_numerator_minor: i64,
    pub rate_denominator: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CatalogPolicies {
    pub generation_retries: u32,
    pub fallback: bool,
    pub poll: String,
    pub artifact_delivery: String,
    pub recovery: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CatalogReadiness {
    pub configured: bool,
    pub credential_reference_present: bool,
    pub production_validated: bool,
    pub live_qualified: bool,
    pub live_qualification: String,
}

pub(crate) fn validate_and_project(
    targets: &ProviderTargetConfig,
    pricing: &PricingCatalog,
) -> Result<Vec<CatalogProfile>, Error> {
    let document = document()?;
    let mut projected = Vec::new();
    for binding in targets.supported_profiles() {
        let profile = document
            .profiles
            .iter()
            .find(|profile| profile.contract == binding.contract)
            .ok_or(Error::UnknownContract)?;
        validate_binding(binding, profile)?;
        validate_target(targets, profile)?;
        validate_pricing(pricing, profile)?;
        projected.push(project(profile)?);
    }
    Ok(projected)
}

fn document() -> Result<Document, Error> {
    let digest = format!("{:x}", Sha256::digest(PROFILE_DOCUMENT.as_bytes()));
    if digest != PROFILE_DOCUMENT_SHA256 {
        return Err(Error::InvalidDocument);
    }
    let document: Document =
        serde_json::from_str(PROFILE_DOCUMENT).map_err(|_| Error::InvalidDocument)?;
    if document.schema_version != 1 || document.profiles.is_empty() {
        return Err(Error::InvalidDocument);
    }
    let mut contracts = BTreeSet::new();
    if document
        .profiles
        .iter()
        .any(|profile| !contracts.insert(profile.contract.clone()))
    {
        return Err(Error::InvalidDocument);
    }
    Ok(document)
}

fn validate_binding(binding: &SupportedProfileBinding, profile: &Profile) -> Result<(), Error> {
    if binding.contract != profile.contract
        || binding.pricing_version != profile.pricing_version
        || binding.poll_policy != profile.policies.poll
        || binding.artifact_delivery_policy != profile.policies.artifact_delivery
        || binding.recovery_policy != profile.policies.recovery
        || binding.generation_retries != profile.policies.generation_retries
        || binding.fallback != profile.policies.fallback
    {
        return Err(Error::BindingMismatch);
    }
    Ok(())
}

fn validate_target(targets: &ProviderTargetConfig, profile: &Profile) -> Result<(), Error> {
    let matches = targets
        .revisions()
        .filter(|target| {
            target.provider_config_version == profile.target.provider_config_version
                && target.workload_type == profile.target.workload_type
                && target.provider == profile.target.provider
                && target.adapter == profile.target.adapter
                && target.model == profile.target.model
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(Error::TargetMismatch);
    }
    let target = matches[0];
    if targets
        .revisions()
        .filter(|candidate| {
            candidate.provider == profile.target.provider
                && candidate.adapter == profile.target.adapter
                && candidate.model == profile.target.model
        })
        .count()
        != 1
    {
        return Err(Error::TargetMismatch);
    }
    if target.is_active() != profile.target.active
        || target.is_execution_enabled() != profile.target.execution_enabled
        || target.settings() != &profile.target.settings
    {
        return Err(Error::TargetMismatch);
    }
    if targets.revisions().any(|other| {
        other.provider != target.provider
            && other.secret_service == target.secret_service
            && other.secret_account == target.secret_account
    }) {
        return Err(Error::CredentialIsolation);
    }
    Ok(())
}

fn validate_pricing(pricing: &PricingCatalog, profile: &Profile) -> Result<(), Error> {
    let mut actual = pricing
        .rules()
        .iter()
        .filter(|rule| {
            rule.provider == profile.target.provider && rule.model == profile.target.model
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut expected = profile.pricing_rules.clone();
    actual.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    expected.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    if actual != expected
        || (pricing.rules().len() == expected.len()
            && pricing.catalog_version() != profile.pricing_version)
    {
        return Err(Error::PricingMismatch);
    }
    Ok(())
}

fn project(profile: &Profile) -> Result<CatalogProfile, Error> {
    let presets = profile
        .capability
        .presets
        .iter()
        .map(|preset| {
            let rule = profile
                .pricing_rules
                .iter()
                .find(|rule| {
                    rule.selector
                        .as_ref()
                        .is_some_and(|selector| selector.image_size == preset.name)
                })
                .ok_or(Error::InvalidDocument)?;
            let component = rule
                .components
                .first()
                .filter(|component| {
                    rule.components.len() == 1 && component.unit == PricingUnit::Image
                })
                .ok_or(Error::InvalidDocument)?;
            Ok(CatalogPreset {
                name: preset.name.clone(),
                width: preset.width,
                height: preset.height,
                currency: rule.currency.clone(),
                rate_numerator_minor: component.rate_numerator_minor,
                rate_denominator: component.rate_denominator,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(CatalogProfile {
        contract: profile.contract.clone(),
        pricing_version: profile.pricing_version.clone(),
        pricing_reviewed_on: profile.pricing_reviewed_on.clone(),
        target: CatalogTarget {
            workload_type: profile.target.workload_type.clone(),
            provider: profile.target.provider.clone(),
            adapter: profile.target.adapter.clone(),
            model: profile.target.model.clone(),
        },
        capability: CatalogCapability {
            image_count: profile.capability.image_count,
            output_formats: profile.capability.output_formats.clone(),
            presets,
        },
        policies: CatalogPolicies {
            generation_retries: profile.policies.generation_retries,
            fallback: profile.policies.fallback,
            poll: profile.policies.poll.clone(),
            artifact_delivery: profile.policies.artifact_delivery.clone(),
            recovery: profile.policies.recovery.clone(),
        },
        readiness: CatalogReadiness {
            configured: true,
            credential_reference_present: false,
            production_validated: true,
            live_qualified: false,
            live_qualification: "not_performed".into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::targets::ProviderTargetConfig;
    use serde_json::json;

    fn exact_targets() -> ProviderTargetConfig {
        serde_json::from_value(json!({
            "schema_version": 3,
            "supported_profiles": [{
                "contract": "hubu.flux-2-pro.text-to-image/v1",
                "pricing_version": "bfl-flux-2-pro-usd-2026-08-28-v1",
                "poll_policy": "bfl-async-status-poll-500ms-v1",
                "artifact_delivery_policy": "bfl-delivery-single-region-label-v1",
                "recovery_policy": "hubu-durable-async-resume-v1",
                "generation_retries": 0,
                "fallback": false
            }],
            "provider_configs": [{
                "provider_config_version": "hubu-flux-2-pro-t2i-2026-08-28-v1",
                "workload_type": "image_generation",
                "provider": "flux",
                "adapter": "flux2_api",
                "model": "flux-2-pro",
                "secret_service": "gongbu.bfl",
                "secret_account": "operator",
                "active": true,
                "execution_enabled": true,
                "settings": {"type":"flux2_api","config":{
                    "endpoint":"https://api.bfl.ai","api_version":"v1",
                    "timeout_ms":270000,"poll_interval_ms":500,"max_retries":0,
                    "idempotency_header":null,"approved_artifact_hosts":[],"headers":{}
                }}
            }]
        }))
        .unwrap()
    }

    fn exact_pricing() -> PricingCatalog {
        PricingCatalog::from_json(
            br#"{"schema_version":2,"catalog_version":"bfl-flux-2-pro-usd-2026-08-28-v1","rules":[{"rule_id":"bfl-flux-2-pro-1k-2026-08-28-v1","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":3,"rate_denominator":1}]},{"rule_id":"bfl-flux-2-pro-2k-2026-08-28-v1","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"2k"},"components":[{"unit":"image","rate_numerator_minor":45,"rate_denominator":10}]},{"rule_id":"bfl-flux-2-pro-4k-2026-08-28-v1","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"4k"},"components":[{"unit":"image","rate_numerator_minor":75,"rate_denominator":10}]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn exact_contract_projects_sanitized_unqualified_catalog() {
        let profiles = validate_and_project(&exact_targets(), &exact_pricing()).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].capability.presets[1].width, 1920);
        assert_eq!(profiles[0].capability.presets[1].rate_numerator_minor, 45);
        assert!(!profiles[0].readiness.credential_reference_present);
        assert!(!profiles[0].readiness.live_qualified);
    }

    #[test]
    fn policy_target_and_pricing_mutations_are_rejected() {
        let mut targets = serde_json::to_value(exact_targets()).unwrap();
        targets["supported_profiles"][0]["poll_policy"] = json!("missing");
        let targets: ProviderTargetConfig = serde_json::from_value(targets).unwrap();
        assert!(matches!(
            validate_and_project(&targets, &exact_pricing()),
            Err(Error::BindingMismatch)
        ));

        let mut targets = serde_json::to_value(exact_targets()).unwrap();
        targets["provider_configs"][0]["settings"]["config"]["poll_interval_ms"] = json!(1000);
        let targets: ProviderTargetConfig = serde_json::from_value(targets).unwrap();
        assert!(matches!(
            validate_and_project(&targets, &exact_pricing()),
            Err(Error::TargetMismatch)
        ));

        let pricing = PricingCatalog::from_json(
            br#"{"schema_version":2,"catalog_version":"bfl-flux-2-pro-usd-2026-08-28-v1","rules":[{"rule_id":"bfl-flux-2-pro-1k-2026-08-28-v1","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":4,"rate_denominator":1}]},{"rule_id":"bfl-flux-2-pro-2k-2026-08-28-v1","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"2k"},"components":[{"unit":"image","rate_numerator_minor":45,"rate_denominator":10}]},{"rule_id":"bfl-flux-2-pro-4k-2026-08-28-v1","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"4k"},"components":[{"unit":"image","rate_numerator_minor":75,"rate_denominator":10}]}]}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_and_project(&exact_targets(), &pricing),
            Err(Error::PricingMismatch)
        ));

        let mut targets = serde_json::to_value(exact_targets()).unwrap();
        targets["provider_configs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "provider_config_version":"flux-edit-bypass",
                "workload_type":"image_edit",
                "provider":"flux",
                "adapter":"flux2_api",
                "model":"flux-2-pro",
                "secret_service":"gongbu.other",
                "secret_account":"other",
                "active":true,
                "execution_enabled":true,
                "settings":{"type":"flux2_api","config":{
                    "endpoint":"https://api.bfl.ai","api_version":"v1",
                    "timeout_ms":270000,"poll_interval_ms":500,"max_retries":0,
                    "idempotency_header":null,"approved_artifact_hosts":[],"headers":{}
                }}
            }));
        let targets: ProviderTargetConfig = serde_json::from_value(targets).unwrap();
        assert!(matches!(
            validate_and_project(&targets, &exact_pricing()),
            Err(Error::TargetMismatch)
        ));

        let mut targets = serde_json::to_value(exact_targets()).unwrap();
        targets["provider_configs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "provider_config_version":"gemini-v1",
                "workload_type":"image_generation",
                "provider":"google",
                "adapter":"gemini_developer_image",
                "model":"gemini-image-v1",
                "secret_service":"gongbu.bfl",
                "secret_account":"operator",
                "active":true,
                "execution_enabled":true,
                "settings":{"type":"gemini_developer_image","config":{
                    "endpoint":"https://generativelanguage.googleapis.com",
                    "api_version":"v1beta","timeout_ms":30000,
                    "max_retries":0,"headers":{}
                }}
            }));
        let targets: ProviderTargetConfig = serde_json::from_value(targets).unwrap();
        assert!(matches!(
            validate_and_project(&targets, &exact_pricing()),
            Err(Error::CredentialIsolation)
        ));
    }
}
