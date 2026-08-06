//! Frozen operator pricing and the normalized provider billing boundary.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

use crate::{
    provider_targets::ProviderConfigVersion,
    secrets::{resolve_selected, ProviderSecret, SecretProvider},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const PRICING_SNAPSHOT_SCHEMA_VERSION: i64 = 1;
const SUPPORTED_CURRENCIES: &[&str] = &["USD"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("pricing catalog is malformed: {0}")]
    Malformed(String),
    #[error("pricing rule is ambiguous for provider/model")]
    AmbiguousRule,
    #[error("pricing rule not found")]
    UnsupportedTarget,
    #[error("cost cannot be safely determined")]
    IndeterminableCost,
    #[error("estimated cost exceeds authorization")]
    InsufficientAuthorization,
    #[error("settlement exceeds authorization")]
    SettlementOverage,
    #[error("retry requires vendor-enforced idempotency")]
    UnsafeRetry,
    #[error("provider error ({code})")]
    Provider { code: String },
    #[error("I/O: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, ContractError>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u32,
    catalog_version: String,
    rules: Vec<PricingRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PricingRule {
    pub rule_id: String,
    pub provider: String,
    pub model: String,
    pub currency: String,
    pub unit: PricingUnit,
    pub unit_amount_minor: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PricingUnit {
    Image,
    InputToken,
    OutputToken,
}

#[derive(Clone, Debug)]
pub struct PricingCatalog(Arc<FrozenCatalog>);

#[derive(Debug)]
struct FrozenCatalog {
    version: String,
    digest: String,
    rules: BTreeMap<(String, String), PricingRule>,
}

impl PricingCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path).map_err(|e| ContractError::Io(e.to_string()))?;
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let mut doc: CatalogDocument =
            serde_json::from_slice(bytes).map_err(|e| ContractError::Malformed(e.to_string()))?;
        if doc.schema_version != 1 || doc.catalog_version.trim().is_empty() || doc.rules.is_empty()
        {
            return Err(ContractError::Malformed(
                "unsupported schema, empty version, or no rules".into(),
            ));
        }
        doc.catalog_version = doc.catalog_version.trim().to_owned();
        for rule in &mut doc.rules {
            rule.rule_id = normalized(&rule.rule_id, "rule_id")?;
            rule.provider = normalized(&rule.provider, "provider")?;
            rule.model = normalized(&rule.model, "model")?;
            rule.currency = rule.currency.trim().to_ascii_uppercase();
        }
        doc.rules.sort_by(|a, b| {
            (&a.provider, &a.model, &a.rule_id).cmp(&(&b.provider, &b.model, &b.rule_id))
        });
        let mut ids = BTreeSet::new();
        let mut rules = BTreeMap::new();
        for rule in &doc.rules {
            if !ids.insert(rule.rule_id.clone()) {
                return Err(ContractError::Malformed("duplicate rule_id".into()));
            }
            if !SUPPORTED_CURRENCIES.contains(&rule.currency.as_str()) {
                return Err(ContractError::Malformed("unsupported currency".into()));
            }
            if rule.unit_amount_minor < 0 {
                return Err(ContractError::Malformed("negative unit amount".into()));
            }
            if rules
                .insert((rule.provider.clone(), rule.model.clone()), rule.clone())
                .is_some()
            {
                return Err(ContractError::AmbiguousRule);
            }
        }
        // Digest the validated canonical representation, never the input formatting.
        let canonical = serde_json::to_vec(&doc).expect("catalog serialization cannot fail");
        let digest = format!("sha256:{:x}", Sha256::digest(canonical));
        Ok(Self(Arc::new(FrozenCatalog {
            version: doc.catalog_version,
            digest,
            rules,
        })))
    }

    pub fn snapshot(&self, request: &NormalizedRequest) -> Result<PricingSnapshot> {
        request.validate()?;
        let rule = self
            .0
            .rules
            .get(&(request.provider.clone(), request.model.clone()))
            .ok_or(ContractError::UnsupportedTarget)?;
        let quantity = request.quantity_for(rule.unit)?;
        let estimated_amount_minor = rule
            .unit_amount_minor
            .checked_mul(quantity)
            .ok_or(ContractError::IndeterminableCost)?;
        Ok(PricingSnapshot {
            provider: request.provider.clone(),
            model: request.model.clone(),
            catalog_version: self.0.version.clone(),
            catalog_digest: self.0.digest.clone(),
            pricing_rule_id: rule.rule_id.clone(),
            unit: rule.unit,
            unit_amount_minor: rule.unit_amount_minor,
            quantity,
            estimated_amount_minor,
            currency: rule.currency.clone(),
        })
    }
}

fn normalized(value: &str, field: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 {
        return Err(ContractError::Malformed(format!("invalid {field}")));
    }
    Ok(value.to_owned())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedRequest {
    pub provider: String,
    pub model: String,
    pub image_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
}

impl NormalizedRequest {
    pub fn validate(&self) -> Result<()> {
        if self.provider.trim().is_empty() || self.model.trim().is_empty() {
            return Err(ContractError::IndeterminableCost);
        }
        for value in [self.image_count, self.input_tokens, self.max_output_tokens]
            .into_iter()
            .flatten()
        {
            if value < 0 {
                return Err(ContractError::IndeterminableCost);
            }
        }
        Ok(())
    }
    fn quantity_for(&self, unit: PricingUnit) -> Result<i64> {
        match unit {
            PricingUnit::Image => self.image_count,
            PricingUnit::InputToken => self.input_tokens,
            PricingUnit::OutputToken => self.max_output_tokens,
        }
        .filter(|v| *v > 0)
        .ok_or(ContractError::IndeterminableCost)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PricingSnapshot {
    pub provider: String,
    pub model: String,
    pub catalog_version: String,
    pub catalog_digest: String,
    pub pricing_rule_id: String,
    pub unit: PricingUnit,
    pub unit_amount_minor: i64,
    pub quantity: i64,
    pub estimated_amount_minor: i64,
    pub currency: String,
}

impl PricingSnapshot {
    pub fn validate_integrity(&self) -> Result<()> {
        let digest = self.catalog_digest.strip_prefix("sha256:");
        if self.provider.trim().is_empty()
            || self.model.trim().is_empty()
            || self.catalog_version.trim().is_empty()
            || self.pricing_rule_id.trim().is_empty()
            || digest.map_or(true, |value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || !SUPPORTED_CURRENCIES.contains(&self.currency.as_str())
            || self.unit_amount_minor < 0
            || self.quantity <= 0
            || self.unit_amount_minor.checked_mul(self.quantity)
                != Some(self.estimated_amount_minor)
        {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }

    pub fn check_authorization(&self, authorized_minor: i64, currency: &str) -> Result<()> {
        self.validate_integrity()?;
        if authorized_minor < 0
            || !self.currency.eq_ignore_ascii_case(currency)
            || self.estimated_amount_minor > authorized_minor
        {
            return Err(ContractError::InsufficientAuthorization);
        }
        Ok(())
    }
    pub fn settle(&self, usage: &NormalizedUsage, authorized_minor: i64) -> Result<i64> {
        self.validate_integrity()?;
        let quantity = usage
            .quantity_for(self.unit)
            .ok_or(ContractError::IndeterminableCost)?;
        let amount = self
            .unit_amount_minor
            .checked_mul(quantity)
            .ok_or(ContractError::IndeterminableCost)?;
        if quantity > self.quantity
            || amount < 0
            || amount > self.estimated_amount_minor
            || amount > authorized_minor
        {
            return Err(ContractError::SettlementOverage);
        }
        Ok(amount)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct NormalizedUsage {
    pub images: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}
impl NormalizedUsage {
    fn quantity_for(&self, unit: PricingUnit) -> Option<i64> {
        match unit {
            PricingUnit::Image => self.images,
            PricingUnit::InputToken => self.input_tokens,
            PricingUnit::OutputToken => self.output_tokens,
        }
        .filter(|v| *v >= 0)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AdapterOutcome {
    pub outcome: OutcomeKind,
    pub usage: Option<NormalizedUsage>,
    pub provider_amount_minor: Option<i64>,
    pub provider_currency: Option<String>,
    pub provider_request_id: Option<String>,
}
impl AdapterOutcome {
    pub fn validate(&self) -> Result<()> {
        if self.provider_amount_minor.is_some() != self.provider_currency.is_some()
            || self.provider_amount_minor.is_some_and(|amount| amount < 0)
        {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Succeeded,
    Failed,
    Ambiguous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub vendor_enforced_idempotency: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
}
impl RetryPolicy {
    pub fn validate(self, c: AdapterCapabilities) -> Result<()> {
        if self.max_retries > 0 && !c.vendor_enforced_idempotency {
            Err(ContractError::UnsafeRetry)
        } else {
            Ok(())
        }
    }
}

pub trait ProviderAdapter {
    fn adapter_id(&self) -> &str;
    fn capabilities(&self) -> AdapterCapabilities;
    fn validate_request(&self, request: &NormalizedRequest) -> Result<()> {
        request.validate()
    }
    fn invoke(
        &self,
        request: &NormalizedRequest,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome>;
    fn redact_error(&self, _error: &(dyn std::error::Error + 'static)) -> ContractError {
        ContractError::Provider {
            code: "provider_failure".into(),
        }
    }
}

/// Preflight the credential before claim/attempt creation. This never invokes the
/// adapter; orchestration retains the secret and invokes only after durable claim
/// and attempt creation.
pub fn preflight_selected_secret(
    adapter: &dyn ProviderAdapter,
    secret_provider: &dyn SecretProvider,
    target: &ProviderConfigVersion,
    request: &NormalizedRequest,
) -> Result<ProviderSecret> {
    if adapter.adapter_id() != target.adapter
        || request.provider != target.provider
        || request.model != target.model
    {
        return Err(ContractError::Provider {
            code: "target_mismatch".into(),
        });
    }
    let secret =
        resolve_selected(secret_provider, target).map_err(|_| ContractError::Provider {
            code: "secret_unavailable".into(),
        })?;
    Ok(secret)
}

pub fn vendor_idempotency_key(
    provider: &str,
    model: &str,
    owner_anchor: &str,
    operation_key: &str,
) -> Result<String> {
    if provider.trim().is_empty()
        || model.trim().is_empty()
        || owner_anchor.trim().is_empty()
        || operation_key.trim().is_empty()
    {
        return Err(ContractError::IndeterminableCost);
    }
    let mut h = Sha256::new();
    for v in ["gongbu-v3", provider, model, owner_anchor, operation_key] {
        h.update((v.len() as u64).to_be_bytes());
        h.update(v.as_bytes());
    }
    Ok(format!("gongbu-{:x}", h.finalize()))
}

/// The executor calls this once before claim and again immediately before invoke.
pub fn enforce_cost(
    snapshot: &PricingSnapshot,
    authorized_minor: i64,
    currency: &str,
) -> Result<()> {
    snapshot.check_authorization(authorized_minor, currency)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CATALOG: &str = r#"{"schema_version":1,"catalog_version":"2026-08-05","rules":[{"rule_id":"image-standard","provider":"vendor","model":"image-v1","currency":"usd","unit":"image","unit_amount_minor":125}]}"#;
    fn request() -> NormalizedRequest {
        NormalizedRequest {
            provider: "vendor".into(),
            model: "image-v1".into(),
            image_count: Some(2),
            input_tokens: None,
            max_output_tokens: None,
        }
    }
    #[test]
    fn canonical_digest_is_format_independent_and_catalog_is_frozen() {
        let a = PricingCatalog::from_json(CATALOG.as_bytes()).unwrap();
        let b = PricingCatalog::from_json(CATALOG.replace(":", ": ").as_bytes()).unwrap();
        let s = a.snapshot(&request()).unwrap();
        assert_eq!(
            s.catalog_digest,
            b.snapshot(&request()).unwrap().catalog_digest
        );
        assert_eq!(s.estimated_amount_minor, 250);

        let spaced = r#"{"schema_version":1,"catalog_version":"v1","rules":[{"rule_id":" z","provider":" p","model":" z","currency":" usd ","unit":"image","unit_amount_minor":1},{"rule_id":"a","provider":"p","model":"a","currency":"USD","unit":"image","unit_amount_minor":2}]}"#;
        let normalized = spaced
            .replace("\" z\"", "\"z\"")
            .replace("\" p\"", "\"p\"")
            .replace("\" usd \"", "\"USD\"");
        let spaced_catalog = PricingCatalog::from_json(spaced.as_bytes()).unwrap();
        let normalized_catalog = PricingCatalog::from_json(normalized.as_bytes()).unwrap();
        assert_eq!(spaced_catalog.0.digest, normalized_catalog.0.digest);
    }
    #[test]
    fn rejects_duplicate_ambiguous_malformed_and_currency() {
        assert!(PricingCatalog::from_json(b"{}").is_err());
        assert!(PricingCatalog::from_json(CATALOG.replace("usd", "EUR").as_bytes()).is_err());
        let duplicate=CATALOG.replace("]}", ",{\"rule_id\":\"other\",\"provider\":\"vendor\",\"model\":\"image-v1\",\"currency\":\"USD\",\"unit\":\"image\",\"unit_amount_minor\":1}]}");
        assert_eq!(
            PricingCatalog::from_json(duplicate.as_bytes()).unwrap_err(),
            ContractError::AmbiguousRule
        );
        let duplicate_id = CATALOG.replace(
            "]}",
            ",{\"rule_id\":\"image-standard\",\"provider\":\"other\",\"model\":\"other\",\"currency\":\"USD\",\"unit\":\"image\",\"unit_amount_minor\":1}]}"
        );
        assert!(
            matches!(PricingCatalog::from_json(duplicate_id.as_bytes()), Err(ContractError::Malformed(message)) if message.contains("duplicate rule_id"))
        );
    }
    #[test]
    fn deterministic_settlement_uses_frozen_rule_without_provider_dollars() {
        let s = PricingCatalog::from_json(CATALOG.as_bytes())
            .unwrap()
            .snapshot(&request())
            .unwrap();
        let usage = NormalizedUsage {
            images: Some(2),
            ..Default::default()
        };
        assert_eq!(s.settle(&usage, 250).unwrap(), 250);
        assert_eq!(
            s.settle(
                &NormalizedUsage {
                    images: Some(3),
                    ..Default::default()
                },
                250
            ),
            Err(ContractError::SettlementOverage)
        );
        assert_eq!(
            s.settle(
                &NormalizedUsage {
                    images: Some(3),
                    ..Default::default()
                },
                1_000
            ),
            Err(ContractError::SettlementOverage)
        );
    }
    #[test]
    fn cost_checks_apply_at_both_gates() {
        let s = PricingCatalog::from_json(CATALOG.as_bytes())
            .unwrap()
            .snapshot(&request())
            .unwrap();
        enforce_cost(&s, 250, "USD").unwrap();
        enforce_cost(&s, 250, "usd").unwrap();
        assert_eq!(
            enforce_cost(&s, 249, "USD"),
            Err(ContractError::InsufficientAuthorization)
        );
    }
    #[test]
    fn retries_require_vendor_idempotency_and_key_is_opaque() {
        assert_eq!(
            RetryPolicy { max_retries: 1 }.validate(AdapterCapabilities {
                vendor_enforced_idempotency: false
            }),
            Err(ContractError::UnsafeRetry)
        );
        RetryPolicy { max_retries: 1 }
            .validate(AdapterCapabilities {
                vendor_enforced_idempotency: true,
            })
            .unwrap();
        let key =
            vendor_idempotency_key("vendor", "model-1", "account-1", "secret-operation").unwrap();
        assert!(!key.contains("secret-operation"));
        assert_ne!(
            key,
            vendor_idempotency_key("vendor", "model-1", "account-2", "secret-operation").unwrap()
        );
        assert_ne!(
            key,
            vendor_idempotency_key("vendor", "model-2", "account-1", "secret-operation").unwrap()
        );
    }
    #[test]
    fn default_redaction_does_not_leak_vendor_error() {
        struct A;
        impl ProviderAdapter for A {
            fn adapter_id(&self) -> &str {
                "a"
            }
            fn capabilities(&self) -> AdapterCapabilities {
                AdapterCapabilities {
                    vendor_enforced_idempotency: false,
                }
            }
            fn invoke(
                &self,
                _: &NormalizedRequest,
                _: &ProviderSecret,
                _: Option<&str>,
            ) -> Result<AdapterOutcome> {
                unreachable!()
            }
        }
        let raw = std::io::Error::other("token=super-secret");
        assert_eq!(
            A.redact_error(&raw).to_string(),
            "provider error (provider_failure)"
        );
    }

    #[test]
    fn selected_secret_is_the_only_credential_passed_and_missing_fails_before_adapter() {
        use crate::secrets::{SecretError, SecretReference};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Secrets(bool);
        impl SecretProvider for Secrets {
            fn resolve(&self, _: &SecretReference) -> crate::secrets::Result<ProviderSecret> {
                if self.0 {
                    Ok(crate::secrets::secret_for_test("selected-canary"))
                } else {
                    Err(SecretError::Unavailable)
                }
            }
        }
        struct Adapter(AtomicUsize);
        impl ProviderAdapter for Adapter {
            fn adapter_id(&self) -> &str {
                "a"
            }
            fn capabilities(&self) -> AdapterCapabilities {
                AdapterCapabilities {
                    vendor_enforced_idempotency: false,
                }
            }
            fn invoke(
                &self,
                _: &NormalizedRequest,
                secret: &ProviderSecret,
                _: Option<&str>,
            ) -> Result<AdapterOutcome> {
                assert_eq!(secret.expose(), b"selected-canary");
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(AdapterOutcome {
                    outcome: OutcomeKind::Succeeded,
                    usage: None,
                    provider_amount_minor: None,
                    provider_currency: None,
                    provider_request_id: None,
                })
            }
        }
        let target = ProviderConfigVersion {
            provider_config_version: "v1".into(),
            workload_type: "image_generation".into(),
            provider: "vendor".into(),
            adapter: "a".into(),
            model: "image-v1".into(),
            secret_service: "gongbu.vendor".into(),
            secret_account: "local".into(),
            enabled: true,
        };
        let adapter = Adapter(AtomicUsize::new(0));
        assert!(
            matches!(preflight_selected_secret(&adapter, &Secrets(false), &target, &request()), Err(ContractError::Provider { code }) if code == "secret_unavailable")
        );
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0);
        let secret =
            preflight_selected_secret(&adapter, &Secrets(true), &target, &request()).unwrap();
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0);
        adapter.invoke(&request(), &secret, None).unwrap();
        assert_eq!(adapter.0.load(Ordering::SeqCst), 1);

        let mut wrong_request = request();
        wrong_request.provider = "other".into();
        assert!(
            matches!(preflight_selected_secret(&adapter, &Secrets(true), &target, &wrong_request), Err(ContractError::Provider { code }) if code == "target_mismatch")
        );
    }
}
