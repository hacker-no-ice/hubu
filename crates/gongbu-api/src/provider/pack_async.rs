//! The async submit-and-poll engine: one reviewed implementation for providers
//! that accept a job and are polled for the result, driven by a provider pack.
//!
//! It submits once, checkpoints the job (host, job ID, request ID and the
//! absolute deadline) before polling, and resumes read-only polling from that
//! checkpoint after a restart without resubmitting or extending the deadline.
//! Holds are the frozen price times the requested units; settlement uses the
//! same frozen price, with the durable job ID as evidence. Only failures proven
//! before transmission, a definitive 4xx at submission, or a provider status
//! the pack declares as a refusal release the claim. Everything else after the
//! provider may have accepted the job goes to reconciliation.

use super::{
    contract::{
        AdapterCapabilities, AdapterOutcome, AdapterSubmission, AsyncProviderOperation,
        ContractError, NoopProviderTransportObserver, NormalizedRequest, PollingRecoveryContext,
        ProviderAdapter, ProviderFailure, ProviderPhase, ProviderTransportInteraction,
        ProviderTransportObserver,
    },
    http_kernel::InvocationDeadline,
    pack::{safe_job_id, select_path, Evidence, JobState, Pack, PollSpec, ProviderResponse},
    pack_sync::{
        EngineDownloads, ReqwestSyncTransport, SyncTransport, TransportFailure, TransportResponse,
    },
    targets::ProviderConfigVersion,
};
use crate::secrets::ProviderSecret;
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{sync::Arc, thread, time::Duration};

pub const ENGINE_ID: &str = "async_poll";
const POLLING_POLICY_VERSION: &str = "pack-poll-v1";

pub struct AsyncPollEngine<T = ReqwestSyncTransport> {
    pack: Arc<Pack>,
    model: String,
    transport: T,
    max_artifact_bytes: usize,
}

impl AsyncPollEngine<ReqwestSyncTransport> {
    pub fn from_target(
        target: &ProviderConfigVersion,
        pack: &Arc<Pack>,
        max_artifact_bytes: u64,
    ) -> Result<Self, ContractError> {
        Self::new(
            pack.clone(),
            &target.model,
            ReqwestSyncTransport,
            max_artifact_bytes,
        )
    }
}

fn reconcile(code: &str, phase: ProviderPhase, evidence: &Evidence) -> ProviderFailure {
    ProviderFailure::reconcile(code.to_owned(), phase)
        .with_evidence(evidence.request_id.clone(), evidence.operation_id.clone())
}

fn release(code: &str, phase: ProviderPhase, evidence: &Evidence) -> ProviderFailure {
    ProviderFailure::release(code.to_owned(), phase)
        .with_evidence(evidence.request_id.clone(), evidence.operation_id.clone())
}

/// The engine-owned failure code for an HTTP rejection.
fn rejection_code(status: u16) -> &'static str {
    match status {
        401 => "provider_authentication_failed",
        402 => "provider_insufficient_credit",
        403 => "provider_permission_denied",
        429 => "provider_rate_limited",
        _ => "provider_rejected",
    }
}

impl<T: SyncTransport> AsyncPollEngine<T> {
    pub fn new(
        pack: Arc<Pack>,
        model: &str,
        transport: T,
        max_artifact_bytes: u64,
    ) -> Result<Self, ContractError> {
        if pack.archetype().engine_id() != ENGINE_ID
            || pack.poll().is_none()
            || pack.model(model).is_none()
            || max_artifact_bytes == 0
            || max_artifact_bytes > usize::MAX as u64
        {
            return Err(ContractError::Provider {
                code: "config_invalid".into(),
            });
        }
        Ok(Self {
            pack,
            model: model.to_owned(),
            transport,
            max_artifact_bytes: max_artifact_bytes as usize,
        })
    }

    fn poll_spec(&self) -> &PollSpec {
        self.pack.poll().expect("validated async pack")
    }

    fn auth(&self, secret: &ProviderSecret) -> Option<(String, String)> {
        let transport = self.pack.transport();
        let secret = std::str::from_utf8(secret.expose()).ok()?;
        Some((
            transport.auth_header.clone(),
            transport.auth_scheme.header_value(secret),
        ))
    }

    fn state_of(&self, body: &Value) -> Option<JobState> {
        select_path(body, &self.poll_spec().status)
            .first()
            .and_then(|value| value.as_str())
            .map(|status| self.poll_spec().classify(status))
    }

    fn submit_inner(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        let none = Evidence::default();
        let pre_send = |code: &str| release(code, ProviderPhase::PreSend, &none);
        if vendor_idempotency_key.is_some() {
            return Err(pre_send("retry_not_supported"));
        }
        self.preflight_input(request, input)
            .map_err(|_| pre_send("invalid_request"))?;
        let prepared = self
            .pack
            .prepare(&self.model, input)
            .map_err(|_| pre_send("invalid_request"))?;
        let (auth_name, auth_value) = self
            .auth(secret)
            .ok_or_else(|| pre_send("credential_invalid"))?;
        let deadline = InvocationDeadline::from_timeout(Duration::from_millis(
            self.pack.transport().timeout_ms,
        ))
        .map_err(|_| pre_send("config_invalid"))?;
        let timeout = deadline
            .remaining()
            .map_err(|_| pre_send("provider_pre_send_failure"))?;
        let response = self
            .transport
            .post_json(
                &self.pack.request_url(),
                (&auth_name, &auth_value),
                &self.pack.transport().headers,
                &prepared.body,
                timeout,
            )
            .map_err(|failure| match failure {
                TransportFailure::BeforeSend => pre_send("provider_pre_send_failure"),
                TransportFailure::UnknownOutcome { request_id } => reconcile(
                    "timeout_unknown_outcome",
                    ProviderPhase::Submission,
                    &Evidence {
                        request_id,
                        operation_id: None,
                    },
                ),
            })?;
        let TransportResponse {
            status,
            headers,
            body,
        } = response;
        let response = ProviderResponse { headers, body };
        let mut evidence = Evidence {
            request_id: self.pack.provider_request_id(&response),
            operation_id: None,
        };
        match status {
            200..=299 => {}
            400..=499 => {
                return Err(release(
                    rejection_code(status),
                    ProviderPhase::Submission,
                    &evidence,
                ))
            }
            _ => {
                return Err(reconcile(
                    "provider_failure",
                    ProviderPhase::Submission,
                    &evidence,
                ))
            }
        }
        // From here the provider has accepted the job.
        evidence.operation_id = select_path(&response.body, &self.poll_spec().job_id)
            .first()
            .and_then(|value| match value {
                Value::String(text) => Some(text.trim().to_owned()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .filter(|id| safe_job_id(id));
        match self.state_of(&response.body) {
            Some(JobState::Ready) => {
                return self
                    .complete(request, &response, &evidence, deadline, observer)
                    .map(AdapterSubmission::Complete)
            }
            Some(JobState::Rejected(code)) => {
                return Err(release(&code, ProviderPhase::Processing, &evidence))
            }
            Some(JobState::Ambiguous(code)) => {
                return Err(reconcile(&code, ProviderPhase::Processing, &evidence))
            }
            Some(JobState::Unknown) => {
                return Err(reconcile(
                    "malformed_response",
                    ProviderPhase::Processing,
                    &evidence,
                ))
            }
            // A submission response without a status is pending once it names a job.
            Some(JobState::Pending) | None => {}
        }
        let job_id = evidence
            .operation_id
            .clone()
            .ok_or_else(|| reconcile("malformed_response", ProviderPhase::Processing, &evidence))?;
        let operation = self.checkpoint(&response.body, &evidence, job_id, deadline)?;
        Ok(AdapterSubmission::Pending(operation))
    }

    /// Choose the polling host and record sanitized evidence about any URL the
    /// provider returned. The checkpoint never stores a URL, only a host that
    /// the pack allows, and the poll URL is always rebuilt from the pack.
    fn checkpoint(
        &self,
        body: &Value,
        evidence: &Evidence,
        job_id: String,
        deadline: InvocationDeadline,
    ) -> Result<AsyncProviderOperation, ProviderFailure> {
        let poll = self.poll_spec();
        let endpoint_host = self
            .pack
            .request_url()
            .host_str()
            .expect("validated endpoint host")
            .to_owned();
        let returned = poll.url.as_deref().map(|path| {
            select_path(body, path)
                .first()
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        });
        let (polling_host, recovery) = match returned {
            None => (endpoint_host, None),
            Some(raw) => {
                let fingerprint = format!(
                    "sha256:{:x}",
                    Sha256::digest(
                        raw.as_deref()
                            .unwrap_or("polling-url-unavailable")
                            .as_bytes()
                    )
                );
                let parsed = raw.as_deref().and_then(|raw| Url::parse(raw).ok());
                let expected = parsed
                    .as_ref()
                    .and_then(|url| self.pack.poll_url(url.host_str()?, &job_id));
                let reason = match (&raw, &parsed, &expected) {
                    (None, _, _) => Some("polling_url_unavailable"),
                    (Some(_), None, _) => Some("malformed_url"),
                    (Some(_), Some(_), None) => Some("host_not_allowlisted"),
                    (Some(_), Some(url), Some(expected)) if url != expected => {
                        Some("path_contract_mismatch")
                    }
                    _ => None,
                };
                let host = match (reason, &parsed) {
                    (None, Some(url)) => url.host_str().map(str::to_owned),
                    _ => None,
                };
                let context = PollingRecoveryContext {
                    schema_version: 1,
                    policy_version: POLLING_POLICY_VERSION.to_owned(),
                    scheme: parsed.as_ref().map(|url| {
                        if url.scheme() == "https" {
                            "https".to_owned()
                        } else {
                            "other".to_owned()
                        }
                    }),
                    normalized_host: host.clone(),
                    explicit_port: None,
                    endpoint_shape: if reason == Some("path_contract_mismatch") {
                        "unexpected".to_owned()
                    } else {
                        "declared".to_owned()
                    },
                    query_keys: poll.query.keys().take(8).cloned().collect(),
                    url_fingerprint: fingerprint,
                    validation_reason: reason.map(str::to_owned),
                };
                // A rejected returned URL falls back to the endpoint host, which
                // always matches the declared poll shape.
                (host.unwrap_or(endpoint_host), Some(context))
            }
        };
        let operation = AsyncProviderOperation {
            provider_request_id: evidence.request_id.clone().filter(|id| safe_job_id(id)),
            provider_operation_id: job_id,
            polling_host,
            polling_recovery: recovery,
            deadline_unix_ms: deadline.unix_millis(),
        };
        operation.validate().map_err(|_| {
            reconcile(
                "invalid_provider_operation",
                ProviderPhase::Processing,
                evidence,
            )
        })?;
        Ok(operation)
    }

    fn poll_inner(
        &self,
        request: &NormalizedRequest,
        input: &Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let mut evidence = Evidence {
            request_id: operation.provider_request_id.clone(),
            operation_id: Some(operation.provider_operation_id.clone()),
        };
        let processing =
            |code: &str, evidence: &Evidence| reconcile(code, ProviderPhase::Processing, evidence);
        self.preflight_input(request, input)
            .map_err(|_| processing("invalid_request", &evidence))?;
        operation
            .validate()
            .map_err(|_| processing("invalid_provider_operation", &evidence))?;
        let deadline = InvocationDeadline::from_unix_millis(operation.deadline_unix_ms)
            .map_err(|_| processing("timeout_unknown_outcome", &evidence))?;
        let poll_url = self
            .pack
            .poll_url(&operation.polling_host, &operation.provider_operation_id)
            .ok_or_else(|| processing("polling_origin_rejected", &evidence))?;
        let (auth_name, auth_value) = self
            .auth(secret)
            .ok_or_else(|| processing("credential_invalid", &evidence))?;
        let interval = Duration::from_millis(self.poll_spec().interval_ms);
        loop {
            let remaining = deadline
                .remaining()
                .map_err(|_| processing("timeout_unknown_outcome", &evidence))?;
            thread::sleep(interval.min(remaining));
            let timeout = deadline
                .remaining()
                .map_err(|_| processing("timeout_unknown_outcome", &evidence))?;
            if !observer.record(ProviderTransportInteraction::Poll) {
                return Err(processing(
                    "provider_transport_evidence_unavailable",
                    &evidence,
                ));
            }
            let TransportResponse {
                status,
                headers,
                body,
            } = self
                .transport
                .get_json(
                    &poll_url,
                    (&auth_name, &auth_value),
                    &self.pack.transport().headers,
                    timeout,
                )
                .map_err(|failure| {
                    let request_id = match failure {
                        TransportFailure::UnknownOutcome { request_id } => request_id,
                        TransportFailure::BeforeSend => None,
                    };
                    processing(
                        "timeout_unknown_outcome",
                        &Evidence {
                            request_id: evidence.request_id.clone().or(request_id),
                            operation_id: evidence.operation_id.clone(),
                        },
                    )
                })?;
            let response = ProviderResponse { headers, body };
            if evidence.request_id.is_none() {
                evidence.request_id = self.pack.provider_request_id(&response);
            }
            // The job was accepted, so even a 4xx on a poll is not proof of no charge.
            if !(200..300).contains(&status) {
                let code = if (400..500).contains(&status) {
                    rejection_code(status)
                } else {
                    "provider_failure"
                };
                return Err(processing(code, &evidence));
            }
            let returned_job = select_path(&response.body, &self.poll_spec().job_id)
                .first()
                .map(|value| match value {
                    Value::String(text) => text.trim().to_owned(),
                    other => other.to_string(),
                });
            if returned_job.is_some_and(|id| id != operation.provider_operation_id) {
                return Err(processing(
                    "provider_operation_identity_mismatch",
                    &evidence,
                ));
            }
            match self.state_of(&response.body) {
                Some(JobState::Pending) => continue,
                Some(JobState::Ready) => {
                    return self.complete(request, &response, &evidence, deadline, observer)
                }
                Some(JobState::Rejected(code)) => {
                    return Err(release(&code, ProviderPhase::Processing, &evidence))
                }
                Some(JobState::Ambiguous(code)) => return Err(processing(&code, &evidence)),
                Some(JobState::Unknown) | None => {
                    return Err(processing("malformed_response", &evidence))
                }
            }
        }
    }

    fn complete(
        &self,
        request: &NormalizedRequest,
        response: &ProviderResponse,
        evidence: &Evidence,
        deadline: InvocationDeadline,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        let downloads = EngineDownloads {
            transport: &self.transport,
            max_bytes: self.max_artifact_bytes as u64,
            deadline,
            observer: Some(observer),
        };
        self.pack.map_response(
            request,
            response,
            evidence,
            &downloads,
            self.max_artifact_bytes,
        )
    }
}

impl<T: SyncTransport> ProviderAdapter for AsyncPollEngine<T> {
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
            return Err(ContractError::Provider {
                code: "invalid_request".into(),
            });
        }
        Ok(())
    }

    fn preflight_input(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
    ) -> Result<(), ContractError> {
        self.validate_request(request)?;
        if self.pack.prepare(&self.model, normalized_input)?.request != *request {
            return Err(ContractError::IndeterminableCost);
        }
        Ok(())
    }

    /// Submit and, when the job is still running, keep polling in-process.
    fn invoke(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.invoke_observed(
            request,
            normalized_input,
            secret,
            vendor_idempotency_key,
            &NoopProviderTransportObserver,
        )
    }

    fn invoke_observed(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        match self.submit_inner(
            request,
            normalized_input,
            secret,
            vendor_idempotency_key,
            observer,
        )? {
            AdapterSubmission::Complete(outcome) => Ok(outcome),
            AdapterSubmission::Pending(operation) => {
                self.poll_inner(request, normalized_input, secret, &operation, observer)
            }
        }
    }

    fn submit(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        self.submit_inner(
            request,
            normalized_input,
            secret,
            vendor_idempotency_key,
            &NoopProviderTransportObserver,
        )
    }

    fn submit_observed(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        vendor_idempotency_key: Option<&str>,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterSubmission, ProviderFailure> {
        self.submit_inner(
            request,
            normalized_input,
            secret,
            vendor_idempotency_key,
            observer,
        )
    }

    fn poll(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.poll_inner(
            request,
            normalized_input,
            secret,
            operation,
            &NoopProviderTransportObserver,
        )
    }

    fn poll_observed(
        &self,
        request: &NormalizedRequest,
        normalized_input: &Value,
        secret: &ProviderSecret,
        operation: &AsyncProviderOperation,
        observer: &dyn ProviderTransportObserver,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        self.poll_inner(request, normalized_input, secret, operation, observer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::{
            conformance::{assert_adapter_conformance, Case, Observation},
            contract::{ActualVendorCost, PricingCatalog, SpendDisposition},
            http_kernel::ArtifactDownloadPolicy,
            pack::ArtifactFetchError,
        },
        secrets::secret_for_test,
    };
    use serde_json::json;
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::Mutex,
    };

    const PACK: &str = include_str!("../../../../fixtures/packs/async-image.toml");
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    const MAX_ARTIFACT: u64 = 1024;

    #[derive(Clone)]
    struct Fake {
        submit: Result<TransportResponse, TransportFailure>,
        polls: Arc<Mutex<VecDeque<Result<TransportResponse, TransportFailure>>>>,
        posts: Arc<Mutex<u32>>,
        poll_urls: Arc<Mutex<Vec<String>>>,
        artifact: Vec<u8>,
    }

    fn ok(body: Value) -> Result<TransportResponse, TransportFailure> {
        status(200, body)
    }
    fn status(code: u16, body: Value) -> Result<TransportResponse, TransportFailure> {
        Ok(TransportResponse {
            status: code,
            headers: BTreeMap::from([("x-request-id".into(), "req-1".into())]),
            body,
        })
    }
    fn accepted() -> Value {
        json!({"id": "job-1", "polling_url": "https://api.eu.async-image.test/v1/get_result?id=job-1"})
    }
    fn ready() -> Value {
        json!({"id": "job-1", "status": "Ready", "cost": 3.5,
               "result": {"sample": "https://delivery.eu1.async-image.test/out.png"}})
    }

    impl Fake {
        fn new(submit: Value, polls: Vec<Value>) -> Self {
            Self {
                submit: ok(submit),
                polls: Arc::new(Mutex::new(polls.into_iter().map(ok).collect())),
                posts: Arc::default(),
                poll_urls: Arc::default(),
                artifact: PNG.to_vec(),
            }
        }
    }

    impl SyncTransport for Fake {
        fn post_json(
            &self,
            _: &Url,
            auth: (&str, &str),
            _: &BTreeMap<String, String>,
            _: &Value,
            _: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            assert_eq!(auth.0, "x-key");
            *self.posts.lock().unwrap() += 1;
            self.submit.clone()
        }
        fn get_json(
            &self,
            url: &Url,
            _: (&str, &str),
            _: &BTreeMap<String, String>,
            _: Duration,
        ) -> Result<TransportResponse, TransportFailure> {
            self.poll_urls.lock().unwrap().push(url.to_string());
            self.polls
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(TransportFailure::UnknownOutcome { request_id: None }))
        }
        fn download(
            &self,
            url: &Url,
            policy: &ArtifactDownloadPolicy,
            _: InvocationDeadline,
        ) -> Result<Vec<u8>, ArtifactFetchError> {
            policy
                .validate_url(url)
                .map_err(|_| ArtifactFetchError::Failed)?;
            if self.artifact.len() > policy.max_bytes() {
                return Err(ArtifactFetchError::TooLarge);
            }
            Ok(self.artifact.clone())
        }
    }

    fn engine(transport: Fake) -> AsyncPollEngine<Fake> {
        AsyncPollEngine::new(
            Arc::new(Pack::from_toml_str(PACK).unwrap()),
            "image-v1",
            transport,
            MAX_ARTIFACT,
        )
        .unwrap()
    }
    fn input() -> Value {
        json!({"prompt": "a lighthouse"})
    }
    fn request() -> NormalizedRequest {
        Pack::from_toml_str(PACK)
            .unwrap()
            .prepare("image-v1", &input())
            .unwrap()
            .request
    }

    #[test]
    fn fixture_async_pack_passes_executor_conformance() {
        assert_adapter_conformance(|case| {
            let mut fake = match case {
                Case::Rejection => Fake {
                    submit: status(400, Value::Null),
                    ..Fake::new(Value::Null, vec![])
                },
                Case::AmbiguousPostSend => Fake {
                    submit: Err(TransportFailure::UnknownOutcome { request_id: None }),
                    ..Fake::new(Value::Null, vec![])
                },
                Case::HostPolicy => {
                    let mut foreign = ready();
                    foreign["result"]["sample"] = json!("https://evil.example/out.png");
                    Fake::new(accepted(), vec![foreign])
                }
                _ => Fake::new(
                    accepted(),
                    vec![json!({"id": "job-1", "status": "Pending"}), ready()],
                ),
            };
            match case {
                Case::EvidenceRetention => fake.artifact = b"not an image".to_vec(),
                Case::ArtifactBound => fake.artifact = vec![0; MAX_ARTIFACT as usize + 1],
                _ => {}
            }
            let posts = fake.posts.clone();
            let engine = engine(fake);
            let mut request = request();
            if matches!(case, Case::InvalidRequest) {
                request.set_quantity("image_count", Some(2));
            }
            let result = engine.invoke(
                &request,
                &input(),
                &secret_for_test("secret-canary"),
                matches!(case, Case::UnsafeRetry).then_some("opaque-key"),
            );
            let submissions = *posts.lock().unwrap();
            Observation {
                result,
                submissions,
            }
        });
    }

    #[test]
    fn restart_during_polling_resumes_from_the_checkpoint_without_resubmitting() {
        let first = Fake::new(accepted(), vec![]);
        let submission = engine(first.clone())
            .submit(&request(), &input(), &secret_for_test("k"), None)
            .unwrap();
        let AdapterSubmission::Pending(operation) = submission else {
            panic!("expected a pending job");
        };
        assert_eq!(*first.posts.lock().unwrap(), 1);
        assert_eq!(operation.provider_operation_id, "job-1");
        assert_eq!(operation.provider_request_id.as_deref(), Some("req-1"));
        assert_eq!(operation.polling_host, "api.eu.async-image.test");
        assert_eq!(
            operation
                .polling_recovery
                .as_ref()
                .unwrap()
                .validation_reason,
            None
        );

        // The checkpoint round-trips through durable storage; a new process
        // with a new engine resumes polling only.
        let stored: AsyncProviderOperation =
            serde_json::from_str(&serde_json::to_string(&operation).unwrap()).unwrap();
        let resumed = Fake::new(
            Value::Null,
            vec![json!({"id": "job-1", "status": "Generating"}), ready()],
        );
        let poll_urls = resumed.poll_urls.clone();
        let posts = resumed.posts.clone();
        let outcome = engine(resumed)
            .poll(&request(), &input(), &secret_for_test("k"), &stored)
            .unwrap();
        assert_eq!(*posts.lock().unwrap(), 0);
        assert_eq!(
            poll_urls.lock().unwrap().as_slice(),
            [
                "https://api.eu.async-image.test/v1/get_result?id=job-1",
                "https://api.eu.async-image.test/v1/get_result?id=job-1"
            ]
        );
        outcome.validate().unwrap();
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("job-1"));
        assert_eq!(outcome.provider_request_id.as_deref(), Some("req-1"));
        // 3.5 one-cent credits are exactly $0.035.
        assert_eq!(
            outcome.actual_vendor_cost,
            Some(ActualVendorCost::new(35, 3, "USD").unwrap())
        );
        let pack = Pack::from_toml_str(PACK).unwrap();
        let catalog = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2, "catalog_version": "v", "rules": pack.pricing_rules()
            }))
            .unwrap(),
        )
        .unwrap();
        let snapshot = catalog.snapshot(&request()).unwrap();
        // 0.5¢ per request + 3¢ per image, held and settled exactly.
        assert_eq!(snapshot.estimated_units, 35_000);
        let settled = snapshot
            .settle_precise(
                outcome.usage.as_ref().unwrap(),
                outcome.actual_vendor_cost.as_ref(),
                snapshot.estimated_units,
            )
            .unwrap();
        assert_eq!(settled.budget_units, 35_000);
    }

    #[test]
    fn a_returned_polling_url_off_the_allowlist_falls_back_to_the_endpoint_host() {
        let mut body = accepted();
        body["polling_url"] = json!("https://attacker.example/v1/get_result?id=job-1");
        let AdapterSubmission::Pending(operation) = engine(Fake::new(body, vec![]))
            .submit(&request(), &input(), &secret_for_test("k"), None)
            .unwrap()
        else {
            panic!("expected a pending job");
        };
        assert_eq!(operation.polling_host, "api.async-image.test");
        let recovery = operation.polling_recovery.unwrap();
        assert_eq!(
            recovery.validation_reason.as_deref(),
            Some("host_not_allowlisted")
        );
        assert_eq!(recovery.normalized_host, None);

        let mut shape = accepted();
        shape["polling_url"] = json!("https://api.eu.async-image.test/v2/other?id=job-1");
        let AdapterSubmission::Pending(operation) = engine(Fake::new(shape, vec![]))
            .submit(&request(), &input(), &secret_for_test("k"), None)
            .unwrap()
        else {
            panic!("expected a pending job");
        };
        assert_eq!(operation.polling_host, "api.async-image.test");
        assert_eq!(
            operation
                .polling_recovery
                .unwrap()
                .validation_reason
                .as_deref(),
            Some("path_contract_mismatch")
        );
    }

    #[test]
    fn job_statuses_map_to_release_or_reconciliation() {
        let run = |polls: Vec<Result<TransportResponse, TransportFailure>>| {
            let fake = Fake {
                polls: Arc::new(Mutex::new(polls.into_iter().collect())),
                ..Fake::new(accepted(), vec![])
            };
            engine(fake)
                .invoke(&request(), &input(), &secret_for_test("k"), None)
                .unwrap_err()
        };
        let moderated = run(vec![ok(
            json!({"id": "job-1", "status": "Content Moderated"}),
        )]);
        assert_eq!(
            (moderated.code.as_str(), moderated.spend_disposition),
            ("provider_content_moderated", SpendDisposition::Release)
        );
        assert_eq!(moderated.evidence.operation_id.as_deref(), Some("job-1"));
        for (polls, code) in [
            (
                vec![ok(json!({"id": "job-1", "status": "Task not found"}))],
                "provider_task_not_found",
            ),
            (
                vec![ok(json!({"id": "job-1", "status": "Exploded"}))],
                "malformed_response",
            ),
            (
                vec![ok(json!({"id": "job-2", "status": "Ready"}))],
                "provider_operation_identity_mismatch",
            ),
            (vec![status(404, Value::Null)], "provider_rejected"),
            (vec![status(502, Value::Null)], "provider_failure"),
            (
                vec![Err(TransportFailure::UnknownOutcome { request_id: None })],
                "timeout_unknown_outcome",
            ),
        ] {
            let failure = run(polls);
            assert_eq!(failure.code, code);
            assert_eq!(
                failure.spend_disposition,
                SpendDisposition::Reconcile,
                "{code}"
            );
            assert_eq!(
                failure.evidence.operation_id.as_deref(),
                Some("job-1"),
                "{code}"
            );
        }

        // A job that expired before a restart is never polled again.
        let mut stale = AsyncProviderOperation {
            provider_request_id: None,
            provider_operation_id: "job-1".into(),
            polling_host: "api.async-image.test".into(),
            polling_recovery: None,
            deadline_unix_ms: 1,
        };
        let fake = Fake::new(Value::Null, vec![ready()]);
        let urls = fake.poll_urls.clone();
        let failure = engine(fake)
            .poll(&request(), &input(), &secret_for_test("k"), &stale)
            .unwrap_err();
        assert_eq!(failure.code, "timeout_unknown_outcome");
        assert!(urls.lock().unwrap().is_empty());
        // A checkpoint naming a host the pack does not allow is refused.
        stale.deadline_unix_ms = InvocationDeadline::from_timeout(Duration::from_secs(60))
            .unwrap()
            .unix_millis();
        stale.polling_host = "attacker.example".into();
        let failure = engine(Fake::new(Value::Null, vec![ready()]))
            .poll(&request(), &input(), &secret_for_test("k"), &stale)
            .unwrap_err();
        assert_eq!(failure.code, "polling_origin_rejected");
    }

    #[test]
    fn a_job_ready_at_submission_completes_without_polling() {
        let fake = Fake::new(ready(), vec![]);
        let urls = fake.poll_urls.clone();
        let outcome = engine(fake)
            .invoke(&request(), &input(), &secret_for_test("k"), None)
            .unwrap();
        assert!(urls.lock().unwrap().is_empty());
        assert_eq!(outcome.artifacts.len(), 1);
        assert_eq!(outcome.provider_operation_id.as_deref(), Some("job-1"));
    }

    #[test]
    fn polls_stop_when_the_observer_cannot_record_them() {
        struct Refuse;
        impl ProviderTransportObserver for Refuse {
            fn record(&self, _: ProviderTransportInteraction) -> bool {
                false
            }
        }
        let fake = Fake::new(accepted(), vec![ready()]);
        let urls = fake.poll_urls.clone();
        let failure = engine(fake)
            .invoke_observed(&request(), &input(), &secret_for_test("k"), None, &Refuse)
            .unwrap_err();
        assert_eq!(failure.code, "provider_transport_evidence_unavailable");
        assert_eq!(failure.spend_disposition, SpendDisposition::Reconcile);
        assert!(urls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_malformed_reported_cost_goes_to_reconciliation() {
        let mut body = ready();
        body["cost"] = json!("free");
        let failure = engine(Fake::new(body, vec![]))
            .invoke(&request(), &input(), &secret_for_test("k"), None)
            .unwrap_err();
        assert_eq!(failure.code, "pack_response_cost_invalid");
        assert_eq!(failure.spend_disposition, SpendDisposition::Reconcile);
        assert_eq!(failure.evidence.operation_id.as_deref(), Some("job-1"));
    }
}
