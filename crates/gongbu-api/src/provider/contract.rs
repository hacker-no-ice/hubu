//! Frozen operator pricing and the normalized provider billing boundary.
use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use crate::{
    provider_targets::{ProviderConfigVersion, TargetKey},
    secrets::{resolve_selected, ProviderSecret, SecretProvider},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const PRICING_SNAPSHOT_SCHEMA_VERSION: i64 = 2;
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

pub type Result<T, E = ContractError> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPhase {
    PreSend,
    Submission,
    Processing,
    Artifact,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpendDisposition {
    Release,
    Reconcile,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderEvidence {
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderFailure {
    /// Stable, persisted machine code. Never contains raw vendor text.
    pub code: String,
    pub phase: ProviderPhase,
    pub spend_disposition: SpendDisposition,
    #[serde(default)]
    pub evidence: ProviderEvidence,
}

impl ProviderFailure {
    pub fn release(code: impl Into<String>, phase: ProviderPhase) -> Self {
        Self {
            code: code.into(),
            phase,
            spend_disposition: SpendDisposition::Release,
            evidence: ProviderEvidence::default(),
        }
    }

    pub fn reconcile(code: impl Into<String>, phase: ProviderPhase) -> Self {
        Self {
            code: code.into(),
            phase,
            spend_disposition: SpendDisposition::Reconcile,
            evidence: ProviderEvidence::default(),
        }
    }

    pub fn with_evidence(
        mut self,
        request_id: Option<String>,
        operation_id: Option<String>,
    ) -> Self {
        self.evidence = ProviderEvidence {
            request_id,
            operation_id,
        };
        self
    }
}

impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "provider error ({})", self.code)
    }
}

impl std::error::Error for ProviderFailure {}

/// Return the canonical media type only when the bytes are a supported image
/// and, when supplied, the provider declaration agrees with the content.
pub fn canonical_image_media_type(declared: Option<&str>, bytes: &[u8]) -> Result<&'static str> {
    let actual = match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        _ => {
            return Err(ContractError::Provider {
                code: "artifact_policy_failure".into(),
            });
        }
    };
    if declared.is_some_and(|value| value != actual) {
        return Err(ContractError::Provider {
            code: "artifact_policy_failure".into(),
        });
    }
    Ok(actual)
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<PricingSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<PricingUnit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_amount_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<PriceComponent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct PricingSelector {
    pub image_size: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PriceComponent {
    pub unit: PricingUnit,
    pub rate_numerator_minor: i64,
    pub rate_denominator: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
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
    schema_version: u32,
    rules: Vec<PricingRule>,
}

impl PricingCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path).map_err(|e| ContractError::Io(e.to_string()))?;
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let mut doc: CatalogDocument =
            serde_json::from_slice(bytes).map_err(|e| ContractError::Malformed(e.to_string()))?;
        if !matches!(doc.schema_version, 1 | 2)
            || doc.catalog_version.trim().is_empty()
            || doc.rules.is_empty()
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
            if let Some(selector) = &mut rule.selector {
                selector.image_size = normalize_image_size(&selector.image_size)?;
            }
            rule.components.sort_by_key(|component| component.unit);
        }
        doc.rules.sort_by(|a, b| {
            (&a.provider, &a.model, &a.rule_id).cmp(&(&b.provider, &b.model, &b.rule_id))
        });
        let mut ids = BTreeSet::new();
        let mut selectors = BTreeSet::new();
        for rule in &doc.rules {
            if !ids.insert(rule.rule_id.clone()) {
                return Err(ContractError::Malformed("duplicate rule_id".into()));
            }
            if !SUPPORTED_CURRENCIES.contains(&rule.currency.as_str()) {
                return Err(ContractError::Malformed("unsupported currency".into()));
            }
            let legacy = rule.unit.zip(rule.unit_amount_minor);
            if doc.schema_version == 1 {
                if rule.selector.is_some() || !rule.components.is_empty() || legacy.is_none() {
                    return Err(ContractError::Malformed("invalid v1 rule shape".into()));
                }
            } else if legacy.is_some()
                || rule.unit.is_some()
                || rule.unit_amount_minor.is_some()
                || rule.components.is_empty()
            {
                return Err(ContractError::Malformed("invalid v2 rule shape".into()));
            }
            if legacy.is_some_and(|(_, amount)| amount < 0)
                || rule
                    .components
                    .iter()
                    .any(|c| c.rate_numerator_minor < 0 || c.rate_denominator <= 0)
            {
                return Err(ContractError::Malformed("invalid rate".into()));
            }
            let mut units = BTreeSet::new();
            if rule.components.iter().any(|c| !units.insert(c.unit)) {
                return Err(ContractError::Malformed("duplicate price component".into()));
            }
            if rule.selector.is_some()
                && (rule.components.len() != 1 || rule.components[0].unit != PricingUnit::Image)
            {
                return Err(ContractError::Malformed(
                    "image selector requires one image component".into(),
                ));
            }
            let key = (
                rule.provider.clone(),
                rule.model.clone(),
                rule.selector.clone(),
            );
            if !selectors.insert(key) {
                return Err(ContractError::AmbiguousRule);
            }
        }
        for rule in &doc.rules {
            if rule.selector.is_none()
                && doc
                    .rules
                    .iter()
                    .filter(|other| other.provider == rule.provider && other.model == rule.model)
                    .count()
                    > 1
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
            schema_version: doc.schema_version,
            rules: doc.rules,
        })))
    }

    pub fn snapshot(&self, request: &NormalizedRequest) -> Result<PricingSnapshot> {
        request.validate()?;
        let candidates: Vec<_> = self
            .0
            .rules
            .iter()
            .filter(|rule| {
                rule.provider == request.provider
                    && rule.model == request.model
                    && match &rule.selector {
                        Some(s) => request.image_size.as_ref() == Some(&s.image_size),
                        None => request.image_size.is_none(),
                    }
            })
            .collect();
        if candidates.len() > 1 {
            return Err(ContractError::AmbiguousRule);
        }
        let rule = candidates
            .first()
            .copied()
            .ok_or(ContractError::UnsupportedTarget)?;
        if self.0.rules.iter().any(|r| {
            r.provider == request.provider && r.model == request.model && r.selector.is_some()
        }) && rule.selector.is_none()
        {
            return Err(ContractError::IndeterminableCost);
        }
        let components = if let Some((unit, amount)) = rule.unit.zip(rule.unit_amount_minor) {
            vec![PriceComponent {
                unit,
                rate_numerator_minor: amount,
                rate_denominator: 1,
            }]
        } else {
            rule.components.clone()
        };
        let mut exact = Rational::zero();
        let mut frozen = Vec::new();
        for component in components {
            let quantity = request.quantity_for(component.unit)?;
            exact = exact.add(
                Rational::new(component.rate_numerator_minor, component.rate_denominator)?
                    .mul(quantity)?,
            )?;
            frozen.push(FrozenPriceComponent {
                unit: component.unit,
                rate_numerator_minor: component.rate_numerator_minor,
                rate_denominator: component.rate_denominator,
                quantity,
            });
        }
        let estimated_amount_minor = exact.ceil_i64()?;
        let legacy = self.0.schema_version == 1;
        let legacy_component = frozen.first().cloned();
        Ok(PricingSnapshot {
            schema_version: self.0.schema_version,
            provider: request.provider.clone(),
            model: request.model.clone(),
            catalog_version: self.0.version.clone(),
            catalog_digest: self.0.digest.clone(),
            pricing_rule_id: rule.rule_id.clone(),
            selector: rule.selector.clone(),
            components: if legacy { Vec::new() } else { frozen },
            exact_estimate_numerator: if legacy {
                String::new()
            } else {
                exact.n.to_string()
            },
            exact_estimate_denominator: if legacy {
                String::new()
            } else {
                exact.d.to_string()
            },
            estimated_amount_minor,
            currency: rule.currency.clone(),
            unit: legacy_component.as_ref().filter(|_| legacy).map(|c| c.unit),
            unit_amount_minor: legacy_component
                .as_ref()
                .filter(|_| legacy)
                .map(|c| c.rate_numerator_minor),
            quantity: legacy_component.filter(|_| legacy).map(|c| c.quantity),
        })
    }

    /// Price only a request already bound to a validated provider target key.
    pub fn snapshot_for_target(
        &self,
        target: &TargetKey,
        request: &NormalizedRequest,
    ) -> Result<PricingSnapshot> {
        if request.provider != target.provider || request.model != target.model {
            return Err(ContractError::UnsupportedTarget);
        }
        self.snapshot(request)
    }

    /// Whether the frozen catalog contains at least one rule for this target.
    /// Selector-specific request validation still occurs when a snapshot is made.
    pub fn supports_target(&self, target: &TargetKey) -> bool {
        self.0
            .rules
            .iter()
            .any(|rule| rule.provider == target.provider && rule.model == target.model)
    }
}

fn normalize_image_size(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if matches!(value.as_str(), "1k" | "2k" | "4k") {
        Ok(value)
    } else {
        Err(ContractError::Malformed("invalid image_size".into()))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,
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
        if let Some(size) = &self.image_size {
            if normalize_image_size(size).map_err(|_| ContractError::IndeterminableCost)? != *size {
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

pub fn validate_image_size_input(
    request: &NormalizedRequest,
    input: &serde_json::Value,
) -> Result<()> {
    let supplied = input.get("image_size").and_then(serde_json::Value::as_str);
    if supplied != request.image_size.as_deref() {
        return Err(ContractError::IndeterminableCost);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageGenerationInputV1 {
    prompt: String,
    #[serde(default)]
    image_count: Option<i64>,
    #[serde(default)]
    image_size: Option<String>,
    #[serde(default)]
    options: Option<serde_json::Map<String, serde_json::Value>>,
}

pub fn validate_image_input_versioned(
    request: &NormalizedRequest,
    input: &serde_json::Value,
    schema_version: i64,
) -> Result<()> {
    if schema_version != 1 {
        return Err(ContractError::IndeterminableCost);
    }
    request.validate()?;
    let typed: ImageGenerationInputV1 =
        serde_json::from_value(input.clone()).map_err(|_| ContractError::IndeterminableCost)?;
    if typed.prompt.trim().is_empty()
        || typed.prompt.len() > 32_000
        || typed
            .image_count
            .is_some_and(|count| Some(count) != request.image_count)
        || typed.image_size.as_deref() != request.image_size.as_deref()
    {
        return Err(ContractError::IndeterminableCost);
    }
    let _ = typed.options;
    Ok(())
}

pub fn validate_image_input(request: &NormalizedRequest, input: &serde_json::Value) -> Result<()> {
    validate_image_input_versioned(request, input, 1)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PricingSnapshot {
    #[serde(default = "legacy_snapshot_version")]
    pub schema_version: u32,
    pub provider: String,
    pub model: String,
    pub catalog_version: String,
    pub catalog_digest: String,
    pub pricing_rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<PricingSelector>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<FrozenPriceComponent>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exact_estimate_numerator: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exact_estimate_denominator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<PricingUnit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_amount_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
    pub estimated_amount_minor: i64,
    pub currency: String,
}

const fn legacy_snapshot_version() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrozenPriceComponent {
    pub unit: PricingUnit,
    pub rate_numerator_minor: i64,
    pub rate_denominator: i64,
    pub quantity: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rational {
    n: i128,
    d: i128,
}
impl Rational {
    fn zero() -> Self {
        Self { n: 0, d: 1 }
    }
    fn new(n: i64, d: i64) -> Result<Self> {
        if n < 0 || d <= 0 {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(Self {
            n: i128::from(n),
            d: i128::from(d),
        })
    }
    fn mul(self, quantity: i64) -> Result<Self> {
        if quantity < 0 {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(Self {
            n: self
                .n
                .checked_mul(i128::from(quantity))
                .ok_or(ContractError::IndeterminableCost)?,
            d: self.d,
        })
    }
    fn add(self, other: Self) -> Result<Self> {
        let n = self
            .n
            .checked_mul(other.d)
            .and_then(|a| other.n.checked_mul(self.d).and_then(|b| a.checked_add(b)))
            .ok_or(ContractError::IndeterminableCost)?;
        let d = self
            .d
            .checked_mul(other.d)
            .ok_or(ContractError::IndeterminableCost)?;
        let g = gcd(n, d);
        Ok(Self { n: n / g, d: d / g })
    }
    fn ceil_i64(self) -> Result<i64> {
        i64::try_from(
            self.n
                .checked_add(self.d - 1)
                .ok_or(ContractError::IndeterminableCost)?
                / self.d,
        )
        .map_err(|_| ContractError::IndeterminableCost)
    }
    // Settlement rounds the aggregate exact amount once, half up to currency minor units.
    fn round_i64(self) -> Result<i64> {
        i64::try_from(
            self.n
                .checked_mul(2)
                .and_then(|n| n.checked_add(self.d))
                .ok_or(ContractError::IndeterminableCost)?
                / self
                    .d
                    .checked_mul(2)
                    .ok_or(ContractError::IndeterminableCost)?,
        )
        .map_err(|_| ContractError::IndeterminableCost)
    }
}
fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.abs().max(1)
}

impl PricingSnapshot {
    pub fn is_image_only(&self) -> bool {
        match self.schema_version {
            1 => self.unit == Some(PricingUnit::Image),
            2 => self.components.len() == 1 && self.components[0].unit == PricingUnit::Image,
            _ => false,
        }
    }
    pub fn has_unit(&self, unit: PricingUnit) -> bool {
        self.unit == Some(unit)
            || self
                .components
                .iter()
                .any(|component| component.unit == unit)
    }
    pub fn estimated_quantity(&self, unit: PricingUnit) -> Option<i64> {
        if self.unit == Some(unit) {
            self.quantity
        } else {
            self.components
                .iter()
                .find(|component| component.unit == unit)
                .map(|component| component.quantity)
        }
    }
    pub fn validate_integrity(&self) -> Result<()> {
        let digest = self.catalog_digest.strip_prefix("sha256:");
        if self.provider.trim().is_empty()
            || self.model.trim().is_empty()
            || self.catalog_version.trim().is_empty()
            || self.pricing_rule_id.trim().is_empty()
            || digest.is_none_or(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || !SUPPORTED_CURRENCIES.contains(&self.currency.as_str())
            || self.estimated_amount_minor < 0
        {
            return Err(ContractError::IndeterminableCost);
        }
        if self.selector.as_ref().is_some_and(|selector| {
            normalize_image_size(&selector.image_size).ok().as_ref() != Some(&selector.image_size)
        }) {
            return Err(ContractError::IndeterminableCost);
        }
        let components = self.effective_components()?;
        let mut units = BTreeSet::new();
        if components.iter().any(|component| {
            component.rate_numerator_minor < 0
                || component.rate_denominator <= 0
                || component.quantity <= 0
                || !units.insert(component.unit)
        }) || (self.selector.is_some()
            && (components.len() != 1 || components[0].unit != PricingUnit::Image))
        {
            return Err(ContractError::IndeterminableCost);
        }
        let exact = self.exact_estimate()?;
        if exact.ceil_i64()? != self.estimated_amount_minor {
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
        let mut exact = Rational::zero();
        for component in self.effective_components()? {
            let quantity = usage
                .quantity_for(component.unit)
                .ok_or(ContractError::IndeterminableCost)?;
            if quantity > component.quantity {
                return Err(ContractError::SettlementOverage);
            }
            exact = exact.add(
                Rational::new(component.rate_numerator_minor, component.rate_denominator)?
                    .mul(quantity)?,
            )?;
        }
        let amount = exact.round_i64()?;
        if amount > self.estimated_amount_minor || amount > authorized_minor {
            return Err(ContractError::SettlementOverage);
        }
        Ok(amount)
    }

    fn effective_components(&self) -> Result<Vec<FrozenPriceComponent>> {
        if self.schema_version == 1 {
            let (unit, rate, quantity) = self
                .unit
                .zip(self.unit_amount_minor)
                .zip(self.quantity)
                .map(|((a, b), c)| (a, b, c))
                .ok_or(ContractError::IndeterminableCost)?;
            if rate < 0 || quantity <= 0 || !self.components.is_empty() {
                return Err(ContractError::IndeterminableCost);
            }
            Ok(vec![FrozenPriceComponent {
                unit,
                rate_numerator_minor: rate,
                rate_denominator: 1,
                quantity,
            }])
        } else if self.schema_version == 2
            && self.unit.is_none()
            && self.unit_amount_minor.is_none()
            && self.quantity.is_none()
            && !self.components.is_empty()
        {
            Ok(self.components.clone())
        } else {
            Err(ContractError::IndeterminableCost)
        }
    }
    fn exact_estimate(&self) -> Result<Rational> {
        if self.schema_version == 1 {
            let c = self.effective_components()?.remove(0);
            Rational::new(c.rate_numerator_minor, c.rate_denominator)?.mul(c.quantity)
        } else {
            let n = self
                .exact_estimate_numerator
                .parse::<i128>()
                .map_err(|_| ContractError::IndeterminableCost)?;
            let d = self
                .exact_estimate_denominator
                .parse::<i128>()
                .map_err(|_| ContractError::IndeterminableCost)?;
            if n < 0 || d <= 0 {
                return Err(ContractError::IndeterminableCost);
            }
            let mut recomputed = Rational::zero();
            for c in self.effective_components()? {
                recomputed = recomputed.add(
                    Rational::new(c.rate_numerator_minor, c.rate_denominator)?.mul(c.quantity)?,
                )?;
            }
            if recomputed != (Rational { n, d }) {
                return Err(ContractError::IndeterminableCost);
            }
            Ok(recomputed)
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
    pub usage: Option<NormalizedUsage>,
    pub provider_amount_minor: Option<i64>,
    pub provider_currency: Option<String>,
    pub provider_request_id: Option<String>,
    pub provider_operation_id: Option<String>,
    pub artifacts: Vec<NormalizedArtifact>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NormalizedArtifact {
    pub media_type: String,
    pub bytes: Vec<u8>,
}
impl AdapterOutcome {
    pub fn validate(&self) -> Result<()> {
        if self.provider_amount_minor.is_some() != self.provider_currency.is_some()
            || self.provider_amount_minor.is_some_and(|amount| amount < 0)
        {
            return Err(ContractError::IndeterminableCost);
        }
        if self.usage.is_none()
            || self.artifacts.is_empty()
            || (self.provider_request_id.is_none() && self.provider_operation_id.is_none())
            || self
                .provider_request_id
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || self
                .provider_operation_id
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ContractError::Provider {
                code: "invalid_provider_success".into(),
            });
        }
        Ok(())
    }
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

pub trait ProviderAdapter: Send + Sync {
    fn adapter_id(&self) -> &str;
    fn capabilities(&self) -> AdapterCapabilities;
    fn validate_request(&self, request: &NormalizedRequest) -> Result<()> {
        request.validate()
    }
    fn preflight_input(
        &self,
        request: &NormalizedRequest,
        _normalized_input: &serde_json::Value,
    ) -> Result<()> {
        self.validate_request(request)
    }
    fn invoke(
        &self,
        request: &NormalizedRequest,
        normalized_input: &serde_json::Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure>;
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
            image_size: None,
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
    fn v2_resolution_tiers_are_selected_and_frozen() {
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":"1k","provider":"vendor","model":"image-v2","currency":"USD","selector":{"image_size":"1K"},"components":[{"unit":"image","rate_numerator_minor":10,"rate_denominator":1}]},{"rule_id":"2k","provider":"vendor","model":"image-v2","currency":"USD","selector":{"image_size":"2k"},"components":[{"unit":"image","rate_numerator_minor":20,"rate_denominator":1}]},{"rule_id":"4k","provider":"vendor","model":"image-v2","currency":"USD","selector":{"image_size":"4k"},"components":[{"unit":"image","rate_numerator_minor":40,"rate_denominator":1}]}]}"#).unwrap();
        let mut request = request();
        request.model = "image-v2".into();
        request.image_size = Some("2k".into());
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.selector.unwrap().image_size, "2k");
        assert_eq!(snapshot.estimated_amount_minor, 40);
        request.image_size = None;
        assert_eq!(
            catalog.snapshot(&request),
            Err(ContractError::UnsupportedTarget)
        );

        let flat = PricingCatalog::from_json(CATALOG.as_bytes()).unwrap();
        request.model = "image-v1".into();
        request.image_size = Some("4k".into());
        assert_eq!(
            flat.snapshot(&request),
            Err(ContractError::UnsupportedTarget)
        );
    }

    #[test]
    fn v2_digest_canonicalizes_rule_component_and_selector_formatting() {
        let a = br#"{"schema_version":2,"catalog_version":" v2 ","rules":[{"rule_id":" text ","provider":" vendor ","model":" model ","currency":" usd ","components":[{"unit":"output_token","rate_numerator_minor":2,"rate_denominator":1000000},{"unit":"input_token","rate_numerator_minor":1,"rate_denominator":1000000}]},{"rule_id":" image ","provider":" vendor ","model":" image ","currency":" usd ","selector":{"image_size":" 2K "},"components":[{"unit":"image","rate_numerator_minor":3,"rate_denominator":1}]}]}"#;
        let b = br#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":"image","provider":"vendor","model":"image","currency":"USD","selector":{"image_size":"2k"},"components":[{"unit":"image","rate_numerator_minor":3,"rate_denominator":1}]},{"rule_id":"text","provider":"vendor","model":"model","currency":"USD","components":[{"unit":"input_token","rate_numerator_minor":1,"rate_denominator":1000000},{"unit":"output_token","rate_numerator_minor":2,"rate_denominator":1000000}]}]}"#;
        assert_eq!(
            PricingCatalog::from_json(a).unwrap().0.digest,
            PricingCatalog::from_json(b).unwrap().0.digest
        );
    }

    #[test]
    fn compound_token_rates_are_exact_and_round_once() {
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"tokens","rules":[{"rule_id":"text","provider":"vendor","model":"text-v1","currency":"USD","components":[{"unit":"input_token","rate_numerator_minor":100,"rate_denominator":1000000},{"unit":"output_token","rate_numerator_minor":300,"rate_denominator":1000000}]}]}"#).unwrap();
        let request = NormalizedRequest {
            provider: "vendor".into(),
            model: "text-v1".into(),
            image_count: None,
            input_tokens: Some(2_500),
            max_output_tokens: Some(2_500),
            image_size: None,
        };
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.exact_estimate_numerator, "1");
        assert_eq!(snapshot.exact_estimate_denominator, "1");
        assert_eq!(snapshot.estimated_amount_minor, 1);
        let usage = NormalizedUsage {
            images: None,
            input_tokens: Some(2_500),
            output_tokens: Some(2_500),
        };
        assert_eq!(snapshot.settle(&usage, 1).unwrap(), 1);
        let serialized = serde_json::to_vec(&snapshot).unwrap();
        let replayed: PricingSnapshot = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(replayed.settle(&usage, 1).unwrap(), 1);
    }

    #[test]
    fn fractional_components_round_only_at_aggregate_and_authorize_by_ceiling() {
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"fractions","rules":[{"rule_id":"text","provider":"vendor","model":"text-v1","currency":"USD","components":[{"unit":"input_token","rate_numerator_minor":1,"rate_denominator":4},{"unit":"output_token","rate_numerator_minor":1,"rate_denominator":4}]}]}"#).unwrap();
        let request = NormalizedRequest {
            provider: "vendor".into(),
            model: "text-v1".into(),
            image_count: None,
            input_tokens: Some(1),
            max_output_tokens: Some(1),
            image_size: None,
        };
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.estimated_amount_minor, 1);
        assert_eq!(
            snapshot
                .settle(
                    &NormalizedUsage {
                        images: None,
                        input_tokens: Some(1),
                        output_tokens: Some(1)
                    },
                    1
                )
                .unwrap(),
            1
        );
        assert_eq!(
            snapshot.check_authorization(0, "USD"),
            Err(ContractError::InsufficientAuthorization)
        );
    }

    #[test]
    fn v2_rejects_bad_denominators_overlap_and_overflow() {
        let bad = br#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":"bad","provider":"v","model":"m","currency":"USD","components":[{"unit":"input_token","rate_numerator_minor":1,"rate_denominator":0}]}]}"#;
        assert!(PricingCatalog::from_json(bad).is_err());
        let overlap = br#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":"flat","provider":"v","model":"m","currency":"USD","components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]},{"rule_id":"tier","provider":"v","model":"m","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]}]}"#;
        assert_eq!(
            PricingCatalog::from_json(overlap).unwrap_err(),
            ContractError::AmbiguousRule
        );
        assert_eq!(
            Rational::new(i64::MAX, 1)
                .unwrap()
                .mul(i64::MAX)
                .unwrap()
                .mul(i64::MAX),
            Err(ContractError::IndeterminableCost)
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
                _: &serde_json::Value,
                _: &ProviderSecret,
                _: Option<&str>,
            ) -> Result<AdapterOutcome, ProviderFailure> {
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
                _: &serde_json::Value,
                secret: &ProviderSecret,
                _: Option<&str>,
            ) -> Result<AdapterOutcome, ProviderFailure> {
                assert_eq!(secret.expose(), b"selected-canary");
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(AdapterOutcome {
                    usage: None,
                    provider_amount_minor: None,
                    provider_currency: None,
                    provider_request_id: None,
                    provider_operation_id: None,
                    artifacts: Vec::new(),
                })
            }
        }
        let catalog: crate::provider_targets::ProviderTargetConfig = serde_json::from_str(
            r#"{"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"vendor","adapter":"a","model":"image-v1","secret_service":"gongbu.vendor","secret_account":"local"}]}"#,
        ).unwrap();
        let target = catalog
            .resolve("image_generation", "vendor", "a", "image-v1")
            .unwrap();
        let adapter = Adapter(AtomicUsize::new(0));
        assert!(
            matches!(preflight_selected_secret(&adapter, &Secrets(false), target, &request()), Err(ContractError::Provider { code }) if code == "secret_unavailable")
        );
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0);
        let secret =
            preflight_selected_secret(&adapter, &Secrets(true), target, &request()).unwrap();
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0);
        adapter
            .invoke(&request(), &serde_json::json!({}), &secret, None)
            .unwrap();
        assert_eq!(adapter.0.load(Ordering::SeqCst), 1);

        let mut wrong_request = request();
        wrong_request.provider = "other".into();
        assert!(
            matches!(preflight_selected_secret(&adapter, &Secrets(true), target, &wrong_request), Err(ContractError::Provider { code }) if code == "target_mismatch")
        );
    }
}
