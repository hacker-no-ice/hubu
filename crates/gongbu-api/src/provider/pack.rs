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
        ActualVendorCost, AdapterCapabilities, AdapterOutcome, ContractError, InputImageSource,
        NormalizedArtifact, NormalizedRequest, NormalizedUsage, OutputDimensions, PriceComponent,
        PricingCatalog, PricingRule, PricingSelector, PricingUnit, ProviderAdapter,
        ProviderFailure, ProviderPhase, DEFAULT_WORKLOAD_TYPE, DERIVED_QUANTITIES,
        MAX_VENDOR_COST_SCALE, REQUEST_QUANTITIES,
    },
    http_kernel::{valid_provider_deadline_ms, validate_https_origin, validate_safe_headers},
    registry::InputImagesParameter,
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

/// Poll path or query value that expands to the job ID.
const JOB_ID_PLACEHOLDER: &str = "{{job_id}}";

/// Template placeholders that expand to the certified output size.
const OUTPUT_WIDTH_PLACEHOLDER: &str = "output_width";
const OUTPUT_HEIGHT_PLACEHOLDER: &str = "output_height";

/// Template placeholder that expands to the target's model ID.
const MODEL_PLACEHOLDER: &str = "model";

/// Usage fields and the request quantity each one settles against.
const USAGE_QUANTITIES: &[(&str, &str)] = &[
    ("images", "image_count"),
    ("input_tokens", "input_tokens"),
    ("output_tokens", "max_output_tokens"),
    ("results", "results"),
    ("pages", "pages"),
    ("seconds", "seconds"),
    ("input_images", "input_images"),
];

/// The request quantity only an `images` parameter sets.
const INPUT_IMAGES_QUANTITY: &str = "input_images";
/// Placeholders an `images` parameter's `item` template may use.
const IMAGE_MEDIA_TYPE_PLACEHOLDER: &str = "image.media_type";
const IMAGE_DATA_PLACEHOLDER: &str = "image.data";
/// The most input images one request may send.
const MAX_INPUT_IMAGES: i64 = 16;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request: Option<RequestTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poll: Option<PollSpec>,
    #[serde(default)]
    parameters: Vec<Parameter>,
    #[serde(default)]
    fixed: Map<String, Value>,
    response: ResponseMapping,
    artifacts: Artifacts,
    models: Vec<Model>,
    /// Certified output sizes keyed by a combination of enum parameter
    /// values, such as image size and aspect ratio. An alternative to a
    /// single parameter's `output_dimensions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    size_presets: Vec<SizePreset>,
    /// How billed megapixels are counted when a pricing rule prices the
    /// `megapixel` or `additional_megapixel` unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    megapixels: Option<MegapixelRule>,
}

/// One certified output size: the image the provider returns when every
/// parameter in `when` has the given value.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SizePreset {
    pub when: BTreeMap<String, String>,
    pub size: [u32; 2],
}

/// How an output image's size becomes billed megapixels: pixels divided by
/// `pixels_per_megapixel`, rounded as declared, never below `minimum`.
/// Integer arithmetic only.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MegapixelRule {
    pub rounding: MegapixelRounding,
    #[serde(default = "default_pixels_per_megapixel")]
    pub pixels_per_megapixel: u64,
    #[serde(default = "default_minimum_megapixels")]
    pub minimum: u32,
}

fn default_pixels_per_megapixel() -> u64 {
    1_000_000
}

fn default_minimum_megapixels() -> u32 {
    1
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MegapixelRounding {
    /// Partial megapixels are free.
    Floor,
    /// Any partial megapixel is billed whole.
    Ceil,
    /// Half a megapixel or more is billed whole.
    Nearest,
}

impl MegapixelRule {
    /// Billed megapixels for one image.
    pub fn billed(&self, width: u32, height: u32) -> i64 {
        let pixels = u64::from(width) * u64::from(height);
        let per = self.pixels_per_megapixel;
        let (whole, rest) = (pixels / per, pixels % per);
        let billed = match self.rounding {
            MegapixelRounding::Floor => whole,
            MegapixelRounding::Ceil => whole + u64::from(rest > 0),
            MegapixelRounding::Nearest => whole + u64::from(rest >= per - per / 2),
        };
        // At most 65,535² pixels, so this always fits.
        i64::try_from(billed.max(u64::from(self.minimum))).unwrap_or(i64::MAX)
    }

    /// `(megapixels, additional_megapixels)` for one image.
    fn quantities(&self, width: u32, height: u32) -> (i64, i64) {
        let billed = self.billed(width, height);
        (billed, (billed - 1).max(0))
    }
}

/// The certified output size discovery shows for one preset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OutputSize {
    pub when: BTreeMap<String, String>,
    pub width: u32,
    pub height: u32,
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
    #[serde(default, skip_serializing_if = "AuthScheme::is_raw")]
    pub auth_scheme: AuthScheme,
}

/// How the credential is written into `auth_header`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    /// The secret as is.
    #[default]
    Raw,
    /// `Bearer <secret>`.
    Bearer,
}

impl AuthScheme {
    fn is_raw(&self) -> bool {
        *self == Self::Raw
    }
    pub fn header_value(self, secret: &str) -> String {
        match self {
            Self::Raw => secret.to_owned(),
            Self::Bearer => format!("Bearer {secret}"),
        }
    }
}

/// The provider request an engine sends: a JSON POST to `path` on the
/// endpoint. `body` is a template; a string that is exactly `{{name}}`
/// becomes the typed value of parameter `name` (or the model ID for
/// `{{model}}`), `{{name|upper}}` and `{{name|lower}}` change a string's case,
/// and a key whose placeholder has no value is left out.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestTemplate {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Map<String, Value>>,
}

/// How an `async_poll` engine follows a submitted job. The engine rebuilds
/// every poll URL from this declared shape and a checkpointed host; it never
/// follows a provider-supplied URL verbatim.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PollSpec {
    /// Response path to the job ID in the submission response.
    pub job_id: String,
    /// Poll path on the polling host, such as `/v1/get_result`.
    pub path: String,
    /// Poll query; a value of exactly `{{job_id}}` becomes the job ID.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Optional response path to a provider-returned polling URL. Its host is
    /// adopted only when it matches `hosts` and its path and query match the
    /// declared shape; otherwise the engine polls the endpoint host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Hosts (or one-label patterns) a returned polling URL may name. They
    /// receive the credential, so each must also be in `transport.allowed_hosts`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    pub interval_ms: u64,
    /// Response path to the job status, in submission and poll responses.
    pub status: String,
    /// Status values (compared case-insensitively) meaning "keep polling".
    pub pending: Vec<String>,
    /// Status values meaning the result is ready to map.
    pub ready: Vec<String>,
    /// Status values the provider uses to refuse the work for good, each with
    /// the failure code to record. The claim is released.
    #[serde(default)]
    pub rejected: BTreeMap<String, String>,
    /// Status values whose outcome is unknown, each with its failure code. The
    /// execution goes to reconciliation.
    #[serde(default)]
    pub ambiguous: BTreeMap<String, String>,
}

/// What a job status means to an engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Ready,
    Rejected(String),
    Ambiguous(String),
    /// A status the pack does not declare.
    Unknown,
}

impl PollSpec {
    pub fn classify(&self, status: &str) -> JobState {
        let status = status.trim().to_ascii_lowercase();
        if self.pending.contains(&status) {
            JobState::Pending
        } else if self.ready.contains(&status) {
            JobState::Ready
        } else if let Some(code) = self.rejected.get(&status) {
            JobState::Rejected(code.clone())
        } else if let Some(code) = self.ambiguous.get(&status) {
            JobState::Ambiguous(code.clone())
        } else {
            JobState::Unknown
        }
    }
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
    /// The certified output size for each value of an image-preset enum. The
    /// chosen size becomes the request's frozen output dimensions and the
    /// `{{output_width}}` / `{{output_height}}` placeholders.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_dimensions: BTreeMap<String, [u32; 2]>,
    /// Ties an integer parameter to a derived output dimension: if supplied,
    /// it must equal the size the preset certifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Derived>,
    /// `images` only: the provider's shape for one input image, using
    /// `{{image.media_type}}` and `{{image.data}}` (base64 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<Map<String, Value>>,
}

/// A value derived from another parameter rather than chosen freely.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Derived {
    OutputWidth,
    OutputHeight,
}

impl Derived {
    fn placeholder(self) -> &'static str {
        match self {
            Self::OutputWidth => OUTPUT_WIDTH_PLACEHOLDER,
            Self::OutputHeight => OUTPUT_HEIGHT_PLACEHOLDER,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    String,
    Integer,
    Boolean,
    Enum,
    /// Artifact IDs of images earlier governed executions delivered to the
    /// same account. The engine loads and sends their verified bytes.
    Images,
}

/// Response paths into the provider's JSON body. A path is a JSON Pointer
/// whose segments may filter an array, as in `/steps[type=model_output]`,
/// which keeps the elements of `steps` whose `type` is `model_output`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResponseMapping {
    /// Response headers that carry the provider request ID, checked in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_id_headers: Vec<String>,
    /// Body path to the provider request ID, used when no header carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    /// Usage quantity → pointer to an integer or an array whose length counts.
    #[serde(default)]
    pub usage: BTreeMap<String, String>,
    /// Informational usage fields the provider may omit. Priced quantities
    /// always remain required when mapped.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub optional_usage: BTreeSet<String>,
    /// Pointer to the provider-reported cost, a decimal in major currency units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost: Option<String>,
    /// Decimal places the reported cost is shifted by: 2 when the provider
    /// reports credits worth one cent each. Defaults to 0 (whole USD).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reported_cost_scale: u32,
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
    /// Narrows an enum parameter to the values this model supports, such as
    /// the size presets a smaller model offers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameter_values: BTreeMap<String, Vec<String>>,
    pub pricing_rules: Vec<ModelPricingRule>,
}

impl Model {
    /// The values `parameter` accepts for this model.
    fn values_for<'a>(&'a self, parameter: &'a Parameter) -> &'a [String] {
        self.parameter_values
            .get(&parameter.name)
            .map_or(parameter.values.as_slice(), Vec::as_slice)
    }
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
    pub fn poll(&self) -> Option<&PollSpec> {
        self.document.poll.as_ref()
    }

    /// The poll URL for `job_id` on `host`, built only from the declared
    /// shape. `host` must be the endpoint host or a declared polling host,
    /// and the job ID must be a plain token.
    pub fn poll_url(&self, host: &str, job_id: &str) -> Option<Url> {
        let poll = self.document.poll.as_ref()?;
        let endpoint = Url::parse(&self.document.transport.endpoint).ok()?;
        if !(endpoint.host_str() == Some(host) || host_allowed(&poll.hosts, host))
            || !safe_job_id(job_id)
        {
            return None;
        }
        let mut url = Url::parse(&format!("https://{host}/")).ok()?;
        url.set_path(&poll.path.replace(JOB_ID_PLACEHOLDER, job_id));
        if !poll.query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in &poll.query {
                pairs.append_pair(
                    key,
                    if value == JOB_ID_PLACEHOLDER {
                        job_id
                    } else {
                        value
                    },
                );
            }
        }
        Some(url)
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
                    workload_type: (self.document.workload_type != DEFAULT_WORKLOAD_TYPE)
                        .then(|| self.document.workload_type.clone()),
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
    /// priced request and the provider request body. A request that sends
    /// input images has a `Null` body here: only the engine, holding the
    /// images, renders it with [`Pack::prepare_with_images`].
    pub fn prepare(&self, model: &str, input: &Value) -> Result<PreparedRequest, ContractError> {
        self.prepare_inner(model, input, None)
    }

    /// [`Pack::prepare`], loading the input images the request references
    /// from `images` and rendering the body that sends them.
    pub fn prepare_with_images(
        &self,
        model: &str,
        input: &Value,
        images: &dyn InputImageSource,
    ) -> Result<PreparedRequest, ContractError> {
        self.prepare_inner(model, input, Some(images))
    }

    /// The parameter that takes input images, for discovery.
    pub fn input_images_parameter(&self) -> Option<InputImagesParameter> {
        self.document
            .parameters
            .iter()
            .find(|p| p.kind == ParameterKind::Images)
            .map(|p| InputImagesParameter {
                parameter: p.name.clone(),
                min: p.min.unwrap_or(1),
                max: p.max.unwrap_or(1),
            })
    }

    /// The artifact IDs `input` names in its images parameters, in order.
    pub fn input_artifact_ids(&self, input: &Value) -> Vec<String> {
        let Some(input) = input.as_object() else {
            return Vec::new();
        };
        self.document
            .parameters
            .iter()
            .filter(|p| p.kind == ParameterKind::Images)
            .filter_map(|p| p.input_value(input)?.as_array())
            .flatten()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect()
    }

    fn prepare_inner(
        &self,
        model: &str,
        input: &Value,
        images: Option<&dyn InputImageSource>,
    ) -> Result<PreparedRequest, ContractError> {
        let indeterminable = || ContractError::IndeterminableCost;
        let model = self.model(model).ok_or(ContractError::UnsupportedTarget)?;
        let input = input.as_object().ok_or_else(indeterminable)?;
        for (key, value) in input {
            let members: Vec<_> = self
                .document
                .parameters
                .iter()
                .filter(|p| p.group() == Some(key.as_str()))
                .collect();
            if members.is_empty() {
                if !self.document.parameters.iter().any(|p| &p.name == key) {
                    return Err(indeterminable());
                }
            } else {
                let group = value.as_object().ok_or_else(indeterminable)?;
                if group.keys().any(|member| {
                    !members
                        .iter()
                        .any(|p| p.name.split_once('.').map(|(_, m)| m) == Some(member.as_str()))
                }) {
                    return Err(indeterminable());
                }
            }
        }
        // The certified output size comes from the preset parameters, so it
        // is known before any parameter tied to it is checked.
        let dimensions = self.output_size(input);
        let mut request = NormalizedRequest::new(&self.document.provider, &model.model)
            .with_workload(&self.document.workload_type);
        let mut values = BTreeMap::from([(MODEL_PLACEHOLDER.to_owned(), json!(model.model))]);
        let mut spliced = BTreeSet::new();
        let mut unrendered = false;
        if let Some(dimensions) = &dimensions {
            values.insert(OUTPUT_WIDTH_PLACEHOLDER.to_owned(), json!(dimensions.width));
            values.insert(
                OUTPUT_HEIGHT_PLACEHOLDER.to_owned(),
                json!(dimensions.height),
            );
        }
        let mut flat = self.document.fixed.clone();
        for parameter in &self.document.parameters {
            let value = match parameter.input_value(input).or(parameter.default.as_ref()) {
                Some(value) => value.clone(),
                None if parameter.required => return Err(indeterminable()),
                None => continue,
            };
            if parameter.check(&value).is_err()
                || (parameter.kind == ParameterKind::Enum
                    && !model
                        .values_for(parameter)
                        .iter()
                        .any(|allowed| Some(allowed.as_str()) == value.as_str()))
            {
                return Err(indeterminable());
            }
            if let Some(derived) = parameter.equals {
                let expected = dimensions.as_ref().map(|d| match derived {
                    Derived::OutputWidth => i64::from(d.width),
                    Derived::OutputHeight => i64::from(d.height),
                });
                if expected.is_none() || value.as_i64() != expected {
                    return Err(indeterminable());
                }
            }
            if parameter.kind == ParameterKind::Images {
                let ids = value.as_array().ok_or_else(indeterminable)?;
                if let Some(quantity) = &parameter.quantity {
                    request.set_quantity(quantity, Some(ids.len() as i64));
                }
                let Some(images) = images else {
                    unrendered = true;
                    continue;
                };
                let item = parameter.item.as_ref().ok_or_else(indeterminable)?;
                let mut items = Vec::with_capacity(ids.len());
                for id in ids {
                    let image = images.load(id.as_str().ok_or_else(indeterminable)?)?;
                    let fields = BTreeMap::from([
                        (
                            IMAGE_MEDIA_TYPE_PLACEHOLDER.to_owned(),
                            json!(image.media_type),
                        ),
                        (
                            IMAGE_DATA_PLACEHOLDER.to_owned(),
                            json!(base64::engine::general_purpose::STANDARD.encode(&image.bytes)),
                        ),
                    ]);
                    items.push(
                        render_template(&Value::Object(item.clone()), &fields, &BTreeSet::new())
                            .ok_or_else(indeterminable)?,
                    );
                }
                spliced.insert(parameter.name.clone());
                values.insert(parameter.name.clone(), Value::Array(items));
                continue;
            }
            if let Some(quantity) = &parameter.quantity {
                request.set_quantity(quantity, value.as_i64());
            }
            if let Some(key) = &parameter.selector {
                request
                    .selector
                    .insert(key, value.as_str().ok_or_else(indeterminable)?);
            }
            flat.insert(parameter.field().to_owned(), value.clone());
            values.insert(parameter.name.clone(), value);
        }
        if let Some(rule) = &self.document.megapixels {
            // Billed megapixels come only from the certified size: an input
            // without one cannot be priced.
            let size = dimensions.as_ref().ok_or_else(indeterminable)?;
            let (megapixels, additional) = rule.quantities(size.width, size.height);
            let images = request.image_count().unwrap_or(1);
            for (name, per_image) in [
                ("megapixels", megapixels),
                ("additional_megapixels", additional),
            ] {
                if self.derived_quantities().contains(&name) {
                    let total = per_image.checked_mul(images).ok_or_else(indeterminable)?;
                    request.set_quantity(name, Some(total));
                }
            }
        }
        request.output_dimensions = dimensions;
        request.validate()?;
        let body = match self.document.request.as_ref().and_then(|r| r.body.as_ref()) {
            Some(_) if unrendered => Value::Null,
            Some(template) => render_template(&Value::Object(template.clone()), &values, &spliced)
                .ok_or_else(indeterminable)?,
            None => Value::Object(flat),
        };
        Ok(PreparedRequest { request, body })
    }

    /// The certified output size `input` selects: from `size_presets`, or
    /// from the one parameter that declares `output_dimensions`. Absent
    /// parameters take their defaults.
    fn output_size(&self, input: &Map<String, Value>) -> Option<OutputDimensions> {
        let value_of = |name: &str| {
            let parameter = self.document.parameters.iter().find(|p| p.name == name)?;
            parameter
                .input_value(input)
                .or(parameter.default.as_ref())?
                .as_str()
        };
        let [width, height] = if self.document.size_presets.is_empty() {
            let parameter = self
                .document
                .parameters
                .iter()
                .find(|p| !p.output_dimensions.is_empty())?;
            *parameter
                .output_dimensions
                .get(value_of(&parameter.name)?)?
        } else {
            self.document
                .size_presets
                .iter()
                .find(|preset| {
                    preset
                        .when
                        .iter()
                        .all(|(name, value)| value_of(name) == Some(value.as_str()))
                })?
                .size
        };
        Some(OutputDimensions { width, height })
    }

    /// Whether sizes are certified by parameter combinations.
    pub fn has_size_presets(&self) -> bool {
        !self.document.size_presets.is_empty()
    }

    /// The megapixel quantities this pack's pricing rules price.
    fn derived_quantities(&self) -> BTreeSet<&'static str> {
        self.document.derived_quantities()
    }

    /// The certified output sizes `model` offers, for discovery.
    pub fn output_sizes(&self, model: &str) -> Vec<OutputSize> {
        let Some(model) = self.model(model) else {
            return Vec::new();
        };
        let offered = |name: &str, value: &str| {
            self.document
                .parameters
                .iter()
                .find(|p| p.name == name)
                .is_some_and(|p| model.values_for(p).iter().any(|v| v == value))
        };
        if self.document.size_presets.is_empty() {
            return self
                .document
                .parameters
                .iter()
                .filter(|p| !p.output_dimensions.is_empty())
                .flat_map(|p| {
                    p.output_dimensions
                        .iter()
                        .filter(|(value, _)| offered(&p.name, value))
                        .map(|(value, [width, height])| OutputSize {
                            when: BTreeMap::from([(p.name.clone(), value.clone())]),
                            width: *width,
                            height: *height,
                        })
                })
                .collect();
        }
        self.document
            .size_presets
            .iter()
            .filter(|preset| preset.when.iter().all(|(name, value)| offered(name, value)))
            .map(|preset| OutputSize {
                when: preset.when.clone(),
                width: preset.size[0],
                height: preset.size[1],
            })
            .collect()
    }

    /// How this pack counts billed megapixels, when it prices them.
    pub fn megapixel_rule(&self) -> Option<&MegapixelRule> {
        self.document.megapixels.as_ref()
    }

    /// The provider request URL: the endpoint plus the declared request path.
    pub fn request_url(&self) -> Url {
        let mut url = Url::parse(&self.document.transport.endpoint).expect("validated endpoint");
        url.set_path(
            self.document
                .request
                .as_ref()
                .map_or("/", |request| request.path.as_str()),
        );
        url
    }

    /// The provider's request ID: the first declared header present, else the
    /// declared body path.
    pub fn provider_request_id(&self, response: &ProviderResponse) -> Option<String> {
        let mapping = &self.document.response;
        mapping
            .request_id_headers
            .iter()
            .find_map(|name| response.headers.get(&name.to_ascii_lowercase()).cloned())
            .or_else(|| {
                mapping.provider_request_id.as_deref().and_then(|path| {
                    select_path(&response.body, path)
                        .first()
                        .and_then(|value| match value {
                            Value::String(s) => Some(s.trim().to_owned()),
                            Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                })
            })
            .filter(|id| !id.is_empty() && id.len() <= 255)
    }

    /// Map a successful provider response to a normalized outcome. Anything the
    /// mapping cannot read is an ambiguous outcome, because the provider
    /// accepted the work; every failure keeps the request ID as evidence.
    ///
    /// Usage the pack does not map settles at the requested quantity: a
    /// fixed-price call delivers what it was asked for unless the response
    /// reports less.
    ///
    /// `known` carries evidence the engine already holds, such as the request
    /// ID from a submission and the job ID being polled. Preserve that request
    /// ID so a poll response cannot replace the durable operation identity;
    /// the response supplies it only when no earlier ID is known.
    pub fn map_response(
        &self,
        request: &NormalizedRequest,
        response: &ProviderResponse,
        known: &Evidence,
        downloads: &dyn ArtifactSource,
        max_artifact_bytes: usize,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let mapping = &self.document.response;
        let request_id = known
            .request_id
            .clone()
            .or_else(|| self.provider_request_id(response));
        let operation_id = known.operation_id.clone();
        let fail = |code: &str, phase: ProviderPhase| {
            ProviderFailure::reconcile(code.to_owned(), phase)
                .with_evidence(request_id.clone(), operation_id.clone())
        };
        let ambiguous = |code: &str| fail(code, ProviderPhase::Processing);
        if request_id.is_none() && operation_id.is_none() {
            return Err(ambiguous("pack_response_request_id_missing"));
        }
        let body = &response.body;
        let mut usage = NormalizedUsage::default();
        for (quantity, path) in &mapping.usage {
            let count = match usage_count(body, path) {
                Ok(count) => count,
                Err(UsagePathError::Missing) if mapping.optional_usage.contains(quantity) => {
                    continue
                }
                Err(_) => return Err(ambiguous("pack_response_usage_invalid")),
            };
            *usage_slot(&mut usage, quantity).expect("validated usage quantity") = Some(count);
        }
        for (field, quantity) in USAGE_QUANTITIES {
            let slot = usage_slot(&mut usage, field).expect("known usage quantity");
            if slot.is_none() {
                *slot = request.quantity(quantity);
            }
        }
        // Fixed-price settlement uses frozen prices and delivered usage;
        // a provider-controlled amount must never override that calculation.
        let actual_vendor_cost = if self.archetype() == Archetype::SyncFixed {
            None
        } else {
            match mapping
                .reported_cost
                .as_deref()
                .and_then(|path| select_path(body, path).first().copied())
            {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    decimal_cost(value, mapping.reported_cost_scale)
                        .ok_or_else(|| ambiguous("pack_response_cost_invalid"))?,
                ),
            }
        };
        let items = match select_path(body, &mapping.artifact_items).as_slice() {
            [Value::Array(items)] => items.iter().collect::<Vec<_>>(),
            items => items.to_vec(),
        };
        if items.is_empty() {
            return Err(ambiguous("pack_response_artifacts_missing"));
        }
        let policy_failure = || fail("artifact_policy_failure", ProviderPhase::Artifact);
        // More images than were asked for (and priced) is a contract breach.
        if request
            .image_count()
            .is_some_and(|count| items.len() as u64 > count.max(0) as u64)
        {
            return Err(policy_failure());
        }
        let mut artifacts = Vec::with_capacity(items.len());
        for item in items {
            let bytes = if let Some(path) = &mapping.artifact_base64 {
                let encoded = select_path(item, path)
                    .first()
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| ambiguous("pack_response_artifact_invalid"))?;
                if encoded.len() > max_artifact_bytes.saturating_mul(4).div_ceil(3) {
                    return Err(policy_failure());
                }
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| ambiguous("pack_response_artifact_invalid"))?
            } else {
                let path = mapping.artifact_url.as_deref().expect("validated mapping");
                let url = select_path(item, path)
                    .first()
                    .and_then(|value| value.as_str())
                    .and_then(|raw| Url::parse(raw).ok())
                    .ok_or_else(|| ambiguous("pack_response_artifact_invalid"))?;
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.port().is_some()
                    || url.fragment().is_some()
                    || !url
                        .host_str()
                        .is_some_and(|host| host_allowed(&self.document.artifacts.hosts, host))
                {
                    return Err(policy_failure());
                }
                match downloads.fetch(&url, max_artifact_bytes) {
                    Ok(bytes) => bytes,
                    Err(ArtifactFetchError::TooLarge) => return Err(policy_failure()),
                    Err(ArtifactFetchError::Failed) => {
                        return Err(fail("artifact_fetch_failed", ProviderPhase::Artifact))
                    }
                }
            };
            if bytes.len() > max_artifact_bytes {
                return Err(policy_failure());
            }
            let media_type = sniff_media_type(&bytes)
                .filter(|media| {
                    self.document
                        .artifacts
                        .media_types
                        .iter()
                        .any(|m| m == media)
                })
                .ok_or_else(policy_failure)?;
            artifacts.push(NormalizedArtifact {
                media_type: media_type.to_owned(),
                bytes,
            });
        }
        // Megapixels settle from what was delivered, never from what was
        // asked: each image's decoded size, under the pack's declared rule.
        if let Some(rule) = &self.document.megapixels {
            let (mut megapixels, mut additional) = (0_i64, 0_i64);
            for artifact in &artifacts {
                let (width, height) =
                    image_dimensions(&artifact.bytes).ok_or_else(policy_failure)?;
                let (billed, extra) = rule.quantities(width, height);
                megapixels = megapixels.saturating_add(billed);
                additional = additional.saturating_add(extra);
            }
            usage.megapixels = Some(megapixels);
            usage.additional_megapixels = Some(additional);
        }
        Ok(AdapterOutcome {
            usage: Some(usage),
            actual_vendor_cost,
            provider_request_id: request_id,
            provider_operation_id: operation_id,
            artifacts,
        })
    }
}

/// Provider identifiers an engine already holds when it maps a response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
}

/// The priced request plus the provider request body an engine sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    pub request: NormalizedRequest,
    pub body: Value,
}

/// A provider response as an engine received it: lowercase header names and
/// the parsed JSON body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderResponse {
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactFetchError {
    /// The artifact is larger than the engine's artifact bound.
    TooLarge,
    Failed,
}

/// Downloads artifact bytes from an already allowlisted URL, never sending
/// credentials and never reading past `max_bytes`.
pub trait ArtifactSource {
    fn fetch(&self, url: &Url, max_bytes: usize) -> Result<Vec<u8>, ArtifactFetchError>;
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
            ParameterKind::Images => {
                let ids = value.as_array().ok_or("must be an array of artifact IDs")?;
                let unique: BTreeSet<_> = ids.iter().filter_map(Value::as_str).collect();
                if unique.len() != ids.len() || !unique.iter().all(|id| valid_artifact_id(id)) {
                    return Err("must name distinct artifact IDs".into());
                }
                let count = ids.len() as i64;
                if count < self.min.unwrap_or(1) || self.max.is_some_and(|max| count > max) {
                    return Err("is outside min..=max images".into());
                }
            }
        }
        Ok(())
    }

    /// The input group this parameter lives in (`options` for `options.seed`).
    fn group(&self) -> Option<&str> {
        self.name.split_once('.').map(|(group, _)| group)
    }

    /// The input value for this parameter: top level, or inside its group.
    fn input_value<'a>(&self, input: &'a Map<String, Value>) -> Option<&'a Value> {
        match self.name.split_once('.') {
            Some((group, member)) => input.get(group)?.as_object()?.get(member),
            None => input.get(&self.name),
        }
    }

    fn validate(&self, field: &str) -> Result<(), PackError> {
        if !valid_parameter_name(&self.name) {
            return Err(invalid(
                format!("{field}.name"),
                "must be a lowercase identifier, optionally in one group such as options.seed",
            ));
        }
        let reserved = match self.name.as_str() {
            MODEL_PLACEHOLDER => Some("model is reserved for the selected target's model ID"),
            OUTPUT_WIDTH_PLACEHOLDER => {
                Some("output_width is reserved for the certified output width")
            }
            OUTPUT_HEIGHT_PLACEHOLDER => {
                Some("output_height is reserved for the certified output height")
            }
            _ => None,
        };
        if let Some(reason) = reserved {
            return Err(invalid(format!("{field}.name"), reason));
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
        let images = self.kind == ParameterKind::Images;
        if self.min.is_some() {
            only(integer || images, "min")?;
        }
        if self.max.is_some() {
            only(integer || images, "max")?;
        }
        if self.quantity.is_some() {
            only(integer || images, "quantity")?;
        }
        if self.item.is_some() {
            only(images, "item")?;
        }
        if images {
            self.validate_images(field)?;
        } else if self.quantity.as_deref() == Some(INPUT_IMAGES_QUANTITY) {
            return Err(invalid(
                format!("{field}.quantity"),
                "input_images is set only by an images parameter",
            ));
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
        if self.equals.is_some() {
            only(integer, "equals")?;
        }
        if !self.output_dimensions.is_empty() {
            only(enumeration && self.selector.is_some(), "output_dimensions")?;
            let keys: BTreeSet<_> = self.output_dimensions.keys().collect();
            let values: BTreeSet<_> = self.values.iter().collect();
            if keys != values
                || self.output_dimensions.values().any(|[width, height]| {
                    *width == 0 || *height == 0 || *width > 65_535 || *height > 65_535
                })
            {
                return Err(invalid(
                    format!("{field}.output_dimensions"),
                    "must give a positive [width, height] for exactly each enum value",
                ));
            }
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
                    selector.insert(self.selector.as_deref().unwrap_or("value"), value.as_str());
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
            let min = if images {
                Some(self.min.unwrap_or(1))
            } else {
                self.min
            };
            if min.is_none_or(|min| min < 0) || self.max.is_none() {
                return Err(invalid(
                    format!("{field}.quantity"),
                    "priced quantities need min >= 0 and a max, so every hold is bounded",
                ));
            }
        }
        if self
            .selector
            .as_deref()
            .is_some_and(|key| !valid_field_name(key))
        {
            return Err(invalid(
                format!("{field}.selector"),
                "must be a lowercase identifier",
            ));
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

impl Parameter {
    /// An `images` parameter is required, bounded, and declares how the
    /// provider receives one image.
    fn validate_images(&self, field: &str) -> Result<(), PackError> {
        if !self.required || self.default.is_some() {
            return Err(invalid(
                format!("{field}.required"),
                "an images parameter is required and has no default",
            ));
        }
        let min = self.min.unwrap_or(1);
        if min < 1
            || self
                .max
                .is_none_or(|max| max < min || max > MAX_INPUT_IMAGES)
        {
            return Err(invalid(
                format!("{field}.max"),
                format!("an images parameter takes 1 to {MAX_INPUT_IMAGES} images, min <= max"),
            ));
        }
        if self
            .quantity
            .as_deref()
            .is_some_and(|quantity| quantity != INPUT_IMAGES_QUANTITY)
        {
            return Err(invalid(
                format!("{field}.quantity"),
                "an images parameter sets only input_images",
            ));
        }
        let item = self.item.as_ref().ok_or_else(|| {
            invalid(
                format!("{field}.item"),
                "an images parameter declares the provider's shape for one image",
            )
        })?;
        let mut referenced = BTreeSet::new();
        collect_placeholders(&Value::Object(item.clone()), &mut referenced)
            .map_err(|reason| invalid(format!("{field}.item"), reason))?;
        if referenced.iter().any(|(name, transform)| {
            transform.is_some()
                || !matches!(
                    name.as_str(),
                    IMAGE_MEDIA_TYPE_PLACEHOLDER | IMAGE_DATA_PLACEHOLDER
                )
        }) || !referenced
            .iter()
            .any(|(name, _)| name == IMAGE_DATA_PLACEHOLDER)
        {
            return Err(invalid(
                format!("{field}.item"),
                "must send {{image.data}} and may use only {{image.media_type}} besides",
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
                if DERIVED_QUANTITIES.contains(&quantity.as_str()) {
                    return Err(invalid(
                        format!("{field}.quantity"),
                        "is derived from the certified output size, never set by input",
                    ));
                }
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
        let presets = self
            .parameters
            .iter()
            .filter(|p| !p.output_dimensions.is_empty())
            .count();
        if presets > 1 {
            return Err(invalid(
                "parameters",
                "only one parameter may declare output_dimensions",
            ));
        }
        if presets > 0 && !self.size_presets.is_empty() {
            return Err(invalid(
                "size_presets",
                "declare sizes in size_presets or in one parameter's output_dimensions, not both",
            ));
        }
        self.validate_size_presets()?;
        if !self.has_size_source() && self.parameters.iter().any(|p| p.equals.is_some()) {
            return Err(invalid(
                "parameters",
                "equals needs size_presets or a parameter that declares output_dimensions",
            ));
        }
        self.validate_megapixels()?;
        quantities.extend(self.derived_quantities());
        if let Some(parameter) = self
            .parameters
            .iter()
            .find(|p| p.kind == ParameterKind::Images)
        {
            // Only the sync_fixed engine loads input images today.
            if self.archetype != Archetype::SyncFixed {
                return Err(invalid(
                    "archetype",
                    format!(
                        "images parameter {} needs the sync_fixed engine",
                        parameter.name
                    ),
                ));
            }
            if self
                .request
                .as_ref()
                .and_then(|r| r.body.as_ref())
                .is_none()
            {
                return Err(invalid(
                    "request.body",
                    format!("images parameter {} needs a body template", parameter.name),
                ));
            }
        }
        if let Some(parameter) = self.parameters.iter().find(|p| p.group().is_some()) {
            if self
                .request
                .as_ref()
                .and_then(|r| r.body.as_ref())
                .is_none()
            {
                return Err(invalid(
                    "request.body",
                    format!("grouped parameter {} needs a body template", parameter.name),
                ));
            }
            if self
                .parameters
                .iter()
                .any(|p| p.group().is_none() && Some(p.name.as_str()) == parameter.group())
            {
                return Err(invalid(
                    "parameters",
                    "a group name collides with a parameter name",
                ));
            }
        }
        if let Some(key) = self.fixed.keys().find(|key| fields.contains(key.as_str())) {
            return Err(invalid(
                format!("fixed.{key}"),
                "collides with a parameter field",
            ));
        }
        self.validate_request()?;
        self.validate_poll()?;
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
        let mut paths = vec![
            (
                "response.provider_request_id",
                response.provider_request_id.as_ref(),
            ),
            ("response.reported_cost", response.reported_cost.as_ref()),
            ("response.artifact_items", Some(&response.artifact_items)),
            ("response.artifact_url", response.artifact_url.as_ref()),
            (
                "response.artifact_base64",
                response.artifact_base64.as_ref(),
            ),
        ];
        for (quantity, path) in &response.usage {
            if !USAGE_QUANTITIES.iter().any(|(field, _)| field == quantity) {
                return Err(invalid(
                    format!("response.usage.{quantity}"),
                    "is not a usage quantity (images, input_tokens, output_tokens, results, pages, seconds)",
                ));
            }
            paths.push(("response.usage", Some(path)));
        }
        for quantity in &response.optional_usage {
            let field = format!("response.optional_usage.{quantity}");
            if !response.usage.contains_key(quantity) {
                return Err(invalid(field, "must name a mapped usage quantity"));
            }
            let request_quantity = USAGE_QUANTITIES
                .iter()
                .find_map(|(usage, request)| (*usage == quantity.as_str()).then_some(*request))
                .expect("validated usage quantity");
            if self
                .parameters
                .iter()
                .any(|parameter| parameter.quantity.as_deref() == Some(request_quantity))
            {
                return Err(invalid(
                    field,
                    "a declared priced quantity cannot have optional usage",
                ));
            }
        }
        for (field, path) in paths {
            if path.is_some_and(|path| !valid_path(path)) {
                return Err(invalid(
                    field,
                    "must be a response path such as /data/0/url or /steps[type=image]",
                ));
            }
        }
        if response.reported_cost_scale > MAX_VENDOR_COST_SCALE {
            return Err(invalid(
                "response.reported_cost_scale",
                format!("must be at most {MAX_VENDOR_COST_SCALE}"),
            ));
        }
        if response.provider_request_id.is_none() && response.request_id_headers.is_empty() {
            return Err(invalid(
                "response",
                "set provider_request_id, request_id_headers, or both",
            ));
        }
        if response
            .request_id_headers
            .iter()
            .any(|name| name.parse::<HeaderName>().is_err())
        {
            return Err(invalid(
                "response.request_id_headers",
                "must be HTTP header names",
            ));
        }
        if response.artifact_url.is_some() == response.artifact_base64.is_some() {
            return Err(invalid(
                "response",
                "set exactly one of artifact_url or artifact_base64",
            ));
        }
        Ok(())
    }

    fn validate_poll(&self) -> Result<(), PackError> {
        let poll = match (&self.poll, self.archetype) {
            (None, Archetype::AsyncPoll) => {
                return Err(invalid("poll", "an async_poll pack declares how to poll"))
            }
            (Some(_), archetype) if archetype != Archetype::AsyncPoll => {
                return Err(invalid("poll", "only async_poll packs poll"))
            }
            (None, _) => return Ok(()),
            (Some(poll), _) => poll,
        };
        for (field, path) in [
            ("poll.job_id", Some(&poll.job_id)),
            ("poll.status", Some(&poll.status)),
            ("poll.url", poll.url.as_ref()),
        ] {
            if path.is_some_and(|path| path.is_empty() || !valid_path(path)) {
                return Err(invalid(field, "must be a response path such as /id"));
            }
        }
        let segments_ok = poll.path.starts_with('/')
            && poll.path.len() <= 256
            && poll.path.split('/').skip(1).all(|segment| {
                segment == JOB_ID_PLACEHOLDER
                    || (valid_poll_token(segment) && !matches!(segment, "." | ".."))
            });
        if !segments_ok {
            return Err(invalid(
                "poll.path",
                "must be an absolute path of plain segments, optionally one {{job_id}}",
            ));
        }
        if poll.query.iter().any(|(key, value)| {
            !valid_poll_token(key)
                || key.to_ascii_lowercase() != *key
                || !(value == JOB_ID_PLACEHOLDER || valid_poll_token(value))
        }) {
            return Err(invalid(
                "poll.query",
                "keys must be lowercase tokens; values plain tokens or exactly {{job_id}}",
            ));
        }
        if !poll.path.contains(JOB_ID_PLACEHOLDER)
            && !poll.query.values().any(|value| value == JOB_ID_PLACEHOLDER)
        {
            return Err(invalid(
                "poll",
                "the poll path or query must carry {{job_id}}",
            ));
        }
        validate_hosts(&poll.hosts, "poll.hosts")?;
        if let Some(host) = poll
            .hosts
            .iter()
            .find(|host| !self.transport.allowed_hosts.contains(host))
        {
            return Err(invalid(
                "poll.hosts",
                format!("{host} receives the credential, so it must be in transport.allowed_hosts"),
            ));
        }
        if poll.url.is_some() && poll.hosts.is_empty() {
            return Err(invalid(
                "poll.hosts",
                "a returned polling URL needs allowed polling hosts",
            ));
        }
        if poll.interval_ms == 0 || poll.interval_ms > self.transport.timeout_ms {
            return Err(invalid(
                "poll.interval_ms",
                "must be between 1 and transport.timeout_ms",
            ));
        }
        if poll.pending.is_empty() || poll.ready.is_empty() {
            return Err(invalid(
                "poll",
                "declare at least one pending and one ready status",
            ));
        }
        let mut statuses = BTreeSet::new();
        for status in poll
            .pending
            .iter()
            .chain(&poll.ready)
            .chain(poll.rejected.keys())
            .chain(poll.ambiguous.keys())
        {
            if status.is_empty()
                || status.len() > 64
                || status.trim() != status
                || status.to_ascii_lowercase() != *status
                || !statuses.insert(status.as_str())
            {
                return Err(invalid(
                    "poll",
                    format!("status {status:?} must be lowercase, trimmed and declared once"),
                ));
            }
        }
        if let Some(code) = poll
            .rejected
            .values()
            .chain(poll.ambiguous.values())
            .find(|code| {
                !code.starts_with("provider_")
                    || code.len() > 64
                    || !code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
        {
            return Err(invalid(
                "poll",
                format!("failure code {code:?} must look like provider_<reason>"),
            ));
        }
        Ok(())
    }

    fn validate_request(&self) -> Result<(), PackError> {
        let Some(request) = &self.request else {
            return Ok(());
        };
        let path_ok = request.path.starts_with('/')
            && request.path.len() <= 256
            && !request.path.contains(['?', '#', '\\'])
            && !request.path.split('/').any(|segment| segment == "..")
            && Url::parse(&self.transport.endpoint)
                .ok()
                .is_some_and(|mut url| {
                    url.set_path(&request.path);
                    url.path() == request.path
                });
        if !path_ok {
            return Err(invalid(
                "request.path",
                "must be an absolute path with no query, fragment or '..'",
            ));
        }
        let Some(body) = &request.body else {
            return Ok(());
        };
        if !self.fixed.is_empty() {
            return Err(invalid(
                "fixed",
                "use request.body instead when a body template is set",
            ));
        }
        if let Some(index) = self.parameters.iter().position(|p| p.field.is_some()) {
            return Err(invalid(
                format!("parameters[{index}].field"),
                "use a request.body placeholder instead when a body template is set",
            ));
        }
        let mut referenced = BTreeSet::new();
        collect_placeholders(&Value::Object(body.clone()), &mut referenced)
            .map_err(|reason| invalid("request.body", reason))?;
        let images: BTreeSet<_> = self
            .parameters
            .iter()
            .filter(|p| p.kind == ParameterKind::Images)
            .map(|p| p.name.as_str())
            .collect();
        if !images_spliced_only(&Value::Object(body.clone()), &images, false) {
            return Err(invalid(
                "request.body",
                "an images placeholder must be a whole element of an array",
            ));
        }
        for (name, transform) in &referenced {
            let parameter = self.parameters.iter().find(|p| &p.name == name);
            let derived = matches!(
                name.as_str(),
                OUTPUT_WIDTH_PLACEHOLDER | OUTPUT_HEIGHT_PLACEHOLDER
            ) && self.has_size_source();
            if name != MODEL_PLACEHOLDER && !derived && parameter.is_none() {
                return Err(invalid(
                    "request.body",
                    format!("{{{{{name}}}}} names no declared parameter"),
                ));
            }
            if transform.is_some()
                && parameter
                    .is_some_and(|p| !matches!(p.kind, ParameterKind::String | ParameterKind::Enum))
            {
                return Err(invalid(
                    "request.body",
                    format!("{{{{{name}}}}} can change case only on a string parameter"),
                ));
            }
        }
        // A parameter the provider never receives must not change the price
        // or what was asked: only a constant may stay out of the body.
        for (index, parameter) in self.parameters.iter().enumerate() {
            let constant = match parameter.kind {
                ParameterKind::Integer => {
                    (parameter.min.is_some() && parameter.min == parameter.max)
                        // A parameter tied to the certified size is sent as
                        // that size, through {{output_width}} / {{output_height}}.
                        || parameter.equals.is_some_and(|derived| {
                            referenced.iter().any(|(name, _)| name == derived.placeholder())
                        })
                }
                ParameterKind::Enum => {
                    parameter.values.len() == 1
                        // A size preset is sent as the size it certifies.
                        || ((!parameter.output_dimensions.is_empty()
                            || self.is_size_preset_parameter(&parameter.name))
                            && referenced.iter().any(|(name, _)| {
                                matches!(
                                    name.as_str(),
                                    OUTPUT_WIDTH_PLACEHOLDER | OUTPUT_HEIGHT_PLACEHOLDER
                                )
                            }))
                }
                _ => false,
            };
            if !constant && !referenced.iter().any(|(name, _)| name == &parameter.name) {
                return Err(invalid(
                    format!("parameters[{index}].name"),
                    "is never sent; reference it in request.body or make it a constant",
                ));
            }
        }
        Ok(())
    }

    fn has_size_source(&self) -> bool {
        !self.size_presets.is_empty()
            || self
                .parameters
                .iter()
                .any(|p| !p.output_dimensions.is_empty())
    }

    fn is_size_preset_parameter(&self, name: &str) -> bool {
        self.size_presets
            .first()
            .is_some_and(|preset| preset.when.contains_key(name))
    }

    /// The megapixel quantities any pricing rule prices.
    fn derived_quantities(&self) -> BTreeSet<&'static str> {
        self.models
            .iter()
            .flat_map(|model| &model.pricing_rules)
            .flat_map(|rule| &rule.components)
            .filter(|c| {
                matches!(
                    c.unit,
                    PricingUnit::Megapixel | PricingUnit::AdditionalMegapixel
                )
            })
            .filter_map(|c| c.unit.quantity_name())
            .collect()
    }

    /// Every preset names the same enum parameters, each combination and
    /// each size appears once, and every combination a model accepts has a
    /// certified size, so no valid input reaches an uncertified size.
    fn validate_size_presets(&self) -> Result<(), PackError> {
        let Some(first) = self.size_presets.first() else {
            return Ok(());
        };
        let names: BTreeSet<&String> = first.when.keys().collect();
        let mut parameters = Vec::new();
        for name in &names {
            let parameter = self
                .parameters
                .iter()
                .find(|p| &&p.name == name && p.kind == ParameterKind::Enum)
                .ok_or_else(|| {
                    invalid(
                        "size_presets",
                        format!("{name} is not a declared enum parameter"),
                    )
                })?;
            if !parameter.required && parameter.default.is_none() {
                return Err(invalid(
                    "size_presets",
                    format!("{name} must be required or have a default, so every input has a size"),
                ));
            }
            parameters.push(parameter);
        }
        let mut combinations = BTreeSet::new();
        let mut sizes = BTreeSet::new();
        for (index, preset) in self.size_presets.iter().enumerate() {
            let field = format!("size_presets[{index}]");
            if preset.when.keys().collect::<BTreeSet<_>>() != names {
                return Err(invalid(
                    format!("{field}.when"),
                    "every preset must name the same parameters",
                ));
            }
            if let Some((name, value)) = preset.when.iter().find(|(name, value)| {
                !parameters
                    .iter()
                    .any(|p| &&p.name == name && p.values.contains(value))
            }) {
                return Err(invalid(
                    format!("{field}.when.{name}"),
                    format!("{value} is not a value of {name}"),
                ));
            }
            let [width, height] = preset.size;
            if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
                return Err(invalid(
                    format!("{field}.size"),
                    "must be a positive [width, height] of at most 65535 each",
                ));
            }
            if !combinations.insert(&preset.when) {
                return Err(invalid(format!("{field}.when"), "is certified twice"));
            }
            // A size identifies its preset, so sending only the size sends
            // everything the agent chose.
            if !sizes.insert(preset.size) {
                return Err(invalid(
                    format!("{field}.size"),
                    "is certified for two presets",
                ));
            }
        }
        for model in &self.models {
            let mut wanted = vec![BTreeMap::new()];
            for parameter in &parameters {
                wanted = wanted
                    .into_iter()
                    .flat_map(|base: BTreeMap<String, String>| {
                        model.values_for(parameter).iter().map(move |value| {
                            let mut next = base.clone();
                            next.insert(parameter.name.clone(), value.clone());
                            next
                        })
                    })
                    .collect();
            }
            if let Some(missing) = wanted.iter().find(|w| !combinations.contains(w)) {
                return Err(invalid(
                    "size_presets",
                    format!(
                        "model {} accepts {} but no preset certifies its size",
                        model.model,
                        serde_json::to_string(missing).expect("combination serializes")
                    ),
                ));
            }
        }
        Ok(())
    }

    /// A pack that prices megapixels declares how to count them, and has a
    /// certified size for every input to count them from.
    fn validate_megapixels(&self) -> Result<(), PackError> {
        let priced = !self.derived_quantities().is_empty();
        let Some(rule) = &self.megapixels else {
            return if priced {
                Err(invalid(
                    "megapixels",
                    "a pack that prices megapixels must declare how they are rounded",
                ))
            } else {
                Ok(())
            };
        };
        if !priced {
            return Err(invalid(
                "megapixels",
                "no pricing rule prices megapixel or additional_megapixel",
            ));
        }
        if rule.pixels_per_megapixel == 0 || rule.minimum == 0 {
            return Err(invalid(
                "megapixels",
                "pixels_per_megapixel and minimum must be positive",
            ));
        }
        if !self.has_size_source() {
            return Err(invalid(
                "megapixels",
                "megapixel pricing needs size_presets or a parameter with output_dimensions",
            ));
        }
        if let Some(parameter) = self
            .parameters
            .iter()
            .find(|p| !p.output_dimensions.is_empty() && !p.required && p.default.is_none())
        {
            return Err(invalid(
                "megapixels",
                format!(
                    "{} must be required or have a default, so every input has a size",
                    parameter.name
                ),
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
        if let Some(host) = artifacts.hosts.iter().find(|host| {
            self.transport
                .allowed_hosts
                .iter()
                .any(|allowed| hosts_overlap(allowed, host))
        }) {
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
            for (name, values) in &model.parameter_values {
                let parameter = self
                    .parameters
                    .iter()
                    .find(|p| &p.name == name && p.kind == ParameterKind::Enum);
                let unique: BTreeSet<_> = values.iter().collect();
                if parameter.is_none_or(|p| {
                    values.is_empty()
                        || unique.len() != values.len()
                        || values.iter().any(|v| !p.values.contains(v))
                        || p.default
                            .as_ref()
                            .is_some_and(|d| !values.iter().any(|v| Some(v.as_str()) == d.as_str()))
                }) {
                    return Err(invalid(
                        format!("{field}.parameter_values.{name}"),
                        "must narrow a declared enum parameter to a unique, non-empty subset that keeps its default",
                    ));
                }
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
        for model in &self.models {
            for combination in &selector_combinations(selectors, model) {
                let mut request = NormalizedRequest::new(&self.provider, &model.model)
                    .with_workload(&self.workload_type);
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
                        "workload_type": self.workload_type,
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
fn selector_combinations(
    selectors: &BTreeMap<&str, &Parameter>,
    model: &Model,
) -> Vec<PricingSelector> {
    let mut combinations = vec![PricingSelector::default()];
    for (key, parameter) in selectors {
        let values = model.values_for(parameter);
        combinations = combinations
            .into_iter()
            .flat_map(|base| {
                values.iter().map(move |value| {
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
        "input_images" => &mut usage.input_images,
        _ => return None,
    })
}

/// A response path segment: a field or index, optionally filtering an array.
struct PathSegment<'a> {
    name: String,
    filter: Option<(&'a str, &'a str)>,
}

fn parse_path(path: &str) -> Option<Vec<PathSegment<'_>>> {
    if path.is_empty() {
        return Some(Vec::new());
    }
    if path.len() > 256 {
        return None;
    }
    path.strip_prefix('/')?
        .split('/')
        .map(|segment| {
            let (name, filter) = match segment.split_once('[') {
                Some((name, rest)) => {
                    let (key, value) = rest.strip_suffix(']')?.split_once('=')?;
                    if key.is_empty() || value.is_empty() || value.contains(['[', ']']) {
                        return None;
                    }
                    (name, Some((key, value)))
                }
                None => (segment, None),
            };
            if name.is_empty() || name.contains(']') {
                return None;
            }
            Some(PathSegment {
                name: name.replace("~1", "/").replace("~0", "~"),
                filter,
            })
        })
        .collect()
}

fn valid_path(path: &str) -> bool {
    parse_path(path).is_some()
}

/// Read a declared usage count without turning a missing path or malformed
/// scalar into a collection count. Only a terminal filter counts selections;
/// every field and array traversed to reach it must exist with the right shape.
fn usage_count(root: &Value, path: &str) -> Result<i64, UsagePathError> {
    let segments = parse_path(path).ok_or(UsagePathError::Invalid)?;
    let filtered = segments
        .last()
        .is_some_and(|segment| segment.filter.is_some());
    let mut current = vec![root];
    for segment in segments {
        let mut next = Vec::new();
        for value in current {
            let child = match value {
                Value::Object(map) => map.get(&segment.name).ok_or(UsagePathError::Missing),
                Value::Array(items) => segment
                    .name
                    .parse::<usize>()
                    .map_err(|_| UsagePathError::Invalid)
                    .and_then(|index| items.get(index).ok_or(UsagePathError::Missing)),
                _ => Err(UsagePathError::Invalid),
            }?;
            match segment.filter {
                None => next.push(child),
                Some((key, expected)) => {
                    let items = child.as_array().ok_or(UsagePathError::Invalid)?;
                    next.extend(items.iter().filter(|item| {
                        item.get(key).is_some_and(|actual| match actual {
                            Value::String(text) => text == expected,
                            Value::Number(number) => number.to_string() == expected,
                            Value::Bool(flag) => flag.to_string() == expected,
                            _ => false,
                        })
                    }));
                }
            }
        }
        current = next;
    }
    if filtered {
        return i64::try_from(current.len()).map_err(|_| UsagePathError::Invalid);
    }
    match current.as_slice() {
        [Value::Array(items)] => i64::try_from(items.len()).map_err(|_| UsagePathError::Invalid),
        [value] => value
            .as_i64()
            .filter(|count| *count >= 0)
            .ok_or(UsagePathError::Invalid),
        [] => Err(UsagePathError::Missing),
        _ => Err(UsagePathError::Invalid),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UsagePathError {
    Missing,
    Invalid,
}

/// Every value a response path selects. A filter segment keeps the array
/// elements whose `key` equals `value` (strings, numbers and booleans compare
/// by their text) and flattens them into the selection.
pub fn select_path<'a>(root: &'a Value, path: &str) -> Vec<&'a Value> {
    let Some(segments) = parse_path(path) else {
        return Vec::new();
    };
    let mut current = vec![root];
    for segment in segments {
        let mut next = Vec::new();
        for value in current {
            let child = match value {
                Value::Object(map) => map.get(&segment.name),
                Value::Array(items) => segment
                    .name
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| items.get(index)),
                _ => None,
            };
            match (child, segment.filter) {
                (Some(child), None) => next.push(child),
                (Some(Value::Array(items)), Some((key, expected))) => {
                    next.extend(items.iter().filter(|item| {
                        item.get(key).is_some_and(|actual| match actual {
                            Value::String(text) => text == expected,
                            Value::Number(number) => number.to_string() == expected,
                            Value::Bool(flag) => flag.to_string() == expected,
                            _ => false,
                        })
                    }))
                }
                _ => {}
            }
        }
        current = next;
    }
    current
}

/// A placeholder string `{{name}}` or `{{name|upper}}` / `{{name|lower}}`.
fn placeholder(text: &str) -> Option<(&str, Option<&str>)> {
    let inner = text.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    let (name, transform) = match inner.split_once('|') {
        Some((name, transform)) => (name.trim(), Some(transform.trim())),
        None => (inner, None),
    };
    valid_parameter_name(name).then_some((name, transform))
}

fn collect_placeholders(
    template: &Value,
    found: &mut BTreeSet<(String, Option<String>)>,
) -> Result<(), String> {
    match template {
        Value::String(text) if text.contains("{{") || text.contains("}}") => {
            let (name, transform) = placeholder(text).ok_or_else(|| {
                format!("{text:?} must be exactly one placeholder such as {{{{prompt}}}}")
            })?;
            if transform.is_some_and(|t| !matches!(t, "upper" | "lower")) {
                return Err(format!(
                    "{text:?} uses an unknown transform; use upper or lower"
                ));
            }
            found.insert((name.to_owned(), transform.map(str::to_owned)));
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_placeholders(value, found)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_placeholders(item, found)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Render a validated body template. `None` means "leave this value out";
/// the top-level object always renders. A `spliced` placeholder is an array
/// element that expands to the elements of its value.
fn render_template(
    template: &Value,
    values: &BTreeMap<String, Value>,
    spliced: &BTreeSet<String>,
) -> Option<Value> {
    fn render(
        template: &Value,
        values: &BTreeMap<String, Value>,
        spliced: &BTreeSet<String>,
    ) -> Option<Value> {
        match template {
            Value::String(text) => match placeholder(text) {
                Some((name, transform)) => {
                    let value = values.get(name)?;
                    Some(match (transform, value.as_str()) {
                        (Some("upper"), Some(text)) => json!(text.to_ascii_uppercase()),
                        (Some("lower"), Some(text)) => json!(text.to_ascii_lowercase()),
                        _ => value.clone(),
                    })
                }
                None => Some(template.clone()),
            },
            Value::Object(map) => Some(Value::Object(
                map.iter()
                    .filter_map(|(key, value)| Some((key.clone(), render(value, values, spliced)?)))
                    .collect(),
            )),
            Value::Array(items) => {
                let mut rendered = Vec::with_capacity(items.len());
                for item in items {
                    let splice = item
                        .as_str()
                        .and_then(placeholder)
                        .filter(|(name, _)| spliced.contains(*name));
                    match splice {
                        Some((name, _)) => {
                            rendered.extend(values.get(name)?.as_array()?.iter().cloned())
                        }
                        None => rendered.extend(render(item, values, spliced)),
                    }
                }
                Some(Value::Array(rendered))
            }
            other => Some(other.clone()),
        }
    }
    render(template, values, spliced).filter(Value::is_object)
}

/// A provider-reported decimal amount in USD, shifted right by `scale`
/// decimal places (2 for one-cent credits).
fn decimal_cost(value: &Value, scale: u32) -> Option<ActualVendorCost> {
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
    let scale = u32::try_from(fraction.len()).ok()?.checked_add(scale)?;
    ActualVendorCost::new(amount, scale, "USD").ok()
}

/// The pixel size an image header declares, without decoding the pixels.
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::io::Reader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
        .filter(|(width, height)| *width > 0 && *height > 0)
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

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// Hosts may be exact names or patterns with one `*` label that matches
/// exactly one DNS label, such as `delivery.*.example.com`. The `*` may not
/// stand in the last two labels.
fn validate_hosts(hosts: &[String], field: &str) -> Result<(), PackError> {
    let mut unique = BTreeSet::new();
    for host in hosts {
        let labels: Vec<_> = host.split('.').collect();
        let wildcards = labels.iter().filter(|label| **label == "*").count();
        // `*` stands only for a whole label, never part of one.
        if labels
            .iter()
            .any(|label| label.contains('*') && *label != "*")
        {
            return Err(invalid(field, format!("{host}: `*` must be a whole label")));
        }
        let concrete = host.replace('*', "wildcard");
        let parsed = Url::parse(&format!("https://{concrete}/")).ok();
        if !unique.insert(host)
            || wildcards > 1
            || (wildcards == 1 && (labels.len() < 3 || labels[labels.len() - 2..].contains(&"*")))
            || parsed.as_ref().and_then(Url::host_str) != Some(concrete.as_str())
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

/// Whether `host` is one of `patterns`, a `*` label matching one DNS label.
pub fn host_allowed(patterns: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let labels: Vec<_> = host.split('.').collect();
    patterns.iter().any(|pattern| {
        let expected: Vec<_> = pattern.split('.').collect();
        expected.len() == labels.len()
            && expected.iter().zip(&labels).all(|(want, got)| {
                want == got
                    || (*want == "*"
                        && !got.is_empty()
                        && got.len() <= 63
                        && got
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                        && !got.starts_with('-')
                        && !got.ends_with('-')
                        // Never let a pattern match an internationalized label.
                        && !got.starts_with("xn--"))
            })
    })
}

/// Whether two host patterns can name the same host.
fn hosts_overlap(left: &str, right: &str) -> bool {
    let left: Vec<_> = left.split('.').collect();
    let right: Vec<_> = right.split('.').collect();
    left.len() == right.len()
        && left
            .iter()
            .zip(&right)
            .all(|(a, b)| a == b || *a == "*" || *b == "*")
}

/// A job ID safe to store as durable evidence and to place in a URL.
pub fn safe_job_id(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value.len() <= 255
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

fn valid_poll_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
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

/// An artifact ID as Gongbu issues it: a lowercase hyphenated UUID.
fn valid_artifact_id(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(index, c)| match index {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

/// Whether every `images` placeholder in `template` is a whole element of
/// an array, where the engine splices in one rendered item per image.
fn images_spliced_only(template: &Value, images: &BTreeSet<&str>, in_array: bool) -> bool {
    match template {
        Value::String(text) => {
            placeholder(text).is_none_or(|(name, _)| !images.contains(name) || in_array)
        }
        Value::Object(map) => map
            .values()
            .all(|value| images_spliced_only(value, images, false)),
        Value::Array(items) => items
            .iter()
            .all(|item| images_spliced_only(item, images, true)),
        _ => true,
    }
}

/// A parameter name: an identifier, or `group.member` with one level.
fn valid_parameter_name(value: &str) -> bool {
    match value.split_once('.') {
        Some((group, member)) => valid_field_name(group) && valid_field_name(member),
        None => valid_field_name(value),
    }
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
    fn fetch(&self, url: &Url, max_bytes: usize) -> Result<Vec<u8>, ArtifactFetchError> {
        let bytes = self.get(url.as_str()).ok_or(ArtifactFetchError::Failed)?;
        if bytes.len() > max_bytes {
            return Err(ArtifactFetchError::TooLarge);
        }
        Ok(bytes.clone())
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
        self.pack.map_response(
            request,
            &ProviderResponse {
                headers: BTreeMap::new(),
                body: self.recorded_response.clone(),
            },
            &Evidence::default(),
            &self.downloads,
            crate::artifact::DEFAULT_MAX_ENCODED_BYTES as usize,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        contract::{PricingSnapshot, PricingUnit},
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
        let request = pack
            .prepare("image-v1", &json!({"prompt": "x", "image_count": 2}))
            .unwrap()
            .request;
        let code = |response: &Value, downloads: &BTreeMap<String, Vec<u8>>| {
            let failure = pack
                .map_response(
                    &request,
                    &ProviderResponse {
                        headers: BTreeMap::new(),
                        body: response.clone(),
                    },
                    &Evidence::default(),
                    downloads,
                    1024,
                )
                .unwrap_err();
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
            decimal_cost(&json!("0.012"), 0),
            Some(ActualVendorCost::new(12, 3, "USD").unwrap())
        );
        // One-cent credits: 4.5 credits are $0.045.
        assert_eq!(
            decimal_cost(&json!(4.5), 2),
            Some(ActualVendorCost::new(45, 3, "USD").unwrap())
        );
        assert_eq!(decimal_cost(&json!(-1), 0), None);
    }

    #[test]
    fn declared_usage_paths_reconcile_when_missing_or_malformed() {
        let pack = Pack::from_toml_str(&edited(
            "usage = { images = \"/data\" }",
            "usage = { images = \"/usage/images\" }",
        ))
        .unwrap();
        let request = pack
            .prepare("image-v1", &json!({"prompt": "x", "image_count": 2}))
            .unwrap()
            .request;
        let invalid_usage = [
            None,
            Some(json!({})),
            Some(json!(null)),
            Some(json!([])),
            Some(json!({"images": null})),
            Some(json!({"images": false})),
            Some(json!({"images": true})),
            Some(json!({"images": "2"})),
            Some(json!({"images": {}})),
            Some(json!({"images": 2.0})),
            Some(json!({"images": -1})),
            Some(json!({"images": 9_223_372_036_854_775_808_u64})),
        ];
        for reported_usage in invalid_usage {
            let mut body: Value = serde_json::from_str(RESPONSE).unwrap();
            if let Some(value) = &reported_usage {
                body["usage"] = value.clone();
            }
            let failure = pack
                .map_response(
                    &request,
                    &ProviderResponse {
                        headers: BTreeMap::new(),
                        body,
                    },
                    &Evidence::default(),
                    &downloads(),
                    1024,
                )
                .unwrap_err();
            assert_eq!(
                failure.code, "pack_response_usage_invalid",
                "{reported_usage:?}"
            );
            assert_eq!(
                failure.spend_disposition,
                crate::provider_contract::SpendDisposition::Reconcile
            );
            assert_eq!(failure.evidence.request_id.as_deref(), Some("sbx-req-7f3a"));
        }
    }

    #[test]
    fn optional_telemetry_preserves_reported_values_without_inventing_missing_usage() {
        let pack = Pack::from_toml_str(&edited(
            "usage = { images = \"/data\" }",
            "usage = { images = \"/data\", input_tokens = \"/usage/input_tokens\", output_tokens = \"/usage/output_tokens\" }\noptional_usage = [\"input_tokens\", \"output_tokens\"]",
        ))
        .unwrap();
        let request = pack
            .prepare("image-v1", &json!({"prompt": "x", "image_count": 2}))
            .unwrap()
            .request;
        for reported in [
            None,
            Some(json!({})),
            Some(json!({"input_tokens": 13})),
            Some(json!({"input_tokens": 0, "output_tokens": 27})),
        ] {
            let mut body: Value = serde_json::from_str(RESPONSE).unwrap();
            if let Some(value) = &reported {
                body["usage"] = value.clone();
            }
            let outcome = pack
                .map_response(
                    &request,
                    &ProviderResponse {
                        headers: BTreeMap::new(),
                        body,
                    },
                    &Evidence::default(),
                    &downloads(),
                    1024,
                )
                .unwrap();
            let usage = outcome.usage.unwrap();
            assert_eq!(usage.images, Some(2));
            assert_eq!(
                usage.input_tokens,
                reported
                    .as_ref()
                    .and_then(|value| value.get("input_tokens"))
                    .and_then(Value::as_i64)
            );
            assert_eq!(
                usage.output_tokens,
                reported
                    .as_ref()
                    .and_then(|value| value.get("output_tokens"))
                    .and_then(Value::as_i64)
            );
        }
        for reported in [
            json!(null),
            json!({"input_tokens": null}),
            json!({"input_tokens": "13"}),
            json!({"input_tokens": true}),
            json!({"output_tokens": -1}),
            json!({"output_tokens": 2.5}),
            json!({"output_tokens": 9_223_372_036_854_775_808_u64}),
        ] {
            let mut body: Value = serde_json::from_str(RESPONSE).unwrap();
            body["usage"] = reported.clone();
            let failure = pack
                .map_response(
                    &request,
                    &ProviderResponse {
                        headers: BTreeMap::new(),
                        body,
                    },
                    &Evidence::default(),
                    &downloads(),
                    1024,
                )
                .unwrap_err();
            assert_eq!(failure.code, "pack_response_usage_invalid", "{reported}");
            assert_eq!(
                failure.spend_disposition,
                crate::provider_contract::SpendDisposition::Reconcile
            );
            assert_eq!(failure.evidence.request_id.as_deref(), Some("sbx-req-7f3a"));
        }
    }

    #[test]
    fn optional_usage_only_names_mapped_informational_quantities() {
        for (usage, request, unit) in [
            ("images", "image_count", PricingUnit::Image),
            ("input_tokens", "input_tokens", PricingUnit::InputToken),
            (
                "output_tokens",
                "max_output_tokens",
                PricingUnit::OutputToken,
            ),
            ("results", "results", PricingUnit::Result),
            ("pages", "pages", PricingUnit::Page),
            ("seconds", "seconds", PricingUnit::Second),
        ] {
            for zero_rate in [false, true] {
                let mut document: PackDocument = toml::from_str(FIXTURE).unwrap();
                document.parameters[1].quantity = Some(request.to_owned());
                document.response.usage = [(usage.to_owned(), "/data".to_owned())]
                    .into_iter()
                    .collect();
                document.response.optional_usage.insert(usage.to_owned());
                for rule in &mut document.models[0].pricing_rules {
                    for component in &mut rule.components {
                        if component.unit == PricingUnit::Image {
                            component.unit = unit;
                            if zero_rate {
                                component.rate_numerator_minor = 0;
                            }
                        }
                    }
                }
                assert_eq!(
                    Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap_err(),
                    invalid(
                        format!("response.optional_usage.{usage}"),
                        "a declared priced quantity cannot have optional usage"
                    ),
                    "{usage} zero_rate={zero_rate}"
                );
            }
        }
        for usage in ["output_tokens", "unknown_quantity"] {
            let mut document: PackDocument = toml::from_str(FIXTURE).unwrap();
            document.response.optional_usage.insert(usage.to_owned());
            assert_eq!(
                Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap_err(),
                invalid(
                    format!("response.optional_usage.{usage}"),
                    "must name a mapped usage quantity"
                )
            );
        }
        let pack = Pack::from_toml_str(FIXTURE).unwrap();
        let explicit_empty = Pack::from_toml_str(&edited(
            "usage = { images = \"/data\" }",
            "usage = { images = \"/data\" }\noptional_usage = []",
        ))
        .unwrap();
        assert_eq!(pack.digest(), explicit_empty.digest());
        assert!(serde_json::to_value(&pack.document.response)
            .unwrap()
            .get("optional_usage")
            .is_none());
    }

    #[test]
    fn usage_counts_distinguish_values_from_explicit_filtered_selections() {
        for (body, expected) in [
            (json!({"count": 0}), Some(0)),
            (json!({"count": i64::MAX}), Some(i64::MAX)),
            (json!({"count": []}), Some(0)),
            (json!({"count": ["a", "b"]}), Some(2)),
        ] {
            assert_eq!(usage_count(&body, "/count").ok(), expected);
        }
        let path = "/steps[type=model_output]/content[type=image]";
        for (body, expected) in [
            (json!({"steps": []}), Some(0)),
            (json!({"steps": [{"type": "thought"}]}), Some(0)),
            (
                json!({"steps": [{"type": "model_output", "content": []}]}),
                Some(0),
            ),
            (
                json!({"steps": [{"type": "model_output", "content": [
                    {"type": "image"}, {"type": "text"}
                ]}]}),
                Some(1),
            ),
            (
                json!({"steps": [
                    {"type": "model_output", "content": [{"type": "image"}]},
                    {"type": "model_output", "content": [{"type": "image"}]}
                ]}),
                Some(2),
            ),
            (json!({}), None),
            (json!({"steps": null}), None),
            (json!({"steps": {}}), None),
            (json!({"steps": [{"type": "model_output"}]}), None),
            (
                json!({"steps": [{"type": "model_output", "content": {}}]}),
                None,
            ),
            (
                json!({"steps": [
                    {"type": "model_output", "content": [{"type": "image"}]},
                    {"type": "model_output"}
                ]}),
                None,
            ),
        ] {
            assert_eq!(usage_count(&body, path).ok(), expected, "{body}");
        }
        // A nonterminal filter does not make a scalar or a set of scalar
        // values into a count of selected items.
        let body = json!({"entries": [
            {"type": "match", "count": 3},
            {"type": "match", "count": 4}
        ]});
        assert_eq!(
            usage_count(&body, "/entries[type=match]/count"),
            Err(UsagePathError::Invalid)
        );
        assert_eq!(
            usage_count(
                &json!({"entries": [{"type": "match", "count": false}]}),
                "/entries[type=match]/count"
            ),
            Err(UsagePathError::Invalid)
        );
        assert_eq!(usage_count(&body, "/entries/0/count"), Ok(3));
        assert_eq!(usage_count(&body, "/missing"), Err(UsagePathError::Missing));
    }

    #[test]
    fn parameters_cannot_shadow_the_reserved_model_placeholder() {
        const INLINE: &str = include_str!("../../../../fixtures/packs/inline-image.toml");
        for (required, default, alias) in [
            (true, None, None),
            (false, Some(json!("expensive-model")), None),
            (true, None, Some("provider_model".to_owned())),
        ] {
            let mut document: PackDocument = toml::from_str(INLINE).unwrap();
            document.parameters[0].name = "model".to_owned();
            document.parameters[0].required = required;
            document.parameters[0].default = default;
            document.parameters[0].field = alias;
            let error = Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap_err();
            assert_eq!(
                error,
                invalid(
                    "parameters[0].name",
                    "model is reserved for the selected target's model ID"
                )
            );
        }
        let pack = Pack::from_toml_str(INLINE).unwrap();
        let prepared = pack
            .prepare("inline-image-v1", &json!({"prompt": "a lighthouse"}))
            .unwrap();
        assert_eq!(prepared.body["model"], "inline-image-v1");
        assert!(pack
            .prepare(
                "inline-image-v1",
                &json!({"prompt": "a lighthouse", "model": "expensive-model"}),
            )
            .is_err());
    }

    #[test]
    fn parameters_cannot_shadow_certified_output_dimensions() {
        const INLINE: &str = include_str!("../../../../fixtures/packs/inline-image.toml");
        const FLUX: &str = include_str!("../../../../packs/verified/bfl-flux-2.toml");
        for source in [INLINE, FLUX] {
            for name in [OUTPUT_WIDTH_PLACEHOLDER, OUTPUT_HEIGHT_PLACEHOLDER] {
                for (required, default, alias) in [
                    (true, None, None),
                    (false, Some(json!(8192)), None),
                    (true, None, Some("provider_dimension".to_owned())),
                ] {
                    let mut document: PackDocument = toml::from_str(source).unwrap();
                    let parameter = &mut document.parameters[0];
                    parameter.name = name.to_owned();
                    parameter.kind = ParameterKind::Integer;
                    parameter.max_length = None;
                    parameter.min = Some(1);
                    parameter.max = Some(8192);
                    parameter.required = required;
                    parameter.default = default;
                    parameter.field = alias;
                    let error =
                        Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap_err();
                    match error {
                        PackError::Invalid { field, reason } => {
                            assert_eq!(field, "parameters[0].name");
                            assert!(reason.starts_with(name));
                            assert!(reason.contains("reserved"));
                        }
                        other => panic!("{other}"),
                    }
                }
            }
        }
    }

    #[test]
    fn grouped_dimension_names_do_not_replace_certified_template_values() {
        const FLUX: &str = include_str!("../../../../packs/verified/bfl-flux-2.toml");
        for (name, field) in [
            ("options.output_width", "width"),
            ("options.output_height", "height"),
        ] {
            let mut document: PackDocument = toml::from_str(FLUX).unwrap();
            let mut parameter = document
                .parameters
                .iter()
                .find(|parameter| parameter.name == "options.width")
                .unwrap()
                .clone();
            parameter.name = name.to_owned();
            parameter.equals = None;
            parameter.min = Some(1);
            parameter.max = Some(8192);
            document.parameters.push(parameter);
            document
                .request
                .as_mut()
                .unwrap()
                .body
                .as_mut()
                .unwrap()
                .insert(
                    "caller_dimension".to_owned(),
                    json!(format!("{{{{{name}}}}}")),
                );
            let pack = Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap();
            let mut input = json!({
                "prompt": "a lighthouse", "image_count": 1, "image_size": "1k",
                "options": {"width": 1024, "height": 1024},
            });
            let (_, member) = name.split_once('.').unwrap();
            input["options"][member] = json!(7);
            let prepared = pack.prepare("flux-2-pro", &input).unwrap();
            assert_eq!(prepared.body[field], 1024);
            assert_eq!(prepared.body["caller_dimension"], 7);
            assert_eq!(
                prepared.request.output_dimensions,
                Some(OutputDimensions {
                    width: 1024,
                    height: 1024
                })
            );
        }
    }

    #[test]
    fn request_templates_and_response_paths_are_validated() {
        const INLINE: &str = include_str!("../../../../fixtures/packs/inline-image.toml");
        let inline = |from: &str, to: &str| {
            assert_eq!(INLINE.matches(from).count(), 1, "{from}");
            INLINE.replace(from, to)
        };
        let field = |source: String| match Pack::from_toml_str(&source).unwrap_err() {
            PackError::Invalid { field, .. } => field,
            other => panic!("{other}"),
        };
        let pack = Pack::from_toml_str(INLINE).unwrap();
        assert_eq!(
            pack.request_url().as_str(),
            "https://api.inline-image.test/v1beta/interactions"
        );
        assert_eq!(
            field(inline("text = \"{{prompt}}\"", "text = \"{{subject}}\"")),
            "request.body"
        );
        assert_eq!(
            field(inline(
                "text = \"{{prompt}}\"",
                "text = \"Draw: {{prompt}}\""
            )),
            "request.body"
        );
        assert_eq!(
            field(inline("{{image_size|upper}}", "{{image_size|reverse}}")),
            "request.body"
        );
        // The prompt is no longer sent anywhere.
        assert_eq!(
            field(inline("text = \"{{prompt}}\"", "text = \"fixed\"")),
            "parameters[0].name"
        );
        assert_eq!(
            field(inline(
                "path = \"/v1beta/interactions\"",
                "path = \"/v1/../admin\""
            )),
            "request.path"
        );
        assert_eq!(
            field(inline(
                "path = \"/v1beta/interactions\"",
                "path = \"v1?x=1\""
            )),
            "request.path"
        );
        assert_eq!(
            field(inline(
                "artifact_items = \"/steps[type=model_output]/content[type=image]\"",
                "artifact_items = \"steps[type=model_output\""
            )),
            "response.artifact_items"
        );
        assert_eq!(
            field(inline(
                "request_id_headers = [\"x-goog-request-id\", \"x-request-id\"]\nprovider_request_id = \"/id\"\n",
                ""
            )),
            "response"
        );
        // The image_count constant may stay out of the body; a range may not.
        assert_eq!(
            field(inline("max = 1\n", "max = 2\n")),
            "parameters[1].name"
        );
        assert_eq!(
            field(format!("{INLINE}\n[fixed]\nkind = \"image\"\n")),
            "fixed"
        );

        let body = json!({"a": [{"t": "x", "v": 1}, {"t": "y", "v": 2}, {"t": "x", "v": 3}], "n": {"m": "k"}});
        let values: Vec<_> = select_path(&body, "/a[t=x]")
            .into_iter()
            .map(|item| item["v"].clone())
            .collect();
        assert_eq!(values, [json!(1), json!(3)]);
        assert_eq!(select_path(&body, "/a/1/v"), [&json!(2)]);
        assert_eq!(select_path(&body, "/n/m"), [&json!("k")]);
        assert!(select_path(&body, "/missing").is_empty());
        assert_eq!(select_path(&body, ""), [&body]);
    }

    #[test]
    fn poll_specs_and_host_patterns_are_validated() {
        const ASYNC: &str = include_str!("../../../../fixtures/packs/async-image.toml");
        let edit = |from: &str, to: &str| {
            assert_eq!(ASYNC.matches(from).count(), 1, "{from}");
            ASYNC.replace(from, to)
        };
        let error = |source: String| Pack::from_toml_str(&source).unwrap_err();
        let field = |source: String| match error(source) {
            PackError::Invalid { field, .. } => field,
            other => panic!("{other}"),
        };
        let pack = Pack::from_toml_str(ASYNC).unwrap();
        assert_eq!(
            pack.poll_url("api.eu.async-image.test", "job-1")
                .unwrap()
                .as_str(),
            "https://api.eu.async-image.test/v1/get_result?id=job-1"
        );
        assert!(pack.poll_url("attacker.example", "job-1").is_none());
        assert!(pack.poll_url("api.async-image.test", "job/1").is_none());
        assert!(pack.poll_url("api.async-image.test", "..").is_none());

        let start = ASYNC.find("[poll]").unwrap();
        let end = ASYNC.find("[response]").unwrap();
        let without_poll = format!("{}{}", &ASYNC[..start], &ASYNC[end..]);
        assert_eq!(field(without_poll), "poll");
        assert_eq!(
            field(edit(
                "archetype = \"async_poll\"",
                "archetype = \"sync_fixed\""
            )),
            "poll"
        );
        assert_eq!(
            field(edit(
                "query = { id = \"{{job_id}}\" }",
                "query = { id = \"x\" }"
            )),
            "poll"
        );
        assert_eq!(
            field(edit(
                "query = { id = \"{{job_id}}\" }",
                "query = { ID = \"{{job_id}}\" }"
            )),
            "poll.query"
        );
        assert_eq!(
            field(edit("path = \"/v1/get_result\"", "path = \"/v1/../x\"")),
            "poll.path"
        );
        assert_eq!(
            field(edit(
                "hosts = [\"api.eu.async-image.test\", \"api.us.async-image.test\"]",
                "hosts = [\"api.ap.async-image.test\"]"
            )),
            "poll.hosts"
        );
        assert_eq!(
            field(edit("interval_ms = 1", "interval_ms = 0")),
            "poll.interval_ms"
        );
        assert_eq!(
            field(edit("ready = [\"ready\"]", "ready = [\"pending\"]")),
            "poll"
        );
        assert_eq!(
            field(edit("ready = [\"ready\"]", "ready = [\"Ready\"]")),
            "poll"
        );
        assert_eq!(
            field(edit(
                "\"error\" = \"provider_error\"",
                "\"error\" = \"boom\""
            )),
            "poll"
        );
        // Wildcards stand for exactly one label, never the registrable domain.
        assert_eq!(
            field(edit(
                "hosts = [\"delivery.*.async-image.test\"]",
                "hosts = [\"delivery.*.test\"]"
            )),
            "artifacts.hosts"
        );
        assert_eq!(
            field(edit(
                "hosts = [\"delivery.*.async-image.test\"]",
                "hosts = [\"*.*.async-image.test\"]"
            )),
            "artifacts.hosts"
        );
        assert_eq!(
            field(edit(
                "hosts = [\"delivery.*.async-image.test\"]",
                "hosts = [\"delivery.*eu.async-image.test\"]"
            )),
            "artifacts.hosts"
        );
        assert_eq!(
            field(edit("reported_cost_scale = 2", "reported_cost_scale = 19")),
            "response.reported_cost_scale"
        );
        // A pattern that could name a credential-bearing host is refused.
        assert_eq!(
            error(edit(
                "hosts = [\"delivery.*.async-image.test\"]",
                "hosts = [\"api.*.async-image.test\"]"
            )),
            PackError::CredentialBearingArtifactHost {
                host: "api.*.async-image.test".into()
            }
        );
        let patterns = vec!["delivery.*.async-image.test".to_owned()];
        assert!(host_allowed(&patterns, "delivery.eu1.async-image.test"));
        assert!(host_allowed(&patterns, "DELIVERY.us2.async-image.test"));
        assert!(!host_allowed(&patterns, "delivery.async-image.test"));
        assert!(!host_allowed(&patterns, "delivery.a.b.async-image.test"));
        assert!(!host_allowed(&patterns, "delivery.-x.async-image.test"));
        assert!(!host_allowed(
            &patterns,
            "delivery.eu1.async-image.test.evil"
        ));
    }

    const MEGAPIXEL: &str = include_str!("../../../../fixtures/packs/megapixel-image.toml");

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::GrayImage::new(width, height)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageOutputFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn delivered(pack: &Pack, request: &NormalizedRequest, sizes: &[(u32, u32)]) -> AdapterOutcome {
        let images: Vec<_> = sizes
            .iter()
            .map(|(w, h)| json!({"b64": base64::engine::general_purpose::STANDARD.encode(png(*w, *h))}))
            .collect();
        pack.map_response(
            request,
            &ProviderResponse {
                headers: BTreeMap::from([("x-request-id".to_owned(), "mp-1".to_owned())]),
                body: json!({"images": images}),
            },
            &Evidence::default(),
            &BTreeMap::new(),
            64 << 20,
        )
        .unwrap()
    }

    fn megapixel_snapshot(pack: &Pack, input: Value) -> (NormalizedRequest, PricingSnapshot) {
        let request = pack.prepare("megapixel-image-v1", &input).unwrap().request;
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": "megapixel-test",
                "rules": pack.pricing_rules(),
            }))
            .unwrap(),
        )
        .unwrap();
        let snapshot = catalog.snapshot(&request).unwrap();
        snapshot.validate_integrity().unwrap();
        (request, snapshot)
    }

    #[test]
    fn a_16_9_header_prices_its_certified_megapixels_and_settles_what_was_delivered() {
        let pack = Pack::from_toml_str(MEGAPIXEL).unwrap();
        let input = json!({"prompt": "a banner", "image_size": "2k", "aspect_ratio": "16:9"});
        let prepared = pack.prepare("megapixel-image-v1", &input).unwrap();
        // The preset's certified size is what the provider is asked for.
        assert_eq!(
            prepared.body,
            json!({"model": "megapixel-image-v1", "prompt": "a banner", "width": 2688, "height": 1536, "count": 1})
        );
        // 2688 × 1536 = 4,128,768 pixels: 4 billed megapixels, 3 beyond the first.
        let (request, snapshot) = megapixel_snapshot(&pack, input);
        assert_eq!(
            request.output_dimensions,
            Some(OutputDimensions {
                width: 2688,
                height: 1536
            })
        );
        assert_eq!(request.quantity("additional_megapixels"), Some(3));
        assert_eq!(
            snapshot.estimated_quantity(PricingUnit::AdditionalMegapixel),
            Some(3)
        );
        // 3¢ + 3 × 1.5¢ = 7.5¢, exactly.
        assert_eq!(snapshot.estimated_units, 75_000);
        assert_eq!(
            (
                snapshot.exact_estimate_numerator.as_str(),
                snapshot.exact_estimate_denominator.as_str()
            ),
            ("15", "2")
        );

        // Settled from the delivered image's decoded size.
        let outcome = delivered(&pack, &request, &[(2688, 1536)]);
        let usage = outcome.usage.unwrap();
        assert_eq!(
            (usage.megapixels, usage.additional_megapixels),
            (Some(4), Some(3))
        );
        assert_eq!(
            snapshot.settle(&usage, snapshot.estimated_units).unwrap(),
            75_000
        );
        // A smaller delivery settles less.
        let smaller = delivered(&pack, &request, &[(1344, 768)]).usage.unwrap();
        assert_eq!(smaller.additional_megapixels, Some(0));
        assert_eq!(
            snapshot.settle(&smaller, snapshot.estimated_units).unwrap(),
            30_000
        );
        // A larger one than was held is an overage, never a bigger charge.
        let larger = delivered(&pack, &request, &[(4096, 4096)]).usage.unwrap();
        assert_eq!(
            snapshot.settle(&larger, snapshot.estimated_units),
            Err(ContractError::SettlementOverage)
        );
    }

    #[test]
    fn megapixel_quantities_cover_every_image_and_may_have_no_additional_megapixels() {
        let pack = Pack::from_toml_str(MEGAPIXEL).unwrap();
        // Two 2k portraits: 2 × (3¢ + 3 × 1.5¢) = 15¢.
        let (request, snapshot) = megapixel_snapshot(
            &pack,
            json!({"prompt": "a story", "image_count": 2, "image_size": "2k", "aspect_ratio": "9:16"}),
        );
        assert_eq!(request.quantity("additional_megapixels"), Some(6));
        assert_eq!(snapshot.estimated_units, 150_000);
        let usage = delivered(&pack, &request, &[(1536, 2688), (1536, 2688)])
            .usage
            .unwrap();
        assert_eq!(usage.additional_megapixels, Some(6));
        assert_eq!(
            snapshot.settle(&usage, snapshot.estimated_units).unwrap(),
            150_000
        );

        // Defaults: one 1k square is one megapixel, so only the first is billed.
        let (request, snapshot) = megapixel_snapshot(&pack, json!({"prompt": "a square"}));
        assert_eq!(
            request.output_dimensions,
            Some(OutputDimensions {
                width: 1024,
                height: 1024
            })
        );
        assert_eq!(
            snapshot.estimated_quantity(PricingUnit::AdditionalMegapixel),
            Some(0)
        );
        assert_eq!(snapshot.estimated_units, 30_000);
        let usage = delivered(&pack, &request, &[(1024, 1024)]).usage.unwrap();
        assert_eq!(
            snapshot.settle(&usage, snapshot.estimated_units).unwrap(),
            30_000
        );

        // A ratio the pack does not offer is refused before pricing.
        assert!(pack
            .prepare(
                "megapixel-image-v1",
                &json!({"prompt": "x", "aspect_ratio": "4:3"})
            )
            .is_err());
        // Zero is a real quantity only for additional megapixels.
        let mut zero_images = snapshot.clone();
        for component in &mut zero_images.components {
            if component.unit == PricingUnit::Image {
                component.quantity = 0;
            }
        }
        assert!(zero_images.validate_integrity().is_err());
    }

    #[test]
    fn megapixel_rounding_is_declared_and_exact() {
        let rule = |rounding, pixels_per_megapixel, minimum| MegapixelRule {
            rounding,
            pixels_per_megapixel,
            minimum,
        };
        let floor = rule(MegapixelRounding::Floor, 1_000_000, 1);
        let ceil = rule(MegapixelRounding::Ceil, 1_000_000, 1);
        let nearest = rule(MegapixelRounding::Nearest, 1_000_000, 1);
        // 2688 × 1536 = 4,128,768 pixels.
        assert_eq!(floor.billed(2688, 1536), 4);
        assert_eq!(ceil.billed(2688, 1536), 5);
        assert_eq!(nearest.billed(2688, 1536), 4);
        // Exactly half rounds up; exact multiples never round.
        assert_eq!(nearest.billed(1500, 1000), 2);
        assert_eq!(nearest.billed(1499, 1000), 1);
        assert_eq!(ceil.billed(2000, 1000), 2);
        // Small images still bill the minimum.
        assert_eq!(floor.billed(512, 512), 1);
        assert_eq!(
            rule(MegapixelRounding::Floor, 1_000_000, 2).billed(512, 512),
            2
        );
        // A binary megapixel: 1024 × 1024 is exactly one.
        let binary = rule(MegapixelRounding::Ceil, 1 << 20, 1);
        assert_eq!(binary.billed(1024, 1024), 1);
        assert_eq!(binary.billed(1024, 1025), 2);
        assert_eq!(floor.quantities(1024, 1024), (1, 0));
        assert_eq!(floor.billed(65_535, 65_535), 4_294);
    }

    #[test]
    fn size_presets_and_megapixel_pricing_are_validated() {
        let check = |edit: &dyn Fn(&mut PackDocument), field: &str, reason: &str| {
            let mut document: PackDocument = toml::from_str(MEGAPIXEL).unwrap();
            edit(&mut document);
            match Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap_err() {
                PackError::Invalid {
                    field: f,
                    reason: r,
                } => {
                    assert_eq!(f, field, "{r}");
                    assert!(r.contains(reason), "{r}");
                }
                other => panic!("{other}"),
            }
        };
        check(
            &|d| {
                d.size_presets.pop();
            },
            "size_presets",
            "no preset certifies its size",
        );
        check(
            &|d| d.size_presets[1].size = [1024, 1024],
            "size_presets[1].size",
            "two presets",
        );
        check(
            &|d| d.size_presets[1].when = BTreeMap::from([("image_size".into(), "1k".into())]),
            "size_presets[1].when",
            "same parameters",
        );
        check(
            &|d| {
                d.size_presets[5]
                    .when
                    .insert("aspect_ratio".into(), "4:3".into());
            },
            "size_presets[5].when.aspect_ratio",
            "not a value",
        );
        check(
            &|d| d.size_presets[0].size = [0, 1024],
            "size_presets[0].size",
            "positive",
        );
        check(
            &|d| d.parameters[3].default = None,
            "size_presets",
            "required or have a default",
        );
        check(
            &|d| d.megapixels = None,
            "megapixels",
            "must declare how they are rounded",
        );
        check(
            &|d| d.megapixels.as_mut().unwrap().pixels_per_megapixel = 0,
            "megapixels",
            "must be positive",
        );
        check(
            &|d| {
                d.models[0].pricing_rules[0].components.pop();
            },
            "megapixels",
            "no pricing rule prices",
        );
        check(
            &|d| {
                d.size_presets.clear();
            },
            "megapixels",
            "needs size_presets",
        );
        check(
            &|d| {
                d.parameters[1].quantity = Some("additional_megapixels".into());
            },
            "parameters[1].quantity",
            "derived from the certified output size",
        );
        check(
            &|d| {
                d.parameters[2].selector = Some("image_size".into());
                d.parameters[2].output_dimensions =
                    BTreeMap::from([("1k".into(), [1024, 1024]), ("2k".into(), [2048, 2048])]);
            },
            "size_presets",
            "not both",
        );
        // A model that narrows a preset parameter needs only its own sizes.
        let mut narrowed: PackDocument = toml::from_str(MEGAPIXEL).unwrap();
        narrowed.models[0]
            .parameter_values
            .insert("aspect_ratio".into(), vec!["1:1".into()]);
        narrowed
            .size_presets
            .retain(|p| p.when["aspect_ratio"] == "1:1");
        let pack = Pack::from_toml_str(&toml::to_string(&narrowed).unwrap()).unwrap();
        assert_eq!(pack.output_sizes("megapixel-image-v1").len(), 2);
    }

    #[test]
    fn size_selected_rules_may_price_megapixels() {
        // One size parameter with output_dimensions that also selects the rule.
        let mut document: PackDocument = toml::from_str(MEGAPIXEL).unwrap();
        document.size_presets.clear();
        document.parameters.truncate(3);
        let size = &mut document.parameters[2];
        size.selector = Some("image_size".into());
        size.output_dimensions =
            BTreeMap::from([("1k".into(), [1024, 1024]), ("2k".into(), [2048, 2048])]);
        let mut rule_2k = document.models[0].pricing_rules[0].clone();
        rule_2k.rule_id = "megapixel-image-v1-2k".into();
        rule_2k.selector = Some(serde_json::from_value(json!({"image_size": "2k"})).unwrap());
        // 2k images carry a 1¢ surcharge per additional megapixel on top.
        rule_2k.components[1].rate_numerator_minor = 5;
        document.models[0].pricing_rules[0].selector =
            Some(serde_json::from_value(json!({"image_size": "1k"})).unwrap());
        document.models[0].pricing_rules.push(rule_2k);
        let pack = Pack::from_toml_str(&toml::to_string(&document).unwrap()).unwrap();
        // 2048² = 4,194,304 pixels: 4 megapixels, 3 additional at 2.5¢.
        let (request, snapshot) =
            megapixel_snapshot(&pack, json!({"prompt": "x", "image_size": "2k"}));
        assert_eq!(request.image_size(), Some("2k"));
        assert_eq!(snapshot.estimated_units, 105_000);
        let usage = delivered(&pack, &request, &[(2048, 2048)]).usage.unwrap();
        assert_eq!(
            snapshot.settle(&usage, snapshot.estimated_units).unwrap(),
            105_000
        );
        // A size still never selects a token or request price.
        assert!(!PricingUnit::prices_images_only([
            PricingUnit::Image,
            PricingUnit::InputToken
        ]));
        assert!(!PricingUnit::prices_images_only([PricingUnit::Request]));
        assert!(PricingUnit::prices_images_only([PricingUnit::Megapixel]));
    }

    #[test]
    fn megapixel_targets_publish_their_sizes_and_rounding() {
        let pack = Pack::from_toml_str(MEGAPIXEL).unwrap();
        let catalog = bind(&pack, &pack).unwrap();
        let targets = catalog.execution_targets();
        let target = &targets[0];
        assert!(target.image_sizes.is_empty());
        assert_eq!(target.output_sizes.len(), 6);
        assert!(target.output_sizes.contains(&OutputSize {
            when: BTreeMap::from([
                ("aspect_ratio".into(), "16:9".into()),
                ("image_size".into(), "2k".into()),
            ]),
            width: 2688,
            height: 1536,
        }));
        assert_eq!(
            target.megapixels.as_ref().unwrap().rounding,
            MegapixelRounding::Floor
        );
        assert_eq!(
            serde_json::to_value(target).unwrap()["megapixels"],
            json!({"rounding": "floor", "pixels_per_megapixel": 1_000_000, "minimum": 1})
        );
        // Targets without presets or megapixels publish exactly what they did.
        let mut plain = target.clone();
        plain.output_sizes.clear();
        plain.megapixels = None;
        let value = serde_json::to_value(&plain).unwrap();
        assert!(value.get("output_sizes").is_none() && value.get("megapixels").is_none());
    }

    const EDIT: &str =
        include_str!("../../../../packs/verified/google-gemini-developer-image-edit.toml");
    const DRAFT: &str = "0f5c2a64-3f0e-4c9b-9a51-2b7d1c4e8a10";
    const SKETCH: &str = "6b1e9d2c-8a47-4f3e-b5c0-91d2e7a4f638";

    fn edit_pack(from: &str, to: &str) -> Result<Pack, PackError> {
        assert_eq!(EDIT.matches(from).count(), 1, "{from}");
        Pack::from_toml_str(&EDIT.replace(from, to))
    }

    use crate::provider::contract::InputImage;

    struct Images(BTreeMap<&'static str, InputImage>);
    impl InputImageSource for Images {
        fn load(&self, artifact_id: &str) -> Result<InputImage, ContractError> {
            self.0
                .get(artifact_id)
                .cloned()
                .ok_or(ContractError::Provider {
                    code: "input_artifact_unavailable".into(),
                })
        }
    }

    #[test]
    fn edit_packs_send_each_loaded_input_image_and_price_it() {
        let pack = Pack::from_toml_str(EDIT).unwrap();
        let input =
            json!({"prompt": "blend them", "image_size": "1k", "input_images": [DRAFT, SKETCH]});
        // Pricing never needs the bytes: the body stays unrendered.
        let priced = pack.prepare("gemini-3.1-flash-image", &input).unwrap();
        assert_eq!(priced.body, Value::Null);
        assert_eq!(priced.request.workload(), "image_edit");
        assert_eq!(priced.request.quantity("input_images"), Some(2));
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": "edit",
                "rules": pack.pricing_rules(),
            }))
            .unwrap(),
        )
        .unwrap();
        // 6.7¢ for the output image and 0.056¢ for each input image.
        assert_eq!(
            catalog.snapshot(&priced.request).unwrap().estimated_units,
            68_120
        );
        assert_eq!(pack.input_artifact_ids(&input), [DRAFT, SKETCH]);

        let images = Images(BTreeMap::from([
            (
                DRAFT,
                InputImage {
                    media_type: "image/png".into(),
                    bytes: PNG.to_vec(),
                },
            ),
            (
                SKETCH,
                InputImage {
                    media_type: "image/jpeg".into(),
                    bytes: vec![0xff, 0xd8],
                },
            ),
        ]));
        let sent = pack
            .prepare_with_images("gemini-3.1-flash-image", &input, &images)
            .unwrap();
        assert_eq!(sent.request, priced.request);
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(
            sent.body,
            json!({
                "model": "gemini-3.1-flash-image",
                "input": [
                    {"type": "text", "text": "blend them"},
                    {"type": "image", "mime_type": "image/png", "data": encode(PNG)},
                    {"type": "image", "mime_type": "image/jpeg", "data": encode(&[0xff, 0xd8])},
                ],
                "response_format": {"type": "image", "image_size": "1K"},
            })
        );
        // An image that can't be loaded stops the request by name.
        let only_draft = Images(BTreeMap::from([(
            DRAFT,
            InputImage {
                media_type: "image/png".into(),
                bytes: PNG.to_vec(),
            },
        )]));
        assert_eq!(
            pack.prepare_with_images("gemini-3.1-flash-image", &input, &only_draft),
            Err(ContractError::Provider {
                code: "input_artifact_unavailable".into()
            })
        );

        for invalid in [
            json!({"prompt": "x", "image_size": "1k"}),
            json!({"prompt": "x", "image_size": "1k", "input_images": []}),
            json!({"prompt": "x", "image_size": "1k", "input_images": [DRAFT, DRAFT]}),
            json!({"prompt": "x", "image_size": "1k", "input_images": ["../executions/x.png"]}),
            json!({"prompt": "x", "image_size": "1k", "input_images": [DRAFT.to_uppercase()]}),
            json!({"prompt": "x", "image_size": "1k", "input_images": DRAFT}),
            json!({"prompt": "x", "image_size": "1k", "input_images": [DRAFT, SKETCH,
                "1b1e9d2c-8a47-4f3e-b5c0-91d2e7a4f638", "2b1e9d2c-8a47-4f3e-b5c0-91d2e7a4f638",
                "3b1e9d2c-8a47-4f3e-b5c0-91d2e7a4f638"]}),
        ] {
            assert!(
                pack.prepare("gemini-3.1-flash-image", &invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn images_parameters_are_required_bounded_and_spliced_into_an_array() {
        let item = r#"item = { type = "image", mime_type = "{{image.media_type}}", data = "{{image.data}}" }"#;
        for (from, to, field) in [
            (item, "", "parameters[1].item"),
            (
                item,
                r#"item = { type = "image", mime_type = "{{image.media_type}}" }"#,
                "parameters[1].item",
            ),
            (
                item,
                r#"item = { type = "image", data = "{{image.data}}", uri = "{{prompt}}" }"#,
                "parameters[1].item",
            ),
            ("max = 4\nquantity = \"input_images\"", "max = 17\nquantity = \"input_images\"", "parameters[1].max"),
            ("min = 1\nmax = 4\nquantity = \"input_images\"", "min = 0\nmax = 4\nquantity = \"input_images\"", "parameters[1].max"),
            ("type = \"images\"\nrequired = true", "type = \"images\"\nrequired = false", "parameters[1].required"),
            ("quantity = \"input_images\"\nitem", "quantity = \"image_count\"\nitem", "parameters[1].quantity"),
            (
                "max = 1\ndefault = 1\nquantity = \"image_count\"",
                "max = 1\ndefault = 1\nquantity = \"input_images\"",
                "parameters[2].quantity",
            ),
            (
                r#"input = [{ type = "text", text = "{{prompt}}" }, "{{input_images}}"]"#,
                "input = [{ type = \"text\", text = \"{{prompt}}\" }]\nimages = \"{{input_images}}\"",
                "request.body",
            ),
        ] {
            let error = edit_pack(from, to).unwrap_err();
            assert!(
                matches!(&error, PackError::Invalid { field: f, .. } if f == field),
                "{from} -> {to}: {error}"
            );
        }
        // Every rule must price the input images an edit sends.
        let error = edit_pack(
            r#"  { unit = "input_image", rate_numerator_minor = 28, rate_denominator = 1000 },"#,
            "",
        )
        .unwrap_err();
        assert!(error.to_string().contains("input_images"), "{error}");
    }
}
