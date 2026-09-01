//! Static adapter factories and the startup-validated provider catalog.

use super::{
    contract::{ContractError, PriceComponent, PricingCatalog, PricingSelector, ProviderAdapter},
    flux2_api::{
        Flux2ApiAdapter, ADAPTER_ID as FLUX_ADAPTER_ID, MODEL_ID as FLUX_MODEL_ID,
        PROVIDER_ID as FLUX_PROVIDER_ID, SUPPORTED_PRESETS as FLUX_SUPPORTED_PRESETS,
    },
    gemini_developer_image::{
        GeminiDeveloperImageAdapter, ADAPTER_ID as GEMINI_DEVELOPER_ADAPTER_ID,
        PROVIDER_ID as GEMINI_DEVELOPER_PROVIDER_ID,
    },
    ideogram_image::{
        IdeogramImageAdapter, ADAPTER_ID as IDEOGRAM_ADAPTER_ID,
        PROVIDER_ID as IDEOGRAM_PROVIDER_ID,
    },
    provider_contracts::{self, CatalogContract},
    targets::{AdapterSettings, ProviderConfigVersion, ProviderTargetConfig, TargetKey},
};
use crate::{artifact::ArtifactLimits, execution_scope::for_target, secrets::ProviderSecret};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use thiserror::Error;

use super::contract::{
    AdapterCapabilities, AdapterOutcome, NormalizedArtifact, NormalizedRequest, ProviderFailure,
    ProviderPhase,
};

pub type BoundAdapter = Arc<dyn ProviderAdapter + Send + Sync>;
type Factory = dyn Fn(&ProviderConfigVersion) -> Result<BoundAdapter, ContractError> + Send + Sync;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExecutionTarget {
    pub target_id: String,
    pub workload_type: String,
    pub provider: String,
    pub model: String,
    pub execution_scope: ExecutionTargetScope,
    pub image_sizes: Vec<String>,
    pub pricing: Vec<ExecutionTargetPricing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExecutionTargetScope {
    pub schema_version: u32,
    pub provider: String,
    pub executor: String,
    pub capability: String,
    pub billing_merchant: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExecutionTargetPricing {
    pub rule_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<PricingSelector>,
    pub currency: String,
    pub components: Vec<PriceComponent>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("enabled provider target has no registered factory: {0}")]
    MissingFactory(String),
    #[error("enabled provider target has no pricing rule: {0}")]
    MissingPricing(String),
    #[error("provider adapter could not be bound: {0}")]
    BindingFailed(String),
    #[error("provider target is not bound: {0}")]
    NotBound(String),
    #[error("provider target is unavailable")]
    TargetUnavailable,
    #[error("provider contract is invalid: {0}")]
    InvalidProviderContract(String),
}

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    factories: BTreeMap<(String, String), Arc<Factory>>,
    require_shipped_contracts: bool,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn production(artifact_limits: &ArtifactLimits) -> Self {
        let max_artifact_bytes = artifact_limits.max_encoded_bytes;
        let mut registry = Self::new();
        registry.require_shipped_contracts = true;
        registry.register(
            GEMINI_DEVELOPER_PROVIDER_ID,
            GEMINI_DEVELOPER_ADAPTER_ID,
            move |target| {
                Ok(Arc::new(
                    GeminiDeveloperImageAdapter::from_target_with_artifact_limit(
                        target,
                        max_artifact_bytes,
                    )?,
                ))
            },
        );
        registry.register(IDEOGRAM_PROVIDER_ID, IDEOGRAM_ADAPTER_ID, move |target| {
            Ok(Arc::new(
                IdeogramImageAdapter::from_target_with_artifact_limit(target, max_artifact_bytes)?,
            ))
        });
        registry.register(FLUX_PROVIDER_ID, FLUX_ADAPTER_ID, move |target| {
            Ok(Arc::new(Flux2ApiAdapter::from_target_with_artifact_limit(
                target,
                max_artifact_bytes,
            )?))
        });
        #[cfg(feature = "local-fixture-canary")]
        if local_fixture_canary_enabled() {
            registry.register("example", "fixture", |_| {
                Ok(Arc::new(DeterministicFixtureAdapter))
            });
        }
        registry
    }

    pub fn sandbox() -> Self {
        let mut registry = Self::new();
        registry.require_shipped_contracts = true;
        registry.register("sandbox", "fixture", |_| {
            Ok(Arc::new(DeterministicFixtureAdapter))
        });
        registry
    }

    pub fn register<F>(&mut self, provider: &str, adapter: &str, factory: F)
    where
        F: Fn(&ProviderConfigVersion) -> Result<BoundAdapter, ContractError>
            + Send
            + Sync
            + 'static,
    {
        self.factories
            .insert((provider.to_owned(), adapter.to_owned()), Arc::new(factory));
    }

    fn factory(&self, target: &ProviderConfigVersion) -> Option<&Arc<Factory>> {
        self.factories
            .get(&(target.provider.clone(), target.adapter.clone()))
    }
}

#[cfg(feature = "local-fixture-canary")]
fn local_fixture_canary_enabled() -> bool {
    std::env::var("GONGBU_LOCAL_FIXTURE_CANARY").as_deref() == Ok("1")
}

struct DeterministicFixtureAdapter;

impl ProviderAdapter for DeterministicFixtureAdapter {
    fn adapter_id(&self) -> &str {
        "fixture"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: true,
        }
    }

    fn invoke(
        &self,
        request: &NormalizedRequest,
        _: &serde_json::Value,
        _: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let provider_request_id = vendor_idempotency_key
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderFailure::release(
                    "fixture_idempotency_key_missing",
                    ProviderPhase::Submission,
                )
            })?;
        let actual_cost_minor = local_fixture_cost_minor(request.image_size.as_deref())?;
        Ok(AdapterOutcome {
            usage: Some(crate::provider_contract::NormalizedUsage {
                images: Some(1),
                ..Default::default()
            }),
            actual_vendor_cost: Some(
                crate::provider_contract::ActualVendorCost::new(actual_cost_minor, 2, "USD")
                    .unwrap(),
            ),
            provider_request_id: Some(format!("local-fixture-request-{provider_request_id}")),
            provider_operation_id: None,
            artifacts: vec![NormalizedArtifact {
                media_type: "image/png".into(),
                bytes: vec![
                    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0,
                    0, 1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99,
                    100, 248, 15, 0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174,
                    66, 96, 130,
                ],
            }],
        })
    }
}

fn local_fixture_cost_minor(image_size: Option<&str>) -> Result<i64, ProviderFailure> {
    match image_size {
        Some("1k") => Ok(1),
        Some("2k") => Ok(2),
        Some("4k") => Ok(3),
        _ => Err(ProviderFailure::release(
            "fixture_image_size_invalid",
            ProviderPhase::Submission,
        )),
    }
}

#[derive(Clone)]
pub struct ValidatedProviderCatalog {
    targets: ProviderTargetConfig,
    pricing: PricingCatalog,
    adapters: BTreeMap<(TargetKey, String), BoundAdapter>,
    provider_contracts: Vec<CatalogContract>,
}

impl ValidatedProviderCatalog {
    pub(crate) fn disabled() -> Self {
        Self {
            targets: ProviderTargetConfig::disabled(),
            pricing: PricingCatalog::disabled(),
            adapters: BTreeMap::new(),
            provider_contracts: Vec::new(),
        }
    }

    pub fn bind(
        targets: ProviderTargetConfig,
        pricing: PricingCatalog,
        registry: &ProviderRegistry,
    ) -> Result<Self, RegistryError> {
        let mut adapters = BTreeMap::new();
        for target in targets
            .revisions()
            .filter(|target| target.is_execution_enabled())
        {
            let canonical = target.target_key().canonical_name();
            let factory = registry
                .factory(target)
                .ok_or_else(|| RegistryError::MissingFactory(canonical.clone()))?;
            if !pricing.supports_target(&target.target_key()) {
                return Err(RegistryError::MissingPricing(canonical));
            }
            if target.provider == FLUX_PROVIDER_ID
                && target.adapter == FLUX_ADAPTER_ID
                && target.model == FLUX_MODEL_ID
                && !FLUX_SUPPORTED_PRESETS
                    .iter()
                    .all(|preset| pricing.supports_image_size(&target.target_key(), preset))
            {
                return Err(RegistryError::MissingPricing(canonical));
            }
            let adapter = factory(target)
                .map_err(|_| RegistryError::BindingFailed(target.target_key().canonical_name()))?;
            if adapter.adapter_id() != target.adapter {
                return Err(RegistryError::BindingFailed(
                    target.target_key().canonical_name(),
                ));
            }
            adapters.insert(
                (target.target_key(), target.provider_config_version.clone()),
                adapter,
            );
        }
        let provider_contracts = if registry.require_shipped_contracts {
            provider_contracts::validate_and_project(&targets, &pricing)
        } else {
            provider_contracts::validate_selected_and_project(&targets, &pricing)
        }
        .map_err(|error| RegistryError::InvalidProviderContract(error.to_string()))?;
        Ok(Self {
            targets,
            pricing,
            adapters,
            provider_contracts,
        })
    }

    pub fn targets(&self) -> &ProviderTargetConfig {
        &self.targets
    }

    pub fn pricing(&self) -> &PricingCatalog {
        &self.pricing
    }

    pub fn provider_contracts(&self) -> &[CatalogContract] {
        &self.provider_contracts
    }

    pub(crate) fn mark_credential_references_present(&mut self) {
        for contract in &mut self.provider_contracts {
            contract.readiness.credential_reference_present = true;
        }
    }

    pub fn execution_targets(&self) -> Vec<ExecutionTarget> {
        self.targets
            .revisions()
            .filter(|target| target.is_active() && target.is_execution_enabled())
            .filter_map(|target| {
                let key = target.target_key();
                let scope = for_target(&key.provider, &key.adapter)?;
                let execution_scope = ExecutionTargetScope {
                    schema_version: scope.schema_version,
                    provider: scope.provider.id,
                    executor: scope.executor.id,
                    capability: scope.capability.id,
                    billing_merchant: scope.billing_merchant.id,
                };
                let pricing = self
                    .pricing
                    .rules_for_target(&key)
                    .into_iter()
                    .map(|rule| ExecutionTargetPricing {
                        rule_id: rule.rule_id,
                        selector: rule.selector,
                        currency: rule.currency,
                        components: rule.components,
                    })
                    .collect::<Vec<_>>();
                let mut image_sizes = pricing
                    .iter()
                    .filter_map(|rule| {
                        rule.selector
                            .as_ref()
                            .map(|selector| selector.image_size.clone())
                    })
                    .collect::<Vec<_>>();
                image_sizes.sort();
                image_sizes.dedup();
                Some(ExecutionTarget {
                    target_id: key.public_id(),
                    workload_type: key.workload_type,
                    provider: key.provider,
                    model: key.model,
                    execution_scope,
                    image_sizes,
                    pricing,
                })
            })
            .collect()
    }

    pub fn resolve_target_id(
        &self,
        target_id: &str,
    ) -> Result<&ProviderConfigVersion, RegistryError> {
        let target = self
            .targets
            .resolve_target_id(target_id)
            .map_err(|_| RegistryError::TargetUnavailable)?;
        self.bound(target)?;
        Ok(target)
    }

    pub fn resolve_active(&self, key: &TargetKey) -> Result<&ProviderConfigVersion, RegistryError> {
        let target = self
            .targets
            .resolve_active(key)
            .map_err(|_| RegistryError::TargetUnavailable)?;
        self.bound(target)?;
        Ok(target)
    }

    pub fn resolve_persisted<'a>(
        &'a self,
        key: &TargetKey,
        version: &str,
        digest: &str,
    ) -> Result<(&'a ProviderConfigVersion, &'a BoundAdapter), RegistryError> {
        let target = self
            .targets
            .resolve_persisted_revision(key, version, digest)
            .map_err(|_| RegistryError::TargetUnavailable)?;
        Ok((target, self.bound(target)?))
    }

    fn bound(&self, target: &ProviderConfigVersion) -> Result<&BoundAdapter, RegistryError> {
        self.adapters
            .get(&(target.target_key(), target.provider_config_version.clone()))
            .ok_or_else(|| RegistryError::NotBound(target.target_key().canonical_name()))
    }

    pub fn needs_stable_idempotency_key(target: &ProviderConfigVersion) -> bool {
        match target.settings() {
            AdapterSettings::Fixture => true,
            AdapterSettings::Flux2Api(config) => config.idempotency_header.is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider_contract::{
            AdapterCapabilities, AdapterOutcome, NormalizedRequest, ProviderFailure,
        },
        secrets::ProviderSecret,
    };
    use serde_json::json;

    struct FixtureAdapter;
    impl ProviderAdapter for FixtureAdapter {
        fn adapter_id(&self) -> &str {
            "fixture"
        }
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities {
                vendor_enforced_idempotency: false,
            }
        }
        fn invoke(
            &self,
            _: &NormalizedRequest,
            _: &serde_json::Value,
            _: &ProviderSecret,
            _: Option<&str>,
        ) -> Result<AdapterOutcome, ProviderFailure> {
            unreachable!()
        }
    }

    fn fixture_targets() -> ProviderTargetConfig {
        serde_json::from_value(json!({"provider_configs":[{
            "provider_config_version":"fixture-v1","workload_type":"image_generation",
            "provider":"example","adapter":"fixture","model":"image-v1",
            "secret_service":"gongbu.example","secret_account":"local"
        }]}))
        .unwrap()
    }

    fn fixture_pricing(provider: &str) -> PricingCatalog {
        PricingCatalog::from_json(
            serde_json::to_string(&json!({
                "schema_version":2,"catalog_version":"v2","rules":[{
                    "rule_id":"image","provider":provider,"model":"image-v1",
                    "currency":"USD","components":[{
                        "unit":"image","rate_numerator_minor":1,"rate_denominator":1
                    }]
                }]
            }))
            .unwrap()
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn disabled_catalog_fails_closed_for_every_target() {
        let catalog = ValidatedProviderCatalog::disabled();
        let key = TargetKey::new("image_generation", "example", "fixture", "image-v1").unwrap();
        assert_eq!(
            catalog.resolve_active(&key).unwrap_err(),
            RegistryError::TargetUnavailable
        );
    }

    #[test]
    fn local_fixture_uses_operation_scoped_provider_request_identity() {
        let targets = fixture_targets();
        let target = targets
            .resolve("image_generation", "example", "fixture", "image-v1")
            .unwrap();
        assert!(ValidatedProviderCatalog::needs_stable_idempotency_key(
            target
        ));
    }

    #[test]
    fn local_fixture_actual_cost_matches_each_selected_size_rule() {
        assert_eq!(local_fixture_cost_minor(Some("1k")).unwrap(), 1);
        assert_eq!(local_fixture_cost_minor(Some("2k")).unwrap(), 2);
        assert_eq!(local_fixture_cost_minor(Some("4k")).unwrap(), 3);
        assert!(local_fixture_cost_minor(None).is_err());
        assert!(local_fixture_cost_minor(Some("unexpected")).is_err());
    }

    #[test]
    fn startup_rejects_missing_factory_and_missing_pricing() {
        assert_eq!(
            ValidatedProviderCatalog::bind(
                fixture_targets(),
                fixture_pricing("example"),
                &ProviderRegistry::new(),
            )
            .err(),
            Some(RegistryError::MissingFactory(
                "image_generation/example/fixture/image-v1".into()
            ))
        );
        let mut registry = ProviderRegistry::new();
        registry.register("example", "fixture", |_| Ok(Arc::new(FixtureAdapter)));
        assert_eq!(
            ValidatedProviderCatalog::bind(
                fixture_targets(),
                fixture_pricing("different"),
                &registry,
            )
            .err(),
            Some(RegistryError::MissingPricing(
                "image_generation/example/fixture/image-v1".into()
            ))
        );
    }

    #[test]
    fn production_registry_binds_gemini_developer_ideogram_and_flux() {
        let document: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        let contracts = document["contracts"].as_array().unwrap();
        let bindings = contracts.iter().map(|contract| {
            let policies = &contract["policies"];
            json!({"contract":contract["contract"],"pricing_version":contract["pricing_version"],"poll_policy":policies["poll"],"artifact_delivery_policy":policies["artifact_delivery"],"recovery_policy":policies["recovery"],"generation_retries":0,"fallback":false})
        }).collect::<Vec<_>>();
        let mut provider_configs = contracts
            .iter()
            .map(|contract| {
                let mut target = contract["target"].clone();
                target["secret_service"] =
                    json!(format!("gongbu.{}", target["provider"].as_str().unwrap()));
                target["secret_account"] = json!("one");
                target
            })
            .collect::<Vec<_>>();
        provider_configs.push(json!({"provider_config_version":"i-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"one","active":true,"execution_enabled":true,"settings":{"type":"ideogram_image","config":{"endpoint":"https://ideogram.example","api_version":"v1","timeout_ms":1000,"approved_artifact_hosts":["ideogram.example"]}}}));
        let targets: ProviderTargetConfig = serde_json::from_value(json!({"schema_version":3,"contract_bindings":bindings,"provider_configs":provider_configs})).unwrap();
        let mut rules = contracts
            .iter()
            .flat_map(|contract| contract["pricing_rules"].as_array().unwrap().clone())
            .collect::<Vec<_>>();
        rules.push(json!({"rule_id":"i","provider":"ideogram","model":"ideogram-v3","currency":"USD","components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]}));
        let pricing = PricingCatalog::from_json(&serde_json::to_vec(&json!({"schema_version":2,"catalog_version":"provider-composite-v1","rules":rules})).unwrap()).unwrap();
        let catalog = ValidatedProviderCatalog::bind(
            targets,
            pricing,
            &ProviderRegistry::production(&ArtifactLimits::default()),
        )
        .unwrap();
        for (provider, adapter, model) in [
            (
                "google",
                "gemini_developer_image",
                "gemini-3.1-flash-lite-image",
            ),
            ("ideogram", "ideogram_image", "ideogram-v3"),
            ("flux", "flux2_api", "flux-2-pro"),
        ] {
            let key = TargetKey::new("image_generation", provider, adapter, model).unwrap();
            assert_eq!(catalog.resolve_active(&key).unwrap().adapter, adapter);
        }
        let execution_targets = catalog.execution_targets();
        assert_eq!(execution_targets.len(), 3);
        let flux = execution_targets
            .iter()
            .find(|target| target.provider == "flux")
            .unwrap();
        assert_eq!(flux.image_sizes, ["1k", "2k", "4k"]);
        assert_eq!(
            flux.execution_scope.billing_merchant,
            "merchant:black-forest-labs"
        );
        assert_eq!(
            catalog.resolve_target_id(&flux.target_id).unwrap().model,
            "flux-2-pro"
        );
    }
}
