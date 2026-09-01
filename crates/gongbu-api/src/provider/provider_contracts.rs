//! Shipped provider contracts and their sanitized catalog projection.

use super::{
    contract::{PricingCatalog, PricingRule, PricingUnit},
    targets::{AdapterSettings, ProviderContractBinding, ProviderTargetConfig},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;

const CONTRACT_DOCUMENT: &str = include_str!("../../../../contracts/provider-contracts-v1.json");
const CONTRACT_DOCUMENT_SHA256: &str =
    "a8693c06ea5c593ab0fd6044b066aab87224bc4afd98c22e8d48a8ea8bb2999a";

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("provider contract document is invalid")]
    InvalidDocument,
    #[error("provider contract is unknown")]
    UnknownContract,
    #[error("provider contract binding does not match its frozen contract")]
    BindingMismatch,
    #[error("provider contract target does not match its frozen contract")]
    TargetMismatch,
    #[error("provider contract credential is not isolated from another provider")]
    CredentialIsolation,
    #[error("provider contract pricing does not match its frozen contract")]
    PricingMismatch,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    contracts: Vec<ContractDefinition>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractDefinition {
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
pub struct CatalogContract {
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
) -> Result<Vec<CatalogContract>, Error> {
    validate_and_project_with_mode(targets, pricing, true)
}

pub(crate) fn validate_selected_and_project(
    targets: &ProviderTargetConfig,
    pricing: &PricingCatalog,
) -> Result<Vec<CatalogContract>, Error> {
    validate_and_project_with_mode(targets, pricing, false)
}

fn validate_and_project_with_mode(
    targets: &ProviderTargetConfig,
    pricing: &PricingCatalog,
    require_shipped_bindings: bool,
) -> Result<Vec<CatalogContract>, Error> {
    let document = document()?;
    let selected = targets
        .contract_bindings()
        .iter()
        .map(|binding| binding.contract.as_str())
        .collect::<BTreeSet<_>>();

    // Shipped provider identities are contract-only. A raw Gemini or FLUX row
    // must never become a parallel route around the frozen target and price.
    for definition in &document.contracts {
        let known_target_present = targets.revisions().any(|target| {
            target.provider == definition.target.provider
                && target.adapter == definition.target.adapter
        });
        let known_pricing_present = pricing.rules().iter().any(|rule| {
            rule.provider == definition.target.provider && rule.model == definition.target.model
        });
        if require_shipped_bindings
            && (known_target_present || known_pricing_present)
            && !selected.contains(definition.contract.as_str())
        {
            return Err(Error::UnknownContract);
        }
    }
    if selected.len() > 1
        && document.contracts.iter().any(|definition| {
            selected.contains(definition.contract.as_str())
                && pricing.catalog_version() == definition.pricing_version
        })
    {
        return Err(Error::PricingMismatch);
    }
    let mut projected = Vec::new();
    for binding in targets.contract_bindings() {
        let contract_definition = document
            .contracts
            .iter()
            .find(|contract_definition| contract_definition.contract == binding.contract)
            .ok_or(Error::UnknownContract)?;
        validate_binding(binding, contract_definition)?;
        validate_target(targets, contract_definition)?;
        validate_pricing(pricing, contract_definition)?;
        projected.push(project(contract_definition)?);
    }
    Ok(projected)
}

fn document() -> Result<Document, Error> {
    let digest = format!("{:x}", Sha256::digest(CONTRACT_DOCUMENT.as_bytes()));
    if digest != CONTRACT_DOCUMENT_SHA256 {
        return Err(Error::InvalidDocument);
    }
    let document: Document =
        serde_json::from_str(CONTRACT_DOCUMENT).map_err(|_| Error::InvalidDocument)?;
    if document.schema_version != 1 || document.contracts.is_empty() {
        return Err(Error::InvalidDocument);
    }
    let mut contracts = BTreeSet::new();
    if document
        .contracts
        .iter()
        .any(|contract_definition| !contracts.insert(contract_definition.contract.clone()))
    {
        return Err(Error::InvalidDocument);
    }
    Ok(document)
}

fn validate_binding(
    binding: &ProviderContractBinding,
    contract_definition: &ContractDefinition,
) -> Result<(), Error> {
    if binding.contract != contract_definition.contract
        || binding.pricing_version != contract_definition.pricing_version
        || binding.poll_policy != contract_definition.policies.poll
        || binding.artifact_delivery_policy != contract_definition.policies.artifact_delivery
        || binding.recovery_policy != contract_definition.policies.recovery
        || binding.generation_retries != contract_definition.policies.generation_retries
        || binding.fallback != contract_definition.policies.fallback
    {
        return Err(Error::BindingMismatch);
    }
    Ok(())
}

fn validate_target(
    targets: &ProviderTargetConfig,
    contract_definition: &ContractDefinition,
) -> Result<(), Error> {
    let matches = targets
        .revisions()
        .filter(|target| {
            target.provider_config_version == contract_definition.target.provider_config_version
                && target.workload_type == contract_definition.target.workload_type
                && target.provider == contract_definition.target.provider
                && target.adapter == contract_definition.target.adapter
                && target.model == contract_definition.target.model
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(Error::TargetMismatch);
    }
    let target = matches[0];
    if targets
        .revisions()
        .filter(|candidate| {
            candidate.provider == contract_definition.target.provider
                && candidate.adapter == contract_definition.target.adapter
        })
        .count()
        != 1
    {
        return Err(Error::TargetMismatch);
    }
    if target.is_active() != contract_definition.target.active
        || target.is_execution_enabled() != contract_definition.target.execution_enabled
        || target.settings() != &contract_definition.target.settings
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

fn validate_pricing(
    pricing: &PricingCatalog,
    contract_definition: &ContractDefinition,
) -> Result<(), Error> {
    let mut actual = pricing
        .rules()
        .iter()
        .filter(|rule| {
            rule.provider == contract_definition.target.provider
                && rule.model == contract_definition.target.model
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut expected = contract_definition.pricing_rules.clone();
    actual.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    expected.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    if actual != expected
        || (pricing.rules().len() == expected.len()
            && pricing.catalog_version() != contract_definition.pricing_version)
    {
        return Err(Error::PricingMismatch);
    }
    Ok(())
}

fn project(contract_definition: &ContractDefinition) -> Result<CatalogContract, Error> {
    let presets = contract_definition
        .capability
        .presets
        .iter()
        .map(|preset| {
            let rule = contract_definition
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
    Ok(CatalogContract {
        contract: contract_definition.contract.clone(),
        pricing_version: contract_definition.pricing_version.clone(),
        pricing_reviewed_on: contract_definition.pricing_reviewed_on.clone(),
        target: CatalogTarget {
            workload_type: contract_definition.target.workload_type.clone(),
            provider: contract_definition.target.provider.clone(),
            adapter: contract_definition.target.adapter.clone(),
            model: contract_definition.target.model.clone(),
        },
        capability: CatalogCapability {
            image_count: contract_definition.capability.image_count,
            output_formats: contract_definition.capability.output_formats.clone(),
            presets,
        },
        policies: CatalogPolicies {
            generation_retries: contract_definition.policies.generation_retries,
            fallback: contract_definition.policies.fallback,
            poll: contract_definition.policies.poll.clone(),
            artifact_delivery: contract_definition.policies.artifact_delivery.clone(),
            recovery: contract_definition.policies.recovery.clone(),
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
    use serde_json::{json, Value};

    fn exact_targets() -> ProviderTargetConfig {
        serde_json::from_value(json!({
            "schema_version": 3,
            "contract_bindings": [{
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

    fn mixed_contract_inputs(shared_credential: bool) -> (ProviderTargetConfig, PricingCatalog) {
        let document: Value = serde_json::from_str(CONTRACT_DOCUMENT).unwrap();
        let contracts = document["contracts"].as_array().unwrap();
        let bindings = contracts
            .iter()
            .rev()
            .map(|contract| {
                let policies = &contract["policies"];
                json!({
                    "contract":contract["contract"],
                    "pricing_version":contract["pricing_version"],
                    "poll_policy":policies["poll"],
                    "artifact_delivery_policy":policies["artifact_delivery"],
                    "recovery_policy":policies["recovery"],
                    "generation_retries":policies["generation_retries"],
                    "fallback":policies["fallback"]
                })
            })
            .collect::<Vec<_>>();
        let provider_configs = contracts
            .iter()
            .map(|contract| {
                let mut target = contract["target"].clone();
                target["secret_service"] = json!(if shared_credential {
                    "operator.shared"
                } else if target["provider"] == "google" {
                    "operator.google"
                } else {
                    "operator.bfl"
                });
                target["secret_account"] = json!(if shared_credential {
                    "same"
                } else if target["provider"] == "google" {
                    "gemini"
                } else {
                    "flux"
                });
                target
            })
            .collect::<Vec<_>>();
        let rules = contracts
            .iter()
            .flat_map(|contract| contract["pricing_rules"].as_array().unwrap().clone())
            .collect::<Vec<_>>();
        let targets = serde_json::from_value(json!({
            "schema_version":3,
            "contract_bindings":bindings,
            "provider_configs":provider_configs
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version":2,
                "catalog_version":"hubu-gemini-flux-composite-2026-09-01-v1",
                "rules":rules
            }))
            .unwrap(),
        )
        .unwrap();
        (targets, pricing)
    }

    #[test]
    fn exact_contract_projects_sanitized_unqualified_catalog() {
        let contracts = validate_and_project(&exact_targets(), &exact_pricing()).unwrap();
        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].capability.presets[1].width, 1920);
        assert_eq!(contracts[0].capability.presets[1].rate_numerator_minor, 45);
        assert!(!contracts[0].readiness.credential_reference_present);
        assert!(!contracts[0].readiness.live_qualified);
    }

    #[test]
    fn mixed_contracts_are_order_independent_and_credentials_are_isolated() {
        let (targets, pricing) = mixed_contract_inputs(false);
        let projected = validate_and_project(&targets, &pricing).unwrap();
        assert_eq!(projected.len(), 2);
        assert!(projected.iter().any(|contract| {
            contract.contract == "hubu.gemini-3.1-flash-lite-image.text-to-image/v1"
                && contract.capability.presets[0].rate_numerator_minor == 336
        }));
        assert!(projected
            .iter()
            .any(|contract| contract.contract == "hubu.flux-2-pro.text-to-image/v1"));

        let (targets, pricing) = mixed_contract_inputs(true);
        assert!(matches!(
            validate_and_project(&targets, &pricing),
            Err(Error::CredentialIsolation)
        ));
    }

    #[test]
    fn policy_target_and_pricing_mutations_are_rejected() {
        let mut targets = serde_json::to_value(exact_targets()).unwrap();
        targets["contract_bindings"][0]["poll_policy"] = json!("missing");
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
                "provider_config_version":"flux-other-model-bypass",
                "workload_type":"image_generation",
                "provider":"flux",
                "adapter":"flux2_api",
                "model":"flux-other-model",
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
        let document: Value = serde_json::from_str(CONTRACT_DOCUMENT).unwrap();
        let mut rules = document["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|contract| contract["contract"] == "hubu.flux-2-pro.text-to-image/v1")
            .unwrap()["pricing_rules"]
            .as_array()
            .unwrap()
            .clone();
        rules.push(json!({
            "rule_id":"flux-other-model-price",
            "provider":"flux",
            "model":"flux-other-model",
            "currency":"USD",
            "components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]
        }));
        let pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version":2,
                "catalog_version":"contract-plus-bypass-v1",
                "rules":rules
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            validate_and_project(&targets, &pricing),
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
            Err(Error::UnknownContract)
        ));
    }
}
