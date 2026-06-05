use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_IMAGE_PROVIDER: &str = "local-mock";
const DEFAULT_IMAGE_MODEL: &str = "mock-image-v1";
const DEFAULT_IMAGE_MERCHANT: &str = "gongbu.image";
const DEFAULT_IMAGE_PRICE_CENTS: i64 = 500;
const DEFAULT_IMAGE_TIMEOUT_MS: u64 = 30_000;
const MAX_IMAGE_PROVIDER_RETRIES: u32 = 3;

#[derive(Debug, Clone)]
pub struct ImageProviderConfig {
    pub provider: String,
    pub model: String,
    pub merchant: String,
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
    pub price_cents: i64,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub http_json_fields: HttpJsonImageProviderFields,
    pub output_dir: PathBuf,
    pub adapter_kind: ImageProviderAdapterKind,
}

impl ImageProviderConfig {
    pub fn from_env(api_key: Option<String>) -> Result<Self> {
        let provider = env::var("GONGBU_IMAGE_PROVIDER_NAME")
            .unwrap_or_else(|_| DEFAULT_IMAGE_PROVIDER.to_string());
        Ok(Self {
            adapter_kind: image_provider_adapter_kind_from_env(&provider),
            provider,
            model: env::var("GONGBU_IMAGE_PROVIDER_MODEL")
                .unwrap_or_else(|_| DEFAULT_IMAGE_MODEL.to_string()),
            merchant: env::var("GONGBU_IMAGE_PROVIDER_MERCHANT")
                .unwrap_or_else(|_| DEFAULT_IMAGE_MERCHANT.to_string()),
            api_key,
            endpoint: env::var("GONGBU_IMAGE_PROVIDER_ENDPOINT").ok(),
            price_cents: image_provider_price_cents_from_env()?,
            timeout_ms: image_provider_timeout_ms_from_env()?,
            max_retries: image_provider_max_retries_from_env()?,
            http_json_fields: HttpJsonImageProviderFields::from_env()?,
            output_dir: image_output_dir_from_env(),
        })
    }

    pub fn resolve(
        &self,
        provider: Option<String>,
        model: Option<String>,
    ) -> Result<(String, String)> {
        let provider = provider.unwrap_or_else(|| self.provider.clone());
        let model = model.unwrap_or_else(|| self.model.clone());
        if provider != self.provider || model != self.model {
            return Err(anyhow!(
                "requested image provider/model is not configured in Gongbu"
            ));
        }
        Ok((provider, model))
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.as_ref().is_some_and(|key| !key.is_empty())
    }

    pub fn adapter(&self) -> Result<Box<dyn ImageProviderAdapter + '_>> {
        match &self.adapter_kind {
            ImageProviderAdapterKind::Mock => {
                if self.provider != DEFAULT_IMAGE_PROVIDER {
                    return Err(anyhow!(
                        "mock image adapter can only be used with the local-mock provider"
                    ));
                }
                Ok(Box::new(MockImageProviderAdapter { config: self }))
            }
            ImageProviderAdapterKind::HttpJson => {
                self.require_endpoint_and_key("http-json image adapter")?;
                Ok(Box::new(HttpJsonImageProviderAdapter { config: self }))
            }
            ImageProviderAdapterKind::GeminiGenerateContent => {
                self.require_endpoint_and_key("gemini-generate-content image adapter")?;
                Ok(Box::new(GeminiGenerateContentImageProviderAdapter {
                    config: self,
                }))
            }
            ImageProviderAdapterKind::Unsupported(adapter) => Err(anyhow!(
                "image provider adapter '{adapter}' is not supported by this Gongbu build"
            )),
        }
    }

    pub fn readiness(&self) -> ImageProviderReadiness {
        let mut missing_configuration = Vec::new();
        if !self.adapter_kind.is_supported() {
            missing_configuration.push("GONGBU_IMAGE_PROVIDER_ADAPTER".to_string());
        }
        match self.adapter_kind {
            ImageProviderAdapterKind::Mock | ImageProviderAdapterKind::Unsupported(_) => {}
            ImageProviderAdapterKind::HttpJson
            | ImageProviderAdapterKind::GeminiGenerateContent => {
                if !self
                    .endpoint
                    .as_ref()
                    .is_some_and(|endpoint| !endpoint.trim().is_empty())
                {
                    missing_configuration.push("GONGBU_IMAGE_PROVIDER_ENDPOINT".to_string());
                }
                if !self.has_api_key() {
                    missing_configuration.push("GONGBU_IMAGE_PROVIDER_API_KEY".to_string());
                }
                if missing_configuration.is_empty() && self.adapter().is_err() {
                    missing_configuration.push("GONGBU_IMAGE_PROVIDER_ENDPOINT".to_string());
                }
            }
        }
        ImageProviderReadiness {
            ready: missing_configuration.is_empty(),
            missing_configuration,
        }
    }

    fn require_endpoint_and_key(&self, adapter: &str) -> Result<()> {
        let endpoint = self
            .endpoint
            .as_deref()
            .filter(|endpoint| !endpoint.trim().is_empty())
            .ok_or_else(|| anyhow!("{adapter} requires GONGBU_IMAGE_PROVIDER_ENDPOINT"))?;
        if !is_allowed_provider_endpoint(endpoint) {
            return Err(anyhow!(
                "{adapter} endpoint must use https, except loopback http for local tests"
            ));
        }
        if !self.has_api_key() {
            return Err(anyhow!("{adapter} requires GONGBU_IMAGE_PROVIDER_API_KEY"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageProviderReadiness {
    pub ready: bool,
    pub missing_configuration: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HttpJsonImageProviderFields {
    provider: Option<String>,
    model: Option<String>,
    prompt: String,
    request_id: Option<String>,
    output_ref: String,
}

impl HttpJsonImageProviderFields {
    fn from_env() -> Result<Self> {
        Ok(Self {
            provider: optional_http_json_field_from_env(
                "GONGBU_IMAGE_PROVIDER_HTTP_JSON_PROVIDER_FIELD",
                Some("provider"),
            )?,
            model: optional_http_json_field_from_env(
                "GONGBU_IMAGE_PROVIDER_HTTP_JSON_MODEL_FIELD",
                Some("model"),
            )?,
            prompt: required_http_json_field_from_env(
                "GONGBU_IMAGE_PROVIDER_HTTP_JSON_PROMPT_FIELD",
                "prompt",
            )?,
            request_id: optional_http_json_field_from_env(
                "GONGBU_IMAGE_PROVIDER_HTTP_JSON_REQUEST_ID_FIELD",
                Some("request_id"),
            )?,
            output_ref: required_http_json_field_from_env(
                "GONGBU_IMAGE_PROVIDER_HTTP_JSON_OUTPUT_REF_FIELD",
                "output_ref",
            )?,
        })
    }

    #[cfg(test)]
    pub fn defaults() -> Self {
        Self {
            provider: Some("provider".to_string()),
            model: Some("model".to_string()),
            prompt: "prompt".to_string(),
            request_id: Some("request_id".to_string()),
            output_ref: "output_ref".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageProviderAdapterKind {
    Mock,
    HttpJson,
    GeminiGenerateContent,
    Unsupported(String),
}

impl ImageProviderAdapterKind {
    pub fn label(&self) -> &str {
        match self {
            Self::Mock => "mock",
            Self::HttpJson => "http-json",
            Self::GeminiGenerateContent => "gemini-generate-content",
            Self::Unsupported(adapter) => adapter,
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(
            self,
            Self::Mock | Self::HttpJson | Self::GeminiGenerateContent
        )
    }

    pub fn writes_local_artifact(&self) -> bool {
        matches!(self, Self::Mock | Self::GeminiGenerateContent)
    }
}

pub struct ImageGenerationRequest<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub artifact_id: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationOutput {
    pub output_ref: String,
}

pub trait ImageProviderAdapter {
    fn generate(
        &self,
        request: ImageGenerationRequest<'_>,
    ) -> Result<ImageGenerationOutput, ImageProviderError>;
}

#[derive(Debug, Clone)]
pub struct ImageProviderError {
    pub code: &'static str,
    pub status: Option<u16>,
    message: String,
}

impl ImageProviderError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            status: None,
            message: message.into(),
        }
    }

    pub fn with_status(code: &'static str, status: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            status: Some(status),
            message: message.into(),
        }
    }

    fn is_retryable(&self) -> bool {
        match self.code {
            "provider_timeout" | "provider_transport" => true,
            "provider_http_status" => self
                .status
                .is_some_and(|status| status == 429 || (500..=599).contains(&status)),
            _ => false,
        }
    }
}

impl std::fmt::Display for ImageProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(formatter, "{} ({status}): {}", self.code, self.message),
            None => write!(formatter, "{}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for ImageProviderError {}

pub fn redact_image_provider_error_message(message: &str, config: &ImageProviderConfig) -> String {
    let mut redacted = message.to_string();
    if let Some(endpoint) = config.endpoint.as_deref().filter(|value| !value.is_empty()) {
        redacted = redacted.replace(endpoint, "[redacted image provider endpoint]");
        if let Some((_, query)) = endpoint.split_once('?') {
            for pair in query.split('&') {
                if let Some((_, value)) = pair.split_once('=') {
                    if !value.is_empty() {
                        redacted =
                            redacted.replace(value, "[redacted image provider endpoint secret]");
                    }
                }
            }
        }
    }
    if let Some(api_key) = config.api_key.as_deref().filter(|value| !value.is_empty()) {
        redacted = redacted.replace(api_key, "[redacted image provider api key]");
    }
    redacted
}

pub fn ensure_image_output_dir_ready(
    output_dir: &Path,
    artifact_id: &str,
) -> Result<(), ImageProviderError> {
    std::fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::new(
            "provider_artifact_write_failed",
            format!(
                "create image output directory {}: {error}",
                output_dir.display()
            ),
        )
    })?;
    let probe_path = output_dir.join(format!(".gongbu-write-test-{artifact_id}"));
    std::fs::write(&probe_path, b"gongbu image output write test").map_err(|error| {
        ImageProviderError::new(
            "provider_artifact_write_failed",
            format!(
                "write image output probe to {}: {error}",
                probe_path.display()
            ),
        )
    })?;
    std::fs::remove_file(&probe_path).map_err(|error| {
        ImageProviderError::new(
            "provider_artifact_write_failed",
            format!(
                "remove image output probe from {}: {error}",
                probe_path.display()
            ),
        )
    })?;
    Ok(())
}

struct MockImageProviderAdapter<'a> {
    config: &'a ImageProviderConfig,
}

impl ImageProviderAdapter for MockImageProviderAdapter<'_> {
    fn generate(
        &self,
        request: ImageGenerationRequest<'_>,
    ) -> Result<ImageGenerationOutput, ImageProviderError> {
        std::fs::create_dir_all(&self.config.output_dir).map_err(|error| {
            ImageProviderError::new(
                "provider_artifact_write_failed",
                format!(
                    "create image output directory {}: {error}",
                    self.config.output_dir.display()
                ),
            )
        })?;
        let path = self
            .config
            .output_dir
            .join(format!("gongbu-image-{}.svg", request.artifact_id));
        std::fs::write(
            &path,
            mock_image_svg(request.provider, request.model, request.prompt),
        )
        .map_err(|error| {
            ImageProviderError::new(
                "provider_artifact_write_failed",
                format!("write mock image artifact to {}: {error}", path.display()),
            )
        })?;
        Ok(ImageGenerationOutput {
            output_ref: file_ref(path)?,
        })
    }
}

struct HttpJsonImageProviderAdapter<'a> {
    config: &'a ImageProviderConfig,
}

impl ImageProviderAdapter for HttpJsonImageProviderAdapter<'_> {
    fn generate(
        &self,
        request: ImageGenerationRequest<'_>,
    ) -> Result<ImageGenerationOutput, ImageProviderError> {
        let endpoint = self.config.endpoint.as_deref().ok_or_else(|| {
            ImageProviderError::new(
                "provider_config_invalid",
                "http-json image adapter requires an endpoint",
            )
        })?;
        let api_key = self.config.api_key.as_deref().ok_or_else(|| {
            ImageProviderError::new(
                "provider_config_invalid",
                "http-json image adapter requires an API key",
            )
        })?;
        let response = send_http_json_image_provider_request(
            endpoint,
            api_key,
            self.config.timeout_ms,
            self.config.max_retries,
            &self.config.http_json_fields,
            &request,
        )?;
        let body: Value = response.into_json().map_err(|error| {
            ImageProviderError::new(
                "provider_response_invalid",
                format!("parse image provider JSON response: {error}"),
            )
        })?;
        image_generation_output_from_provider_body(&body, &self.config.http_json_fields)
    }
}

struct GeminiGenerateContentImageProviderAdapter<'a> {
    config: &'a ImageProviderConfig,
}

impl ImageProviderAdapter for GeminiGenerateContentImageProviderAdapter<'_> {
    fn generate(
        &self,
        request: ImageGenerationRequest<'_>,
    ) -> Result<ImageGenerationOutput, ImageProviderError> {
        let endpoint = self.config.endpoint.as_deref().ok_or_else(|| {
            ImageProviderError::new(
                "provider_config_invalid",
                "gemini-generate-content image adapter requires an endpoint",
            )
        })?;
        let api_key = self.config.api_key.as_deref().ok_or_else(|| {
            ImageProviderError::new(
                "provider_config_invalid",
                "gemini-generate-content image adapter requires an API key",
            )
        })?;
        let response = send_gemini_generate_content_image_provider_request(
            endpoint,
            api_key,
            self.config.timeout_ms,
            self.config.max_retries,
            &request,
        )?;
        let body: Value = response.into_json().map_err(|error| {
            ImageProviderError::new(
                "provider_response_invalid",
                format!("parse Gemini image provider JSON response: {error}"),
            )
        })?;
        let image = gemini_image_from_provider_body(&body)?;
        write_gemini_image_artifact(&self.config.output_dir, request.artifact_id, image)
    }
}

#[derive(Debug)]
struct GeminiInlineImage<'a> {
    mime_type: &'a str,
    data: &'a str,
}

fn send_http_json_image_provider_request(
    endpoint: &str,
    api_key: &str,
    timeout_ms: u64,
    max_retries: u32,
    fields: &HttpJsonImageProviderFields,
    request: &ImageGenerationRequest<'_>,
) -> Result<ureq::Response, ImageProviderError> {
    let headers = http_json_image_provider_headers(api_key, request.artifact_id);
    let payload = http_json_image_provider_payload(request, fields);
    send_provider_json(endpoint, timeout_ms, max_retries, headers, payload)
}

fn send_gemini_generate_content_image_provider_request(
    endpoint: &str,
    api_key: &str,
    timeout_ms: u64,
    max_retries: u32,
    request: &ImageGenerationRequest<'_>,
) -> Result<ureq::Response, ImageProviderError> {
    let headers = gemini_generate_content_headers(api_key, request.artifact_id);
    let payload = gemini_generate_content_payload(request);
    send_provider_json(endpoint, timeout_ms, max_retries, headers, payload)
}

fn send_provider_json(
    endpoint: &str,
    timeout_ms: u64,
    max_retries: u32,
    headers: BTreeMap<&'static str, String>,
    payload: Value,
) -> Result<ureq::Response, ImageProviderError> {
    let timeout = Duration::from_millis(timeout_ms);
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();
    let mut attempts = 0;
    loop {
        let mut request = agent.post(endpoint);
        for (name, value) in &headers {
            request = request.set(name, value);
        }
        let response = request
            .send_json(payload.clone())
            .map_err(classify_http_json_provider_error);
        match response {
            Ok(response) => return Ok(response),
            Err(error) if attempts < max_retries && error.is_retryable() => attempts += 1,
            Err(error) => return Err(error),
        }
    }
}

fn image_generation_output_from_provider_body(
    body: &Value,
    fields: &HttpJsonImageProviderFields,
) -> Result<ImageGenerationOutput, ImageProviderError> {
    let output_ref = body
        .get(&fields.output_ref)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ImageProviderError::new(
                "provider_response_invalid",
                format!(
                    "image provider response missing non-empty {}",
                    fields.output_ref
                ),
            )
        })?;
    Ok(ImageGenerationOutput {
        output_ref: output_ref.to_string(),
    })
}

fn gemini_image_from_provider_body(
    body: &Value,
) -> Result<GeminiInlineImage<'_>, ImageProviderError> {
    let parts = body
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImageProviderError::new(
                "provider_response_invalid",
                "Gemini response missing candidates[0].content.parts",
            )
        })?;

    for part in parts {
        let inline_data = part
            .get("inlineData")
            .or_else(|| part.get("inline_data"))
            .and_then(Value::as_object);
        let Some(inline_data) = inline_data else {
            continue;
        };
        let mime_type = inline_data
            .get("mimeType")
            .or_else(|| inline_data.get("mime_type"))
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("image/"))
            .ok_or_else(|| {
                ImageProviderError::new(
                    "provider_response_invalid",
                    "Gemini inline image missing image mime type",
                )
            })?;
        let data = inline_data
            .get("data")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ImageProviderError::new(
                    "provider_response_invalid",
                    "Gemini inline image missing base64 data",
                )
            })?;
        return Ok(GeminiInlineImage { mime_type, data });
    }

    Err(ImageProviderError::new(
        "provider_response_invalid",
        "Gemini response did not include inline image data",
    ))
}

fn write_gemini_image_artifact(
    output_dir: &Path,
    artifact_id: &str,
    image: GeminiInlineImage<'_>,
) -> Result<ImageGenerationOutput, ImageProviderError> {
    std::fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::new(
            "provider_artifact_write_failed",
            format!(
                "create image output directory {}: {error}",
                output_dir.display()
            ),
        )
    })?;
    let extension = image_extension_for_mime_type(image.mime_type)?;
    let image_bytes = BASE64_STANDARD.decode(image.data).map_err(|error| {
        ImageProviderError::new(
            "provider_response_invalid",
            format!("decode Gemini inline image data: {error}"),
        )
    })?;
    let path = output_dir.join(format!("gongbu-image-{artifact_id}.{extension}"));
    std::fs::write(&path, image_bytes).map_err(|error| {
        ImageProviderError::new(
            "provider_artifact_write_failed",
            format!("write Gemini image artifact to {}: {error}", path.display()),
        )
    })?;
    Ok(ImageGenerationOutput {
        output_ref: file_ref(path)?,
    })
}

fn image_extension_for_mime_type(mime_type: &str) -> Result<&'static str, ImageProviderError> {
    match mime_type {
        "image/png" => Ok("png"),
        "image/jpeg" | "image/jpg" => Ok("jpg"),
        "image/webp" => Ok("webp"),
        other => Err(ImageProviderError::new(
            "provider_response_invalid",
            format!("unsupported Gemini image mime type: {other}"),
        )),
    }
}

fn http_json_image_provider_payload(
    request: &ImageGenerationRequest<'_>,
    fields: &HttpJsonImageProviderFields,
) -> Value {
    let mut payload = serde_json::Map::new();
    if let Some(field) = &fields.provider {
        payload.insert(field.clone(), json!(request.provider));
    }
    if let Some(field) = &fields.model {
        payload.insert(field.clone(), json!(request.model));
    }
    payload.insert(fields.prompt.clone(), json!(request.prompt));
    if let Some(field) = &fields.request_id {
        payload.insert(field.clone(), json!(request.artifact_id));
    }
    Value::Object(payload)
}

fn gemini_generate_content_payload(request: &ImageGenerationRequest<'_>) -> Value {
    json!({
        "contents": [{
            "parts": [{
                "text": request.prompt,
            }],
        }],
        "generationConfig": {
            "responseModalities": ["IMAGE"],
        },
    })
}

fn http_json_image_provider_headers(
    api_key: &str,
    artifact_id: &str,
) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("Authorization", format!("Bearer {api_key}")),
        ("Idempotency-Key", artifact_id.to_string()),
        ("X-Gongbu-Request-Id", artifact_id.to_string()),
    ])
}

fn gemini_generate_content_headers(
    api_key: &str,
    artifact_id: &str,
) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("x-goog-api-key", api_key.to_string()),
        ("X-Gongbu-Request-Id", artifact_id.to_string()),
    ])
}

fn classify_http_json_provider_error(error: ureq::Error) -> ImageProviderError {
    match error {
        ureq::Error::Status(status, response) => ImageProviderError::with_status(
            "provider_http_status",
            status,
            format!(
                "image provider returned HTTP {status}: {}",
                response.status_text()
            ),
        ),
        ureq::Error::Transport(transport) => {
            let message = transport.to_string();
            let lower = message.to_ascii_lowercase();
            let code = if lower.contains("timed out") || lower.contains("timeout") {
                "provider_timeout"
            } else {
                "provider_transport"
            };
            ImageProviderError::new(code, format!("call image provider endpoint: {message}"))
        }
    }
}

pub fn is_allowed_provider_endpoint(endpoint: &str) -> bool {
    if let Some(authority) = endpoint.strip_prefix("https://").and_then(authority) {
        return !authority.is_empty() && !authority.contains('@');
    }
    let Some(authority) = endpoint.strip_prefix("http://").and_then(authority) else {
        return false;
    };
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = authority
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| authority.split(':').next().unwrap_or(authority));
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn authority(rest: &str) -> Option<&str> {
    Some(rest.split('/').next().unwrap_or(rest))
}

fn image_provider_adapter_kind_from_env(provider: &str) -> ImageProviderAdapterKind {
    match env::var("GONGBU_IMAGE_PROVIDER_ADAPTER") {
        Ok(adapter) if adapter == "mock" => ImageProviderAdapterKind::Mock,
        Ok(adapter) if adapter == "http-json" => ImageProviderAdapterKind::HttpJson,
        Ok(adapter) if adapter == "gemini-generate-content" => {
            ImageProviderAdapterKind::GeminiGenerateContent
        }
        Ok(adapter) => ImageProviderAdapterKind::Unsupported(adapter),
        Err(_) if provider == DEFAULT_IMAGE_PROVIDER => ImageProviderAdapterKind::Mock,
        Err(_) => ImageProviderAdapterKind::Unsupported("unconfigured".to_string()),
    }
}

fn image_provider_price_cents_from_env() -> Result<i64> {
    match env::var("GONGBU_IMAGE_PROVIDER_PRICE_CENTS") {
        Ok(value) => {
            let price_cents = value
                .parse::<i64>()
                .context("parse GONGBU_IMAGE_PROVIDER_PRICE_CENTS as cents")?;
            if price_cents <= 0 {
                return Err(anyhow!(
                    "GONGBU_IMAGE_PROVIDER_PRICE_CENTS must be positive"
                ));
            }
            Ok(price_cents)
        }
        Err(_) => Ok(DEFAULT_IMAGE_PRICE_CENTS),
    }
}

fn image_provider_timeout_ms_from_env() -> Result<u64> {
    match env::var("GONGBU_IMAGE_PROVIDER_TIMEOUT_MS") {
        Ok(value) => parse_positive_millis_env("GONGBU_IMAGE_PROVIDER_TIMEOUT_MS", &value),
        Err(_) => Ok(DEFAULT_IMAGE_TIMEOUT_MS),
    }
}

fn image_provider_max_retries_from_env() -> Result<u32> {
    match env::var("GONGBU_IMAGE_PROVIDER_MAX_RETRIES") {
        Ok(value) => parse_max_retries_env(&value),
        Err(_) => Ok(0),
    }
}

fn parse_max_retries_env(value: &str) -> Result<u32> {
    let max_retries = value
        .parse::<u32>()
        .context("parse GONGBU_IMAGE_PROVIDER_MAX_RETRIES as a count")?;
    if max_retries > MAX_IMAGE_PROVIDER_RETRIES {
        return Err(anyhow!(
            "GONGBU_IMAGE_PROVIDER_MAX_RETRIES must be {MAX_IMAGE_PROVIDER_RETRIES} or less"
        ));
    }
    Ok(max_retries)
}

fn parse_positive_millis_env(name: &str, value: &str) -> Result<u64> {
    let millis = value
        .parse::<u64>()
        .with_context(|| format!("parse {name} as milliseconds"))?;
    if millis == 0 {
        return Err(anyhow!("{name} must be positive"));
    }
    Ok(millis)
}

fn required_http_json_field_from_env(name: &str, default: &str) -> Result<String> {
    let value = env::var(name).unwrap_or_else(|_| default.to_string());
    let value = value.trim();
    if value.is_empty() {
        return Err(anyhow!("{name} must not be empty"));
    }
    Ok(value.to_string())
}

fn optional_http_json_field_from_env(name: &str, default: Option<&str>) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => {
            let value = value.trim();
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.to_string()))
            }
        }
        Err(_) => Ok(default.map(str::to_string)),
    }
}

fn image_output_dir_from_env() -> PathBuf {
    env::var("GONGBU_IMAGE_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("target")
                .join("gongbu-image-outputs")
        })
}

fn file_ref(path: PathBuf) -> Result<String, ImageProviderError> {
    let absolute_path = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|error| {
                ImageProviderError::new(
                    "provider_artifact_write_failed",
                    format!("resolve current directory: {error}"),
                )
            })?
            .join(path)
    };
    Ok(format!("file://{}", absolute_path.display()))
}

fn mock_image_svg(provider: &str, model: &str, prompt: &str) -> String {
    let prompt_preview = prompt.chars().take(120).collect::<String>();
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" role="img">
  <title>Gongbu mock image artifact</title>
  <desc>Generated by Gongbu mock provider {provider} using {model}. Prompt: {prompt}</desc>
  <rect width="1024" height="1024" fill="#f5f7fa"/>
  <circle cx="512" cy="424" r="224" fill="#1f6feb"/>
  <path d="M318 604h388v70H318z" fill="#24292f"/>
  <text x="512" y="438" text-anchor="middle" font-family="Arial, sans-serif" font-size="116" font-weight="700" fill="#fff">GB</text>
  <text x="512" y="760" text-anchor="middle" font-family="Arial, sans-serif" font-size="28" fill="#57606a">{prompt_preview}</text>
</svg>"##,
        provider = escape_xml(provider),
        model = escape_xml(model),
        prompt = escape_xml(&prompt_preview),
        prompt_preview = escape_xml(&prompt_preview),
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_allows_https_and_loopback_http_only() {
        assert!(is_allowed_provider_endpoint(
            "https://vendor.example/v1/images"
        ));
        assert!(is_allowed_provider_endpoint(
            "http://127.0.0.1:9000/v1/images"
        ));
        assert!(is_allowed_provider_endpoint(
            "http://localhost:9000/v1/images"
        ));
        assert!(!is_allowed_provider_endpoint(
            "http://vendor.example/v1/images"
        ));
        assert!(!is_allowed_provider_endpoint(
            "http://127.0.0.1:9000@vendor.example/v1/images"
        ));
        assert!(!is_allowed_provider_endpoint(
            "https://token@vendor.example/v1/images"
        ));
    }

    #[test]
    fn gemini_payload_and_headers_keep_api_key_server_side() {
        let request = ImageGenerationRequest {
            provider: "google-gemini",
            model: "gemini-2.5-flash-image",
            prompt: "Create a crisp logo for Project Hubu",
            artifact_id: "sat_test",
        };
        let payload = gemini_generate_content_payload(&request);
        let headers = gemini_generate_content_headers("server-side-secret", request.artifact_id);

        assert_eq!(
            payload["contents"][0]["parts"][0]["text"],
            "Create a crisp logo for Project Hubu"
        );
        assert_eq!(
            payload["generationConfig"]["responseModalities"][0],
            "IMAGE"
        );
        assert!(!payload.to_string().contains("server-side-secret"));
        assert_eq!(headers["x-goog-api-key"], "server-side-secret");
    }

    #[test]
    fn gemini_inline_image_writes_artifact() {
        let body = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": BASE64_STANDARD.encode(b"png-bytes"),
                        }
                    }]
                }
            }]
        });
        let image = gemini_image_from_provider_body(&body).expect("inline image should parse");
        let output_dir =
            std::env::temp_dir().join(format!("gongbu-gemini-image-output-{}", std::process::id()));
        let output =
            write_gemini_image_artifact(&output_dir, "sat_test", image).expect("artifact writes");
        let output_path = output
            .output_ref
            .strip_prefix("file://")
            .expect("output should be file URI");
        assert!(output_path.ends_with("gongbu-image-sat_test.png"));
        assert_eq!(
            std::fs::read(output_path).expect("artifact readable"),
            b"png-bytes"
        );
        std::fs::remove_file(output_path).ok();
        std::fs::remove_dir(output_dir).ok();
    }

    #[test]
    fn redaction_hides_server_side_config() {
        let config = ImageProviderConfig {
            provider: "google-gemini".to_string(),
            model: "gemini-2.5-flash-image".to_string(),
            merchant: "gongbu.image".to_string(),
            api_key: Some("server-side-secret".to_string()),
            endpoint: Some("https://vendor.example/v1/images?signature=query-secret".to_string()),
            price_cents: 500,
            timeout_ms: 30_000,
            max_retries: 0,
            http_json_fields: HttpJsonImageProviderFields::defaults(),
            output_dir: std::env::temp_dir(),
            adapter_kind: ImageProviderAdapterKind::GeminiGenerateContent,
        };
        let redacted = redact_image_provider_error_message(
            "https://vendor.example/v1/images?signature=query-secret failed with server-side-secret and query-secret",
            &config,
        );
        assert!(!redacted.contains("server-side-secret"));
        assert!(!redacted.contains("query-secret"));
        assert!(!redacted.contains("vendor.example"));
    }
}
