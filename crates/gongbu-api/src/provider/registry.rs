//! Static adapter factories and the startup-validated provider catalog.

use super::{
    contract::{ContractError, PricingCatalog, ProviderAdapter},
    flux2_api::{Flux2ApiAdapter, ADAPTER_ID as FLUX_ADAPTER_ID, PROVIDER_ID as FLUX_PROVIDER_ID},
    gemini_developer_image::{
        GeminiDeveloperImageAdapter, ADAPTER_ID as GEMINI_DEVELOPER_ADAPTER_ID,
        PROVIDER_ID as GEMINI_DEVELOPER_PROVIDER_ID,
    },
    gemini_image::{
        GeminiImageAdapter, ADAPTER_ID as GEMINI_ADAPTER_ID, PROVIDER_ID as GEMINI_PROVIDER_ID,
    },
    ideogram_image::{
        IdeogramImageAdapter, ADAPTER_ID as IDEOGRAM_ADAPTER_ID,
        PROVIDER_ID as IDEOGRAM_PROVIDER_ID,
    },
    targets::{AdapterSettings, ProviderConfigVersion, ProviderTargetConfig, TargetKey},
};
use std::{collections::BTreeMap, sync::Arc};
use thiserror::Error;

pub type BoundAdapter = Arc<dyn ProviderAdapter + Send + Sync>;
type Factory = dyn Fn(&ProviderConfigVersion) -> Result<BoundAdapter, ContractError> + Send + Sync;

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
}

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    factories: BTreeMap<(String, String), Arc<Factory>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn production() -> Self {
        let mut registry = Self::new();
        registry.register(GEMINI_PROVIDER_ID, GEMINI_ADAPTER_ID, |target| {
            Ok(Arc::new(GeminiImageAdapter::from_target(target)?))
        });
        registry.register(
            GEMINI_DEVELOPER_PROVIDER_ID,
            GEMINI_DEVELOPER_ADAPTER_ID,
            |target| Ok(Arc::new(GeminiDeveloperImageAdapter::from_target(target)?)),
        );
        registry.register(IDEOGRAM_PROVIDER_ID, IDEOGRAM_ADAPTER_ID, |target| {
            Ok(Arc::new(IdeogramImageAdapter::from_target(target)?))
        });
        registry.register(FLUX_PROVIDER_ID, FLUX_ADAPTER_ID, |target| {
            Ok(Arc::new(Flux2ApiAdapter::from_target(target)?))
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

#[derive(Clone)]
pub struct ValidatedProviderCatalog {
    targets: ProviderTargetConfig,
    pricing: PricingCatalog,
    adapters: BTreeMap<(TargetKey, String), BoundAdapter>,
}

impl ValidatedProviderCatalog {
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
        Ok(Self {
            targets,
            pricing,
            adapters,
        })
    }

    pub fn targets(&self) -> &ProviderTargetConfig {
        &self.targets
    }

    pub fn pricing(&self) -> &PricingCatalog {
        &self.pricing
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
        matches!(
            target.settings(),
            AdapterSettings::Flux2Api(config) if config.idempotency_header.is_some()
        )
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
                "schema_version":1,"catalog_version":"v1","rules":[{
                    "rule_id":"image","provider":provider,"model":"image-v1",
                    "currency":"USD","unit":"image","unit_amount_minor":1
                }]
            }))
            .unwrap()
            .as_bytes(),
        )
        .unwrap()
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
    fn production_registry_binds_gemini_ideogram_and_flux() {
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version":2,"provider_configs":[
                {"provider_config_version":"g-v1","workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-image-v1","secret_service":"gongbu.google","secret_account":"one","active":true,"execution_enabled":true,"settings":{"type":"gemini_image","config":{"endpoint":"https://google.example","api_version":"v1","project":"project","location":"us","timeout_ms":1000}}},
                {"provider_config_version":"i-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"one","active":true,"execution_enabled":true,"settings":{"type":"ideogram_image","config":{"endpoint":"https://ideogram.example","api_version":"v1","timeout_ms":1000,"approved_artifact_hosts":["ideogram.example"]}}},
                {"provider_config_version":"f-v1","workload_type":"image_generation","provider":"flux","adapter":"flux2_api","model":"flux-2-pro","secret_service":"gongbu.flux","secret_account":"one","active":true,"execution_enabled":true,"settings":{"type":"flux2_api","config":{"endpoint":"https://flux.example","api_version":"v1","timeout_ms":1000,"poll_interval_ms":10,"idempotency_header":"x-idempotency-key","approved_artifact_hosts":["flux.example"]}}}
            ]
        })).unwrap();
        let pricing = PricingCatalog::from_json(br#"{"schema_version":1,"catalog_version":"v1","rules":[{"rule_id":"g","provider":"google","model":"gemini-image-v1","currency":"USD","unit":"image","unit_amount_minor":1},{"rule_id":"i","provider":"ideogram","model":"ideogram-v3","currency":"USD","unit":"image","unit_amount_minor":1},{"rule_id":"f","provider":"flux","model":"flux-2-pro","currency":"USD","unit":"image","unit_amount_minor":1}]}"#).unwrap();
        let catalog =
            ValidatedProviderCatalog::bind(targets, pricing, &ProviderRegistry::production())
                .unwrap();
        for (provider, adapter, model) in [
            ("google", "gemini_image", "gemini-image-v1"),
            ("ideogram", "ideogram_image", "ideogram-v3"),
            ("flux", "flux2_api", "flux-2-pro"),
        ] {
            let key = TargetKey::new("image_generation", provider, adapter, model).unwrap();
            assert_eq!(catalog.resolve_active(&key).unwrap().adapter, adapter);
        }
    }
}
