//! Frozen operator pricing and the normalized provider billing boundary.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

use crate::{
    provider_targets::{ProviderConfigVersion, TargetKey},
    secrets::{resolve_selected, ProviderSecret, SecretProvider},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// New snapshots freeze their estimate in micro-units (schema 3). Schema 2
/// snapshots created before v0.2.4 still load, validate and settle.
pub const PRICING_SNAPSHOT_SCHEMA_VERSION: i64 = 3;
pub const MAX_VENDOR_COST_SCALE: u32 = 18;
const SUPPORTED_CURRENCIES: &[&str] = &["USD"];
/// Hubu accounts budgets in micro-units: 10^-6 of the currency.
pub const UNITS_SCALE: u32 = 6;
/// Micro-units per catalog minor unit (cent).
pub const UNITS_PER_MINOR: i64 = 10_000;

/// The whole-cent projection of a micro-unit amount, rounded up so a cent
/// reader never sees less committed spend than exists.
pub fn units_to_minor_ceil(units: i64) -> i64 {
    units.div_euclid(UNITS_PER_MINOR) + i64::from(units.rem_euclid(UNITS_PER_MINOR) != 0)
}

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

/// An exact external-vendor charge in decimal major-currency units.
///
/// `amount / 10^scale` is the exact value reported by the provider (or derived
/// from the frozen catalog). Budget accounting is deliberately a separate,
/// conservative conversion so a sub-cent charge is never lost.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActualVendorCost {
    pub amount: i64,
    pub scale: u32,
    pub currency: String,
}

impl ActualVendorCost {
    pub fn new(amount: i64, scale: u32, currency: impl AsRef<str>) -> Result<Self> {
        let currency = currency.as_ref().trim().to_ascii_uppercase();
        let value = Self {
            amount,
            scale,
            currency,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        if self.amount < 0
            || self.scale > MAX_VENDOR_COST_SCALE
            || self.currency.len() != 3
            || !self.currency.bytes().all(|byte| byte.is_ascii_uppercase())
        {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }

    /// Parse a JSON decimal lexeme in major currency units without converting
    /// through binary floating point.
    pub fn from_decimal_major_units(raw: &str, currency: &str) -> Result<Self> {
        Self::from_decimal_scaled_units(raw, currency, 0)
    }

    /// Parse a JSON decimal lexeme expressed in source units worth
    /// `10^-source_unit_scale` major currency units each.
    ///
    /// Applying the scale offset while parsing avoids an overflowing
    /// intermediate source-unit coefficient when the converted currency value
    /// itself still fits the exact representation.
    pub fn from_decimal_scaled_units(
        raw: &str,
        currency: &str,
        source_unit_scale: u32,
    ) -> Result<Self> {
        let (significand, exponent) = match raw.split_once(['e', 'E']) {
            Some((significand, exponent))
                if !significand.is_empty()
                    && !exponent.is_empty()
                    && !exponent.contains(['e', 'E']) =>
            {
                let exponent = exponent
                    .parse::<i32>()
                    .map_err(|_| ContractError::IndeterminableCost)?;
                (significand, exponent)
            }
            Some(_) => return Err(ContractError::IndeterminableCost),
            None => (raw, 0),
        };
        if significand.is_empty() || significand.starts_with(['-', '+']) {
            return Err(ContractError::IndeterminableCost);
        }
        let (whole, fraction) = match significand.split_once('.') {
            Some((whole, fraction))
                if !whole.is_empty() && !fraction.is_empty() && !fraction.contains('.') =>
            {
                (whole, fraction)
            }
            Some(_) => return Err(ContractError::IndeterminableCost),
            None => (significand, ""),
        };
        if !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ContractError::IndeterminableCost);
        }
        let digits = format!("{whole}{fraction}");
        let coefficient = digits
            .parse::<i128>()
            .map_err(|_| ContractError::IndeterminableCost)?;
        let source_unit_scale =
            i32::try_from(source_unit_scale).map_err(|_| ContractError::IndeterminableCost)?;
        let decimal_places = i32::try_from(fraction.len())
            .map_err(|_| ContractError::IndeterminableCost)?
            .checked_sub(exponent)
            .and_then(|places| places.checked_add(source_unit_scale))
            .ok_or(ContractError::IndeterminableCost)?;
        let (amount, scale) = if decimal_places <= 0 {
            let power =
                u32::try_from(-decimal_places).map_err(|_| ContractError::IndeterminableCost)?;
            let amount = coefficient
                .checked_mul(checked_pow10(power)?)
                .ok_or(ContractError::IndeterminableCost)?;
            (amount, 0)
        } else {
            let scale =
                u32::try_from(decimal_places).map_err(|_| ContractError::IndeterminableCost)?;
            if scale > MAX_VENDOR_COST_SCALE {
                return Err(ContractError::IndeterminableCost);
            }
            (coefficient, scale)
        };
        Self::new(
            i64::try_from(amount).map_err(|_| ContractError::IndeterminableCost)?,
            scale,
            currency,
        )
    }

    /// Convert to budget micro-units (scale 6) by ceiling once, exactly as
    /// Hubu charges the budget. Costs at scale 6 or coarser convert exactly;
    /// this never understates a positive external charge.
    pub fn to_budget_units(&self, budget_currency: &str) -> Result<i64> {
        self.validate()?;
        if !self.currency.eq_ignore_ascii_case(budget_currency) {
            return Err(ContractError::IndeterminableCost);
        }
        let amount = i128::from(self.amount);
        let units = if self.scale > UNITS_SCALE {
            let divisor = checked_pow10(self.scale - UNITS_SCALE)?;
            amount
                .checked_add(divisor - 1)
                .ok_or(ContractError::IndeterminableCost)?
                / divisor
        } else {
            amount
                .checked_mul(checked_pow10(UNITS_SCALE - self.scale)?)
                .ok_or(ContractError::IndeterminableCost)?
        };
        i64::try_from(units).map_err(|_| ContractError::IndeterminableCost)
    }
}

fn checked_pow10(power: u32) -> Result<i128> {
    10_i128
        .checked_pow(power)
        .ok_or(ContractError::IndeterminableCost)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreciseSettlement {
    pub actual_vendor_cost: ActualVendorCost,
    /// The budget charge in micro-units, as Hubu computes it.
    pub budget_units: i64,
}

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
    /// The workload this rule prices, so a model can carry separate
    /// generation and edit prices. A rule without one prices every workload
    /// of its model that no rule names; contracts read it as
    /// `image_generation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_type: Option<String>,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<PricingSelector>,
    pub components: Vec<PriceComponent>,
}

/// The workload a pricing rule or request without one belongs to.
pub const DEFAULT_WORKLOAD_TYPE: &str = "image_generation";

impl PricingRule {
    /// The workload a contract reads this rule as pricing.
    pub fn workload(&self) -> &str {
        self.workload_type
            .as_deref()
            .unwrap_or(DEFAULT_WORKLOAD_TYPE)
    }
}

impl FrozenCatalog {
    /// Whether `rule` prices `workload` of this model. A rule naming a
    /// workload prices only it; a rule without one prices the workloads of
    /// its model that no rule names, as every rule did before rules named
    /// workloads.
    fn rule_prices(&self, rule: &PricingRule, provider: &str, model: &str, workload: &str) -> bool {
        rule.provider == provider
            && rule.model == model
            && match &rule.workload_type {
                Some(named) => named == workload,
                None => !self.rules.iter().any(|other| {
                    other.provider == provider
                        && other.model == model
                        && other.workload_type.as_deref() == Some(workload)
                }),
            }
    }
}

/// Named dimensions that pick one pricing rule for a request, such as
/// `image_size`, `aspect_ratio` or `mode`. A rule's selector must equal the
/// request's selector exactly; a rule without one prices only requests that
/// carry no selector. Single-key `image_size` selectors serialize exactly as
/// the original struct did, so frozen catalogs and snapshots are unchanged.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct PricingSelector(BTreeMap<String, String>);

impl PricingSelector {
    pub fn for_image_size(image_size: impl Into<String>) -> Self {
        Self(BTreeMap::from([(
            IMAGE_SIZE_SELECTOR.to_owned(),
            image_size.into(),
        )]))
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    pub fn image_size(&self) -> Option<&str> {
        self.get(IMAGE_SIZE_SELECTOR)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.0
    }
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }
    /// The canonical form: keys are lowercase identifiers, values are trimmed
    /// and lowercased, and `image_size` names a supported preset.
    pub fn normalized(&self) -> Result<Self> {
        self.0
            .iter()
            .map(|(key, value)| {
                if !valid_selector_key(key) {
                    return Err(ContractError::Malformed("invalid pricing selector".into()));
                }
                let value = if key == IMAGE_SIZE_SELECTOR {
                    normalize_image_size(value)?
                } else {
                    normalize_selector_value(value)?
                };
                Ok((key.clone(), value))
            })
            .collect::<Result<_>>()
            .map(Self)
    }
}

pub const IMAGE_SIZE_SELECTOR: &str = "image_size";

fn valid_selector_key(key: &str) -> bool {
    let mut chars = key.chars();
    key.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn normalize_selector_value(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
    {
        return Err(ContractError::Malformed("invalid pricing selector".into()));
    }
    Ok(value)
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
    /// One per provider call, whatever the call returns.
    Request,
    Result,
    Page,
    Second,
    /// Every billed megapixel of every output image, as the pack's declared
    /// rounding derives it from the output dimensions.
    Megapixel,
    /// Billed megapixels beyond the first of each output image. With an
    /// `image` component for the first, it prices "first megapixel, then each
    /// additional one". It is the one unit whose quantity may be zero.
    AdditionalMegapixel,
    /// Each input image an edit request sends, priced where the provider
    /// charges for reading it.
    InputImage,
}

impl PricingUnit {
    /// The named request quantity this unit is priced against. A `request`
    /// component always prices exactly one call and has no named quantity.
    pub fn quantity_name(self) -> Option<&'static str> {
        match self {
            Self::Image => Some("image_count"),
            Self::InputToken => Some("input_tokens"),
            Self::OutputToken => Some("max_output_tokens"),
            Self::Request => None,
            Self::Result => Some("results"),
            Self::Page => Some("pages"),
            Self::Second => Some("seconds"),
            Self::Megapixel => Some("megapixels"),
            Self::AdditionalMegapixel => Some("additional_megapixels"),
            Self::InputImage => Some("input_images"),
        }
    }

    /// Whether a frozen quantity of zero is a real price. Only additional
    /// megapixels may be: a one-megapixel image has none.
    pub fn allows_zero(self) -> bool {
        self == Self::AdditionalMegapixel
    }
}

/// Every quantity a normalized request may carry.
pub const REQUEST_QUANTITIES: &[&str] = &[
    "image_count",
    "input_tokens",
    "max_output_tokens",
    "results",
    "pages",
    "seconds",
    "megapixels",
    "additional_megapixels",
    "input_images",
];

/// Request quantities a pack derives from the output dimensions; no input
/// parameter may set them.
pub const DERIVED_QUANTITIES: &[&str] = &["megapixels", "additional_megapixels"];

#[derive(Clone, Debug)]
pub struct PricingCatalog(Arc<FrozenCatalog>);

#[derive(Debug)]
struct FrozenCatalog {
    version: String,
    digest: String,
    rules: Vec<PricingRule>,
}

impl PricingCatalog {
    pub(crate) fn disabled() -> Self {
        Self(Arc::new(FrozenCatalog {
            version: "disabled".into(),
            digest: "disabled".into(),
            rules: Vec::new(),
        }))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path).map_err(|e| ContractError::Io(e.to_string()))?;
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let mut doc: CatalogDocument =
            serde_json::from_slice(bytes).map_err(|e| ContractError::Malformed(e.to_string()))?;
        if doc.schema_version != 2 || doc.catalog_version.trim().is_empty() || doc.rules.is_empty()
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
            if let Some(workload) = &rule.workload_type {
                rule.workload_type = Some(normalized(workload, "workload_type")?);
            }
            rule.currency = rule.currency.trim().to_ascii_uppercase();
            if let Some(selector) = &mut rule.selector {
                if selector.is_empty() {
                    return Err(ContractError::Malformed("empty pricing selector".into()));
                }
                *selector = selector.normalized()?;
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
            if rule.components.is_empty() {
                return Err(ContractError::Malformed("invalid v2 rule shape".into()));
            }
            if rule
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
            // An image-size rule prices output images, plus the input images
            // an edit sends.
            if rule
                .selector
                .as_ref()
                .is_some_and(|selector| selector.image_size().is_some())
                && (rule.components[0].unit != PricingUnit::Image
                    || rule.components[1..]
                        .iter()
                        .any(|c| c.unit != PricingUnit::InputImage))
            {
                return Err(ContractError::Malformed(
                    "image selector requires one image component".into(),
                ));
            }
            let key = (
                rule.provider.clone(),
                rule.model.clone(),
                rule.workload_type.clone(),
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
                    .filter(|other| {
                        other.provider == rule.provider
                            && other.model == rule.model
                            && other.workload_type == rule.workload_type
                    })
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
                self.0
                    .rule_prices(rule, &request.provider, &request.model, request.workload())
                    && match &rule.selector {
                        Some(selector) => *selector == request.selector,
                        None => request.selector.is_empty(),
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
            self.0
                .rule_prices(r, &request.provider, &request.model, request.workload())
                && r.selector.is_some()
        }) && rule.selector.is_none()
        {
            return Err(ContractError::IndeterminableCost);
        }
        let mut exact = Rational::zero();
        let mut frozen = Vec::new();
        for component in &rule.components {
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
        let estimated_units = exact.to_units_ceil()?;
        Ok(PricingSnapshot {
            schema_version: 3,
            provider: request.provider.clone(),
            model: request.model.clone(),
            catalog_version: self.0.version.clone(),
            catalog_digest: self.0.digest.clone(),
            pricing_rule_id: rule.rule_id.clone(),
            selector: rule.selector.clone(),
            output_dimensions: request.output_dimensions.clone(),
            components: frozen,
            exact_estimate_numerator: exact.n.to_string(),
            exact_estimate_denominator: exact.d.to_string(),
            estimated_units,
            currency: rule.currency.clone(),
        })
    }

    /// Price only a request already bound to a validated provider target key.
    pub fn snapshot_for_target(
        &self,
        target: &TargetKey,
        request: &NormalizedRequest,
    ) -> Result<PricingSnapshot> {
        if request.provider != target.provider
            || request.model != target.model
            || request.workload() != target.workload_type
        {
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
            .any(|rule| self.matches_target(rule, target))
    }

    /// Whether this target has an exact, selector-qualified image pricing rule.
    pub fn supports_image_size(&self, target: &TargetKey, image_size: &str) -> bool {
        self.0.rules.iter().any(|rule| {
            self.matches_target(rule, target)
                && rule
                    .selector
                    .as_ref()
                    .is_some_and(|selector| selector.image_size() == Some(image_size))
        })
    }

    pub fn catalog_version(&self) -> &str {
        &self.0.version
    }

    pub(crate) fn rules(&self) -> &[PricingRule] {
        &self.0.rules
    }

    /// Sanitized pricing rules that make the runtime choices for one target discoverable.
    pub fn rules_for_target(&self, target: &TargetKey) -> Vec<PricingRule> {
        self.0
            .rules
            .iter()
            .filter(|rule| self.matches_target(rule, target))
            .cloned()
            .collect()
    }

    fn matches_target(&self, rule: &PricingRule, target: &TargetKey) -> bool {
        self.0
            .rule_prices(rule, &target.provider, &target.model, &target.workload_type)
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

/// The priced shape of one provider call: named, bounded quantities (see
/// [`REQUEST_QUANTITIES`]) and the selector dimensions that pick a rule.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedRequest {
    pub provider: String,
    pub model: String,
    /// The workload priced; absent means `image_generation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quantities: BTreeMap<String, i64>,
    #[serde(default, skip_serializing_if = "PricingSelector::is_empty")]
    pub selector: PricingSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dimensions: Option<OutputDimensions>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutputDimensions {
    pub width: u32,
    pub height: u32,
}

impl NormalizedRequest {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            workload_type: None,
            quantities: BTreeMap::new(),
            selector: PricingSelector::default(),
            output_dimensions: None,
        }
    }
    /// Price this request as `workload`; the default workload is left unset.
    pub fn with_workload(mut self, workload: &str) -> Self {
        self.workload_type = (workload != DEFAULT_WORKLOAD_TYPE).then(|| workload.to_owned());
        self
    }
    pub fn workload(&self) -> &str {
        self.workload_type
            .as_deref()
            .unwrap_or(DEFAULT_WORKLOAD_TYPE)
    }
    pub fn with_quantity(mut self, name: &str, value: Option<i64>) -> Self {
        self.set_quantity(name, value);
        self
    }
    pub fn with_selector(mut self, key: &str, value: Option<String>) -> Self {
        if let Some(value) = value {
            self.selector.insert(key, value);
        }
        self
    }
    pub fn with_output_dimensions(mut self, dimensions: Option<OutputDimensions>) -> Self {
        self.output_dimensions = dimensions;
        self
    }
    pub fn set_quantity(&mut self, name: &str, value: Option<i64>) {
        match value {
            Some(value) => self.quantities.insert(name.to_owned(), value),
            None => self.quantities.remove(name),
        };
    }
    pub fn quantity(&self, name: &str) -> Option<i64> {
        self.quantities.get(name).copied()
    }
    pub fn image_count(&self) -> Option<i64> {
        self.quantity("image_count")
    }
    pub fn image_size(&self) -> Option<&str> {
        self.selector.image_size()
    }
    pub fn validate(&self) -> Result<()> {
        if self.provider.trim().is_empty() || self.model.trim().is_empty() {
            return Err(ContractError::IndeterminableCost);
        }
        if self
            .quantities
            .iter()
            .any(|(name, value)| !REQUEST_QUANTITIES.contains(&name.as_str()) || *value < 0)
        {
            return Err(ContractError::IndeterminableCost);
        }
        if self.selector.normalized().ok().as_ref() != Some(&self.selector) {
            return Err(ContractError::IndeterminableCost);
        }
        if self
            .output_dimensions
            .as_ref()
            .is_some_and(|dimensions| dimensions.width == 0 || dimensions.height == 0)
        {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }
    fn quantity_for(&self, unit: PricingUnit) -> Result<i64> {
        match unit.quantity_name() {
            None => Ok(1),
            Some(name) => self
                .quantity(name)
                .filter(|v| *v > 0 || (*v == 0 && unit.allows_zero()))
                .ok_or(ContractError::IndeterminableCost),
        }
    }
}

pub fn validate_image_size_input(
    request: &NormalizedRequest,
    input: &serde_json::Value,
) -> Result<()> {
    let supplied = input.get("image_size").and_then(serde_json::Value::as_str);
    if supplied != request.image_size() {
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
            .is_some_and(|count| Some(count) != request.image_count())
        || typed.image_size.as_deref() != request.image_size()
    {
        return Err(ContractError::IndeterminableCost);
    }
    let _ = typed.options;
    Ok(())
}

pub fn validate_image_input(request: &NormalizedRequest, input: &serde_json::Value) -> Result<()> {
    validate_image_input_versioned(request, input, 1)
}

/// A frozen price estimate. Schema 3 records the estimate in micro-units
/// (`estimated_units`). Schema 2 snapshots, frozen before v0.2.4, keep their
/// whole-cent `estimated_amount_minor` on the wire byte for byte so their
/// receipts and retries never change; in memory both carry micro-units.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "PricingSnapshotWire", into = "PricingSnapshotWire")]
pub struct PricingSnapshot {
    pub schema_version: u32,
    pub provider: String,
    pub model: String,
    pub catalog_version: String,
    pub catalog_digest: String,
    pub pricing_rule_id: String,
    pub selector: Option<PricingSelector>,
    pub output_dimensions: Option<OutputDimensions>,
    pub components: Vec<FrozenPriceComponent>,
    pub exact_estimate_numerator: String,
    pub exact_estimate_denominator: String,
    /// The exact estimate rounded up to micro-units.
    pub estimated_units: i64,
    pub currency: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PricingSnapshotWire {
    schema_version: u32,
    provider: String,
    model: String,
    catalog_version: String,
    catalog_digest: String,
    pricing_rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selector: Option<PricingSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_dimensions: Option<OutputDimensions>,
    components: Vec<FrozenPriceComponent>,
    exact_estimate_numerator: String,
    exact_estimate_denominator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimated_amount_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimated_units: Option<i64>,
    currency: String,
}

impl TryFrom<PricingSnapshotWire> for PricingSnapshot {
    type Error = String;

    fn try_from(wire: PricingSnapshotWire) -> std::result::Result<Self, Self::Error> {
        let estimated_units = match (
            wire.schema_version,
            wire.estimated_amount_minor,
            wire.estimated_units,
        ) {
            (2, Some(minor), None) => minor
                .checked_mul(UNITS_PER_MINOR)
                .ok_or("schema 2 pricing estimate is out of range")?,
            (3, None, Some(units)) => units,
            (2, _, _) => {
                return Err("schema 2 pricing snapshots carry estimated_amount_minor".into())
            }
            (3, _, _) => return Err("schema 3 pricing snapshots carry estimated_units".into()),
            (version, _, _) => {
                return Err(format!("unsupported pricing snapshot schema {version}"))
            }
        };
        Ok(Self {
            schema_version: wire.schema_version,
            provider: wire.provider,
            model: wire.model,
            catalog_version: wire.catalog_version,
            catalog_digest: wire.catalog_digest,
            pricing_rule_id: wire.pricing_rule_id,
            selector: wire.selector,
            output_dimensions: wire.output_dimensions,
            components: wire.components,
            exact_estimate_numerator: wire.exact_estimate_numerator,
            exact_estimate_denominator: wire.exact_estimate_denominator,
            estimated_units,
            currency: wire.currency,
        })
    }
}

impl From<PricingSnapshot> for PricingSnapshotWire {
    fn from(snapshot: PricingSnapshot) -> Self {
        let (estimated_amount_minor, estimated_units) = if snapshot.schema_version == 2 {
            (Some(snapshot.estimated_units / UNITS_PER_MINOR), None)
        } else {
            (None, Some(snapshot.estimated_units))
        };
        Self {
            schema_version: snapshot.schema_version,
            provider: snapshot.provider,
            model: snapshot.model,
            catalog_version: snapshot.catalog_version,
            catalog_digest: snapshot.catalog_digest,
            pricing_rule_id: snapshot.pricing_rule_id,
            selector: snapshot.selector,
            output_dimensions: snapshot.output_dimensions,
            components: snapshot.components,
            exact_estimate_numerator: snapshot.exact_estimate_numerator,
            exact_estimate_denominator: snapshot.exact_estimate_denominator,
            estimated_amount_minor,
            estimated_units,
            currency: snapshot.currency,
        }
    }
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
    /// A minor-unit (cent) rational rounded up once to micro-units.
    fn to_units_ceil(self) -> Result<i64> {
        Self {
            n: self
                .n
                .checked_mul(i128::from(UNITS_PER_MINOR))
                .ok_or(ContractError::IndeterminableCost)?,
            d: self.d,
        }
        .ceil_i64()
    }
    fn to_actual_vendor_cost(self, currency: &str) -> Result<ActualVendorCost> {
        let major_denominator = self
            .d
            .checked_mul(100)
            .ok_or(ContractError::IndeterminableCost)?;
        let divisor = gcd(self.n, major_denominator);
        let numerator = self.n / divisor;
        let mut denominator = major_denominator / divisor;
        let mut twos = 0_u32;
        let mut fives = 0_u32;
        while denominator % 2 == 0 {
            denominator /= 2;
            twos += 1;
        }
        while denominator % 5 == 0 {
            denominator /= 5;
            fives += 1;
        }
        if denominator != 1 {
            return Err(ContractError::IndeterminableCost);
        }
        let scale = twos.max(fives);
        if scale > MAX_VENDOR_COST_SCALE {
            return Err(ContractError::IndeterminableCost);
        }
        let decimal_denominator = checked_pow10(scale)?;
        let amount = numerator
            .checked_mul(decimal_denominator / (major_denominator / divisor))
            .ok_or(ContractError::IndeterminableCost)?;
        ActualVendorCost::new(
            i64::try_from(amount).map_err(|_| ContractError::IndeterminableCost)?,
            scale,
            currency,
        )
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
        matches!(self.schema_version, 2 | 3)
            && self.components.len() == 1
            && self.components[0].unit == PricingUnit::Image
    }
    pub fn has_unit(&self, unit: PricingUnit) -> bool {
        self.components
            .iter()
            .any(|component| component.unit == unit)
    }
    pub fn estimated_quantity(&self, unit: PricingUnit) -> Option<i64> {
        self.components
            .iter()
            .find(|component| component.unit == unit)
            .map(|component| component.quantity)
    }
    pub fn validate_integrity(&self) -> Result<()> {
        let digest = self.catalog_digest.strip_prefix("sha256:");
        if !matches!(self.schema_version, 2 | 3)
            || self.provider.trim().is_empty()
            || self.model.trim().is_empty()
            || self.catalog_version.trim().is_empty()
            || self.pricing_rule_id.trim().is_empty()
            || digest.is_none_or(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || !SUPPORTED_CURRENCIES.contains(&self.currency.as_str())
            || self.estimated_units < 0
        {
            return Err(ContractError::IndeterminableCost);
        }
        if self.selector.as_ref().is_some_and(|selector| {
            selector.is_empty() || selector.normalized().ok().as_ref() != Some(selector)
        }) {
            return Err(ContractError::IndeterminableCost);
        }
        let image_size = self.selector.as_ref().and_then(PricingSelector::image_size);
        if self
            .output_dimensions
            .as_ref()
            .is_some_and(|dimensions| dimensions.width == 0 || dimensions.height == 0)
        {
            return Err(ContractError::IndeterminableCost);
        }
        let components = self.effective_components()?;
        let mut units = BTreeSet::new();
        if components.iter().any(|component| {
            component.rate_numerator_minor < 0
                || component.rate_denominator <= 0
                || component.quantity < 0
                || (component.quantity == 0 && !component.unit.allows_zero())
                || !units.insert(component.unit)
        }) || (image_size.is_some()
            && (components[0].unit != PricingUnit::Image
                || components[1..]
                    .iter()
                    .any(|component| component.unit != PricingUnit::InputImage)))
        {
            return Err(ContractError::IndeterminableCost);
        }
        let exact = self.exact_estimate()?;
        // Schema 2 froze a whole-cent estimate; schema 3 freezes micro-units.
        let expected = if self.schema_version == 2 {
            exact
                .ceil_i64()?
                .checked_mul(UNITS_PER_MINOR)
                .ok_or(ContractError::IndeterminableCost)?
        } else {
            exact.to_units_ceil()?
        };
        if expected != self.estimated_units {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }

    /// `authorized_units` is the Hubu authorization in micro-units.
    pub fn check_authorization(&self, authorized_units: i64, currency: &str) -> Result<()> {
        self.validate_integrity()?;
        if authorized_units < 0
            || !self.currency.eq_ignore_ascii_case(currency)
            || self.estimated_units > authorized_units
        {
            return Err(ContractError::InsufficientAuthorization);
        }
        Ok(())
    }
    pub fn settle_precise(
        &self,
        usage: &NormalizedUsage,
        provider_cost: Option<&ActualVendorCost>,
        authorized_units: i64,
    ) -> Result<PreciseSettlement> {
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
        let actual_vendor_cost = match provider_cost {
            Some(cost) => {
                cost.validate()?;
                if cost.currency != self.currency {
                    return Err(ContractError::IndeterminableCost);
                }
                cost.clone()
            }
            None => exact.to_actual_vendor_cost(&self.currency)?,
        };
        let units = actual_vendor_cost.to_budget_units(&self.currency)?;
        if units > self.estimated_units || units > authorized_units {
            return Err(ContractError::SettlementOverage);
        }
        Ok(PreciseSettlement {
            actual_vendor_cost,
            budget_units: units,
        })
    }

    /// The budget charge in micro-units for `usage`.
    pub fn settle(&self, usage: &NormalizedUsage, authorized_units: i64) -> Result<i64> {
        Ok(self
            .settle_precise(usage, None, authorized_units)?
            .budget_units)
    }

    fn effective_components(&self) -> Result<Vec<FrozenPriceComponent>> {
        if matches!(self.schema_version, 2 | 3) && !self.components.is_empty() {
            Ok(self.components.clone())
        } else {
            Err(ContractError::IndeterminableCost)
        }
    }
    fn exact_estimate(&self) -> Result<Rational> {
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
            recomputed = recomputed
                .add(Rational::new(c.rate_numerator_minor, c.rate_denominator)?.mul(c.quantity)?)?;
        }
        if recomputed != (Rational { n, d }) {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(recomputed)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedUsage {
    pub images: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
    /// Billed megapixels of the delivered images, from their decoded size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub megapixels: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_megapixels: Option<i64>,
    /// Input images the request sent to the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_images: Option<i64>,
}
impl NormalizedUsage {
    fn quantity_for(&self, unit: PricingUnit) -> Option<i64> {
        match unit {
            PricingUnit::Image => self.images,
            PricingUnit::InputToken => self.input_tokens,
            PricingUnit::OutputToken => self.output_tokens,
            // A settled call is exactly one request.
            PricingUnit::Request => Some(1),
            PricingUnit::Result => self.results,
            PricingUnit::Page => self.pages,
            PricingUnit::Second => self.seconds,
            PricingUnit::Megapixel => self.megapixels,
            PricingUnit::AdditionalMegapixel => self.additional_megapixels,
            PricingUnit::InputImage => self.input_images,
        }
        .filter(|v| *v >= 0)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AdapterOutcome {
    pub usage: Option<NormalizedUsage>,
    pub actual_vendor_cost: Option<ActualVendorCost>,
    pub provider_request_id: Option<String>,
    pub provider_operation_id: Option<String>,
    pub artifacts: Vec<NormalizedArtifact>,
}

/// A provider-boundary call whose durable count is part of operator evidence.
/// Observers are called immediately before entering the transport method; a
/// rejected observation must prevent that transport call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderTransportInteraction {
    Poll,
    ArtifactFetch,
}

pub trait ProviderTransportObserver: Send + Sync {
    fn record(&self, interaction: ProviderTransportInteraction) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopProviderTransportObserver;

impl ProviderTransportObserver for NoopProviderTransportObserver {
    fn record(&self, _interaction: ProviderTransportInteraction) -> bool {
        true
    }
}

/// Safe, resumable state for a provider operation that was accepted but did
/// not complete in the submission response. This is deliberately an allowlist:
/// raw provider bodies, credentials, signed URLs, and artifact storage paths
/// have no representation here.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PollingRecoveryContext {
    pub schema_version: u32,
    pub policy_version: String,
    pub scheme: Option<String>,
    pub normalized_host: Option<String>,
    pub explicit_port: Option<u16>,
    pub endpoint_shape: String,
    pub query_keys: Vec<String>,
    pub url_fingerprint: String,
    pub validation_reason: Option<String>,
}

/// Minimum remaining time required before recovery may reopen provider
/// polling. FLUX sleeps before each status GET, so a shorter window is
/// guaranteed to expire without observing the operation.
pub const POLLING_RECOVERY_MIN_REINSPECT_WINDOW_MS: i64 = 1_000;

pub fn polling_deadline_allows_reinspect(deadline_unix_ms: i64, now_unix_ms: i64) -> bool {
    deadline_unix_ms
        .checked_sub(now_unix_ms)
        .is_some_and(|remaining| remaining >= POLLING_RECOVERY_MIN_REINSPECT_WINDOW_MS)
}

impl PollingRecoveryContext {
    pub fn validate(&self) -> Result<()> {
        let safe_token = |value: &str, max: usize| {
            !value.is_empty()
                && value.len() <= max
                && value.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
                })
        };
        if self.schema_version != 1
            || !safe_token(&self.policy_version, 64)
            || self
                .scheme
                .as_deref()
                .is_some_and(|value| !safe_token(value, 16))
            || self
                .normalized_host
                .as_deref()
                .is_some_and(|value| !safe_token(value, 253))
            || !safe_token(&self.endpoint_shape, 64)
            || self.query_keys.len() > 8
            || self.query_keys.iter().any(|value| !safe_token(value, 64))
            || self.url_fingerprint.len() != 71
            || !self.url_fingerprint.starts_with("sha256:")
            || !self.url_fingerprint[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || self
                .validation_reason
                .as_deref()
                .is_some_and(|value| !safe_token(value, 64))
        {
            return Err(ContractError::Provider {
                code: "invalid_provider_operation".into(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AsyncProviderOperation {
    pub provider_request_id: Option<String>,
    pub provider_operation_id: String,
    /// Validated hostname only. The adapter reconstructs the documented poll
    /// endpoint from this host, its frozen API version, and the operation ID.
    pub polling_host: String,
    /// Sanitized evidence about the provider-returned polling URL. This is
    /// persisted even when the origin is rejected after a possibly billable
    /// submission; it never grants authority to perform network access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling_recovery: Option<PollingRecoveryContext>,
    /// Absolute Unix epoch deadline shared by submission, polling, and artifact
    /// download. Resuming an operation never resets this budget.
    pub deadline_unix_ms: i64,
}

impl AsyncProviderOperation {
    pub fn validate(&self) -> Result<()> {
        fn safe_evidence(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 255
                && value.trim() == value
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
                })
        }

        if !safe_evidence(&self.provider_operation_id)
            || self
                .provider_request_id
                .as_deref()
                .is_some_and(|value| !safe_evidence(value))
            || self.polling_host.is_empty()
            || self.polling_host.len() > 253
            || self.polling_host.bytes().any(|byte| {
                !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.'))
            })
            || self.polling_host.starts_with(['-', '.'])
            || self.polling_host.ends_with(['-', '.'])
            || self.polling_host.contains("..")
            || self.deadline_unix_ms <= 0
            || self
                .polling_recovery
                .as_ref()
                .is_some_and(|context| context.validate().is_err())
        {
            return Err(ContractError::Provider {
                code: "invalid_provider_operation".into(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterSubmission {
    Complete(AdapterOutcome),
    Pending(AsyncProviderOperation),
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NormalizedArtifact {
    pub media_type: String,
    pub bytes: Vec<u8>,
}
impl AdapterOutcome {
    pub fn validate(&self) -> Result<()> {
        if self
            .actual_vendor_cost
            .as_ref()
            .is_some_and(|cost| cost.validate().is_err())
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

/// The failure code for an input image that can't be loaded. Nothing has
/// been sent when it is returned, so the claim is released.
pub const INPUT_ARTIFACT_UNAVAILABLE: &str = "input_artifact_unavailable";

/// One input image an edit request sends: the verified bytes of an artifact
/// an earlier governed execution delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputImage {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Loads input images by artifact ID, only from the executing account.
pub trait InputImageSource {
    fn load(&self, artifact_id: &str) -> Result<InputImage>;
}

/// The source for a call that may not send input images.
pub struct NoInputImages;

impl InputImageSource for NoInputImages {
    fn load(&self, _artifact_id: &str) -> Result<InputImage> {
        Err(ContractError::Provider {
            code: INPUT_ARTIFACT_UNAVAILABLE.into(),
        })
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
    fn invoke_observed(
        &self,
        request: &NormalizedRequest,
        normalized_input: &serde_json::Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        _observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.invoke(request, normalized_input, secret, vendor_idempotency_key)
    }
    /// Submit provider work once. Synchronous adapters retain their existing
    /// behavior through this default implementation.
    fn submit(
        &self,
        request: &NormalizedRequest,
        normalized_input: &serde_json::Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        self.invoke(request, normalized_input, secret, vendor_idempotency_key)
            .map(AdapterSubmission::Complete)
    }
    fn submit_observed(
        &self,
        request: &NormalizedRequest,
        normalized_input: &serde_json::Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        _observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        self.submit(request, normalized_input, secret, vendor_idempotency_key)
    }
    /// Submit with the input images an edit request references. Adapters
    /// without image inputs ignore the source.
    fn submit_with_inputs(
        &self,
        request: &NormalizedRequest,
        normalized_input: &serde_json::Value,
        _inputs: &dyn InputImageSource,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        self.submit_observed(
            request,
            normalized_input,
            secret,
            vendor_idempotency_key,
            observer,
        )
    }
    /// Poll a previously checkpointed asynchronous operation. The default is
    /// fail-closed because synchronous adapters never create such checkpoints.
    fn poll(
        &self,
        _request: &NormalizedRequest,
        _normalized_input: &serde_json::Value,
        _secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        Err(
            ProviderFailure::reconcile("provider_resume_unsupported", ProviderPhase::Processing)
                .with_evidence(
                    operation.provider_request_id.clone(),
                    Some(operation.provider_operation_id.clone()),
                ),
        )
    }
    fn poll_observed(
        &self,
        request: &NormalizedRequest,
        normalized_input: &serde_json::Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
        _observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.poll(request, normalized_input, secret, operation)
    }
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
    authorized_units: i64,
    currency: &str,
) -> Result<()> {
    snapshot.check_authorization(authorized_units, currency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const CATALOG: &str = r#"{"schema_version":2,"catalog_version":"2026-08-05","rules":[{"rule_id":"image-standard","provider":"vendor","model":"image-v1","currency":"usd","components":[{"unit":"image","rate_numerator_minor":125,"rate_denominator":1}]}]}"#;
    fn request() -> NormalizedRequest {
        NormalizedRequest::new("vendor", "image-v1").with_quantity("image_count", Some(2))
    }
    #[test]
    fn exact_decimal_cost_converts_to_micro_units_and_rounds_only_beyond_scale_six() {
        // Scale 6 or coarser converts exactly: a sub-cent charge is no
        // longer rounded up to a whole cent.
        for (amount, scale, expected_units) in [
            (1, 3, 1_000),
            (10, 3, 10_000),
            (11, 3, 11_000),
            (1, 4, 100),
            (58, 3, 58_000),
            (3, 2, 30_000),
            (1, 6, 1),
        ] {
            let cost = ActualVendorCost::new(amount, scale, "USD").unwrap();
            assert_eq!(cost.to_budget_units("USD").unwrap(), expected_units);
        }
        assert_eq!(
            ActualVendorCost::new(1, 7, "USD")
                .unwrap()
                .to_budget_units("USD")
                .unwrap(),
            1,
            "only digits beyond scale 6 round up, to one micro-unit"
        );
        assert_eq!(units_to_minor_ceil(1), 1);
        assert_eq!(units_to_minor_ceil(10_000), 1);
        assert_eq!(units_to_minor_ceil(10_001), 2);
        assert_eq!(units_to_minor_ceil(0), 0);
    }

    #[test]
    fn exact_decimal_parser_preserves_scale_and_rejects_float_style_loss() {
        assert_eq!(
            ActualVendorCost::from_decimal_major_units("0.0100", "usd").unwrap(),
            ActualVendorCost::new(100, 4, "USD").unwrap()
        );
        assert_eq!(
            ActualVendorCost::from_decimal_major_units("1e-4", "USD").unwrap(),
            ActualVendorCost::new(1, 4, "USD").unwrap()
        );
        assert!(ActualVendorCost::from_decimal_major_units("-0.01", "USD").is_err());
        assert!(
            ActualVendorCost::from_decimal_major_units("0.0000000000000000001", "USD").is_err()
        );
    }

    #[test]
    fn exact_scaled_unit_parser_applies_offset_before_checked_bounds() {
        assert_eq!(
            ActualVendorCost::from_decimal_scaled_units("1.4", "USD", 2).unwrap(),
            ActualVendorCost::new(14, 3, "USD").unwrap()
        );
        assert_eq!(
            ActualVendorCost::from_decimal_scaled_units("0.0100", "USD", 2).unwrap(),
            ActualVendorCost::new(100, 6, "USD").unwrap()
        );
        assert_eq!(
            ActualVendorCost::from_decimal_scaled_units("100e18", "USD", 2).unwrap(),
            ActualVendorCost::new(1_000_000_000_000_000_000, 0, "USD").unwrap(),
            "conversion must happen before the source-unit coefficient could overflow i64"
        );
        assert_eq!(
            ActualVendorCost::from_decimal_scaled_units("9223372036854775807e2", "USD", 2).unwrap(),
            ActualVendorCost::new(i64::MAX, 0, "USD").unwrap()
        );
        assert!(
            ActualVendorCost::from_decimal_scaled_units("9223372036854775808e2", "USD", 2).is_err()
        );
        assert_eq!(
            ActualVendorCost::from_decimal_scaled_units("0.0000000000000001", "USD", 2).unwrap(),
            ActualVendorCost::new(1, MAX_VENDOR_COST_SCALE, "USD").unwrap()
        );
        assert!(
            ActualVendorCost::from_decimal_scaled_units("0.00000000000000001", "USD", 2).is_err()
        );
    }

    #[test]
    fn provider_exact_cost_is_authoritative_at_and_over_the_maximum() {
        let snapshot = PricingCatalog::from_json(CATALOG.as_bytes())
            .unwrap()
            .snapshot(&request())
            .unwrap();
        let usage = NormalizedUsage {
            images: Some(2),
            ..Default::default()
        };
        let exact_max = ActualVendorCost::new(250, 2, "USD").unwrap();
        let settlement = snapshot
            .settle_precise(&usage, Some(&exact_max), 250_0000)
            .unwrap();
        assert_eq!(settlement.actual_vendor_cost, exact_max);
        assert_eq!(settlement.budget_units, 250_0000);
        assert_eq!(
            snapshot.settle_precise(
                &usage,
                Some(&ActualVendorCost::new(25001, 4, "USD").unwrap()),
                250_0000
            ),
            Err(ContractError::SettlementOverage)
        );
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
        assert_eq!(s.estimated_units, 250_0000);

        let spaced = r#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":" z","provider":" p","model":" z","currency":" usd ","components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]},{"rule_id":"a","provider":"p","model":"a","currency":"USD","components":[{"unit":"image","rate_numerator_minor":2,"rate_denominator":1}]}]}"#;
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
        let duplicate = br#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":"image-standard","provider":"vendor","model":"image-v1","currency":"USD","components":[{"unit":"image","rate_numerator_minor":125,"rate_denominator":1}]},{"rule_id":"other","provider":"vendor","model":"image-v1","currency":"USD","components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]}]}"#;
        assert_eq!(
            PricingCatalog::from_json(duplicate).unwrap_err(),
            ContractError::AmbiguousRule
        );
        let duplicate_id = br#"{"schema_version":2,"catalog_version":"v2","rules":[{"rule_id":"image-standard","provider":"vendor","model":"image-v1","currency":"USD","components":[{"unit":"image","rate_numerator_minor":125,"rate_denominator":1}]},{"rule_id":"image-standard","provider":"other","model":"other","currency":"USD","components":[{"unit":"image","rate_numerator_minor":1,"rate_denominator":1}]}]}"#;
        assert!(
            matches!(PricingCatalog::from_json(duplicate_id), Err(ContractError::Malformed(message)) if message.contains("duplicate rule_id"))
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
        assert_eq!(s.settle(&usage, 250_0000).unwrap(), 250_0000);
        assert_eq!(
            s.settle(
                &NormalizedUsage {
                    images: Some(3),
                    ..Default::default()
                },
                250_0000
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
        request.selector = PricingSelector::for_image_size("2k");
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.selector.unwrap().image_size(), Some("2k"));
        assert_eq!(snapshot.estimated_units, 40_0000);
        request.selector = PricingSelector::default();
        assert_eq!(
            catalog.snapshot(&request),
            Err(ContractError::UnsupportedTarget)
        );

        let flat = PricingCatalog::from_json(CATALOG.as_bytes()).unwrap();
        request.model = "image-v1".into();
        request.selector = PricingSelector::for_image_size("4k");
        assert_eq!(
            flat.snapshot(&request),
            Err(ContractError::UnsupportedTarget)
        );
    }

    #[test]
    fn flux_preset_rule_and_exact_dimensions_round_trip_as_one_frozen_contract() {
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"flux-v1","rules":[{"rule_id":"flux-1k","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":10,"rate_denominator":1}]},{"rule_id":"flux-2k","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"2k"},"components":[{"unit":"image","rate_numerator_minor":20,"rate_denominator":1}]},{"rule_id":"flux-4k","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"4k"},"components":[{"unit":"image","rate_numerator_minor":40,"rate_denominator":1}]}]}"#).unwrap();
        for (preset, width, height, rule_id, amount) in [
            ("1k", 1024, 1024, "flux-1k", 10),
            ("2k", 1920, 1088, "flux-2k", 20),
            ("4k", 2048, 2048, "flux-4k", 40),
        ] {
            let request = NormalizedRequest::new("flux", "flux-2-pro")
                .with_quantity("image_count", Some(1))
                .with_selector("image_size", Some(preset.into()))
                .with_output_dimensions(Some(OutputDimensions { width, height }));
            let snapshot = catalog.snapshot(&request).unwrap();
            assert_eq!(snapshot.pricing_rule_id, rule_id);
            assert_eq!(snapshot.estimated_units, amount * 10_000);
            assert_eq!(
                snapshot
                    .selector
                    .as_ref()
                    .and_then(|selector| selector.image_size()),
                Some(preset)
            );
            assert_eq!(
                snapshot.output_dimensions,
                Some(OutputDimensions { width, height })
            );
            let replayed_request: NormalizedRequest =
                serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
            let replayed_snapshot: PricingSnapshot =
                serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
            assert_eq!(replayed_request, request);
            assert_eq!(replayed_snapshot, snapshot);
            replayed_snapshot.validate_integrity().unwrap();
        }

        let incomplete = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"flux-incomplete","rules":[{"rule_id":"flux-1k","provider":"flux","model":"flux-2-pro","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":10,"rate_denominator":1}]}]}"#).unwrap();
        let missing_rule = NormalizedRequest::new("flux", "flux-2-pro")
            .with_quantity("image_count", Some(1))
            .with_selector("image_size", Some("2k".into()))
            .with_output_dimensions(Some(OutputDimensions {
                width: 1920,
                height: 1088,
            }));
        assert_eq!(
            incomplete.snapshot(&missing_rule),
            Err(ContractError::UnsupportedTarget)
        );
    }

    #[test]
    fn pricing_v1_catalogs_and_snapshots_are_rejected() {
        let legacy_catalog = br#"{"schema_version":1,"catalog_version":"v1","rules":[{"rule_id":"image","provider":"vendor","model":"image-v1","currency":"USD","unit":"image","unit_amount_minor":125}]}"#;
        assert!(matches!(
            PricingCatalog::from_json(legacy_catalog),
            Err(ContractError::Malformed(_))
        ));

        let legacy_snapshot = serde_json::json!({
            "schema_version": 1,
            "provider": "vendor",
            "model": "image-v1",
            "catalog_version": "v1",
            "catalog_digest": format!("sha256:{}", "a".repeat(64)),
            "pricing_rule_id": "image",
            "unit": "image",
            "unit_amount_minor": 125,
            "quantity": 1,
            "estimated_amount_minor": 125,
            "currency": "USD"
        });
        assert!(serde_json::from_value::<PricingSnapshot>(legacy_snapshot).is_err());
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
        let request = NormalizedRequest::new("vendor", "text-v1")
            .with_quantity("input_tokens", Some(2_500))
            .with_quantity("max_output_tokens", Some(2_500));
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.exact_estimate_numerator, "1");
        assert_eq!(snapshot.exact_estimate_denominator, "1");
        assert_eq!(snapshot.estimated_units, 1_0000);
        let usage = NormalizedUsage {
            images: None,
            input_tokens: Some(2_500),
            output_tokens: Some(2_500),
            ..Default::default()
        };
        assert_eq!(snapshot.settle(&usage, 1_0000).unwrap(), 1_0000);
        let serialized = serde_json::to_vec(&snapshot).unwrap();
        let replayed: PricingSnapshot = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(replayed.settle(&usage, 1_0000).unwrap(), 1_0000);
    }

    #[test]
    fn fractional_cent_rates_quote_and_settle_exactly_in_micro_units() {
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"fractions","rules":[{"rule_id":"text","provider":"vendor","model":"text-v1","currency":"USD","components":[{"unit":"input_token","rate_numerator_minor":1,"rate_denominator":4},{"unit":"output_token","rate_numerator_minor":1,"rate_denominator":4}]}]}"#).unwrap();
        let request = NormalizedRequest::new("vendor", "text-v1")
            .with_quantity("input_tokens", Some(1))
            .with_quantity("max_output_tokens", Some(1));
        // Two quarter-cent tokens cost exactly half a cent: no whole-cent
        // over-reservation or over-charge.
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.estimated_units, 5_000);
        assert_eq!(
            snapshot
                .settle(
                    &NormalizedUsage {
                        images: None,
                        input_tokens: Some(1),
                        output_tokens: Some(1),
                        ..Default::default()
                    },
                    5_000
                )
                .unwrap(),
            5_000
        );
        snapshot.check_authorization(5_000, "USD").unwrap();
        assert_eq!(
            snapshot.check_authorization(4_999, "USD"),
            Err(ContractError::InsufficientAuthorization)
        );
        // A rate finer than a micro-unit rounds up once, at the aggregate.
        let finer = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"finer","rules":[{"rule_id":"text","provider":"vendor","model":"text-v1","currency":"USD","components":[{"unit":"input_token","rate_numerator_minor":1,"rate_denominator":30000},{"unit":"output_token","rate_numerator_minor":1,"rate_denominator":30000}]}]}"#).unwrap();
        assert_eq!(finer.snapshot(&request).unwrap().estimated_units, 1);
        // A frozen schema 2 snapshot keeps its whole-cent estimate on the wire.
        let mut legacy = serde_json::to_value(&snapshot).unwrap();
        legacy["schema_version"] = json!(2);
        legacy.as_object_mut().unwrap().remove("estimated_units");
        legacy["estimated_amount_minor"] = json!(1);
        let legacy_snapshot: PricingSnapshot = serde_json::from_value(legacy.clone()).unwrap();
        legacy_snapshot.validate_integrity().unwrap();
        assert_eq!(legacy_snapshot.estimated_units, 1_0000);
        assert_eq!(serde_json::to_value(&legacy_snapshot).unwrap(), legacy);
        assert!(serde_json::from_value::<PricingSnapshot>(json!({
            "schema_version": 3, "provider": "vendor", "model": "text-v1",
            "catalog_version": "fractions", "catalog_digest": snapshot.catalog_digest,
            "pricing_rule_id": "text", "components": [], "exact_estimate_numerator": "1",
            "exact_estimate_denominator": "2", "estimated_amount_minor": 1, "currency": "USD"
        }))
        .is_err());
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
        enforce_cost(&s, 250_0000, "USD").unwrap();
        enforce_cost(&s, 250_0000, "usd").unwrap();
        assert_eq!(
            enforce_cost(&s, 249_9999, "USD"),
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
                    actual_vendor_cost: None,
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

    #[test]
    fn shipped_contract_pricing_is_byte_identical_to_the_pre_generalization_baseline() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/gongbu-pricing-parity-v1.json"
        ))
        .unwrap();
        let contracts: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        let mut rules = Vec::new();
        // The baseline predates image editing; generation prices are unchanged.
        for contract in contracts["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|contract| contract["target"]["workload_type"] == "image_generation")
        {
            rules.extend(
                contract["pricing_rules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned(),
            );
        }
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": baseline["catalog_version"],
                "rules": rules,
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            catalog.0.digest,
            baseline["catalog_digest"].as_str().unwrap()
        );
        let mut checked = 0;
        for contract in contracts["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|contract| contract["target"]["workload_type"] == "image_generation")
        {
            let provider = contract["target"]["provider"].as_str().unwrap();
            let model = contract["target"]["model"].as_str().unwrap();
            for preset in contract["capability"]["presets"].as_array().unwrap() {
                let name = preset["name"].as_str().unwrap();
                let request = NormalizedRequest::new(provider, model)
                    .with_quantity("image_count", Some(1))
                    .with_selector(IMAGE_SIZE_SELECTOR, Some(name.to_owned()))
                    .with_output_dimensions(Some(OutputDimensions {
                        width: preset["width"].as_u64().unwrap() as u32,
                        height: preset["height"].as_u64().unwrap() as u32,
                    }));
                let expected = baseline["snapshots"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| {
                        entry["provider"] == provider
                            && entry["model"] == model
                            && entry["image_size"] == name
                    })
                    .unwrap()["snapshot"]
                    .as_str()
                    .unwrap();
                let snapshot = catalog.snapshot(&request).unwrap();
                assert_eq!(serde_json::to_string(&snapshot).unwrap(), expected);
                // Frozen rows written before the change still read back and settle.
                let frozen: PricingSnapshot = serde_json::from_str(expected).unwrap();
                assert_eq!(frozen, snapshot);
                let usage = NormalizedUsage {
                    images: Some(1),
                    ..Default::default()
                };
                assert_eq!(
                    frozen.settle(&usage, frozen.estimated_units).unwrap(),
                    frozen.estimated_units
                );
                checked += 1;
            }
        }
        assert_eq!(checked, baseline["snapshots"].as_array().unwrap().len());
    }

    #[test]
    fn a_model_prices_generation_and_editing_separately() {
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"edit-v1","rules":[
            {"rule_id":"generate-1k","provider":"google","model":"image-v1","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":67,"rate_denominator":10}]},
            {"rule_id":"edit-1k","provider":"google","model":"image-v1","workload_type":"image_edit","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":67,"rate_denominator":10},{"unit":"input_image","rate_numerator_minor":56,"rate_denominator":1000}]},
            {"rule_id":"legacy-text","provider":"example","model":"text-v1","currency":"USD","components":[{"unit":"request","rate_numerator_minor":1,"rate_denominator":1}]}
        ]}"#).unwrap();
        let request = |workload: &str| {
            NormalizedRequest::new("google", "image-v1")
                .with_workload(workload)
                .with_quantity("image_count", Some(1))
                .with_quantity("input_images", Some(2))
                .with_selector(IMAGE_SIZE_SELECTOR, Some("1k".into()))
        };
        let generate = catalog.snapshot(&request("image_generation")).unwrap();
        assert_eq!(generate.pricing_rule_id, "generate-1k");
        assert_eq!(generate.estimated_units, 67_000);
        let edit = catalog.snapshot(&request("image_edit")).unwrap();
        assert_eq!(edit.pricing_rule_id, "edit-1k");
        assert_eq!(edit.estimated_units, 68_120);
        // A snapshot settles the input images the request sent.
        let usage = NormalizedUsage {
            images: Some(1),
            input_images: Some(2),
            ..Default::default()
        };
        assert_eq!(edit.settle(&usage, edit.estimated_units).unwrap(), 68_120);
        // A rule that names no workload still prices the workloads no rule names.
        let text = NormalizedRequest::new("example", "text-v1").with_workload("text_generation");
        assert_eq!(
            catalog.snapshot(&text).unwrap().pricing_rule_id,
            "legacy-text"
        );
        let target =
            |workload: &str| TargetKey::new(workload, "google", "gemini", "image-v1").unwrap();
        assert_eq!(
            catalog.rules_for_target(&target("image_generation")).len(),
            1
        );
        assert_eq!(
            catalog.rules_for_target(&target("image_edit"))[0].rule_id,
            "edit-1k"
        );
        assert!(catalog
            .snapshot_for_target(&target("image_edit"), &request("image_generation"))
            .is_err());
        // Only input images may join an image-size rule's output image.
        assert!(PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"edit-v1","rules":[
            {"rule_id":"edit-1k","provider":"google","model":"image-v1","workload_type":"image_edit","currency":"USD","selector":{"image_size":"1k"},"components":[{"unit":"image","rate_numerator_minor":67,"rate_denominator":10},{"unit":"request","rate_numerator_minor":1,"rate_denominator":1}]}
        ]}"#).is_err());
    }

    #[test]
    fn per_request_plus_per_result_target_settles_exactly() {
        // A search-shaped target: $0.005 per call plus $0.001 per result,
        // with a cheaper "fast" mode picked by a non-image selector.
        let catalog = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"search-v1","rules":[
            {"rule_id":"search-auto","provider":"search","model":"web-v1","currency":"USD","selector":{"mode":"auto"},"components":[{"unit":"request","rate_numerator_minor":5,"rate_denominator":10},{"unit":"result","rate_numerator_minor":1,"rate_denominator":10}]},
            {"rule_id":"search-fast","provider":"search","model":"web-v1","currency":"USD","selector":{"mode":" FAST "},"components":[{"unit":"request","rate_numerator_minor":25,"rate_denominator":100},{"unit":"result","rate_numerator_minor":1,"rate_denominator":10}]}
        ]}"#).unwrap();
        let request = NormalizedRequest::new("search", "web-v1")
            .with_quantity("results", Some(10))
            .with_selector("mode", Some("auto".into()));
        let snapshot = catalog.snapshot(&request).unwrap();
        assert_eq!(snapshot.pricing_rule_id, "search-auto");
        assert_eq!(
            snapshot.selector.as_ref().unwrap().get("mode"),
            Some("auto")
        );
        // 0.5¢ + 10 × 0.1¢ = 1.5¢ exactly, held as 15,000 micro-units.
        assert_eq!(
            (
                snapshot.exact_estimate_numerator.as_str(),
                snapshot.exact_estimate_denominator.as_str()
            ),
            ("3", "2")
        );
        assert_eq!(snapshot.estimated_units, 15_000);
        assert_eq!(snapshot.estimated_quantity(PricingUnit::Request), Some(1));
        assert_eq!(snapshot.estimated_quantity(PricingUnit::Result), Some(10));
        snapshot.validate_integrity().unwrap();
        // Seven delivered results settle 0.5¢ + 0.7¢ = 1.2¢, exactly.
        let delivered = NormalizedUsage {
            results: Some(7),
            ..Default::default()
        };
        let settled = snapshot
            .settle_precise(&delivered, None, snapshot.estimated_units)
            .unwrap();
        assert_eq!(settled.budget_units, 12_000);
        assert_eq!(
            settled.actual_vendor_cost,
            ActualVendorCost::new(12, 3, "USD").unwrap()
        );
        // More results than were held is an overage, never a silent overcharge.
        let overage = NormalizedUsage {
            results: Some(11),
            ..Default::default()
        };
        assert_eq!(
            snapshot.settle(&overage, snapshot.estimated_units),
            Err(ContractError::SettlementOverage)
        );
        // The mode selector is normalized in the catalog and must match exactly.
        let fast = catalog
            .snapshot(
                &NormalizedRequest::new("search", "web-v1")
                    .with_quantity("results", Some(1))
                    .with_selector("mode", Some("fast".into())),
            )
            .unwrap();
        assert_eq!(fast.pricing_rule_id, "search-fast");
        assert_eq!(fast.estimated_units, 3_500);
        for unmatched in [
            NormalizedRequest::new("search", "web-v1").with_quantity("results", Some(1)),
            NormalizedRequest::new("search", "web-v1")
                .with_quantity("results", Some(1))
                .with_selector("mode", Some("deep".into())),
        ] {
            assert!(catalog.snapshot(&unmatched).is_err());
        }
        // A result component without a results quantity cannot be priced.
        assert_eq!(
            catalog.snapshot(
                &NormalizedRequest::new("search", "web-v1")
                    .with_selector("mode", Some("auto".into()))
            ),
            Err(ContractError::IndeterminableCost)
        );
    }

    #[test]
    fn pricing_selectors_and_request_quantities_are_validated() {
        let catalog = |selector: &str, unit: &str| {
            PricingCatalog::from_json(
                format!(
                    r#"{{"schema_version":2,"catalog_version":"v","rules":[{{"rule_id":"r","provider":"p","model":"m","currency":"USD","selector":{selector},"components":[{{"unit":"{unit}","rate_numerator_minor":1,"rate_denominator":1}}]}}]}}"#
                )
                .as_bytes(),
            )
        };
        let aspect = catalog(r#"{"image_size":"1K","aspect_ratio":" 16:9 "}"#, "image").unwrap();
        let selector = aspect.rules()[0].selector.as_ref().unwrap();
        assert_eq!(selector.image_size(), Some("1k"));
        assert_eq!(selector.get("aspect_ratio"), Some("16:9"));
        for (selector, unit) in [
            ("{}", "image"),
            (r#"{"Image_Size":"1k"}"#, "image"),
            (r#"{"image_size":"8k"}"#, "image"),
            (r#"{"mode":"two words"}"#, "request"),
            (r#"{"mode":""}"#, "request"),
            (r#"{"image_size":"1k"}"#, "request"),
        ] {
            assert!(
                matches!(catalog(selector, unit), Err(ContractError::Malformed(_))),
                "{selector} / {unit}"
            );
        }
        assert!(catalog(r#"{"mode":"fast"}"#, "second").is_ok());

        let mut request = NormalizedRequest::new("p", "m").with_quantity("image_count", Some(1));
        request.validate().unwrap();
        request.set_quantity("widgets", Some(1));
        assert_eq!(request.validate(), Err(ContractError::IndeterminableCost));
        request.set_quantity("widgets", None);
        request.set_quantity("pages", Some(-1));
        assert_eq!(request.validate(), Err(ContractError::IndeterminableCost));
        request.set_quantity("pages", None);
        request.selector.insert("mode", "FAST");
        assert_eq!(request.validate(), Err(ContractError::IndeterminableCost));
    }

    #[test]
    fn image_usage_serializes_without_the_new_quantities() {
        let usage = NormalizedUsage {
            images: Some(1),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&usage).unwrap(),
            r#"{"images":1,"input_tokens":null,"output_tokens":null}"#
        );
        let request = NormalizedRequest::new("flux", "flux-2-pro")
            .with_quantity("image_count", Some(1))
            .with_selector(IMAGE_SIZE_SELECTOR, Some("1k".into()));
        assert_eq!(
            serde_json::to_value(&request.selector).unwrap(),
            json!({"image_size": "1k"})
        );
    }
}
