//! Provider pack schema v1: one TOML file describes a provider integration as
//! data, and a reviewed archetype engine runs it.
//!
//! A pack declares identity, transport, agent-settable parameters, pricing,
//! response mapping and artifact rules. Engines, not packs, own the safety
//! invariants: zero mutation retries, the proven-before-transmission rule,
//! reconciliation of ambiguous outcomes, settlement from the frozen pricing
//! snapshot, and sending credentials only to the provider API hosts.
//! Validation rejects any pack field that would weaken them.

use super::{
    contract::{
        ActualVendorCost, AdapterCapabilities, AdapterOutcome, ContractError, NormalizedArtifact,
        NormalizedRequest, NormalizedUsage, PriceComponent, PricingCatalog, PricingRule,
        PricingSelector, ProviderAdapter, ProviderFailure, ProviderPhase, REQUEST_QUANTITIES,
    },
    http_kernel::{valid_provider_deadline_ms, validate_https_origin, validate_safe_headers},
    targets::{PackTargetConfig, TargetKey},
};
use crate::secrets::{ProviderSecret, SecretReference};
use base64::Engine as _;
use reqwest::{header::HeaderName, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use thiserror::Error;

pub const PACK_SCHEMA_VERSION: u32 = 1;

/// Artifact media types the Gongbu artifact store accepts today.
const PACK_ARTIFACT_MEDIA_TYPES: &[&str] = &["image/png", "image/jpeg"];

/// Field names that would hand a pack control the engines keep. They are
/// rejected wherever they appear, with an error naming the field.
const ENGINE_OWNED_FIELDS: &[(&str, &str)] = &[
    ("retries", "Gongbu never retries a provider mutation"),
    ("max_retries", "Gongbu never retries a provider mutation"),
    (
        "generation_retries",
        "Gongbu never retries a provider mutation",
    ),
    ("retry", "Gongbu never retries a provider mutation"),
    (
        "fallback",
        "the engine never falls back to another provider",
    ),
    (
        "reconciliation",
        "the engine sends every ambiguous outcome to reconciliation",
    ),
    ("recovery", "the engine owns durable checkpoints and resume"),
    (
        "settlement",
        "the engine settles only from the frozen pricing snapshot",
    ),
    (
        "credential_hosts",
        "credentials go only to transport.allowed_hosts",
    ),
    ("idempotency_header", "the engine owns vendor idempotency"),
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackError {
    #[error("pack is not valid TOML: {0}")]
    Parse(String),
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("{field}: engine-owned field is not allowed in a pack; {reason}")]
    EngineOwned { field: String, reason: &'static str },
    #[error(
        "artifacts.hosts: {host} also receives the provider credential (transport.allowed_hosts); \
         artifact downloads must never go to a credential-bearing host"
    )]
    CredentialBearingArtifactHost { host: String },
    #[error("read pack {path}: {reason}")]
    Read { path: String, reason: String },
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> PackError {
    PackError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

/// The archetype engine that runs a pack.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Archetype {
    /// One request; the price is fully set by the request parameters.
    SyncFixed,
    /// Submit a job once, then poll read-only for the result.
    AsyncPoll,
    /// Hold the worst case; settle from usage the provider reports.
    Metered,
}

impl Archetype {
    pub fn engine_id(self) -> &'static str {
        match self {
            Self::SyncFixed => "sync_fixed",
            Self::AsyncPoll => "async_poll",
            Self::Metered => "metered",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PricingTrust {
    /// Shipped in the repository with reviewed pricing.
    Verified,
    /// Written by a user; live use requires an explicit acknowledgement.
    Unverified,
}

/// A validated provider pack. Invalid packs cannot inhabit this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pack {
    document: PackDocument,
    digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PackDocument {
    schema_version: u32,
    pack_id: String,
    pack_version: String,
    provider: String,
    display_name: String,
    billing_merchant: String,
    capability: String,
    workload_type: String,
    archetype: Archetype,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adapter: Option<String>,
    pricing_trust: PricingTrust,
    transport: Transport,
    #[serde(default)]
    parameters: Vec<Parameter>,
    #[serde(default)]
    fixed: Map<String, Value>,
    response: ResponseMapping,
    artifacts: Artifacts,
    models: Vec<Model>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Transport {
    pub endpoint: String,
    pub api_version: String,
    pub allowed_hosts: Vec<String>,
    pub auth_header: String,
    /// Credential alias bound to a Keychain item by the stack profile.
    pub credential: String,
    pub timeout_ms: u64,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ParameterKind,
    /// Provider request field; defaults to `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// The priced request quantity this integer parameter sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    /// The pricing selector key this enum parameter sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    String,
    Integer,
    Boolean,
    Enum,
}

/// JSON Pointers (RFC 6901) into the provider's response.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResponseMapping {
    pub provider_request_id: String,
    /// Usage quantity → pointer to an integer or an array whose length counts.
    #[serde(default)]
    pub usage: BTreeMap<String, String>,
    /// Pointer to the provider-reported cost, a decimal in major currency units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost: Option<String>,
    /// Pointer to the array of artifact items.
    pub artifact_items: String,
    /// Pointer inside each item to a download URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_url: Option<String>,
    /// Pointer inside each item to inline base64 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_base64: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifacts {
    pub media_types: Vec<String>,
    /// Hosts artifacts may be downloaded from. They never receive credentials.
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub model: String,
    pub provider_config_version: String,
    pub pricing_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_reviewed_on: Option<String>,
    pub pricing_rules: Vec<ModelPricingRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelPricingRule {
    pub rule_id: String,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<PricingSelector>,
    pub components: Vec<PriceComponent>,
}

impl Pack {
    pub fn from_toml_str(source: &str) -> Result<Self, PackError> {
        let raw: toml::Table = source
            .parse()
            .map_err(|error: toml::de::Error| PackError::Parse(error.message().to_owned()))?;
        reject_engine_owned_fields(&raw, "")?;
        let document: PackDocument = toml::Value::Table(raw)
            .try_into()
            .map_err(|error: toml::de::Error| PackError::Parse(error.message().to_owned()))?;
        document.validate()?;
        let canonical = serde_json::to_vec(&canonicalize(
            &serde_json::to_value(&document).expect("pack serialization cannot fail"),
        ))
        .expect("pack serialization cannot fail");
        let digest = format!("sha256:{:x}", Sha256::digest(canonical));
        Ok(Self { document, digest })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, PackError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|error| PackError::Read {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        Self::from_toml_str(&source)
    }

    pub fn pack_id(&self) -> &str {
        &self.document.pack_id
    }
    pub fn pack_version(&self) -> &str {
        &self.document.pack_version
    }
    /// Digest of the validated canonical pack; it changes with any content change.
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn provider(&self) -> &str {
        &self.document.provider
    }
    pub fn workload_type(&self) -> &str {
        &self.document.workload_type
    }
    pub fn archetype(&self) -> Archetype {
        self.document.archetype
    }
    pub fn pricing_trust(&self) -> PricingTrust {
        self.document.pricing_trust
    }
    pub fn billing_merchant(&self) -> &str {
        &self.document.billing_merchant
    }
    pub fn capability(&self) -> &str {
        &self.document.capability
    }
    pub fn display_name(&self) -> &str {
        &self.document.display_name
    }
    pub fn transport(&self) -> &Transport {
        &self.document.transport
    }
    pub fn response(&self) -> &ResponseMapping {
        &self.document.response
    }
    pub fn artifacts(&self) -> &Artifacts {
        &self.document.artifacts
    }
    pub fn parameters(&self) -> &[Parameter] {
        &self.document.parameters
    }
    pub fn models(&self) -> &[Model] {
        &self.document.models
    }
    /// The adapter ID recorded on this pack's targets.
    pub fn adapter_id(&self) -> &str {
        self.document
            .adapter
            .as_deref()
            .unwrap_or(self.document.archetype.engine_id())
    }
    pub fn model(&self, model: &str) -> Option<&Model> {
        self.document.models.iter().find(|m| m.model == model)
    }
    pub fn target_key(&self, model: &Model) -> TargetKey {
        TargetKey::new(
            &self.document.workload_type,
            &self.document.provider,
            self.adapter_id(),
            &model.model,
        )
        .expect("validated pack identifiers")
    }

    /// Pricing rules for every model, in the frozen catalog format.
    pub fn pricing_rules(&self) -> Vec<PricingRule> {
        self.document
            .models
            .iter()
            .flat_map(|model| {
                model.pricing_rules.iter().map(|rule| PricingRule {
                    rule_id: rule.rule_id.clone(),
                    provider: self.document.provider.clone(),
                    model: model.model.clone(),
                    currency: rule.currency.clone(),
                    selector: rule.selector.clone(),
                    components: rule.components.clone(),
                })
            })
            .collect()
    }

    /// Provider target revisions for every model, in the target config format.
    /// The pack's credential alias is resolved by the caller to `secret`.
    pub fn provider_configs(&self, secret: &SecretReference) -> Vec<Value> {
        self.document
            .models
            .iter()
            .map(|model| {
                json!({
                    "provider_config_version": model.provider_config_version,
                    "workload_type": self.document.workload_type,
                    "provider": self.document.provider,
                    "adapter": self.adapter_id(),
                    "model": model.model,
                    "secret_service": secret.service(),
                    "secret_account": secret.account(),
                    "active": true,
                    "execution_enabled": true,
                    "settings": {"type": "pack", "config": self.target_config()},
                })
            })
            .collect()
    }

    pub fn target_config(&self) -> PackTargetConfig {
        PackTargetConfig {
            pack_id: self.document.pack_id.clone(),
            pack_version: self.document.pack_version.clone(),
            pack_digest: self.digest.clone(),
            archetype: self.document.archetype.engine_id().to_owned(),
        }
    }

    /// Validate agent input against the declared parameters and build both the
    /// priced request and the provider request body.
    pub fn prepare(&self, model: &str, input: &Value) -> Result<PreparedRequest, ContractError> {
        let indeterminable = || ContractError::IndeterminableCost;
        let model = self.model(model).ok_or(ContractError::UnsupportedTarget)?;
        let input = input.as_object().ok_or_else(indeterminable)?;
        if input
            .keys()
            .any(|key| !self.document.parameters.iter().any(|p| &p.name == key))
        {
            return Err(indeterminable());
        }
        let mut request = NormalizedRequest::new(&self.document.provider, &model.model);
        let mut body = self.document.fixed.clone();
        for parameter in &self.document.parameters {
            let value = match input.get(&parameter.name).or(parameter.default.as_ref()) {
                Some(value) => value.clone(),
                None if parameter.required => return Err(indeterminable()),
                None => continue,
            };
            if parameter.check(&value).is_err() {
                return Err(indeterminable());
            }
            if let Some(quantity) = &parameter.quantity {
                request.set_quantity(quantity, value.as_i64());
            }
            if let Some(key) = &parameter.selector {
                request
                    .selector
                    .insert(key, value.as_str().ok_or_else(indeterminable)?);
            }
            body.insert(parameter.field().to_owned(), value);
        }
        request.validate()?;
        Ok(PreparedRequest {
            request,
            body: Value::Object(body),
        })
    }

    /// Map a provider response to a normalized outcome. Anything the mapping
    /// cannot read is an ambiguous outcome: the provider accepted the work.
    pub fn map_response(
        &self,
        response: &Value,
        downloads: &dyn ArtifactSource,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let mapping = &self.document.response;
        let ambiguous =
            |code: &str| ProviderFailure::reconcile(code.to_owned(), ProviderPhase::Processing);
        let provider_request_id = response
            .pointer(&mapping.provider_request_id)
            .and_then(|value| match value {
                Value::String(s) => Some(s.trim().to_owned()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .filter(|id| !id.is_empty() && id.len() <= 255)
            .ok_or_else(|| ambiguous("pack_response_request_id_missing"))?;
        let mut usage = NormalizedUsage::default();
        for (quantity, pointer) in &mapping.usage {
            let count = match response.pointer(pointer) {
                Some(Value::Array(items)) => i64::try_from(items.len()).ok(),
                Some(value) => value.as_i64().filter(|v| *v >= 0),
                None => None,
            }
            .ok_or_else(|| ambiguous("pack_response_usage_invalid"))?;
            *usage_slot(&mut usage, quantity).expect("validated usage quantity") = Some(count);
        }
        let actual_vendor_cost = match &mapping.reported_cost {
            None => None,
            Some(pointer) => Some(
                response
                    .pointer(pointer)
                    .and_then(decimal_cost)
                    .ok_or_else(|| ambiguous("pack_response_cost_invalid"))?,
            ),
        };
        let items = response
            .pointer(&mapping.artifact_items)
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .ok_or_else(|| ambiguous("pack_response_artifacts_missing"))?;
        let mut artifacts = Vec::with_capacity(items.len());
        for item in items {
            let (media_type, bytes) = if let Some(pointer) = &mapping.artifact_base64 {
                let encoded = item
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .ok_or_else(|| ambiguous("pack_response_artifact_invalid"))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| ambiguous("pack_response_artifact_invalid"))?;
                (sniff_media_type(&bytes), bytes)
            } else {
                let pointer = mapping.artifact_url.as_deref().expect("validated mapping");
                let url = item
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .and_then(|raw| Url::parse(raw).ok())
                    .ok_or_else(|| ambiguous("pack_response_artifact_invalid"))?;
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.port().is_some()
                    || !url
                        .host_str()
                        .is_some_and(|host| self.document.artifacts.hosts.iter().any(|h| h == host))
                {
                    return Err(ProviderFailure::reconcile(
                        "artifact_policy_failure",
                        ProviderPhase::Artifact,
                    ));
                }
                let bytes = downloads.fetch(&url).ok_or_else(|| {
                    ProviderFailure::reconcile("artifact_fetch_failed", ProviderPhase::Artifact)
                })?;
                (sniff_media_type(&bytes), bytes)
            };
            let media_type = media_type
                .filter(|media| {
                    self.document
                        .artifacts
                        .media_types
                        .iter()
                        .any(|m| m == media)
                })
                .ok_or_else(|| {
                    ProviderFailure::reconcile("artifact_policy_failure", ProviderPhase::Artifact)
                })?;
            artifacts.push(NormalizedArtifact {
                media_type: media_type.to_owned(),
                bytes,
            });
        }
        Ok(AdapterOutcome {
            usage: Some(usage),
            actual_vendor_cost,
            provider_request_id: Some(provider_request_id),
            provider_operation_id: None,
            artifacts,
        })
    }
}

/// The priced request plus the provider request body an engine sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    pub request: NormalizedRequest,
    pub body: Value,
}

/// Downloads artifact bytes from an already allowlisted URL.
pub trait ArtifactSource {
    /// `None` when the bytes could not be fetched.
    fn fetch(&self, url: &Url) -> Option<Vec<u8>>;
}

impl Parameter {
    fn field(&self) -> &str {
        self.field.as_deref().unwrap_or(&self.name)
    }

    fn check(&self, value: &Value) -> Result<(), String> {
        match self.kind {
            ParameterKind::String => {
                let text = value.as_str().ok_or("must be a string")?;
                if text.trim().is_empty() {
                    return Err("must not be empty".into());
                }
                if self
                    .max_length
                    .is_some_and(|max| text.chars().count() > max)
                {
                    return Err("is longer than max_length".into());
                }
            }
            ParameterKind::Integer => {
                let number = value.as_i64().ok_or("must be an integer")?;
                if self.min.is_some_and(|min| number < min)
                    || self.max.is_some_and(|max| number > max)
                {
                    return Err("is outside min..=max".into());
                }
            }
            ParameterKind::Boolean => {
                value.as_bool().ok_or("must be a boolean")?;
            }
            ParameterKind::Enum => {
                let text = value.as_str().ok_or("must be a string")?;
                if !self.values.iter().any(|allowed| allowed == text) {
                    return Err("is not one of values".into());
                }
            }
        }
        Ok(())
    }

    fn validate(&self, field: &str) -> Result<(), PackError> {
        if !valid_field_name(&self.name) {
            return Err(invalid(
                format!("{field}.name"),
                "must be a lowercase identifier",
            ));
        }
        if self
            .field
            .as_deref()
            .is_some_and(|f| f.is_empty() || f.len() > 128)
        {
            return Err(invalid(
                format!("{field}.field"),
                "must be 1 to 128 characters",
            ));
        }
        let only = |allowed: bool, name: &str| {
            if allowed {
                Ok(())
            } else {
                Err(invalid(
                    format!("{field}.{name}"),
                    format!("is not allowed on a {:?} parameter", self.kind).to_lowercase(),
                ))
            }
        };
        let integer = self.kind == ParameterKind::Integer;
        let enumeration = self.kind == ParameterKind::Enum;
        if self.min.is_some() {
            only(integer, "min")?;
        }
        if self.max.is_some() {
            only(integer, "max")?;
        }
        if self.quantity.is_some() {
            only(integer, "quantity")?;
        }
        if self.max_length.is_some() {
            only(self.kind == ParameterKind::String, "max_length")?;
        }
        if !self.values.is_empty() {
            only(enumeration, "values")?;
        }
        if self.selector.is_some() {
            only(enumeration, "selector")?;
        }
        if self.min.zip(self.max).is_some_and(|(min, max)| min > max) {
            return Err(invalid(format!("{field}.min"), "must not exceed max"));
        }
        if self.kind == ParameterKind::String && self.max_length.is_none() {
            return Err(invalid(
                format!("{field}.max_length"),
                "string parameters must be bounded",
            ));
        }
        if enumeration {
            let unique: BTreeSet<_> = self.values.iter().collect();
            if self.values.is_empty()
                || unique.len() != self.values.len()
                || self.values.iter().any(|value| {
                    let mut selector = PricingSelector::default();
                    selector.insert("value", value.as_str());
                    selector.normalized().ok().as_ref() != Some(&selector)
                })
            {
                return Err(invalid(
                    format!("{field}.values"),
                    "must be unique, non-empty, normalized selector values",
                ));
            }
        }
        if let Some(quantity) = &self.quantity {
            if !REQUEST_QUANTITIES.contains(&quantity.as_str()) {
                return Err(invalid(
                    format!("{field}.quantity"),
                    format!("must be one of {}", REQUEST_QUANTITIES.join(", ")),
                ));
            }
            if self.min.is_none_or(|min| min < 0) || self.max.is_none() {
                return Err(invalid(
                    format!("{field}.quantity"),
                    "priced quantities need min >= 0 and a max, so every hold is bounded",
                ));
            }
        }
        if let Some(selector) = &self.selector {
            let mut probe = PricingSelector::default();
            probe.insert(selector.as_str(), "probe");
            if probe.normalized().is_err() {
                return Err(invalid(
                    format!("{field}.selector"),
                    "must be a lowercase identifier",
                ));
            }
        }
        if self.required && self.default.is_some() {
            return Err(invalid(
                format!("{field}.default"),
                "a required parameter has no default",
            ));
        }
        if let Some(default) = &self.default {
            self.check(default)
                .map_err(|reason| invalid(format!("{field}.default"), reason))?;
        }
        if (self.quantity.is_some() || self.selector.is_some())
            && !self.required
            && self.default.is_none()
        {
            return Err(invalid(
                format!("{field}.default"),
                "a priced parameter must be required or have a default",
            ));
        }
        Ok(())
    }
}

impl PackDocument {
    fn validate(&self) -> Result<(), PackError> {
        if self.schema_version != PACK_SCHEMA_VERSION {
            return Err(invalid(
                "schema_version",
                format!("unsupported; this Gongbu reads schema {PACK_SCHEMA_VERSION}"),
            ));
        }
        for (field, value) in [
            ("pack_id", &self.pack_id),
            ("pack_version", &self.pack_version),
            ("provider", &self.provider),
            ("workload_type", &self.workload_type),
        ] {
            if !valid_slug(value) {
                return Err(invalid(
                    field,
                    "must be a lowercase slug (a-z, 0-9, '.', '_', '-')",
                ));
            }
        }
        if let Some(adapter) = &self.adapter {
            if !valid_slug(adapter) {
                return Err(invalid("adapter", "must be a lowercase slug"));
            }
        }
        if self.display_name.trim().is_empty() || self.display_name.len() > 128 {
            return Err(invalid("display_name", "must be 1 to 128 characters"));
        }
        for (field, value, prefix) in [
            ("billing_merchant", &self.billing_merchant, "merchant:"),
            ("capability", &self.capability, "capability:"),
        ] {
            if !value.strip_prefix(prefix).is_some_and(valid_scope_tail) {
                return Err(invalid(field, format!("must look like {prefix}<name>")));
            }
        }
        self.validate_transport()?;
        let mut names = BTreeSet::new();
        let mut fields = BTreeSet::new();
        let mut quantities = BTreeSet::new();
        let mut selectors = BTreeMap::new();
        for (index, parameter) in self.parameters.iter().enumerate() {
            let field = format!("parameters[{index}]");
            parameter.validate(&field)?;
            if !names.insert(parameter.name.as_str()) {
                return Err(invalid(format!("{field}.name"), "is declared twice"));
            }
            if !fields.insert(parameter.field()) {
                return Err(invalid(format!("{field}.field"), "is sent twice"));
            }
            if let Some(quantity) = &parameter.quantity {
                if !quantities.insert(quantity.as_str()) {
                    return Err(invalid(
                        format!("{field}.quantity"),
                        "is set by two parameters",
                    ));
                }
            }
            if let Some(selector) = &parameter.selector {
                if selectors.insert(selector.as_str(), parameter).is_some() {
                    return Err(invalid(
                        format!("{field}.selector"),
                        "is set by two parameters",
                    ));
                }
            }
        }
        if let Some(key) = self.fixed.keys().find(|key| fields.contains(key.as_str())) {
            return Err(invalid(
                format!("fixed.{key}"),
                "collides with a parameter field",
            ));
        }
        self.validate_response()?;
        self.validate_artifacts()?;
        self.validate_models(&quantities, &selectors)
    }

    fn validate_transport(&self) -> Result<(), PackError> {
        let transport = &self.transport;
        let url = Url::parse(&transport.endpoint)
            .ok()
            .filter(|url| validate_https_origin(url, None).is_ok())
            .ok_or_else(|| {
                invalid(
                    "transport.endpoint",
                    "must be an https origin with no port, path or query",
                )
            })?;
        validate_hosts(&transport.allowed_hosts, "transport.allowed_hosts")?;
        if transport.allowed_hosts.is_empty()
            || !url
                .host_str()
                .is_some_and(|host| transport.allowed_hosts.iter().any(|h| h == host))
        {
            return Err(invalid(
                "transport.allowed_hosts",
                "must include the endpoint host",
            ));
        }
        if !valid_slug(&transport.api_version) {
            return Err(invalid("transport.api_version", "must be a lowercase slug"));
        }
        if transport.auth_header.parse::<HeaderName>().is_err() {
            return Err(invalid(
                "transport.auth_header",
                "must be an HTTP header name",
            ));
        }
        if !valid_slug(&transport.credential) {
            return Err(invalid(
                "transport.credential",
                "must be a credential alias slug",
            ));
        }
        if !valid_provider_deadline_ms(transport.timeout_ms) {
            return Err(invalid(
                "transport.timeout_ms",
                "is outside the provider deadline range",
            ));
        }
        if validate_safe_headers(
            &transport.headers,
            &["authorization", "cookie", transport.auth_header.as_str()],
        )
        .is_err()
        {
            return Err(invalid(
                "transport.headers",
                "must be safe, non-secret headers and must not set the auth header",
            ));
        }
        Ok(())
    }

    fn validate_response(&self) -> Result<(), PackError> {
        let response = &self.response;
        let mut pointers = vec![
            (
                "response.provider_request_id",
                Some(&response.provider_request_id),
            ),
            ("response.reported_cost", response.reported_cost.as_ref()),
            ("response.artifact_items", Some(&response.artifact_items)),
            ("response.artifact_url", response.artifact_url.as_ref()),
            (
                "response.artifact_base64",
                response.artifact_base64.as_ref(),
            ),
        ];
        for (quantity, pointer) in &response.usage {
            if usage_slot(&mut NormalizedUsage::default(), quantity).is_none() {
                return Err(invalid(
                    format!("response.usage.{quantity}"),
                    "is not a usage quantity (images, input_tokens, output_tokens, results, pages, seconds)",
                ));
            }
            pointers.push(("response.usage", Some(pointer)));
        }
        for (field, pointer) in pointers {
            if pointer.is_some_and(|p| !(p.is_empty() || p.starts_with('/')) || p.len() > 256) {
                return Err(invalid(field, "must be a JSON Pointer such as /data/0/url"));
            }
        }
        if response.artifact_url.is_some() == response.artifact_base64.is_some() {
            return Err(invalid(
                "response",
                "set exactly one of artifact_url or artifact_base64",
            ));
        }
        Ok(())
    }

    fn validate_artifacts(&self) -> Result<(), PackError> {
        let artifacts = &self.artifacts;
        if artifacts.media_types.is_empty()
            || artifacts
                .media_types
                .iter()
                .any(|media| !PACK_ARTIFACT_MEDIA_TYPES.contains(&media.as_str()))
        {
            return Err(invalid(
                "artifacts.media_types",
                format!(
                    "must be a subset of {}",
                    PACK_ARTIFACT_MEDIA_TYPES.join(", ")
                ),
            ));
        }
        validate_hosts(&artifacts.hosts, "artifacts.hosts")?;
        if let Some(host) = artifacts
            .hosts
            .iter()
            .find(|host| self.transport.allowed_hosts.contains(host))
        {
            return Err(PackError::CredentialBearingArtifactHost { host: host.clone() });
        }
        if self.response.artifact_url.is_some() && artifacts.hosts.is_empty() {
            return Err(invalid(
                "artifacts.hosts",
                "URL artifacts need at least one download host",
            ));
        }
        Ok(())
    }

    fn validate_models(
        &self,
        quantities: &BTreeSet<&str>,
        selectors: &BTreeMap<&str, &Parameter>,
    ) -> Result<(), PackError> {
        if self.models.is_empty() {
            return Err(invalid("models", "a pack needs at least one model"));
        }
        let mut models = BTreeSet::new();
        let mut versions = BTreeSet::new();
        for (index, model) in self.models.iter().enumerate() {
            let field = format!("models[{index}]");
            if !valid_slug(&model.model) || !models.insert(model.model.as_str()) {
                return Err(invalid(
                    format!("{field}.model"),
                    "must be a unique lowercase slug",
                ));
            }
            if !valid_slug(&model.provider_config_version)
                || !versions.insert(model.provider_config_version.as_str())
            {
                return Err(invalid(
                    format!("{field}.provider_config_version"),
                    "must be a unique lowercase slug",
                ));
            }
            if !valid_slug(&model.pricing_version) {
                return Err(invalid(
                    format!("{field}.pricing_version"),
                    "must be a lowercase slug",
                ));
            }
            match (&model.pricing_reviewed_on, self.pricing_trust) {
                (Some(date), _) if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() => {
                    return Err(invalid(
                        format!("{field}.pricing_reviewed_on"),
                        "must be a YYYY-MM-DD date",
                    ))
                }
                (None, PricingTrust::Verified) => {
                    return Err(invalid(
                        format!("{field}.pricing_reviewed_on"),
                        "verified packs record when their pricing was reviewed",
                    ))
                }
                _ => {}
            }
            if model.pricing_rules.is_empty() {
                return Err(invalid(
                    format!("{field}.pricing_rules"),
                    "no pack goes live without a price",
                ));
            }
            for (rule_index, rule) in model.pricing_rules.iter().enumerate() {
                let rule_field = format!("{field}.pricing_rules[{rule_index}]");
                for component in &rule.components {
                    if let Some(quantity) = component.unit.quantity_name() {
                        if !quantities.contains(quantity) {
                            return Err(invalid(
                                format!("{rule_field}.components"),
                                format!(
                                    "prices {quantity}, but no bounded parameter sets that quantity"
                                ),
                            ));
                        }
                    }
                }
                for quantity in quantities {
                    if !rule
                        .components
                        .iter()
                        .any(|component| component.unit.quantity_name() == Some(*quantity))
                    {
                        return Err(invalid(
                            format!("{rule_field}.components"),
                            format!(
                                "declares priced quantity {quantity}, but this rule has no component for it"
                            ),
                        ));
                    }
                }
            }
        }
        // Freeze the rules exactly as the catalog will, so every catalog rule
        // (normalization, ambiguity, rates) holds before a pack can be used.
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": "pack-validation",
                "rules": self.provider_rules(),
            }))
            .expect("pack pricing serialization cannot fail"),
        )
        .map_err(|error| invalid("models.pricing_rules", error.to_string()))?;
        // Every combination of selector parameter values must price exactly one
        // rule, so no valid agent input can reach an unpriced request.
        let combinations = selector_combinations(selectors);
        for model in &self.models {
            for combination in &combinations {
                let mut request = NormalizedRequest::new(&self.provider, &model.model);
                request.selector = combination.clone();
                for quantity in quantities {
                    request.set_quantity(quantity, Some(1));
                }
                catalog.snapshot(&request).map_err(|_| {
                    invalid(
                        format!("models.{}.pricing_rules", model.model),
                        format!(
                            "no single rule prices selector {}",
                            serde_json::to_string(combination).expect("selector serializes")
                        ),
                    )
                })?;
            }
        }
        Ok(())
    }

    fn provider_rules(&self) -> Vec<Value> {
        self.models
            .iter()
            .flat_map(|model| {
                model.pricing_rules.iter().map(|rule| {
                    json!({
                        "rule_id": rule.rule_id,
                        "provider": self.provider,
                        "model": model.model,
                        "currency": rule.currency,
                        "selector": rule.selector,
                        "components": rule.components,
                    })
                })
            })
            .map(|mut rule| {
                if rule["selector"].is_null() {
                    rule.as_object_mut().unwrap().remove("selector");
                }
                rule
            })
            .collect()
    }
}

/// Every selector a valid agent input can produce: the product of the enum
/// parameters bound to selector keys (one empty selector when there are none).
fn selector_combinations(selectors: &BTreeMap<&str, &Parameter>) -> Vec<PricingSelector> {
    let mut combinations = vec![PricingSelector::default()];
    for (key, parameter) in selectors {
        combinations = combinations
            .into_iter()
            .flat_map(|base| {
                parameter.values.iter().map(move |value| {
                    let mut next = base.clone();
                    next.insert(*key, value.as_str());
                    next
                })
            })
            .collect();
    }
    combinations
}

fn reject_engine_owned_fields(table: &toml::Table, prefix: &str) -> Result<(), PackError> {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some((_, reason)) = ENGINE_OWNED_FIELDS.iter().find(|(name, _)| name == key) {
            return Err(PackError::EngineOwned {
                field: path,
                reason,
            });
        }
        match value {
            toml::Value::Table(inner) => reject_engine_owned_fields(inner, &path)?,
            toml::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    if let toml::Value::Table(inner) = item {
                        reject_engine_owned_fields(inner, &format!("{path}[{index}]"))?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn usage_slot<'a>(usage: &'a mut NormalizedUsage, quantity: &str) -> Option<&'a mut Option<i64>> {
    Some(match quantity {
        "images" => &mut usage.images,
        "input_tokens" => &mut usage.input_tokens,
        "output_tokens" => &mut usage.output_tokens,
        "results" => &mut usage.results,
        "pages" => &mut usage.pages,
        "seconds" => &mut usage.seconds,
        _ => return None,
    })
}

/// A provider-reported decimal amount in major currency units (USD only).
fn decimal_cost(value: &Value) -> Option<ActualVendorCost> {
    let text = match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 18
    {
        return None;
    }
    let amount = format!("{whole}{fraction}").parse::<i64>().ok()?;
    ActualVendorCost::new(amount, u32::try_from(fraction.len()).ok()?, "USD").ok()
}

fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

fn validate_hosts(hosts: &[String], field: &str) -> Result<(), PackError> {
    let mut unique = BTreeSet::new();
    for host in hosts {
        let parsed = Url::parse(&format!("https://{host}/")).ok();
        if !unique.insert(host)
            || parsed.as_ref().and_then(Url::host_str) != Some(host.as_str())
            || parsed.as_ref().is_some_and(|url| url.port().is_some())
        {
            return Err(invalid(
                field,
                format!("{host} is not a unique bare host name"),
            ));
        }
    }
    Ok(())
}

fn valid_slug(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= 128
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn valid_scope_tail(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.split(':').all(valid_slug)
}

fn valid_field_name(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

/// The sandbox engine runs any pack without provider traffic: it validates the
/// agent input exactly as a live engine would, then replays one recorded
/// provider response through the pack's response mapping.
pub struct SandboxPackAdapter {
    pack: std::sync::Arc<Pack>,
    model: String,
    recorded_response: Value,
    downloads: BTreeMap<String, Vec<u8>>,
}

impl SandboxPackAdapter {
    pub fn new(
        pack: std::sync::Arc<Pack>,
        model: impl Into<String>,
        recorded_response: Value,
        downloads: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            pack,
            model: model.into(),
            recorded_response,
            downloads,
        }
    }
}

impl ArtifactSource for BTreeMap<String, Vec<u8>> {
    fn fetch(&self, url: &Url) -> Option<Vec<u8>> {
        self.get(url.as_str()).cloned()
    }
}

impl ProviderAdapter for SandboxPackAdapter {
    fn adapter_id(&self) -> &str {
        self.pack.adapter_id()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: false,
        }
    }

    fn validate_request(&self, request: &NormalizedRequest) -> Result<(), ContractError> {
        request.validate()?;
        if request.provider != self.pack.provider() || request.model != self.model {
            return Err(ContractError::UnsupportedTarget);
        }
        Ok(())
    }

    fn preflight_input(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
    ) -> Result<(), ContractError> {
        self.validate_request(request)?;
        let prepared = self.pack.prepare(&self.model, normalized_input)?;
        if prepared.request != *request {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }

    fn invoke(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        _secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.preflight_input(request, normalized_input)
            .map_err(|_| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        if vendor_idempotency_key.is_some() {
            return Err(ProviderFailure::release(
                "retry_not_supported",
                ProviderPhase::PreSend,
            ));
        }
        self.pack
            .map_response(&self.recorded_response, &self.downloads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        contract::PricingUnit,
        registry::{ProviderRegistry, RegistryError, ValidatedProviderCatalog},
        targets::ProviderTargetConfig,
    };
    use std::sync::Arc;

    const FIXTURE: &str = include_str!("../../../../fixtures/packs/sandbox-image.toml");
    const RESPONSE: &str = include_str!("../../../../fixtures/packs/sandbox-image.response.json");
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn edited(from: &str, to: &str) -> String {
        assert_eq!(FIXTURE.matches(from).count(), 1, "{from}");
        FIXTURE.replace(from, to)
    }

    fn downloads() -> BTreeMap<String, Vec<u8>> {
        [0, 1]
            .map(|i| {
                (
                    format!("https://cdn.sandbox-image.test/out/7f3a-{i}.png"),
                    PNG.to_vec(),
                )
            })
            .into_iter()
            .collect()
    }

    fn sandbox_registry(pack: &Pack) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register_pack(pack.clone());
        registry.register_pack_engine(Archetype::SyncFixed, |target, pack| {
            Ok(Arc::new(SandboxPackAdapter::new(
                pack.clone(),
                target.model.clone(),
                serde_json::from_str(RESPONSE).unwrap(),
                downloads(),
            )))
        });
        registry
    }

    fn bind(pack: &Pack, targets_from: &Pack) -> Result<ValidatedProviderCatalog, RegistryError> {
        let secret = SecretReference::new("gongbu.sandbox-image", "fixture").unwrap();
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version": 2,
            "provider_configs": targets_from.provider_configs(&secret),
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": targets_from.models()[0].pricing_version,
                "rules": targets_from.pricing_rules(),
            }))
            .unwrap(),
        )
        .unwrap();
        ValidatedProviderCatalog::bind(targets, pricing, &sandbox_registry(pack))
    }

    #[test]
    fn fixture_pack_loads_validates_and_runs_on_the_sandbox_engine() {
        let pack = Pack::from_toml_str(FIXTURE).unwrap();
        assert_eq!(pack.pack_id(), "sandbox-image");
        assert_eq!(pack.adapter_id(), "sync_fixed");
        assert_eq!(pack.pricing_trust(), PricingTrust::Verified);
        assert!(pack.digest().starts_with("sha256:") && pack.digest().len() == 71);
        // Formatting does not change the digest; content does.
        let reformatted = Pack::from_toml_str(&FIXTURE.replace("\n\n", "\n\n\n")).unwrap();
        assert_eq!(reformatted.digest(), pack.digest());
        let repriced = Pack::from_toml_str(&edited(
            "rate_numerator_minor = 6,",
            "rate_numerator_minor = 7,",
        ))
        .unwrap();
        assert_ne!(repriced.digest(), pack.digest());

        let catalog = bind(&pack, &pack).unwrap();
        let key = pack.target_key(&pack.models()[0]);
        let target = catalog.resolve_active(&key).unwrap();
        assert_eq!(target.pack().unwrap().pack_digest, pack.digest());

        let input = json!({"prompt": "a red circle", "image_count": 2, "quality": "final"});
        let prepared = pack.prepare("image-v1", &input).unwrap();
        assert_eq!(prepared.request.image_count(), Some(2));
        assert_eq!(prepared.request.selector.get("quality"), Some("final"));
        assert_eq!(
            prepared.body,
            json!({"prompt": "a red circle", "n": 2, "quality": "final", "response_format": "url"})
        );
        // 0.1¢ per request + 2 × 6¢ per final image = 12.1¢, exactly.
        let snapshot = catalog
            .pricing()
            .snapshot_for_target(&key, &prepared.request)
            .unwrap();
        assert_eq!(snapshot.estimated_units, 121_000);
        assert_eq!(snapshot.estimated_quantity(PricingUnit::Request), Some(1));

        let (_, adapter) = catalog
            .resolve_persisted(&key, &target.provider_config_version, target.digest())
            .unwrap();
        assert_eq!(adapter.adapter_id(), "sync_fixed");
        let outcome = adapter
            .invoke(
                &prepared.request,
                &input,
                &crate::secrets::secret_for_test("sandbox"),
                None,
            )
            .unwrap();
        outcome.validate().unwrap();
        assert_eq!(outcome.provider_request_id.as_deref(), Some("sbx-req-7f3a"));
        assert_eq!(outcome.artifacts.len(), 2);
        assert!(outcome
            .artifacts
            .iter()
            .all(|a| a.media_type == "image/png"));
        let usage = outcome.usage.unwrap();
        assert_eq!(usage.images, Some(2));
        assert_eq!(
            snapshot.settle(&usage, snapshot.estimated_units).unwrap(),
            121_000
        );

        // Defaults apply, and draft pricing is picked by the default selector.
        let draft = pack
            .prepare("image-v1", &json!({"prompt": "a square"}))
            .unwrap();
        assert_eq!(draft.request.image_count(), Some(1));
        assert_eq!(draft.request.selector.get("quality"), Some("draft"));
        assert_eq!(
            catalog
                .pricing()
                .snapshot_for_target(&key, &draft.request)
                .unwrap()
                .estimated_units,
            16_000
        );
    }

    #[test]
    fn agent_input_is_checked_against_declared_parameters() {
        let pack = Pack::from_toml_str(FIXTURE).unwrap();
        for input in [
            json!({}),
            json!({"prompt": ""}),
            json!({"prompt": "x", "image_count": 5}),
            json!({"prompt": "x", "image_count": 0}),
            json!({"prompt": "x", "quality": "premium"}),
            json!({"prompt": "x", "seed": 7}),
            json!({"prompt": "x".repeat(4001)}),
            json!(["prompt"]),
        ] {
            assert_eq!(
                pack.prepare("image-v1", &input),
                Err(ContractError::IndeterminableCost),
                "{input}"
            );
        }
        assert_eq!(
            pack.prepare("image-v2", &json!({"prompt": "x"})),
            Err(ContractError::UnsupportedTarget)
        );
    }

    #[test]
    fn packs_cannot_take_over_engine_owned_safety_fields() {
        let retries = Pack::from_toml_str(&edited(
            "timeout_ms = 30000\n",
            "timeout_ms = 30000\nmax_retries = 2\n",
        ))
        .unwrap_err();
        assert_eq!(
            retries,
            PackError::EngineOwned {
                field: "transport.max_retries".into(),
                reason: "Gongbu never retries a provider mutation",
            }
        );
        assert!(retries
            .to_string()
            .contains("engine-owned field is not allowed"));
        let nested = Pack::from_toml_str(&edited(
            "rule_id = \"sandbox-image-v1-final-2026-10-19\"\n",
            "rule_id = \"sandbox-image-v1-final-2026-10-19\"\nsettlement = \"reported\"\n",
        ))
        .unwrap_err();
        assert_eq!(
            nested.to_string(),
            "models[0].pricing_rules[1].settlement: engine-owned field is not allowed in a pack; \
             the engine settles only from the frozen pricing snapshot"
        );

        let leaked = Pack::from_toml_str(&edited(
            "hosts = [\"cdn.sandbox-image.test\"]",
            "hosts = [\"cdn.sandbox-image.test\", \"api.sandbox-image.test\"]",
        ))
        .unwrap_err();
        assert_eq!(
            leaked,
            PackError::CredentialBearingArtifactHost {
                host: "api.sandbox-image.test".into()
            }
        );
        assert!(leaked.to_string().contains("credential-bearing host"));
    }

    #[test]
    fn pricing_must_cover_every_input_and_every_hold_is_bounded() {
        let field = |source: String| match Pack::from_toml_str(&source).unwrap_err() {
            PackError::Invalid { field, .. } => field,
            other => panic!("{other}"),
        };
        // Removing the final-quality rule leaves a selectable input unpriced.
        let start = FIXTURE.rfind("[[models.pricing_rules]]").unwrap();
        assert_eq!(
            field(FIXTURE[..start].to_owned()),
            "models.image-v1.pricing_rules"
        );
        assert_eq!(field(edited("max = 4\n", "")), "parameters[1].quantity");
        assert_eq!(
            field(edited("quantity = \"image_count\"\n", "")),
            "models[0].pricing_rules[0].components"
        );
        assert_eq!(
            field(edited("max_length = 4000\n", "")),
            "parameters[0].max_length"
        );
        assert_eq!(
            field(edited("pricing_reviewed_on = \"2026-10-19\"\n", "")),
            "models[0].pricing_reviewed_on"
        );
        assert_eq!(
            field(edited(
                "allowed_hosts = [\"api.sandbox-image.test\"]",
                "allowed_hosts = [\"other.sandbox-image.test\"]"
            )),
            "transport.allowed_hosts"
        );
        assert_eq!(
            field(edited("response_format = \"url\"", "n = 3")),
            "fixed.n"
        );
        assert_eq!(
            field(edited(
                "media_types = [\"image/png\"]",
                "media_types = [\"text/html\"]"
            )),
            "artifacts.media_types"
        );
        // An unverified pack may omit its review date.
        let unverified = edited(
            "pricing_trust = \"verified\"",
            "pricing_trust = \"unverified\"",
        )
        .replace("pricing_reviewed_on = \"2026-10-19\"\n", "");
        assert_eq!(
            Pack::from_toml_str(&unverified).unwrap().pricing_trust(),
            PricingTrust::Unverified
        );
        assert!(matches!(
            Pack::from_toml_str("schema_version = 1\nnot = [valid"),
            Err(PackError::Parse(_))
        ));
        assert!(matches!(
            Pack::from_toml_str(&edited("display_name", "unknown_field = 1\ndisplay_name")),
            Err(PackError::Parse(_))
        ));
    }

    #[test]
    fn every_model_and_selector_rule_must_freeze_declared_quantities() {
        for (model_index, rule_index) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let mut document: PackDocument = toml::from_str(FIXTURE).unwrap();
            let mut second_model = document.models[0].clone();
            second_model.model = "image-v2".into();
            second_model.provider_config_version = "sandbox-image-v2-2026-10-19".into();
            for rule in &mut second_model.pricing_rules {
                rule.rule_id.push_str("-v2");
            }
            document.models.push(second_model);
            document.models[model_index].pricing_rules[rule_index]
                .components
                .retain(|component| component.unit != PricingUnit::Image);

            let error = Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap_err();
            match error {
                PackError::Invalid { field, reason } => {
                    assert_eq!(
                        field,
                        format!("models[{model_index}].pricing_rules[{rule_index}].components")
                    );
                    assert!(reason.contains("declares priced quantity image_count"));
                }
                other => panic!("{other}"),
            }
        }
    }

    #[test]
    fn flat_request_bundles_allow_bounded_parameters_and_zero_rate_quantities() {
        for freeze_quantity in [false, true] {
            let mut document: PackDocument = toml::from_str(FIXTURE).unwrap();
            if !freeze_quantity {
                document.parameters[1].quantity = None;
            }
            for rule in &mut document.models[0].pricing_rules {
                if freeze_quantity {
                    for component in &mut rule.components {
                        if component.unit == PricingUnit::Image {
                            component.rate_numerator_minor = 0;
                        }
                    }
                } else {
                    rule.components
                        .retain(|component| component.unit != PricingUnit::Image);
                }
            }
            let pack = Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap();
            let catalog = bind(&pack, &pack).unwrap();
            for count in [1, 4] {
                let prepared = pack
                    .prepare(
                        "image-v1",
                        &json!({"prompt": "a red circle", "image_count": count}),
                    )
                    .unwrap();
                assert_eq!(prepared.body["n"], count);
                let snapshot = catalog.pricing().snapshot(&prepared.request).unwrap();
                assert_eq!(snapshot.estimated_units, 1_000);
                assert_eq!(
                    snapshot.estimated_quantity(PricingUnit::Image),
                    freeze_quantity.then_some(count)
                );
                let usage = NormalizedUsage {
                    images: Some(count),
                    ..Default::default()
                };
                assert_eq!(snapshot.settle(&usage, 1_000).unwrap(), 1_000);
                if freeze_quantity {
                    assert_eq!(
                        snapshot.settle(
                            &NormalizedUsage {
                                images: Some(count + 1),
                                ..Default::default()
                            },
                            1_000,
                        ),
                        Err(ContractError::SettlementOverage)
                    );
                }
            }
            assert!(pack
                .prepare(
                    "image-v1",
                    &json!({"prompt": "a red circle", "image_count": 5}),
                )
                .is_err());
        }
    }

    #[test]
    fn a_target_pinned_to_another_pack_digest_does_not_bind() {
        let pack = Pack::from_toml_str(FIXTURE).unwrap();
        let edited_pack =
            Pack::from_toml_str(&edited("max_length = 4000", "max_length = 2000")).unwrap();
        // Targets frozen against the original pack, registry holding the edit.
        assert!(matches!(
            bind(&edited_pack, &pack),
            Err(RegistryError::BindingFailed(_))
        ));
        let no_engine = {
            let secret = SecretReference::new("gongbu.sandbox-image", "fixture").unwrap();
            let targets: ProviderTargetConfig = serde_json::from_value(json!({
                "schema_version": 2,
                "provider_configs": pack.provider_configs(&secret),
            }))
            .unwrap();
            let pricing = PricingCatalog::from_json(
                &serde_json::to_vec(&json!({
                    "schema_version": 2,
                    "catalog_version": "v",
                    "rules": pack.pricing_rules(),
                }))
                .unwrap(),
            )
            .unwrap();
            let mut registry = ProviderRegistry::new();
            registry.register_pack(pack.clone());
            ValidatedProviderCatalog::bind(targets, pricing, &registry)
        };
        assert!(matches!(no_engine, Err(RegistryError::MissingFactory(_))));
    }

    #[test]
    fn response_mapping_failures_after_acceptance_go_to_reconciliation() {
        let pack = Pack::from_toml_str(FIXTURE).unwrap();
        let response: Value = serde_json::from_str(RESPONSE).unwrap();
        let code = |response: &Value, downloads: &BTreeMap<String, Vec<u8>>| {
            let failure = pack.map_response(response, downloads).unwrap_err();
            assert_eq!(
                failure.spend_disposition,
                crate::provider_contract::SpendDisposition::Reconcile
            );
            failure.code
        };
        let mut missing_id = response.clone();
        missing_id.as_object_mut().unwrap().remove("id");
        assert_eq!(
            code(&missing_id, &downloads()),
            "pack_response_request_id_missing"
        );
        let mut foreign = response.clone();
        foreign["data"][0]["url"] = json!("https://evil.test/out.png");
        assert_eq!(code(&foreign, &downloads()), "artifact_policy_failure");
        let mut not_png = downloads();
        not_png.insert(
            "https://cdn.sandbox-image.test/out/7f3a-0.png".into(),
            b"<html>".to_vec(),
        );
        assert_eq!(code(&response, &not_png), "artifact_policy_failure");
        assert_eq!(code(&response, &BTreeMap::new()), "artifact_fetch_failed");
        assert_eq!(
            decimal_cost(&json!("0.012")),
            Some(ActualVendorCost::new(12, 3, "USD").unwrap())
        );
        assert_eq!(decimal_cost(&json!(-1)), None);
    }
}
