//! Black Forest Labs FLUX.2 asynchronous image-generation adapter.
//!
//! A generation is submitted exactly once. If it is asynchronous, the returned
//! operation URL is polled under the same overall deadline; polling never
//! resubmits the generation. Returned bytes remain untrusted until the shared
//! artifact service validates and stores them.

use super::{
    contract::{
        canonical_image_media_type, ActualVendorCost, AdapterCapabilities, AdapterOutcome,
        AdapterSubmission, AsyncProviderOperation, ContractError, NoopProviderTransportObserver,
        NormalizedArtifact, NormalizedRequest, NormalizedUsage, OutputDimensions, ProviderAdapter,
        ProviderFailure, ProviderPhase, ProviderTransportInteraction, ProviderTransportObserver,
        Result, RetryPolicy,
    },
    http_kernel::{
        provider_request_id, read_bounded, shared_client, url_has_explicit_port,
        validate_https_origin, ArtifactDownloadPolicy, CredentialForwarding, InvocationDeadline,
    },
    targets::{
        valid_artifact_hosts, valid_bfl_api_host, valid_bfl_delivery_host, Flux2ApiConfig,
        ProviderConfigVersion,
    },
};
use crate::{redaction::Redactor, secrets::ProviderSecret};
use reqwest::{
    header::{HeaderName, HeaderValue},
    Url,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, error::Error as StdError, fmt, io::Read, thread, time::Duration};

pub const PROVIDER_ID: &str = "flux";
pub const ADAPTER_ID: &str = "flux2_api";
pub const MODEL_ID: &str = "flux-2-pro";
pub const MIN_DIMENSION: u32 = 64;
pub const DIMENSION_MULTIPLE: u32 = 16;
pub const MAX_OUTPUT_PIXELS: u64 = 2048 * 2048;
pub const SUPPORTED_PRESETS: &[&str] = &["1k", "2k", "4k"];
/// BFL bills in credits and defines one credit as exactly USD 0.01.
const BFL_CREDIT_USD_SCALE: u32 = 2;
#[cfg(test)]
const MAX_ARTIFACT_BYTES: usize = crate::artifact::DEFAULT_MAX_ENCODED_BYTES as usize;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct TransportResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: Value,
}

pub trait Flux2Transport: Send + Sync {
    fn submit(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
        idempotency: Option<(&str, &str)>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;
    fn poll(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>>;
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>>;
}

impl<T: Flux2Transport + ?Sized> Flux2Transport for std::sync::Arc<T> {
    fn submit(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
        idempotency: Option<(&str, &str)>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref()
            .submit(url, credential, timeout, headers, idempotency, body)
    }
    fn poll(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        self.as_ref().poll(url, credential, timeout, headers)
    }
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        self.as_ref().fetch_artifact(url, timeout, max_bytes)
    }
}

#[derive(Debug)]
enum HttpFailure {
    BeforeSend,
    UnknownOutcome,
}
impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FLUX HTTP failure")
    }
}
impl StdError for HttpFailure {}

pub struct ReqwestFlux2Transport;
impl ReqwestFlux2Transport {
    fn response(
        mut response: reqwest::blocking::Response,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let status = response.status().as_u16();
        let request_id =
            provider_request_id(response.headers(), &["x-request-id", "x-bfl-request-id"]);
        if (400..500).contains(&status) {
            return Ok(TransportResponse {
                status,
                request_id,
                body: Value::Null,
            });
        }
        let bytes =
            read_provider_response_bounded(&mut response, MAX_RESPONSE_BYTES).map_err(|_| {
                Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
            })?;
        let body = serde_json::from_slice(&bytes).map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
        })?;
        Ok(TransportResponse {
            status,
            request_id,
            body,
        })
    }
}
impl Flux2Transport for ReqwestFlux2Transport {
    fn submit(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
        idempotency: Option<(&str, &str)>,
        body: &Value,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let credential = std::str::from_utf8(credential)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .post(url.clone())
            .timeout(timeout)
            .header("x-key", credential)
            .json(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some((name, value)) = idempotency {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
        })?;
        Self::response(response)
    }
    fn poll(
        &self,
        url: &Url,
        credential: &[u8],
        timeout: Duration,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
        let credential = std::str::from_utf8(credential)
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?;
        let mut request = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .get(url.clone())
            .timeout(timeout)
            .header("x-key", credential);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Self::response(request.send().map_err(|_| {
            Box::new(HttpFailure::UnknownOutcome) as Box<dyn StdError + Send + Sync>
        })?)
    }
    fn fetch_artifact(
        &self,
        url: &Url,
        timeout: Duration,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
        let mut response = shared_client()
            .map_err(|_| Box::new(HttpFailure::BeforeSend) as Box<dyn StdError + Send + Sync>)?
            .get(url.clone())
            .timeout(timeout)
            .send()?;
        if !response.status().is_success() {
            return Err(Box::new(HttpFailure::UnknownOutcome));
        }
        read_artifact_response_bounded(&mut response, max_bytes)
    }
}

fn read_provider_response_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    read_bounded(reader, limit).map_err(|error| Box::new(error) as _)
}

fn read_artifact_response_bounded(
    reader: &mut impl Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
    read_bounded(reader, limit).map_err(|error| Box::new(error) as _)
}

pub struct Flux2ApiAdapter<T = ReqwestFlux2Transport> {
    config: Flux2ApiConfig,
    model: String,
    transport: T,
    max_artifact_bytes: usize,
}
impl Flux2ApiAdapter<ReqwestFlux2Transport> {
    pub fn from_target(target: &ProviderConfigVersion) -> Result<Self> {
        Self::from_target_with_artifact_limit(target, crate::artifact::DEFAULT_MAX_ENCODED_BYTES)
    }

    pub fn from_target_with_artifact_limit(
        target: &ProviderConfigVersion,
        max_artifact_bytes: u64,
    ) -> Result<Self> {
        if target.provider != PROVIDER_ID || target.adapter != ADAPTER_ID {
            return Err(provider_error("target_mismatch"));
        }
        Self::new_with_artifact_limit(
            target
                .flux2_api()
                .cloned()
                .ok_or_else(|| provider_error("config_invalid"))?,
            target.model.clone(),
            ReqwestFlux2Transport,
            max_artifact_bytes,
        )
    }
}
impl<T: Flux2Transport> Flux2ApiAdapter<T> {
    pub fn new(config: Flux2ApiConfig, model: String, transport: T) -> Result<Self> {
        Self::new_with_artifact_limit(
            config,
            model,
            transport,
            crate::artifact::DEFAULT_MAX_ENCODED_BYTES,
        )
    }

    pub fn new_with_artifact_limit(
        config: Flux2ApiConfig,
        model: String,
        transport: T,
        max_artifact_bytes: u64,
    ) -> Result<Self> {
        RetryPolicy {
            max_retries: config.max_retries,
        }
        .validate(AdapterCapabilities {
            // Header support is not evidence that the vendor enforces idempotency.
            vendor_enforced_idempotency: false,
        })?;
        if model != MODEL_ID
            || config.timeout_ms == 0
            || config.poll_interval_ms == 0
            || config.max_retries != 0
            || !valid_artifact_hosts(&config.approved_artifact_hosts, false)
            || !config
                .approved_artifact_hosts
                .iter()
                .all(|host| valid_bfl_delivery_host(host))
            || max_artifact_bytes == 0
            || max_artifact_bytes > usize::MAX as u64
        {
            return Err(provider_error("config_invalid"));
        }
        for (name, value) in &config.headers {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            HeaderValue::from_str(value).map_err(|_| provider_error("config_invalid"))?;
        }
        if let Some(name) = &config.idempotency_header {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| provider_error("config_invalid"))?;
            if name.eq_ignore_ascii_case("x-key")
                || name.eq_ignore_ascii_case("authorization")
                || config
                    .headers
                    .keys()
                    .any(|header| header.eq_ignore_ascii_case(name))
            {
                return Err(provider_error("config_invalid"));
            }
        }
        submit_url(&config, &model)?;
        Ok(Self {
            config,
            model,
            transport,
            max_artifact_bytes: max_artifact_bytes as usize,
        })
    }

    fn submission_body(
        &self,
        request: &NormalizedRequest,
        input: &Value,
    ) -> Result<Value, ProviderFailure> {
        self.preflight_input(request, input)
            .map_err(|_| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty() && p.len() <= 32_000)
            .ok_or_else(|| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let dimensions = request
            .output_dimensions
            .as_ref()
            .ok_or_else(|| ProviderFailure::release("invalid_request", ProviderPhase::PreSend))?;
        let mut body = json!({
            "prompt": prompt,
            "width": dimensions.width,
            "height": dimensions.height,
        });
        let options = input.get("options").and_then(Value::as_object);
        for field in ["seed", "safety_tolerance", "output_format"] {
            if let Some(value) = options.and_then(|options| options.get(field)) {
                body[field] = value.clone();
            }
        }
        Ok(body)
    }

    fn submit_once_inner(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        let body = self.submission_body(request, input)?;
        let deadline =
            InvocationDeadline::from_timeout(Duration::from_millis(self.config.timeout_ms))
                .map_err(|_| ProviderFailure::release("config_invalid", ProviderPhase::PreSend))?;
        let idempotency = match (&self.config.idempotency_header, idempotency_key) {
            (Some(name), Some(key)) => Some((name.as_str(), key)),
            _ => None,
        };
        let response = self
            .transport
            .submit(
                &submit_url(&self.config, &self.model).map_err(|_| {
                    ProviderFailure::release("config_invalid", ProviderPhase::PreSend)
                })?,
                secret.expose(),
                deadline.remaining().map_err(|_| {
                    ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
                })?,
                &self.config.headers,
                idempotency,
                &body,
            )
            .map_err(|error| classify_transport(error, secret, true, None, None))?;
        let request_id = safe_evidence(response.request_id, secret)
            .or_else(|| safe_string_at(&response.body, &["request_id", "requestId"], secret));
        if (400..500).contains(&response.status) {
            return Err(classify_http_status(
                response.status,
                ProviderPhase::Submission,
                false,
                request_id,
                None,
            ));
        }
        if !(200..300).contains(&response.status) {
            return Err(reconcile_with_evidence(
                "provider_failure",
                ProviderPhase::Submission,
                request_id,
                None,
            ));
        }
        let operation_id = safe_string_at(
            &response.body,
            &["id", "operation_id", "operationId"],
            secret,
        );
        let current = response.body;
        let state = classify_result_state(&current, true).map_err(|_| {
            with_evidence(
                "malformed_response",
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        if let Some(failure) = state.failure(request_id.clone(), operation_id.clone()) {
            return Err(failure);
        }
        if state == BflResultState::Ready {
            return self
                .complete_ready(
                    current,
                    request_id,
                    operation_id,
                    secret,
                    deadline,
                    observer,
                )
                .map(AdapterSubmission::Complete);
        }

        let operation_id = operation_id
            .ok_or_else(|| with_evidence("malformed_response", request_id.clone(), None))?;
        let polling_host =
            polling_checkpoint_host(&current, &operation_id, &self.config.api_version).map_err(
                |error| {
                    let code = match error {
                        ContractError::Provider { code } => code,
                        _ => "provider_contract_failure".into(),
                    };
                    ProviderFailure::reconcile(code, ProviderPhase::Processing)
                        .with_evidence(request_id.clone(), Some(operation_id.clone()))
                },
            )?;
        let operation = AsyncProviderOperation {
            provider_request_id: request_id,
            provider_operation_id: operation_id,
            polling_host,
            deadline_unix_ms: deadline.unix_millis(),
        };
        operation.validate().map_err(|_| {
            with_evidence(
                "invalid_provider_operation",
                operation.provider_request_id.clone(),
                Some(operation.provider_operation_id.clone()),
            )
        })?;
        Ok(AdapterSubmission::Pending(operation))
    }

    fn poll_existing_inner(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let evidence = || {
            (
                operation.provider_request_id.clone(),
                Some(operation.provider_operation_id.clone()),
            )
        };
        self.preflight_input(request, input).map_err(|_| {
            let (request_id, operation_id) = evidence();
            with_evidence("invalid_request", request_id, operation_id)
        })?;
        operation.validate().map_err(|_| {
            let (request_id, operation_id) = evidence();
            with_evidence("invalid_provider_operation", request_id, operation_id)
        })?;
        let deadline =
            InvocationDeadline::from_unix_millis(operation.deadline_unix_ms).map_err(|_| {
                let (request_id, operation_id) = evidence();
                with_evidence("timeout_unknown_outcome", request_id, operation_id)
            })?;
        let poll_url = operation_poll_url(&self.config, operation).map_err(|_| {
            let (request_id, operation_id) = evidence();
            with_evidence("invalid_provider_operation", request_id, operation_id)
        })?;
        let operation_id = operation.provider_operation_id.clone();
        let mut request_id = operation.provider_request_id.clone();
        loop {
            let wait = Duration::from_millis(self.config.poll_interval_ms).min(
                deadline.remaining().map_err(|_| {
                    with_evidence(
                        "timeout_unknown_outcome",
                        request_id.clone(),
                        Some(operation_id.clone()),
                    )
                })?,
            );
            thread::sleep(wait);
            let poll_timeout = deadline.remaining().map_err(|_| {
                with_evidence(
                    "timeout_unknown_outcome",
                    request_id.clone(),
                    Some(operation_id.clone()),
                )
            })?;
            if !observer.record(ProviderTransportInteraction::Poll) {
                return Err(reconcile_with_evidence(
                    "provider_transport_evidence_unavailable",
                    ProviderPhase::Processing,
                    request_id,
                    Some(operation_id),
                ));
            }
            let response = self
                .transport
                .poll(
                    &poll_url,
                    secret.expose(),
                    poll_timeout,
                    &self.config.headers,
                )
                .map_err(|error| {
                    classify_transport(
                        error,
                        secret,
                        false,
                        request_id.clone(),
                        Some(operation_id.clone()),
                    )
                })?;
            let poll_request_id = safe_evidence(response.request_id, secret)
                .or_else(|| safe_string_at(&response.body, &["request_id", "requestId"], secret));
            if request_id.is_none() {
                request_id = poll_request_id;
            }
            if (400..500).contains(&response.status) {
                return Err(classify_http_status(
                    response.status,
                    ProviderPhase::Processing,
                    true,
                    request_id,
                    Some(operation_id),
                ));
            }
            if !(200..300).contains(&response.status) {
                return Err(reconcile_with_evidence(
                    "provider_failure",
                    ProviderPhase::Processing,
                    request_id,
                    Some(operation_id),
                ));
            }
            let current = response.body;
            validate_poll_operation_identity(&current, &operation_id, request_id.clone(), secret)?;
            let state = classify_result_state(&current, false).map_err(|_| {
                with_evidence(
                    "malformed_response",
                    request_id.clone(),
                    Some(operation_id.clone()),
                )
            })?;
            match state {
                BflResultState::Ready => {
                    return self.complete_ready(
                        current,
                        request_id,
                        Some(operation_id),
                        secret,
                        deadline,
                        observer,
                    );
                }
                BflResultState::Pending => {}
                _ => {
                    return Err(state
                        .failure(request_id.clone(), Some(operation_id.clone()))
                        .expect("terminal BFL state has a failure"));
                }
            }
        }
    }

    fn complete_ready(
        &self,
        current: Value,
        request_id: Option<String>,
        operation_id: Option<String>,
        secret: &ProviderSecret,
        deadline: InvocationDeadline,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let raw_artifact_url = artifact_url(&current).ok_or_else(|| {
            with_evidence(
                if current.get("result").is_some() {
                    "missing_image"
                } else {
                    "malformed_response"
                },
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        let artifact_url = Url::parse(raw_artifact_url)
            .map_err(|_| artifact_failure(request_id.clone(), operation_id.clone()))?;
        if url_has_explicit_port(raw_artifact_url)
            || artifact_url
                .host_str()
                .is_none_or(|host| !valid_bfl_delivery_host(host))
        {
            return Err(artifact_failure(request_id, operation_id));
        }
        let policy_hosts = if self.config.approved_artifact_hosts.is_empty() {
            vec![artifact_url
                .host_str()
                .expect("validated BFL delivery URL has a host")
                .to_owned()]
        } else {
            self.config.approved_artifact_hosts.clone()
        };
        let artifact_policy = ArtifactDownloadPolicy::new(
            &policy_hosts,
            self.max_artifact_bytes as u64,
            CredentialForwarding::Prohibited,
        )
        .map_err(|_| artifact_failure(request_id.clone(), operation_id.clone()))?;
        if artifact_policy.validate_url(&artifact_url).is_err() {
            return Err(artifact_failure(request_id, operation_id));
        }
        let artifact_timeout = deadline
            .remaining()
            .map_err(|_| artifact_failure(request_id.clone(), operation_id.clone()))?;
        if !observer.record(ProviderTransportInteraction::ArtifactFetch) {
            return Err(reconcile_with_evidence(
                "provider_transport_evidence_unavailable",
                ProviderPhase::Artifact,
                request_id,
                operation_id,
            ));
        }
        let bytes = self
            .transport
            .fetch_artifact(&artifact_url, artifact_timeout, self.max_artifact_bytes)
            .map_err(|error| {
                let _ = Redactor::new([secret.expose()]).error_chain(error.as_ref());
                artifact_failure(request_id.clone(), operation_id.clone())
            })?;
        if bytes.len() > self.max_artifact_bytes {
            return Err(artifact_failure(request_id, operation_id));
        }
        let media_type = canonical_image_media_type(None, &bytes)
            .map_err(|_| artifact_failure(request_id.clone(), operation_id.clone()))?;
        let actual_vendor_cost = settled_cost(&current).map_err(|_| {
            with_evidence(
                "invalid_provider_cost",
                request_id.clone(),
                operation_id.clone(),
            )
        })?;
        Ok(AdapterOutcome {
            usage: Some(NormalizedUsage {
                images: Some(1),
                ..Default::default()
            }),
            actual_vendor_cost,
            provider_request_id: request_id,
            provider_operation_id: operation_id,
            artifacts: vec![NormalizedArtifact {
                media_type: media_type.into(),
                bytes,
            }],
        })
    }
}

impl<T: Flux2Transport> ProviderAdapter for Flux2ApiAdapter<T> {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: false,
        }
    }
    fn validate_request(&self, request: &NormalizedRequest) -> Result<()> {
        request.validate()?;
        if request.provider != PROVIDER_ID
            || request.model != MODEL_ID
            || request.model != self.model
            || request.image_count != Some(1)
        {
            return Err(provider_error("invalid_request"));
        }
        let expected = contract_dimensions(
            request
                .image_size
                .as_deref()
                .ok_or_else(|| provider_error("invalid_request"))?,
        )?;
        let dimensions = request
            .output_dimensions
            .as_ref()
            .ok_or_else(|| provider_error("invalid_request"))?;
        validate_dimensions(dimensions)?;
        if dimensions != &expected {
            return Err(provider_error("invalid_request"));
        }
        Ok(())
    }
    fn preflight_input(&self, request: &NormalizedRequest, input: &Value) -> Result<()> {
        self.validate_request(request)?;
        crate::provider_contract::validate_image_input(request, input)?;
        validate_flux_options(request, input)
    }
    fn invoke(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.invoke_observed(
            request,
            input,
            secret,
            vendor_idempotency_key,
            &NoopProviderTransportObserver,
        )
    }
    fn invoke_observed(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        match self.submit_observed(request, input, secret, vendor_idempotency_key, observer)? {
            AdapterSubmission::Complete(outcome) => Ok(outcome),
            AdapterSubmission::Pending(operation) => {
                self.poll_observed(request, input, secret, &operation, observer)
            }
        }
    }
    fn submit(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        self.submit_observed(
            request,
            input,
            secret,
            vendor_idempotency_key,
            &NoopProviderTransportObserver,
        )
    }
    fn submit_observed(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        if vendor_idempotency_key.is_some() != self.config.idempotency_header.is_some() {
            return Err(ProviderFailure::release(
                "idempotency_policy_mismatch",
                ProviderPhase::PreSend,
            ));
        }
        self.submit_once_inner(request, input, secret, vendor_idempotency_key, observer)
    }
    fn poll(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.poll_observed(
            request,
            input,
            secret,
            operation,
            &NoopProviderTransportObserver,
        )
    }
    fn poll_observed(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.poll_existing_inner(request, input, secret, operation, observer)
    }
    fn redact_error(&self, error: &(dyn StdError + 'static)) -> ContractError {
        let _ = Redactor::default().error_chain(error);
        provider_error("provider_failure")
    }
}

pub(crate) fn bind_output_dimensions(input: &mut Value) -> Result<OutputDimensions> {
    let preset = input
        .get("image_size")
        .and_then(Value::as_str)
        .ok_or_else(|| provider_error("invalid_request"))?;
    let expected = contract_dimensions(preset)?;
    let input = input
        .as_object_mut()
        .ok_or_else(|| provider_error("invalid_request"))?;
    let options = input.entry("options").or_insert_with(|| json!({}));
    if options.is_null() {
        *options = json!({});
    }
    let options = options
        .as_object_mut()
        .ok_or_else(|| provider_error("invalid_request"))?;
    let supplied_width = supplied_dimension(options, "width")?;
    let supplied_height = supplied_dimension(options, "height")?;
    match (supplied_width, supplied_height) {
        (None, None) => {}
        (Some(width), Some(height)) => {
            let supplied = OutputDimensions { width, height };
            validate_dimensions(&supplied)?;
            if supplied != expected {
                return Err(provider_error("invalid_request"));
            }
        }
        _ => return Err(provider_error("invalid_request")),
    }
    options.insert("width".into(), json!(expected.width));
    options.insert("height".into(), json!(expected.height));
    Ok(expected)
}

/// Reconstruct the certified dimensions for a pre-HUB-168 frozen request.
///
/// Legacy schema-v2 snapshots do not contain `output_dimensions`, so recovery
/// uses their frozen selector and a cloned persisted input. Any explicit
/// dimensions still have to be complete and exactly match the certified
/// mapping; the caller never rewrites the durable legacy evidence.
pub(crate) fn bind_legacy_output_dimensions(
    frozen_preset: &str,
    input: &mut Value,
) -> Result<OutputDimensions> {
    if input.get("image_size").and_then(Value::as_str) != Some(frozen_preset) {
        return Err(provider_error("invalid_request"));
    }
    bind_output_dimensions(input)
}

fn supplied_dimension(
    options: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<u32>> {
    match options.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| provider_error("invalid_request")),
    }
}

pub(crate) fn contract_dimensions(preset: &str) -> Result<OutputDimensions> {
    let dimensions = match preset {
        "1k" => OutputDimensions {
            width: 1024,
            height: 1024,
        },
        "2k" => OutputDimensions {
            width: 1920,
            height: 1088,
        },
        "4k" => OutputDimensions {
            width: 2048,
            height: 2048,
        },
        _ => return Err(provider_error("invalid_request")),
    };
    validate_dimensions(&dimensions)?;
    Ok(dimensions)
}

pub(crate) fn validate_dimensions(dimensions: &OutputDimensions) -> Result<()> {
    if dimensions.width < MIN_DIMENSION
        || dimensions.height < MIN_DIMENSION
        || !dimensions.width.is_multiple_of(DIMENSION_MULTIPLE)
        || !dimensions.height.is_multiple_of(DIMENSION_MULTIPLE)
        || u64::from(dimensions.width) * u64::from(dimensions.height) > MAX_OUTPUT_PIXELS
    {
        return Err(provider_error("invalid_request"));
    }
    Ok(())
}

fn validate_flux_options(request: &NormalizedRequest, input: &Value) -> Result<()> {
    let Some(options) = input.get("options") else {
        return Err(provider_error("invalid_request"));
    };
    let options = options
        .as_object()
        .ok_or_else(|| provider_error("invalid_request"))?;
    if options.keys().any(|key| {
        !matches!(
            key.as_str(),
            "width" | "height" | "seed" | "safety_tolerance" | "output_format"
        )
    }) {
        return Err(provider_error("invalid_request"));
    }
    let dimensions = OutputDimensions {
        width: supplied_dimension(options, "width")?
            .ok_or_else(|| provider_error("invalid_request"))?,
        height: supplied_dimension(options, "height")?
            .ok_or_else(|| provider_error("invalid_request"))?,
    };
    validate_dimensions(&dimensions)?;
    if request.output_dimensions.as_ref() != Some(&dimensions) {
        return Err(provider_error("invalid_request"));
    }
    if options
        .get("seed")
        .is_some_and(|value| value.as_u64().is_none())
        || options
            .get("safety_tolerance")
            .is_some_and(|value| !matches!(value.as_u64(), Some(0..=5)))
        || options
            .get("output_format")
            .is_some_and(|value| !matches!(value.as_str(), Some("png" | "jpeg")))
    {
        return Err(provider_error("invalid_request"));
    }
    Ok(())
}

fn submit_url(config: &Flux2ApiConfig, model: &str) -> Result<Url> {
    let mut url = Url::parse(&config.endpoint).map_err(|_| provider_error("config_invalid"))?;
    if validate_https_origin(&url, None).is_err()
        || url_has_explicit_port(&config.endpoint)
        || !valid_bfl_api_host(url.host_str())
    {
        return Err(provider_error("config_invalid"));
    }
    url.set_path(&format!(
        "{}/{}",
        config.api_version.trim_matches('/'),
        model
    ));
    Ok(url)
}
fn poll_url(body: &Value) -> Result<Url> {
    let value = string_at(body, &["polling_url", "pollingUrl"])
        .ok_or_else(|| provider_error("malformed_response"))?;
    let url = Url::parse(&value).map_err(|_| provider_error("malformed_response"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url_has_explicit_port(&value)
        || url.fragment().is_some()
        || !valid_bfl_api_host(url.host_str())
    {
        return Err(provider_error("polling_origin_rejected"));
    }
    Ok(url)
}

fn polling_checkpoint_host(body: &Value, operation_id: &str, api_version: &str) -> Result<String> {
    let url = poll_url(body)?;
    let expected_path = format!("/{}/get_result", api_version.trim_matches('/'));
    let query = url.query_pairs().collect::<Vec<_>>();
    if url.path() != expected_path
        || query.len() != 1
        || query[0].0 != "id"
        || query[0].1 != operation_id
    {
        return Err(provider_error("malformed_response"));
    }
    url.host_str()
        .map(str::to_owned)
        .ok_or_else(|| provider_error("polling_origin_rejected"))
}

fn operation_poll_url(config: &Flux2ApiConfig, operation: &AsyncProviderOperation) -> Result<Url> {
    if !valid_bfl_api_host(Some(&operation.polling_host)) {
        return Err(provider_error("polling_origin_rejected"));
    }
    let mut url = Url::parse(&format!("https://{}/", operation.polling_host))
        .map_err(|_| provider_error("invalid_provider_operation"))?;
    url.set_path(&format!(
        "{}/get_result",
        config.api_version.trim_matches('/')
    ));
    url.query_pairs_mut()
        .append_pair("id", &operation.provider_operation_id);
    Ok(url)
}

fn string_at(body: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| body.get(*name)?.as_str().map(str::to_owned))
}
fn safe_string_at(body: &Value, names: &[&str], secret: &ProviderSecret) -> Option<String> {
    safe_evidence(string_at(body, names), secret)
}
fn safe_evidence(value: Option<String>, secret: &ProviderSecret) -> Option<String> {
    value.filter(|value| {
        !value.is_empty()
            && value.len() <= 255
            && value.trim() == value
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
            && (secret.expose().is_empty()
                || !value
                    .as_bytes()
                    .windows(secret.expose().len())
                    .any(|window| window == secret.expose()))
    })
}
fn validate_poll_operation_identity(
    body: &Value,
    expected_operation_id: &str,
    request_id: Option<String>,
    secret: &ProviderSecret,
) -> Result<(), ProviderFailure> {
    for name in ["id", "operation_id", "operationId"] {
        let Some(value) = body.get(name) else {
            continue;
        };
        let returned_operation_id = value
            .as_str()
            .map(str::to_owned)
            .and_then(|value| safe_evidence(Some(value), secret));
        if returned_operation_id.as_deref() != Some(expected_operation_id) {
            return Err(with_evidence(
                "provider_operation_identity_mismatch",
                request_id,
                Some(expected_operation_id.to_owned()),
            ));
        }
    }
    Ok(())
}
fn status(body: &Value) -> Option<String> {
    body.get("status")
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_lowercase())
}
fn artifact_url(body: &Value) -> Option<&str> {
    body.pointer("/result/sample")
        .or_else(|| body.pointer("/result/image/url"))
        .or_else(|| body.get("sample"))
        .and_then(Value::as_str)
}

/// Normalize BFL's documented top-level settled `cost` from credits into the
/// HUB-33 exact USD representation. The coefficient and provider decimal
/// precision remain intact; the fixed credit-to-USD conversion is one scale
/// offset, not a floating-point multiplication.
fn settled_cost(body: &Value) -> Result<Option<ActualVendorCost>> {
    match body.get("cost") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => bfl_credit_cost_to_usd(number.as_str()).map(Some),
        Some(_) => Err(provider_error("invalid_provider_cost")),
    }
}

pub(crate) fn bfl_credit_cost_to_usd(raw: &str) -> Result<ActualVendorCost> {
    ActualVendorCost::from_decimal_scaled_units(raw, "USD", BFL_CREDIT_USD_SCALE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BflResultState {
    Pending,
    Ready,
    Rejected(&'static str),
    Ambiguous(&'static str),
}

impl BflResultState {
    fn failure(
        self,
        request_id: Option<String>,
        operation_id: Option<String>,
    ) -> Option<ProviderFailure> {
        match self {
            Self::Pending | Self::Ready => None,
            Self::Rejected(code) => Some(
                ProviderFailure::release(code, ProviderPhase::Processing)
                    .with_evidence(request_id, operation_id),
            ),
            Self::Ambiguous(code) => Some(
                ProviderFailure::reconcile(code, ProviderPhase::Processing)
                    .with_evidence(request_id, operation_id),
            ),
        }
    }
}

fn classify_result_state(body: &Value, allow_async_submission: bool) -> Result<BflResultState> {
    let state = match status(body).as_deref() {
        Some("pending" | "reasoning" | "generating") => BflResultState::Pending,
        Some("ready" | "succeeded" | "completed") => BflResultState::Ready,
        Some("request moderated") => BflResultState::Rejected("provider_request_moderated"),
        Some("content moderated") => BflResultState::Rejected("provider_content_moderated"),
        Some("error" | "failed") => BflResultState::Rejected("provider_error"),
        Some("rejected") => BflResultState::Rejected("provider_rejected"),
        Some("task not found") => BflResultState::Ambiguous("provider_task_not_found"),
        Some(_) => return Err(provider_error("malformed_response")),
        None if artifact_url(body).is_some() => BflResultState::Ready,
        // The documented asynchronous submission response has no status. Its
        // required operation id and polling URL are validated by the caller.
        None if allow_async_submission && body.get("id").is_some() => BflResultState::Pending,
        None => return Err(provider_error("malformed_response")),
    };
    Ok(state)
}
fn provider_error(code: &str) -> ContractError {
    ContractError::Provider { code: code.into() }
}
fn with_evidence(
    code: &str,
    request_id: Option<String>,
    operation_id: Option<String>,
) -> ProviderFailure {
    ProviderFailure::reconcile(code, ProviderPhase::Processing)
        .with_evidence(request_id, operation_id)
}
fn reconcile_with_evidence(
    code: &str,
    phase: ProviderPhase,
    request_id: Option<String>,
    operation_id: Option<String>,
) -> ProviderFailure {
    ProviderFailure::reconcile(code, phase).with_evidence(request_id, operation_id)
}
fn artifact_failure(request_id: Option<String>, operation_id: Option<String>) -> ProviderFailure {
    reconcile_with_evidence(
        "artifact_policy_failure",
        ProviderPhase::Artifact,
        request_id,
        operation_id,
    )
}
fn classify_http_status(
    status: u16,
    phase: ProviderPhase,
    accepted_operation: bool,
    request_id: Option<String>,
    operation_id: Option<String>,
) -> ProviderFailure {
    let code = match status {
        401 => "provider_authentication_failed",
        402 => "provider_insufficient_credit",
        403 => "provider_permission_denied",
        429 => "provider_rate_limited",
        _ => "provider_rejected",
    };
    let failure = if accepted_operation {
        ProviderFailure::reconcile(code, phase)
    } else {
        ProviderFailure::release(code, phase)
    };
    failure.with_evidence(request_id, operation_id)
}
fn classify_transport(
    error: Box<dyn StdError + Send + Sync>,
    secret: &ProviderSecret,
    submission: bool,
    request_id: Option<String>,
    operation_id: Option<String>,
) -> ProviderFailure {
    let _ = Redactor::new([secret.expose()]).error_chain(error.as_ref());
    match error.downcast_ref::<HttpFailure>() {
        Some(HttpFailure::BeforeSend) if submission => {
            ProviderFailure::release("provider_pre_send_failure", ProviderPhase::PreSend)
        }
        _ if submission => with_evidence("timeout_unknown_outcome", request_id, operation_id),
        _ => with_evidence("timeout_unknown_outcome", request_id, operation_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::conformance::{
            assert_adapter_conformance, assert_body_and_artifact_bounds, assert_redirect_blocked,
            Case, Observation,
        },
        provider_contract::SpendDisposition,
        secrets::secret_for_test,
    };
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use std::{
        io::Cursor,
        sync::{Arc, Mutex},
    };

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
            .unwrap();
        bytes
    }

    #[derive(Clone)]
    struct Fixture {
        submit: Arc<Mutex<u32>>,
        polls: Arc<Mutex<Vec<Value>>>,
        artifact: Vec<u8>,
        fail_submit: Option<String>,
        fail_poll: Option<String>,
        submit_body: Option<Value>,
        expected_options: Option<Value>,
    }
    impl Flux2Transport for Fixture {
        fn submit(
            &self,
            _: &Url,
            credential: &[u8],
            _: Duration,
            _: &BTreeMap<String, String>,
            idempotency: Option<(&str, &str)>,
            body: &Value,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            assert_eq!(credential, b"secret-canary");
            assert_eq!(idempotency, Some(("x-idempotency-key", "opaque-key")));
            assert_eq!(body["prompt"], "cat");
            assert!(body.get("image_size").is_none());
            if let Some(expected) = &self.expected_options {
                for field in [
                    "width",
                    "height",
                    "seed",
                    "safety_tolerance",
                    "output_format",
                ] {
                    assert_eq!(body.get(field), expected.get(field));
                }
            }
            *self.submit.lock().unwrap() += 1;
            if self.fail_submit.is_some() {
                return Err(Box::new(HttpFailure::UnknownOutcome));
            }
            Ok(TransportResponse {
                status: 202,
                request_id: Some("req-1".into()),
                body: self.submit_body.clone().unwrap_or_else(|| json!({"id":"op-1","polling_url":"https://api.bfl.ai/v1/get_result?id=op-1","status":"Pending"})),
            })
        }
        fn poll(
            &self,
            url: &Url,
            credential: &[u8],
            _: Duration,
            _: &BTreeMap<String, String>,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            assert!(valid_bfl_api_host(url.host_str()));
            assert_eq!(credential, b"secret-canary");
            if self.fail_poll.is_some() {
                return Err(Box::new(HttpFailure::UnknownOutcome));
            }
            let body = self.polls.lock().unwrap().remove(0);
            Ok(TransportResponse {
                status: body
                    .get("_http_status")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(200),
                request_id: Some("poll-1".into()),
                body,
            })
        }
        fn fetch_artifact(
            &self,
            url: &Url,
            _: Duration,
            _: usize,
        ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
            assert!(url.host_str().is_some_and(valid_bfl_delivery_host));
            Ok(self.artifact.clone())
        }
    }

    type CredentialedPollTraffic = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    #[derive(Clone, Default)]
    struct Traffic {
        submissions: Arc<Mutex<Vec<String>>>,
        polls: CredentialedPollTraffic,
        artifacts: Arc<Mutex<Vec<String>>>,
    }

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<ProviderTransportInteraction>>);

    impl ProviderTransportObserver for RecordingObserver {
        fn record(&self, interaction: ProviderTransportInteraction) -> bool {
            self.0.lock().unwrap().push(interaction);
            true
        }
    }

    #[derive(Clone)]
    struct SecurityFixture {
        traffic: Traffic,
        submit_response: TransportResponse,
        poll_responses: Arc<Mutex<Vec<TransportResponse>>>,
        artifact: Vec<u8>,
        poll_delay_ms: u64,
    }

    impl Flux2Transport for SecurityFixture {
        fn submit(
            &self,
            url: &Url,
            credential: &[u8],
            _: Duration,
            _: &BTreeMap<String, String>,
            _: Option<(&str, &str)>,
            _: &Value,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            assert_eq!(credential, b"secret-canary");
            self.traffic
                .submissions
                .lock()
                .unwrap()
                .push(url.to_string());
            Ok(self.submit_response.clone())
        }

        fn poll(
            &self,
            url: &Url,
            credential: &[u8],
            _: Duration,
            _: &BTreeMap<String, String>,
        ) -> std::result::Result<TransportResponse, Box<dyn StdError + Send + Sync>> {
            self.traffic
                .polls
                .lock()
                .unwrap()
                .push((url.to_string(), credential.to_vec()));
            thread::sleep(Duration::from_millis(self.poll_delay_ms));
            Ok(self.poll_responses.lock().unwrap().remove(0))
        }

        fn fetch_artifact(
            &self,
            url: &Url,
            _: Duration,
            _: usize,
        ) -> std::result::Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
            self.traffic.artifacts.lock().unwrap().push(url.to_string());
            Ok(self.artifact.clone())
        }
    }
    fn config() -> Flux2ApiConfig {
        Flux2ApiConfig {
            endpoint: "https://api.bfl.ai".into(),
            api_version: "v1".into(),
            timeout_ms: 1000,
            poll_interval_ms: 1,
            max_retries: 0,
            idempotency_header: Some("x-idempotency-key".into()),
            approved_artifact_hosts: vec!["delivery.us.bfl.ai".into()],
            headers: BTreeMap::new(),
        }
    }
    fn security_fixture(
        submit_status: u16,
        submit_body: Value,
        polls: Vec<(u16, Value)>,
    ) -> (Flux2ApiAdapter<SecurityFixture>, Traffic) {
        let traffic = Traffic::default();
        let transport = SecurityFixture {
            traffic: traffic.clone(),
            submit_response: TransportResponse {
                status: submit_status,
                request_id: Some("req-1".into()),
                body: submit_body,
            },
            poll_responses: Arc::new(Mutex::new(
                polls
                    .into_iter()
                    .map(|(status, body)| TransportResponse {
                        status,
                        request_id: Some("poll-1".into()),
                        body,
                    })
                    .collect(),
            )),
            artifact: png(),
            poll_delay_ms: 0,
        };
        (
            Flux2ApiAdapter::new(config(), MODEL_ID.into(), transport).unwrap(),
            traffic,
        )
    }
    fn request() -> NormalizedRequest {
        request_for("1k")
    }
    fn request_for(preset: &str) -> NormalizedRequest {
        NormalizedRequest {
            provider: PROVIDER_ID.into(),
            model: MODEL_ID.into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: Some(preset.into()),
            output_dimensions: Some(contract_dimensions(preset).unwrap()),
        }
    }
    fn input() -> Value {
        input_for("1k")
    }
    fn input_for(preset: &str) -> Value {
        let dimensions = contract_dimensions(preset).unwrap();
        json!({
            "prompt":"cat",
            "image_size":preset,
            "options":{"width":dimensions.width,"height":dimensions.height}
        })
    }
    fn fixture(polls: Vec<Value>) -> (Flux2ApiAdapter<Fixture>, Arc<Mutex<u32>>) {
        let submit = Arc::new(Mutex::new(0));
        (
            Flux2ApiAdapter::new(
                config(),
                "flux-2-pro".into(),
                Fixture {
                    submit: submit.clone(),
                    polls: Arc::new(Mutex::new(polls)),
                    artifact: png(),
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: None,
                    expected_options: None,
                },
            )
            .unwrap(),
            submit,
        )
    }
    fn code(error: ProviderFailure) -> String {
        error.code
    }

    fn invoke_ready_json(raw: &str) -> std::result::Result<AdapterOutcome, ProviderFailure> {
        let ready = serde_json::from_str(raw).unwrap();
        let (adapter, _) = fixture(vec![ready]);
        adapter.invoke(
            &request(),
            &input(),
            &secret_for_test("secret-canary"),
            Some("opaque-key"),
        )
    }

    #[test]
    fn cross_adapter_conformance_matrix() {
        assert_body_and_artifact_bounds(
            |reader, limit| read_provider_response_bounded(reader, limit).is_err(),
            |reader, limit| read_artifact_response_bounded(reader, limit).is_err(),
        );
        assert_adapter_conformance(|case| {
            let (adapter, calls) = match case {
                Case::Rejection => fixture(vec![json!({"id":"op-1","status":"Rejected"})]),
                Case::AmbiguousPostSend => {
                    let calls = Arc::new(Mutex::new(0));
                    (
                        Flux2ApiAdapter::new(
                            config(),
                            "flux-2-pro".into(),
                            Fixture {
                                submit: calls.clone(),
                                polls: Arc::new(Mutex::new(vec![])),
                                artifact: Vec::new(),
                                fail_submit: Some("secret-canary".into()),
                                fail_poll: None,
                                submit_body: None,
                                expected_options: None,
                            },
                        )
                        .unwrap(),
                        calls,
                    )
                }
                Case::EvidenceRetention => {
                    let (mut adapter, calls) = fixture(vec![
                        json!({"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
                    ]);
                    adapter.transport.artifact = vec![0];
                    (adapter, calls)
                }
                Case::HostPolicy => fixture(vec![
                    json!({"id":"op-1","status":"Ready","result":{"sample":"https://evil.example/out.png"}}),
                ]),
                Case::ArtifactBound => {
                    let (mut adapter, calls) = fixture(vec![
                        json!({"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
                    ]);
                    adapter.transport.artifact = vec![0; MAX_ARTIFACT_BYTES + 1];
                    (adapter, calls)
                }
                Case::UnsafeRetry => fixture(Vec::new()),
                Case::InvalidRequest => fixture(Vec::new()),
            };
            let mut conformance_request = request();
            if matches!(case, Case::InvalidRequest) {
                conformance_request.image_count = Some(2);
            }
            let result = adapter.invoke(
                &conformance_request,
                &input(),
                &secret_for_test("secret-canary"),
                (!matches!(case, Case::UnsafeRetry)).then_some("opaque-key"),
            );
            let submissions = *calls.lock().unwrap();
            Observation {
                result,
                submissions,
            }
        });
    }

    #[test]
    fn conformance_artifact_redirect_is_blocked() {
        assert_redirect_blocked(|url| {
            ReqwestFlux2Transport
                .fetch_artifact(url, Duration::from_secs(2), MAX_ARTIFACT_BYTES)
                .is_err()
        });
    }

    #[test]
    fn credentialed_poll_redirect_is_blocked() {
        assert_redirect_blocked(|url| {
            ReqwestFlux2Transport
                .poll(
                    url,
                    b"credential-canary",
                    Duration::from_secs(2),
                    &BTreeMap::new(),
                )
                .is_err()
        });
    }

    #[test]
    fn provider_returned_polling_urls_use_only_explicit_bfl_api_origins() {
        for host in ["api.bfl.ai", "api.eu.bfl.ai", "api.us.bfl.ai"] {
            let polling_url = format!("https://{host}/v1/get_result?id=op-1");
            let signed_artifact =
                "https://delivery.us.bfl.ai/out.png?signature=artifact-url-canary";
            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":polling_url.clone()}),
                vec![(
                    200,
                    json!({"id":"op-1","status":"Ready","result":{"sample":signed_artifact}}),
                )],
            );
            let outcome = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap();
            assert_eq!(
                traffic.polls.lock().unwrap().as_slice(),
                &[(polling_url, b"secret-canary".to_vec())]
            );
            assert_eq!(
                traffic.artifacts.lock().unwrap().as_slice(),
                &[signed_artifact.to_owned()]
            );
            let persisted_shape = serde_json::to_string(&outcome).unwrap();
            assert!(!persisted_shape.contains("artifact-url-canary"));
            assert!(!persisted_shape.contains("delivery.us.bfl.ai"));
        }
    }

    #[test]
    fn unsafe_polling_urls_fail_before_forwarding_x_key() {
        for polling_url in [
            "http://api.bfl.ai/v1/get_result?id=op-1",
            "https://user@api.bfl.ai/v1/get_result?id=op-1",
            "https://user:password@api.bfl.ai/v1/get_result?id=op-1",
            "https://api.bfl.ai:443/v1/get_result?id=op-1",
            "HTTPS://api.bfl.ai:443/v1/get_result?id=op-1",
            "https:/\\api.bfl.ai:443/v1/get_result?id=op-1",
            "https:////api.bfl.ai:443/v1/get_result?id=op-1",
            "https://api.bfl.ai:8443/v1/get_result?id=op-1",
            "https://api.bfl.ai/v1/get_result?id=op-1#fragment",
            "https://api.bfl.ai.evil.example/v1/get_result?id=op-1",
            "https://evil.api.bfl.ai/v1/get_result?id=op-1",
            "https://api.bfl.ai./v1/get_result?id=op-1",
            "https://delivery.us.bfl.ai/v1/get_result?id=op-1",
        ] {
            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":polling_url}),
                Vec::new(),
            );
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, "polling_origin_rejected", "{polling_url}");
            assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
            assert_eq!(error.evidence.request_id.as_deref(), Some("req-1"));
            assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
            assert!(traffic.polls.lock().unwrap().is_empty(), "{polling_url}");
            assert!(
                traffic.artifacts.lock().unwrap().is_empty(),
                "{polling_url}"
            );
        }
    }

    #[test]
    fn missing_or_non_string_polling_url_is_malformed_without_polling() {
        for body in [
            json!({"id":"op-1"}),
            json!({"id":"op-1","polling_url":null}),
            json!({"id":"op-1","polling_url":42}),
        ] {
            let (adapter, traffic) = security_fixture(202, body, Vec::new());
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, "malformed_response");
            assert_eq!(error.phase, ProviderPhase::Processing);
            assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
            assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
            assert!(traffic.polls.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn result_states_are_terminal_or_pollable_without_timeout_fallthrough() {
        for (status, expected_code, disposition) in [
            (
                "Request Moderated",
                "provider_request_moderated",
                SpendDisposition::Release,
            ),
            (
                "Content Moderated",
                "provider_content_moderated",
                SpendDisposition::Release,
            ),
            ("Error", "provider_error", SpendDisposition::Release),
            (
                "Task not found",
                "provider_task_not_found",
                SpendDisposition::Reconcile,
            ),
        ] {
            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":"https://api.bfl.ai/v1/get_result?id=op-1"}),
                vec![
                    (
                        200,
                        json!({"id":"op-1","status":status,"details":{"raw":"secret-canary body-canary"}}),
                    ),
                    (
                        200,
                        json!({"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
                    ),
                ],
            );
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, expected_code, "{status}");
            assert_eq!(error.spend_disposition, disposition, "{status}");
            assert_eq!(traffic.polls.lock().unwrap().len(), 1, "{status}");
            assert!(traffic.artifacts.lock().unwrap().is_empty(), "{status}");
            let redacted = serde_json::to_string(&error).unwrap();
            assert!(!redacted.contains("secret-canary"), "{status}");
            assert!(!redacted.contains("body-canary"), "{status}");
        }

        for body in [
            json!({"id":"op-1","status":"Undocumented"}),
            json!({"id":"op-1","details":{"raw":"body-canary"}}),
        ] {
            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":"https://api.bfl.ai/v1/get_result?id=op-1"}),
                vec![
                    (200, body),
                    (
                        200,
                        json!({"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
                    ),
                ],
            );
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, "malformed_response");
            assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
            assert_eq!(traffic.polls.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn documented_polling_states_continue_to_ready() {
        let (adapter, traffic) = security_fixture(
            202,
            json!({"id":"op-1","polling_url":"https://api.us.bfl.ai/v1/get_result?id=op-1"}),
            vec![
                (200, json!({"id":"op-1","status":"Pending"})),
                (200, json!({"id":"op-1","status":"Reasoning"})),
                (200, json!({"id":"op-1","status":"Generating"})),
                (
                    200,
                    json!({"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
                ),
            ],
        );
        adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        assert_eq!(traffic.polls.lock().unwrap().len(), 4);
        assert_eq!(traffic.artifacts.lock().unwrap().len(), 1);
    }

    #[test]
    fn authentication_credit_and_rate_limit_http_outcomes_are_redacted() {
        for (status, expected_code) in [
            (401, "provider_authentication_failed"),
            (402, "provider_insufficient_credit"),
            (403, "provider_permission_denied"),
            (429, "provider_rate_limited"),
        ] {
            let (adapter, traffic) = security_fixture(
                status,
                json!({"detail":"secret-canary raw-body-canary"}),
                Vec::new(),
            );
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, expected_code, "HTTP {status}");
            assert_eq!(error.phase, ProviderPhase::Submission, "HTTP {status}");
            assert_eq!(error.spend_disposition, SpendDisposition::Release);
            assert_eq!(traffic.submissions.lock().unwrap().len(), 1);
            assert!(traffic.polls.lock().unwrap().is_empty());
            let redacted = serde_json::to_string(&error).unwrap();
            assert!(!redacted.contains("secret-canary"));
            assert!(!redacted.contains("raw-body-canary"));

            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":"https://api.bfl.ai/v1/get_result?id=op-1"}),
                vec![(status, json!({"detail":"secret-canary raw-body-canary"}))],
            );
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, expected_code, "poll HTTP {status}");
            assert_eq!(error.phase, ProviderPhase::Processing, "HTTP {status}");
            assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
            assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
            assert_eq!(traffic.polls.lock().unwrap().len(), 1);
            let redacted = serde_json::to_string(&error).unwrap();
            assert!(!redacted.contains("secret-canary"));
            assert!(!redacted.contains("raw-body-canary"));
        }
    }

    #[test]
    fn bfl_delivery_host_policy_is_exactly_one_safe_region_label() {
        for host in [
            "delivery.us.bfl.ai",
            "delivery.eu-1.bfl.ai",
            "delivery.us1.bfl.ai",
        ] {
            assert!(valid_bfl_delivery_host(host), "{host}");
        }
        for host in [
            "delivery.bfl.ai",
            "delivery..bfl.ai",
            "delivery.-us.bfl.ai",
            "delivery.us-.bfl.ai",
            "delivery.us.east.bfl.ai",
            "delivery.us.bfl.ai.evil.example",
            "evil.delivery.us.bfl.ai",
            "delivery-us.bfl.ai",
            "api.us.bfl.ai",
        ] {
            assert!(!valid_bfl_delivery_host(host), "{host}");
        }
    }

    #[test]
    fn unsafe_artifact_origins_never_reach_credential_free_fetch() {
        for artifact_url in [
            "http://delivery.us.bfl.ai/out.png?signature=x",
            "https://user@delivery.us.bfl.ai/out.png?signature=x",
            "https://delivery.us.bfl.ai:443/out.png?signature=x",
            "HTTPS://delivery.us.bfl.ai:443/out.png?signature=x",
            "https:\\/delivery.us.bfl.ai:443/out.png?signature=x",
            "https://delivery.us.bfl.ai:8443/out.png?signature=x",
            "https://delivery.us.bfl.ai/out.png?signature=x#fragment",
            "https://delivery.bfl.ai/out.png?signature=x",
            "https://delivery.us.east.bfl.ai/out.png?signature=x",
            "https://delivery.us.bfl.ai.evil.example/out.png?signature=x",
            "https://evil.delivery.us.bfl.ai/out.png?signature=x",
            "https://api.bfl.ai/out.png?signature=x",
        ] {
            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":"https://api.bfl.ai/v1/get_result?id=op-1"}),
                vec![(
                    200,
                    json!({"id":"op-1","status":"Ready","result":{"sample":artifact_url}}),
                )],
            );
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, "artifact_policy_failure", "{artifact_url}");
            assert_eq!(error.phase, ProviderPhase::Artifact, "{artifact_url}");
            assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
            assert_eq!(traffic.polls.lock().unwrap().len(), 1);
            assert!(
                traffic.artifacts.lock().unwrap().is_empty(),
                "{artifact_url}"
            );
            let redacted = serde_json::to_string(&error).unwrap();
            assert!(!redacted.contains("signature=x"));
            assert!(!redacted.contains("secret-canary"));
        }
    }

    #[test]
    fn only_compact_non_secret_provider_evidence_is_retained() {
        let secret = secret_for_test("secret-canary");
        assert_eq!(
            safe_evidence(Some("request_123:part-2".into()), &secret).as_deref(),
            Some("request_123:part-2")
        );
        for unsafe_value in [
            "secret-canary",
            "prefix-secret-canary-suffix",
            "https://api.bfl.ai/v1/get_result?id=secret-canary",
            " request-1",
            "request/1",
            "request?token=x",
        ] {
            assert_eq!(safe_evidence(Some(unsafe_value.into()), &secret), None);
        }
        assert_eq!(safe_evidence(Some("a".repeat(256)), &secret), None);
    }

    #[test]
    fn staged_submit_returns_only_safe_resumable_operation_evidence() {
        let (adapter, traffic) = security_fixture(
            202,
            json!({
                "id":"op-1",
                "request_id":"body-request-id",
                "polling_url":"https://api.us.bfl.ai/v1/get_result?id=op-1",
                "status":"Pending",
                "debug":{
                    "raw":"raw-body-canary secret-canary",
                    "signed_url":"https://delivery.us.bfl.ai/out.png?signature=signed-url-canary"
                }
            }),
            Vec::new(),
        );
        let now_unix_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();

        let submission = adapter
            .submit(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        let AdapterSubmission::Pending(operation) = submission else {
            panic!("asynchronous submit must return a resumable operation");
        };
        let after_submit_unix_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();

        assert_eq!(operation.provider_request_id.as_deref(), Some("req-1"));
        assert_eq!(operation.provider_operation_id, "op-1");
        assert_eq!(operation.polling_host, "api.us.bfl.ai");
        assert!(operation.deadline_unix_ms > now_unix_ms);
        assert!(operation.deadline_unix_ms <= after_submit_unix_ms + 1_000);
        assert_eq!(traffic.submissions.lock().unwrap().len(), 1);
        assert!(traffic.polls.lock().unwrap().is_empty());

        let durable = serde_json::to_string(&operation).unwrap();
        for forbidden in [
            "secret-canary",
            "raw-body-canary",
            "signed-url-canary",
            "polling_url",
            "delivery.us.bfl.ai",
            "https://",
        ] {
            assert!(!durable.contains(forbidden), "leaked {forbidden}");
        }
    }

    #[test]
    fn staged_resume_polls_and_fetches_without_a_second_submission() {
        let (adapter, traffic) = security_fixture(
            202,
            json!({
                "id":"op-1",
                "polling_url":"https://api.us.bfl.ai/v1/get_result?id=op-1",
                "status":"Pending"
            }),
            vec![(
                200,
                json!({
                    "id":"op-1",
                    "status":"Ready",
                    "result":{"sample":"https://delivery.us.bfl.ai/out.png?signature=short-lived"}
                }),
            )],
        );
        let secret = secret_for_test("secret-canary");
        let AdapterSubmission::Pending(operation) = adapter
            .submit(&request(), &input(), &secret, Some("opaque-key"))
            .unwrap()
        else {
            panic!("expected an asynchronous operation");
        };
        let persisted: AsyncProviderOperation =
            serde_json::from_slice(&serde_json::to_vec(&operation).unwrap()).unwrap();
        let submissions_before_resume = traffic.submissions.lock().unwrap().len();

        let outcome = adapter
            .poll(&request(), &input(), &secret, &persisted)
            .unwrap();

        assert_eq!(submissions_before_resume, 1);
        assert_eq!(traffic.submissions.lock().unwrap().len(), 1);
        assert_eq!(traffic.polls.lock().unwrap().len(), 1);
        assert_eq!(traffic.artifacts.lock().unwrap().len(), 1);
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("op-1"));
    }

    #[test]
    fn observed_resume_records_each_transport_boundary_before_the_call() {
        let (adapter, traffic) = security_fixture(
            202,
            json!({
                "id":"op-1",
                "polling_url":"https://api.us.bfl.ai/v1/get_result?id=op-1",
                "status":"Pending"
            }),
            vec![
                (200, json!({"id":"op-1","status":"Pending"})),
                (
                    200,
                    json!({
                        "id":"op-1",
                        "status":"Ready",
                        "result":{"sample":"https://delivery.us.bfl.ai/out.png?signature=short-lived"}
                    }),
                ),
            ],
        );
        let secret = secret_for_test("secret-canary");
        let observer = RecordingObserver::default();
        let AdapterSubmission::Pending(operation) = adapter
            .submit_observed(&request(), &input(), &secret, Some("opaque-key"), &observer)
            .unwrap()
        else {
            panic!("expected an asynchronous operation");
        };

        adapter
            .poll_observed(&request(), &input(), &secret, &operation, &observer)
            .unwrap();

        assert_eq!(traffic.submissions.lock().unwrap().len(), 1);
        assert_eq!(traffic.polls.lock().unwrap().len(), 2);
        assert_eq!(traffic.artifacts.lock().unwrap().len(), 1);
        assert_eq!(
            observer.0.lock().unwrap().as_slice(),
            &[
                ProviderTransportInteraction::Poll,
                ProviderTransportInteraction::Poll,
                ProviderTransportInteraction::ArtifactFetch,
            ]
        );
    }

    #[test]
    fn rejected_transport_observation_prevents_the_provider_call() {
        struct Reject;
        impl ProviderTransportObserver for Reject {
            fn record(&self, _interaction: ProviderTransportInteraction) -> bool {
                false
            }
        }

        let (adapter, traffic) = security_fixture(
            202,
            json!({
                "id":"op-1",
                "polling_url":"https://api.us.bfl.ai/v1/get_result?id=op-1",
                "status":"Pending"
            }),
            vec![(200, json!({"id":"op-1","status":"Pending"}))],
        );
        let secret = secret_for_test("secret-canary");
        let AdapterSubmission::Pending(operation) = adapter
            .submit(&request(), &input(), &secret, Some("opaque-key"))
            .unwrap()
        else {
            panic!("expected an asynchronous operation");
        };

        let error = adapter
            .poll_observed(&request(), &input(), &secret, &operation, &Reject)
            .unwrap_err();

        assert_eq!(error.code, "provider_transport_evidence_unavailable");
        assert!(traffic.polls.lock().unwrap().is_empty());
        assert!(traffic.artifacts.lock().unwrap().is_empty());
    }

    #[test]
    fn resumed_poll_rejects_a_mismatched_operation_before_artifact_fetch() {
        let (adapter, traffic) = security_fixture(
            202,
            json!({
                "id":"op-1",
                "polling_url":"https://api.bfl.ai/v1/get_result?id=op-1",
                "status":"Pending"
            }),
            vec![(
                200,
                json!({
                    "id":"different-operation",
                    "status":"Ready",
                    "result":{"sample":"https://delivery.us.bfl.ai/out.png"}
                }),
            )],
        );
        let secret = secret_for_test("secret-canary");
        let AdapterSubmission::Pending(operation) = adapter
            .submit(&request(), &input(), &secret, Some("opaque-key"))
            .unwrap()
        else {
            panic!("expected an asynchronous operation");
        };

        let error = adapter
            .poll(&request(), &input(), &secret, &operation)
            .unwrap_err();

        assert_eq!(error.code, "provider_operation_identity_mismatch");
        assert_eq!(error.phase, ProviderPhase::Processing);
        assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
        assert_eq!(error.evidence.request_id.as_deref(), Some("req-1"));
        assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
        assert_eq!(traffic.submissions.lock().unwrap().len(), 1);
        assert_eq!(traffic.polls.lock().unwrap().len(), 1);
        assert!(traffic.artifacts.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_or_mismatched_polling_routes_reconcile_without_polling() {
        for polling_url in [
            "https://api.bfl.ai/v1/jobs?id=op-1",
            "https://api.bfl.ai/v1/get_result",
            "https://api.bfl.ai/v1/get_result?id=other-op",
            "https://api.bfl.ai/v1/get_result?id=op-1&token=unsafe",
            "https://api.bfl.ai/v1/get_result?id=op-1&id=op-1",
        ] {
            let (adapter, traffic) = security_fixture(
                202,
                json!({"id":"op-1","polling_url":polling_url,"status":"Pending"}),
                Vec::new(),
            );
            let error = adapter
                .submit(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();

            assert_eq!(error.code, "malformed_response", "{polling_url}");
            assert_eq!(error.phase, ProviderPhase::Processing, "{polling_url}");
            assert_eq!(
                error.spend_disposition,
                SpendDisposition::Reconcile,
                "{polling_url}"
            );
            assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
            assert!(traffic.polls.lock().unwrap().is_empty(), "{polling_url}");
            assert!(
                traffic.artifacts.lock().unwrap().is_empty(),
                "{polling_url}"
            );
        }
    }

    #[test]
    fn expired_resumed_operation_times_out_without_polling_or_fetching() {
        let (adapter, traffic) =
            security_fixture(202, json!({"id":"unused","status":"Pending"}), Vec::new());
        let expired_unix_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            - 1;
        let operation = AsyncProviderOperation {
            provider_request_id: Some("req-1".into()),
            provider_operation_id: "op-1".into(),
            polling_host: "api.us.bfl.ai".into(),
            deadline_unix_ms: expired_unix_ms,
        };

        let error = adapter
            .poll(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                &operation,
            )
            .unwrap_err();

        assert_eq!(error.code, "timeout_unknown_outcome");
        assert_eq!(error.phase, ProviderPhase::Processing);
        assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
        assert!(traffic.submissions.lock().unwrap().is_empty());
        assert!(traffic.polls.lock().unwrap().is_empty());
        assert!(traffic.artifacts.lock().unwrap().is_empty());
    }

    #[test]
    fn poll_deadline_expires_before_transport_evidence_is_recorded() {
        let (mut adapter, traffic) =
            security_fixture(202, json!({"id":"unused","status":"Pending"}), Vec::new());
        adapter.config.poll_interval_ms = 50;
        let deadline_unix_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 10;
        let operation = AsyncProviderOperation {
            provider_request_id: Some("req-1".into()),
            provider_operation_id: "op-1".into(),
            polling_host: "api.us.bfl.ai".into(),
            deadline_unix_ms,
        };
        let observer = RecordingObserver::default();

        let error = adapter
            .poll_observed(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                &operation,
                &observer,
            )
            .unwrap_err();

        assert_eq!(error.code, "timeout_unknown_outcome");
        assert!(traffic.polls.lock().unwrap().is_empty());
        assert!(traffic.artifacts.lock().unwrap().is_empty());
        assert!(observer.0.lock().unwrap().is_empty());
    }

    #[test]
    fn artifact_deadline_expires_before_fetch_evidence_is_recorded() {
        let (mut adapter, traffic) = security_fixture(
            202,
            json!({"id":"unused","status":"Pending"}),
            vec![(
                200,
                json!({
                    "id":"op-1",
                    "status":"Ready",
                    "result":{"sample":"https://delivery.us.bfl.ai/out.png"}
                }),
            )],
        );
        adapter.transport.poll_delay_ms = 30;
        let deadline_unix_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 15;
        let operation = AsyncProviderOperation {
            provider_request_id: Some("req-1".into()),
            provider_operation_id: "op-1".into(),
            polling_host: "api.us.bfl.ai".into(),
            deadline_unix_ms,
        };
        let observer = RecordingObserver::default();

        let error = adapter
            .poll_observed(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                &operation,
                &observer,
            )
            .unwrap_err();

        assert_eq!(error.code, "artifact_policy_failure");
        assert_eq!(traffic.polls.lock().unwrap().len(), 1);
        assert!(traffic.artifacts.lock().unwrap().is_empty());
        assert_eq!(
            observer.0.lock().unwrap().as_slice(),
            &[ProviderTransportInteraction::Poll]
        );
    }

    #[test]
    fn direct_invoke_remains_synchronous_with_exactly_one_submission() {
        let (adapter, traffic) = security_fixture(
            202,
            json!({
                "id":"op-1",
                "polling_url":"https://api.bfl.ai/v1/get_result?id=op-1",
                "status":"Pending"
            }),
            vec![(
                200,
                json!({
                    "id":"op-1",
                    "status":"Ready",
                    "result":{"sample":"https://delivery.us.bfl.ai/out.png"}
                }),
            )],
        );

        let outcome = adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();

        assert_eq!(traffic.submissions.lock().unwrap().len(), 1);
        assert_eq!(traffic.polls.lock().unwrap().len(), 1);
        assert_eq!(traffic.artifacts.lock().unwrap().len(), 1);
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("op-1"));
    }

    #[test]
    fn submits_once_polls_same_operation_and_normalizes_missing_cost() {
        let (adapter, submits) = fixture(vec![
            json!({"id":"op-1","status":"Pending"}),
            json!({"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
        ]);
        assert!(!adapter.capabilities().vendor_enforced_idempotency);
        let outcome = adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        assert_eq!(*submits.lock().unwrap(), 1);
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("op-1"));
        assert_eq!(outcome.provider_request_id.as_deref(), Some("req-1"));
        assert_eq!(outcome.actual_vendor_cost, None);
        assert_eq!(outcome.usage.unwrap().images, Some(1));
        assert_eq!(outcome.artifacts[0].bytes, png());
    }

    #[test]
    fn parses_top_level_fractional_credit_cost_from_the_json_decimal_lexeme() {
        let outcome = invoke_ready_json(
            r#"{"id":"op-1","status":"Ready","cost":1.400,"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
        )
        .unwrap();
        assert_eq!(
            outcome.actual_vendor_cost,
            Some(ActualVendorCost::new(1400, 5, "USD").unwrap())
        );
    }

    #[test]
    fn ignores_undocumented_nested_cost_and_normalizes_missing_or_null_cost() {
        for raw in [
            r#"{"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png","cost":999}}"#,
            r#"{"id":"op-1","status":"Ready","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
            r#"{"id":"op-1","status":"Ready","cost":null,"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
        ] {
            assert_eq!(invoke_ready_json(raw).unwrap().actual_vendor_cost, None);
        }
    }

    #[test]
    fn top_level_cost_is_authoritative_over_nested_result_data() {
        let outcome = invoke_ready_json(
            r#"{"id":"op-1","status":"Ready","cost":4.5,"result":{"sample":"https://delivery.us.bfl.ai/out.png","cost":900}}"#,
        )
        .unwrap();
        assert_eq!(
            outcome.actual_vendor_cost,
            Some(ActualVendorCost::new(45, 3, "USD").unwrap())
        );
    }

    #[test]
    fn credit_to_usd_conversion_is_exact_at_decimal_and_integer_boundaries() {
        for (raw, expected, budget_cents) in [
            ("0", ActualVendorCost::new(0, 2, "USD").unwrap(), 0),
            ("1", ActualVendorCost::new(1, 2, "USD").unwrap(), 1),
            ("1.0001", ActualVendorCost::new(10001, 6, "USD").unwrap(), 2),
            (
                "0.0000000000000001",
                ActualVendorCost::new(1, 18, "USD").unwrap(),
                1,
            ),
            (
                "100e18",
                ActualVendorCost::new(1_000_000_000_000_000_000, 0, "USD").unwrap(),
                i64::MAX,
            ),
            (
                "9223372036854775807e2",
                ActualVendorCost::new(i64::MAX, 0, "USD").unwrap(),
                i64::MAX,
            ),
        ] {
            let converted = bfl_credit_cost_to_usd(raw).unwrap();
            assert_eq!(converted, expected, "raw BFL credits: {raw}");
            if raw == "100e18" || raw == "9223372036854775807e2" {
                assert!(converted.to_budget_minor_units("USD").is_err());
            } else {
                assert_eq!(
                    converted.to_budget_minor_units("USD").unwrap(),
                    budget_cents,
                    "raw BFL credits: {raw}"
                );
            }
        }
    }

    #[test]
    fn rejects_malformed_negative_out_of_scale_and_overflowing_costs() {
        for raw in [
            r#"{"id":"op-1","status":"Ready","cost":"1.5","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
            r#"{"id":"op-1","status":"Ready","cost":{},"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
            r#"{"id":"op-1","status":"Ready","cost":-0.01,"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
            r#"{"id":"op-1","status":"Ready","cost":0.00000000000000001,"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
            r#"{"id":"op-1","status":"Ready","cost":9223372036854775808e2,"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
        ] {
            let error = invoke_ready_json(raw).unwrap_err();
            assert_eq!(error.code, "invalid_provider_cost");
            assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
            assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
        }
    }

    #[test]
    fn synchronous_readiness_matches_every_supported_artifact_result_shape() {
        for body in [
            json!({"id":"op-1","result":{"sample":"https://delivery.us.bfl.ai/out.png"}}),
            json!({"id":"op-1","result":{"image":{"url":"https://delivery.us.bfl.ai/out.png"}}}),
            json!({"id":"op-1","sample":"https://delivery.us.bfl.ai/out.png"}),
        ] {
            let submits = Arc::new(Mutex::new(0));
            let adapter = Flux2ApiAdapter::new(
                config(),
                "flux-2-pro".into(),
                Fixture {
                    submit: submits.clone(),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: png(),
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: Some(body),
                    expected_options: None,
                },
            )
            .unwrap();
            let outcome = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap();
            assert_eq!(outcome.artifacts[0].media_type, "image/png");
            assert_eq!(*submits.lock().unwrap(), 1);
        }
    }

    #[test]
    fn invalid_artifact_pins_reject_adapter_before_submission() {
        for hosts in [
            vec!["https://delivery.us.bfl.ai".into()],
            vec!["cdn.bfl.ai".into()],
            vec!["delivery.us.bfl.ai.evil.example".into()],
        ] {
            let submits = Arc::new(Mutex::new(0));
            let mut invalid = config();
            invalid.approved_artifact_hosts = hosts;
            assert!(Flux2ApiAdapter::new(
                invalid,
                "flux-2-pro".into(),
                Fixture {
                    submit: submits.clone(),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: png(),
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: None,
                    expected_options: None,
                }
            )
            .is_err());
            assert_eq!(*submits.lock().unwrap(), 0);
        }
    }

    #[test]
    fn empty_artifact_pins_follow_only_the_narrow_bfl_delivery_family() {
        let signed_artifact = "https://delivery.eu-2.bfl.ai/out.png?signature=short-lived";
        let (template, traffic) = security_fixture(
            202,
            json!({"id":"op-1","polling_url":"https://api.eu.bfl.ai/v1/get_result?id=op-1"}),
            vec![(
                200,
                json!({"id":"op-1","status":"Ready","result":{"sample":signed_artifact}}),
            )],
        );
        let mut unpinned = config();
        unpinned.approved_artifact_hosts.clear();
        let adapter = Flux2ApiAdapter::new(unpinned, MODEL_ID.into(), template.transport).unwrap();
        adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        assert_eq!(
            traffic.artifacts.lock().unwrap().as_slice(),
            &[signed_artifact.to_owned()]
        );
    }

    #[test]
    fn adapter_configuration_accepts_only_explicit_bfl_api_origins() {
        let (template, traffic) = security_fixture(
            202,
            json!({"id":"op-1","status":"Request Moderated"}),
            Vec::new(),
        );
        for host in ["api.bfl.ai", "api.eu.bfl.ai", "api.us.bfl.ai"] {
            let mut valid = config();
            valid.endpoint = format!("https://{host}");
            assert!(
                Flux2ApiAdapter::new(valid, MODEL_ID.into(), template.transport.clone()).is_ok()
            );
        }
        for endpoint in [
            "http://api.bfl.ai",
            "https://user@api.bfl.ai",
            "https://api.bfl.ai:443",
            "HTTPS://api.bfl.ai:443",
            "https:///api.bfl.ai:443",
            "https://api.bfl.ai:8443",
            "https://api.bfl.ai/path",
            "https://api.bfl.ai?query=x",
            "https://api.bfl.ai#fragment",
            "https://api.bfl.ai.evil.example",
            "https://evil.api.bfl.ai",
            "https://api.us1.bfl.ai",
        ] {
            let mut invalid = config();
            invalid.endpoint = endpoint.into();
            assert!(
                Flux2ApiAdapter::new(invalid, MODEL_ID.into(), template.transport.clone()).is_err(),
                "{endpoint}"
            );
        }
        assert!(traffic.submissions.lock().unwrap().is_empty());
    }

    #[test]
    fn certified_contract_is_literal_complete_and_within_bfl_constraints() {
        let cases = [("1k", 1024, 1024), ("2k", 1920, 1088), ("4k", 2048, 2048)];
        assert_eq!(SUPPORTED_PRESETS, cases.map(|(preset, _, _)| preset));
        for (preset, width, height) in cases {
            let expected = OutputDimensions { width, height };
            assert_eq!(contract_dimensions(preset).unwrap(), expected);
            validate_dimensions(&expected).unwrap();

            let mut input = json!({"prompt":"cat","image_size":preset});
            assert_eq!(bind_output_dimensions(&mut input).unwrap(), expected);
            assert_eq!(input["options"]["width"], width);
            assert_eq!(input["options"]["height"], height);
        }
        assert!(contract_dimensions("preview").is_err());
    }

    #[test]
    fn dimension_constraints_reject_minimum_multiple_and_pixel_violations() {
        for dimensions in [
            OutputDimensions {
                width: MIN_DIMENSION - 1,
                height: MIN_DIMENSION,
            },
            OutputDimensions {
                width: MIN_DIMENSION,
                height: MIN_DIMENSION - 1,
            },
            OutputDimensions {
                width: 1024,
                height: 1000,
            },
            OutputDimensions {
                width: 2064,
                height: 2048,
            },
        ] {
            assert!(validate_dimensions(&dimensions).is_err());
        }
        validate_dimensions(&OutputDimensions {
            width: 2048,
            height: 2048,
        })
        .unwrap();
    }

    #[test]
    fn binding_rejects_missing_partial_conflicting_and_arbitrary_dimensions() {
        for mut input in [
            json!({"prompt":"cat"}),
            json!({"prompt":"cat","image_size":"preview"}),
            json!({"prompt":"cat","image_size":"1k","options":{"width":1024}}),
            json!({"prompt":"cat","image_size":"1k","options":{"height":1024}}),
            json!({"prompt":"cat","image_size":"1k","options":{"width":1024,"height":1008}}),
            json!({"prompt":"cat","image_size":"1k","options":{"width":2048,"height":2048}}),
            json!({"prompt":"cat","image_size":"4k","options":{"width":2064,"height":2048}}),
        ] {
            assert!(bind_output_dimensions(&mut input).is_err());
        }
    }

    #[test]
    fn legacy_snapshot_recovery_derives_only_the_certified_mapping() {
        for (preset, width, height) in [("1k", 1024, 1024), ("2k", 1920, 1088), ("4k", 2048, 2048)]
        {
            for mut input in [
                json!({"prompt":"cat","image_size":preset}),
                json!({"prompt":"cat","image_size":preset,"options":{"width":width,"height":height}}),
            ] {
                assert_eq!(
                    bind_legacy_output_dimensions(preset, &mut input).unwrap(),
                    OutputDimensions { width, height }
                );
                assert_eq!(input["options"]["width"], width);
                assert_eq!(input["options"]["height"], height);
            }
        }
    }

    #[test]
    fn legacy_snapshot_recovery_rejects_selector_and_dimension_tampering() {
        for (frozen_preset, mut input) in [
            ("1k", json!({"prompt":"cat"})),
            ("1k", json!({"prompt":"cat","image_size":"2k"})),
            ("preview", json!({"prompt":"cat","image_size":"preview"})),
            (
                "1k",
                json!({"prompt":"cat","image_size":"1k","options":{"width":1024}}),
            ),
            (
                "1k",
                json!({"prompt":"cat","image_size":"1k","options":{"height":1024}}),
            ),
            (
                "1k",
                json!({"prompt":"cat","image_size":"1k","options":{"width":1024,"height":1008}}),
            ),
            (
                "1k",
                json!({"prompt":"cat","image_size":"1k","options":{"width":2048,"height":2048}}),
            ),
            (
                "4k",
                json!({"prompt":"cat","image_size":"4k","options":{"width":2064,"height":2048}}),
            ),
        ] {
            assert!(bind_legacy_output_dimensions(frozen_preset, &mut input).is_err());
        }
    }

    #[test]
    fn every_certified_preset_transmits_exact_dimensions_only() {
        for (preset, width, height) in [("1k", 1024, 1024), ("2k", 1920, 1088), ("4k", 2048, 2048)]
        {
            let submits = Arc::new(Mutex::new(0));
            let adapter = Flux2ApiAdapter::new(
                config(),
                MODEL_ID.into(),
                Fixture {
                    submit: submits.clone(),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: vec![],
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: Some(json!({"id":"op-rejected","status":"Rejected"})),
                    expected_options: Some(json!({"width":width,"height":height})),
                },
            )
            .unwrap();
            assert_eq!(
                adapter
                    .invoke(
                        &request_for(preset),
                        &input_for(preset),
                        &secret_for_test("secret-canary"),
                        Some("opaque-key"),
                    )
                    .unwrap_err()
                    .code,
                "provider_rejected"
            );
            assert_eq!(*submits.lock().unwrap(), 1);
        }
    }

    #[test]
    fn adapter_pins_the_non_preview_model() {
        let submit = Arc::new(Mutex::new(0));
        assert!(Flux2ApiAdapter::new(
            config(),
            "flux-2-pro-preview".into(),
            Fixture {
                submit: submit.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: None,
                submit_body: None,
                expected_options: None,
            },
        )
        .is_err());
        assert_eq!(*submit.lock().unwrap(), 0);
    }

    #[test]
    fn transmits_frozen_dimensions_and_supported_options_without_image_size() {
        let submits = Arc::new(Mutex::new(0));
        let options = json!({"width":1024,"height":1024,"seed":42,"safety_tolerance":2,"output_format":"png"});
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: None,
                submit_body: Some(json!({"id":"op-rejected","status":"Rejected"})),
                expected_options: Some(options.clone()),
            },
        )
        .unwrap();
        let error = adapter
            .invoke(
                &request(),
                &json!({"prompt":"cat","image_size":"1k","options":options}),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(error.code, "provider_rejected");
        assert_eq!(error.evidence.operation_id.as_deref(), Some("op-rejected"));
        assert_eq!(*submits.lock().unwrap(), 1);
    }

    #[test]
    fn flux2_safety_tolerance_is_exactly_zero_through_five() {
        for tolerance in 0..=5 {
            let (adapter, submits) = fixture(Vec::new());
            let options = json!({
                "width":1024,
                "height":1024,
                "safety_tolerance":tolerance
            });
            adapter
                .preflight_input(
                    &request(),
                    &json!({"prompt":"cat","image_size":"1k","options":options}),
                )
                .unwrap();
            assert_eq!(*submits.lock().unwrap(), 0);
        }

        for tolerance in [
            json!(6),
            json!(7),
            json!(-1),
            json!(1.5),
            json!("5"),
            Value::Null,
        ] {
            let (adapter, submits) = fixture(Vec::new());
            let options = json!({
                "width":1024,
                "height":1024,
                "safety_tolerance":tolerance
            });
            let error = adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat","image_size":"1k","options":options}),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(error.code, "invalid_request");
            assert_eq!(error.phase, ProviderPhase::PreSend);
            assert_eq!(*submits.lock().unwrap(), 0);
        }
    }

    #[test]
    fn invalid_options_are_rejected_before_submission() {
        for options in [
            json!({"width":1024,"height":1024,"unknown":1}),
            json!({"width":63,"height":1024}),
            json!({"width":1024}),
            json!({"width":1024,"height":1024,"seed":-1}),
            json!({"width":1024,"height":1024,"safety_tolerance":6}),
            json!({"width":1024,"height":1024,"output_format":"gif"}),
        ] {
            let (adapter, submits) = fixture(Vec::new());
            assert!(adapter
                .preflight_input(
                    &request(),
                    &json!({"prompt":"cat","image_size":"1k","options":options})
                )
                .is_err());
            assert!(adapter
                .invoke(
                    &request(),
                    &json!({"prompt":"cat","image_size":"1k","options":options}),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .is_err());
            assert_eq!(*submits.lock().unwrap(), 0);
        }
    }
    #[test]
    fn stable_rejection_malformed_missing_image_and_artifact_policy_failures() {
        for (body, expected) in [
            (
                json!({"id":"op-1","status":"Rejected"}),
                "provider_rejected",
            ),
            (json!({"id":"op-1","status":"Ready"}), "malformed_response"),
            (
                json!({"id":"op-1","status":"Ready","result":{}}),
                "missing_image",
            ),
            (
                json!({"id":"op-1","status":"Ready","result":{"sample":"https://evil.example/out.png"}}),
                "artifact_policy_failure",
            ),
        ] {
            let (adapter, submits) = fixture(vec![body]);
            let error = adapter
                .invoke(
                    &request(),
                    &input(),
                    &secret_for_test("secret-canary"),
                    Some("opaque-key"),
                )
                .unwrap_err();
            assert_eq!(code(error), expected);
            assert_eq!(*submits.lock().unwrap(), 1);
        }
    }
    #[test]
    fn ambiguous_submission_and_poll_timeout_never_resubmit() {
        let submits = Arc::new(Mutex::new(0));
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: Some("secret-canary".into()),
                fail_poll: None,
                submit_body: None,
                expected_options: None,
            },
        )
        .unwrap();
        let error = adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(code(error), "timeout_unknown_outcome");
        assert_eq!(*submits.lock().unwrap(), 1);
        let submits = Arc::new(Mutex::new(0));
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits.clone(),
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: Some("nested secret-canary".into()),
                submit_body: None,
                expected_options: None,
            },
        )
        .unwrap();
        let error = adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "provider error (timeout_unknown_outcome)"
        );
        assert!(!error.to_string().contains("secret-canary"));
        assert_eq!(*submits.lock().unwrap(), 1);

        let (adapter, submits) = fixture(vec![json!({"_http_status":401})]);
        let error = adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap_err();
        assert_eq!(error.code, "provider_authentication_failed");
        assert_eq!(error.spend_disposition, SpendDisposition::Reconcile);
        assert_eq!(error.evidence.operation_id.as_deref(), Some("op-1"));
        assert_eq!(*submits.lock().unwrap(), 1);
    }
    #[test]
    fn rejects_invalid_request_and_retry_policy_before_network() {
        let (adapter, submits) = fixture(vec![]);
        let mut bad = request();
        bad.image_count = Some(2);
        assert_eq!(
            code(
                adapter
                    .invoke(
                        &bad,
                        &input(),
                        &secret_for_test("secret-canary"),
                        Some("opaque-key")
                    )
                    .unwrap_err()
            ),
            "invalid_request"
        );
        assert_eq!(*submits.lock().unwrap(), 0);
        let mut invalid = config();
        invalid.max_retries = 1;
        assert!(Flux2ApiAdapter::new(
            invalid,
            "flux-2-pro".into(),
            Fixture {
                submit: submits,
                polls: Arc::new(Mutex::new(vec![])),
                artifact: vec![],
                fail_submit: None,
                fail_poll: None,
                submit_body: None,
                expected_options: None
            }
        )
        .is_err());

        for name in ["x-key", "Authorization", "X-STATIC"] {
            let mut invalid = config();
            invalid.idempotency_header = Some(name.into());
            invalid.headers.insert("x-static".into(), "operator".into());
            assert!(Flux2ApiAdapter::new(
                invalid,
                "flux-2-pro".into(),
                Fixture {
                    submit: Arc::new(Mutex::new(0)),
                    polls: Arc::new(Mutex::new(vec![])),
                    artifact: vec![],
                    fail_submit: None,
                    fail_poll: None,
                    submit_body: None,
                    expected_options: None,
                }
            )
            .is_err());
        }
    }

    #[test]
    fn fixture_output_uses_shared_artifact_policy_and_frozen_pricing_for_settlement() {
        use crate::{
            artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference, Repository},
            provider::contract::PricingCatalog,
        };
        use tempfile::tempdir;

        let png = png();
        let submits = Arc::new(Mutex::new(0));
        let adapter = Flux2ApiAdapter::new(
            config(),
            "flux-2-pro".into(),
            Fixture {
                submit: submits,
                polls: Arc::new(Mutex::new(vec![serde_json::from_str(
                    r#"{"id":"op-1","status":"Ready","cost":45,"result":{"sample":"https://delivery.us.bfl.ai/out.png"}}"#,
                )
                .unwrap()])),
                artifact: png.clone(),
                fail_submit: None,
                fail_poll: None,
                submit_body: None,
                expected_options: None,
            },
        )
        .unwrap();
        let outcome = adapter
            .invoke(
                &request(),
                &input(),
                &secret_for_test("secret-canary"),
                Some("opaque-key"),
            )
            .unwrap();
        let pricing = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"fixture-v2","rules":[{"rule_id":"flux2-pro-1k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"1k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":45,"rate_denominator":1}]}]}"#).unwrap();
        let snapshot = pricing.snapshot(&request()).unwrap();
        assert_eq!(snapshot.estimated_amount_minor, 45);
        let settlement = snapshot
            .settle_precise(
                outcome.usage.as_ref().unwrap(),
                outcome.actual_vendor_cost.as_ref(),
                45,
            )
            .unwrap();
        assert_eq!(settlement.budget_amount_minor, 45);
        assert_eq!(
            settlement.actual_vendor_cost,
            ActualVendorCost::new(45, 2, "USD").unwrap(),
            "45 BFL credits are USD 0.45 and must not be multiplied by 100 twice"
        );

        let repository = Repository::in_memory().unwrap();
        let execution = repository
            .create_execution(&CreateExecutionParams {
                account_id: "account".into(),
                operation_key: "flux-fixture".into(),
                hubu_authorization_id: "token-ref".into(),
                hubu_claim_id: Some("claim".into()),
                hubu_token_reference: HubuTokenReference::new("token-ref").unwrap(),
                authorized_minor: 45,
                authorization_currency: "USD".into(),
                normalized_input: input(),
                input_hash: "hash".into(),
                input_schema_version: 1,
                target: "flux/flux-2-pro".into(),
                config_version: "cfg".into(),
                workload_type: "image_generation".into(),
                provider: "flux".into(),
                adapter: "flux2_api".into(),
                model: "flux-2-pro".into(),
                provider_config_version: "pcv".into(),
                provider_config_digest: format!("sha256:{}", "a".repeat(64)),
                pricing_snapshot: serde_json::to_value(snapshot).unwrap(),
                pricing_schema_version: 2,
                execution_scope: None,
                created_at: "now".into(),
            })
            .unwrap();
        let root = tempdir().unwrap();
        let service = ArtifactService::new(
            repository,
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let artifact = service
            .store_image(
                &execution.execution_id,
                None,
                &outcome.artifacts[0].media_type,
                &outcome.artifacts[0].bytes,
                "now",
            )
            .unwrap();
        assert_eq!(
            service
                .retrieve_for_account(&artifact.artifact_id, "account")
                .unwrap()
                .bytes,
            png
        );
    }
}
