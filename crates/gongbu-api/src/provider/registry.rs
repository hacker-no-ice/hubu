//! Static adapter factories and the startup-validated provider catalog.

use super::{
    contract::{ContractError, PriceComponent, PricingCatalog, PricingSelector, ProviderAdapter},
    flux_contract::{
        ADAPTER_ID as FLUX_ADAPTER_ID, MODEL_ID as FLUX_MODEL_ID, PROVIDER_ID as FLUX_PROVIDER_ID,
        SUPPORTED_PRESETS as FLUX_SUPPORTED_PRESETS,
    },
    pack::{Archetype, MegapixelRule, OutputSize, Pack},
    pack_async::AsyncPollEngine,
    pack_sync::SyncFixedEngine,
    provider_contracts::{self, CatalogContract},
    targets::{
        AdapterSettings, PackTargetConfig, ProviderConfigVersion, ProviderTargetConfig, TargetKey,
    },
    verified_packs::verified_packs,
};
use crate::{
    artifact::ArtifactLimits, execution_scope::for_workload_target, secrets::ProviderSecret,
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use thiserror::Error;

use super::contract::{
    AdapterCapabilities, AdapterOutcome, NormalizedArtifact, NormalizedRequest, ProviderFailure,
    ProviderPhase,
};

pub type BoundAdapter = Arc<dyn ProviderAdapter + Send + Sync>;
type Factory = dyn Fn(&ProviderConfigVersion) -> Result<BoundAdapter, ContractError> + Send + Sync;
type PackEngineFactory =
    dyn Fn(&ProviderConfigVersion, &Arc<Pack>) -> Result<BoundAdapter, ContractError> + Send + Sync;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExecutionTarget {
    pub target_id: String,
    pub workload_type: String,
    pub provider: String,
    pub model: String,
    pub execution_scope: ExecutionTargetScope,
    pub image_sizes: Vec<String>,
    pub pricing: Vec<ExecutionTargetPricing>,
    /// Certified output sizes by parameter values, for pack targets that
    /// certify sizes beyond `image_sizes` (aspect ratios, for example).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_sizes: Vec<OutputSize>,
    /// How billed megapixels are counted, when the target prices them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub megapixels: Option<MegapixelRule>,
    /// The input parameter that takes artifact IDs to edit, for image-edit
    /// targets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_images: Option<InputImagesParameter>,
}

/// Where an edit target takes its input images: artifact IDs of images the
/// same account's earlier governed executions delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputImagesParameter {
    pub parameter: String,
    pub min: i64,
    pub max: i64,
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
    /// Archetype engines keyed by engine ID; they run pack targets.
    pack_engines: BTreeMap<String, Arc<PackEngineFactory>>,
    /// Validated packs keyed by pack ID and version, so successive
    /// versions of one pack can back different contract revisions.
    packs: BTreeMap<(String, String), Arc<Pack>>,
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
        for pack in verified_packs() {
            registry.register_pack(pack);
        }
        registry.register_pack_engine(Archetype::AsyncPoll, move |target, pack| {
            Ok(Arc::new(AsyncPollEngine::from_target(
                target,
                pack,
                max_artifact_bytes,
            )?))
        });
        registry.register_pack_engine(Archetype::SyncFixed, move |target, pack| {
            Ok(Arc::new(SyncFixedEngine::from_target(
                target,
                pack,
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

    /// Make a validated pack available to targets that name it.
    pub fn register_pack(&mut self, pack: Pack) {
        self.packs.insert(
            (pack.pack_id().to_owned(), pack.pack_version().to_owned()),
            Arc::new(pack),
        );
    }

    fn pack_for(&self, config: &PackTargetConfig) -> Option<&Arc<Pack>> {
        self.packs
            .get(&(config.pack_id.clone(), config.pack_version.clone()))
    }

    /// Bind an archetype engine. Pack targets are bound by engine, never by a
    /// per-provider factory.
    pub fn register_pack_engine<F>(&mut self, archetype: Archetype, factory: F)
    where
        F: Fn(&ProviderConfigVersion, &Arc<Pack>) -> Result<BoundAdapter, ContractError>
            + Send
            + Sync
            + 'static,
    {
        self.pack_engines
            .insert(archetype.engine_id().to_owned(), Arc::new(factory));
    }

    fn bind_target(
        &self,
        target: &ProviderConfigVersion,
        pricing: &PricingCatalog,
    ) -> Result<BoundAdapter, RegistryError> {
        let canonical = || target.target_key().canonical_name();
        let Some(config) = target.pack() else {
            let factory = self
                .factories
                .get(&(target.provider.clone(), target.adapter.clone()))
                .ok_or_else(|| RegistryError::MissingFactory(canonical()))?;
            return factory(target).map_err(|_| RegistryError::BindingFailed(canonical()));
        };
        let pack = self
            .pack_for(config)
            .ok_or_else(|| RegistryError::MissingFactory(canonical()))?;
        // The revision pins one exact pack; any drift is a binding failure.
        let model = pack.model(&target.model);
        if pack.digest() != config.pack_digest
            || pack.pack_version() != config.pack_version
            || pack.archetype().engine_id() != config.archetype
            || model.is_none_or(|model| {
                pack.target_key(model) != target.target_key()
                    || model.provider_config_version != target.provider_config_version
            })
        {
            return Err(RegistryError::BindingFailed(canonical()));
        }
        // The pack digest pins its per-model pricing version and rules. A
        // separately supplied catalog must use those exact normalized rules;
        // its aggregate version may also describe unrelated models/providers.
        let pack_pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "catalog_version": "pinned-pack-validation",
                "rules": pack.pricing_rules(),
            }))
            .expect("validated pack pricing serializes"),
        )
        .map_err(|_| RegistryError::BindingFailed(canonical()))?;
        if pricing.rules_for_target(&target.target_key())
            != pack_pricing.rules_for_target(&target.target_key())
        {
            return Err(RegistryError::BindingFailed(canonical()));
        }
        let engine = self
            .pack_engines
            .get(&config.archetype)
            .ok_or_else(|| RegistryError::MissingFactory(canonical()))?;
        engine(target, pack).map_err(|_| RegistryError::BindingFailed(canonical()))
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
        let actual_cost_minor = local_fixture_cost_minor(request.image_size())?;
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
    /// The pinned pack of each bound pack target revision.
    packs: BTreeMap<(TargetKey, String), Arc<Pack>>,
    provider_contracts: Vec<CatalogContract>,
}

impl ValidatedProviderCatalog {
    pub(crate) fn disabled() -> Self {
        Self {
            targets: ProviderTargetConfig::disabled(),
            pricing: PricingCatalog::disabled(),
            adapters: BTreeMap::new(),
            packs: BTreeMap::new(),
            provider_contracts: Vec::new(),
        }
    }

    pub fn bind(
        targets: ProviderTargetConfig,
        pricing: PricingCatalog,
        registry: &ProviderRegistry,
    ) -> Result<Self, RegistryError> {
        let mut adapters = BTreeMap::new();
        let mut packs = BTreeMap::new();
        for target in targets
            .revisions()
            .filter(|target| target.is_execution_enabled())
        {
            let canonical = target.target_key().canonical_name();
            if !pricing.supports_target(&target.target_key()) {
                return Err(RegistryError::MissingPricing(canonical));
            }
            // The v1 FLUX contract prices each certified preset by size; a
            // pack that certifies its own sizes prices them itself.
            let certifies_sizes = target
                .pack()
                .and_then(|config| registry.pack_for(config))
                .is_some_and(|pack| pack.has_size_presets());
            if target.provider == FLUX_PROVIDER_ID
                && target.adapter == FLUX_ADAPTER_ID
                && target.model == FLUX_MODEL_ID
                && !certifies_sizes
                && !FLUX_SUPPORTED_PRESETS
                    .iter()
                    .all(|preset| pricing.supports_image_size(&target.target_key(), preset))
            {
                return Err(RegistryError::MissingPricing(canonical));
            }
            let adapter = registry.bind_target(target, &pricing)?;
            if adapter.adapter_id() != target.adapter {
                return Err(RegistryError::BindingFailed(
                    target.target_key().canonical_name(),
                ));
            }
            let revision = (target.target_key(), target.provider_config_version.clone());
            if let Some(pack) = target.pack().and_then(|config| registry.pack_for(config)) {
                packs.insert(revision.clone(), Arc::clone(pack));
            }
            adapters.insert(revision, adapter);
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
            packs,
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
                let scope = for_workload_target(&key.workload_type, &key.provider, &key.adapter)?;
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
                            .and_then(|selector| selector.image_size().map(str::to_owned))
                    })
                    .collect::<Vec<_>>();
                image_sizes.sort();
                image_sizes.dedup();
                let pack = self.pack(target);
                let output_sizes = pack
                    .filter(|pack| pack.megapixel_rule().is_some() || pack.has_size_presets())
                    .map(|pack| pack.output_sizes(&key.model))
                    .unwrap_or_default();
                Some(ExecutionTarget {
                    target_id: key.public_id(),
                    workload_type: key.workload_type,
                    provider: key.provider,
                    model: key.model,
                    execution_scope,
                    image_sizes,
                    pricing,
                    output_sizes,
                    megapixels: pack.and_then(|pack| pack.megapixel_rule().cloned()),
                    input_images: pack.and_then(|pack| pack.input_images_parameter()),
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

    /// The pinned pack a bound pack target revision runs.
    pub fn pack(&self, target: &ProviderConfigVersion) -> Option<&Arc<Pack>> {
        self.packs
            .get(&(target.target_key(), target.provider_config_version.clone()))
    }

    fn bound(&self, target: &ProviderConfigVersion) -> Result<&BoundAdapter, RegistryError> {
        self.adapters
            .get(&(target.target_key(), target.provider_config_version.clone()))
            .ok_or_else(|| RegistryError::NotBound(target.target_key().canonical_name()))
    }

    pub fn needs_stable_idempotency_key(target: &ProviderConfigVersion) -> bool {
        matches!(target.settings(), AdapterSettings::Fixture)
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

    fn bind_pack_pricing(
        pack: &Pack,
        rules: serde_json::Value,
    ) -> Result<ValidatedProviderCatalog, RegistryError> {
        let secret = crate::secrets::SecretReference::new("gongbu.pack", "fixture").unwrap();
        let targets = serde_json::from_value(json!({
            "schema_version": 2,
            "provider_configs": pack.provider_configs(&secret),
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": "operator-composite-v1",
                "rules": rules,
            }))
            .unwrap(),
        )
        .unwrap();
        let mut registry = ProviderRegistry::new();
        registry.register_pack(pack.clone());
        registry.register_pack_engine(Archetype::SyncFixed, |target, pack| {
            Ok(Arc::new(super::super::pack::SandboxPackAdapter::new(
                pack.clone(),
                target.model.clone(),
                json!({}),
                BTreeMap::new(),
            )))
        });
        ValidatedProviderCatalog::bind(targets, pricing, &registry)
    }

    #[test]
    fn pack_binding_rejects_catalog_rules_outside_the_pinned_pack() {
        let pack = Pack::from_toml_str(include_str!(
            "../../../../fixtures/packs/sandbox-image.toml"
        ))
        .unwrap();
        let rules = serde_json::to_value(pack.pricing_rules()).unwrap();
        assert!(bind_pack_pricing(&pack, rules.clone()).is_ok());
        for change in [
            "rate",
            "denominator",
            "rule_id",
            "unit",
            "selector",
            "missing",
            "extra",
        ] {
            let mut changed = rules.clone();
            match change {
                "rate" => changed[0]["components"][0]["rate_numerator_minor"] = json!(99),
                "denominator" => changed[0]["components"][0]["rate_denominator"] = json!(99),
                "rule_id" => changed[0]["rule_id"] = json!("unreviewed-rule"),
                "unit" => changed[0]["components"][1]["unit"] = json!("page"),
                "selector" => changed[0]["selector"]["quality"] = json!("unreviewed"),
                "missing" => {
                    changed.as_array_mut().unwrap().remove(0);
                }
                "extra" => {
                    let mut extra = changed[0].clone();
                    extra["rule_id"] = json!("extra-rule");
                    extra["selector"]["quality"] = json!("unreviewed");
                    changed.as_array_mut().unwrap().push(extra);
                }
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    bind_pack_pricing(&pack, changed),
                    Err(RegistryError::BindingFailed(_))
                ),
                "catalog mutation {change} must fail before binding the engine"
            );
        }
    }

    #[test]
    fn pack_binding_accepts_canonical_rules_in_a_multi_model_composite_catalog() {
        let source = include_str!("../../../../fixtures/packs/sandbox-image.toml");
        let second_model =
            source[source.find("[[models]]").unwrap()..].replace("image-v1", "image-v2");
        let pack = Pack::from_toml_str(&format!("{source}\n{second_model}")).unwrap();
        assert_ne!(
            pack.models()[0].pricing_version,
            pack.models()[1].pricing_version
        );
        let mut rules = serde_json::to_value(pack.pricing_rules()).unwrap();
        rules.as_array_mut().unwrap().reverse();
        for rule in rules.as_array_mut().unwrap() {
            rule["currency"] = json!("usd");
            rule["components"].as_array_mut().unwrap().reverse();
            let quality = rule["selector"]["quality"].as_str().unwrap().to_uppercase();
            rule["selector"]["quality"] = json!(quality);
        }
        let mut unrelated = rules[0].clone();
        unrelated["provider"] = json!("unrelated-provider");
        unrelated["rule_id"] = json!("unrelated-rule");
        rules.as_array_mut().unwrap().push(unrelated);
        let catalog = bind_pack_pricing(&pack, rules.clone()).unwrap();
        for model in pack.models() {
            assert!(catalog.resolve_active(&pack.target_key(model)).is_ok());
            let mut changed = rules.clone();
            let rule = changed
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|rule| rule["provider"] == pack.provider() && rule["model"] == model.model)
                .unwrap();
            rule["components"][0]["rate_numerator_minor"] = json!(99);
            assert!(matches!(
                bind_pack_pricing(&pack, changed),
                Err(RegistryError::BindingFailed(_))
            ));
        }
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
    fn production_registry_binds_gemini_developer_and_flux() {
        let document: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        // One revision per target: the v1 contracts, which share no target.
        let contracts: Vec<&serde_json::Value> = document["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|contract| contract["contract"].as_str().unwrap().ends_with("/v1"))
            .collect();
        let bindings = contracts.iter().map(|contract| {
            let policies = &contract["policies"];
            json!({"contract":contract["contract"],"pricing_version":contract["pricing_version"],"poll_policy":policies["poll"],"artifact_delivery_policy":policies["artifact_delivery"],"recovery_policy":policies["recovery"],"generation_retries":0,"fallback":false})
        }).collect::<Vec<_>>();
        let provider_configs = contracts
            .iter()
            .map(|contract| {
                let mut target = contract["target"].clone();
                target["secret_service"] =
                    json!(format!("gongbu.{}", target["provider"].as_str().unwrap()));
                target["secret_account"] = json!("one");
                target
            })
            .collect::<Vec<_>>();
        let targets: ProviderTargetConfig = serde_json::from_value(json!({"schema_version":3,"contract_bindings":bindings,"provider_configs":provider_configs})).unwrap();
        let rules = contracts
            .iter()
            .flat_map(|contract| contract["pricing_rules"].as_array().unwrap().clone())
            .collect::<Vec<_>>();
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
            ("google", "gemini_developer_image", "gemini-3.1-flash-image"),
            ("google", "gemini_developer_image", "gemini-3-pro-image"),
            ("flux", "flux2_api", "flux-2-pro"),
            ("flux", "flux2_api", "flux-3-image"),
        ] {
            let key = TargetKey::new("image_generation", provider, adapter, model).unwrap();
            assert_eq!(catalog.resolve_active(&key).unwrap().adapter, adapter);
        }
        let execution_targets = catalog.execution_targets();
        assert_eq!(execution_targets.len(), 8);
        // Pro is its own target, discovered by model beside Flash and Lite,
        // with an aspect-ratio certified size at every tier.
        let pro = execution_targets
            .iter()
            .find(|target| {
                target.model == "gemini-3-pro-image" && target.workload_type == "image_generation"
            })
            .unwrap();
        assert_eq!(pro.image_sizes, ["1k", "2k", "4k"]);
        assert_eq!(pro.output_sizes.len(), 30);
        let flux_3 = execution_targets
            .iter()
            .find(|target| target.model == "flux-3-image")
            .unwrap();
        assert_eq!(flux_3.image_sizes, ["1k", "2k"]);
        let gemini_full = execution_targets
            .iter()
            .find(|target| {
                target.model == "gemini-3.1-flash-image"
                    && target.workload_type == "image_generation"
            })
            .unwrap();
        let gemini_edit = execution_targets
            .iter()
            .find(|target| {
                target.model == "gemini-3.1-flash-image" && target.workload_type == "image_edit"
            })
            .unwrap();
        assert_eq!(
            gemini_edit.execution_scope.capability,
            "capability:image:edit"
        );
        assert_eq!(
            gemini_full.execution_scope.capability,
            "capability:image:generate"
        );
        assert_ne!(gemini_edit.target_id, gemini_full.target_id);
        assert!(gemini_edit
            .pricing
            .iter()
            .all(|rule| rule.components.len() == 2));
        assert_eq!(gemini_full.image_sizes, ["1k", "2k", "4k"]);
        let flux = execution_targets
            .iter()
            .find(|target| target.model == "flux-2-pro")
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
