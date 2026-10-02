use crate::{
    execution::Execution,
    execution_scope::ExecutionScope,
    workflow::{ActivityError, HubuActivities},
};
use serde::{Deserialize, Serialize};

mod transport;

use self::transport as simple_http;
pub use self::transport::HttpClientError;

const CREDENTIAL_CHECK_PATH: &str = "/agents?operational_probe=gongbu_credential_check";

#[derive(Clone)]
pub struct HubuClient {
    base_url: String,
    bearer_token: Option<BearerToken>,
}

#[derive(Clone)]
struct BearerToken(Vec<u8>);

impl Drop for BearerToken {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl std::fmt::Debug for HubuClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HubuClient")
            .field("base_url", &self.base_url)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl HubuClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: trim_trailing_slash(base_url.into()),
            bearer_token: None,
        }
    }

    pub fn with_bearer_token(mut self, token: impl Into<Vec<u8>>) -> Self {
        self.bearer_token = Some(BearerToken(token.into()));
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn health(&self) -> Result<serde_json::Value, HttpClientError> {
        self.get_json("/health")
    }

    pub fn version(&self) -> Result<HubuVersion, HttpClientError> {
        self.get_json("/version")
    }

    /// Verify that the configured bearer reaches a protected Hubu route
    /// without performing an execution or mutating Hubu state.
    pub fn check_credential(&self) -> Result<serde_json::Value, HttpClientError> {
        self.get_json(CREDENTIAL_CHECK_PATH)
    }

    pub fn validate(
        &self,
        request: &ExecutorSpendRequest,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        self.post_json("/spend/executor/validate", request)
    }

    pub fn resolve(
        &self,
        request: &ExecutorSpendResolveRequest,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        self.post_json("/spend/executor/resolve", request)
    }

    pub fn claim(
        &self,
        request: &ExecutorSpendClaimRequest,
    ) -> Result<ExecutorSpendClaimResponse, HttpClientError> {
        self.post_json("/spend/executor/claim", request)
    }

    pub fn inspect_claim(
        &self,
        claim_id: &str,
    ) -> Result<ExecutorSpendClaimResponse, HttpClientError> {
        let url = format!(
            "{}/spend/executor/claim?claim_id={}",
            self.base_url,
            percent_encode_query(claim_id)
        );
        match self.bearer_token.as_ref().map(|token| token.0.as_slice()) {
            Some(token) => simple_http::get_json_authenticated(&url, Some(token)),
            None => simple_http::get_json(&url),
        }
    }

    pub fn settle(
        &self,
        request: &ExecutorSpendFinalizationRequest,
    ) -> Result<ExecutorSpendSettlementResponse, HttpClientError> {
        self.post_json("/spend/executor/settle", request)
    }

    pub fn release(
        &self,
        request: &ExecutorSpendFinalizationRequest,
    ) -> Result<ExecutorSpendClaimResponse, HttpClientError> {
        self.post_json("/spend/executor/release", request)
    }

    fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R, HttpClientError>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.base_url, path);
        match self.bearer_token.as_ref().map(|token| token.0.as_slice()) {
            Some(token) => simple_http::post_json_authenticated(&url, body, Some(token)),
            None => simple_http::post_json(&url, body),
        }
    }

    fn get_json<R>(&self, path: &str) -> Result<R, HttpClientError>
    where
        R: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.base_url, path);
        match self.bearer_token.as_ref().map(|token| token.0.as_slice()) {
            Some(token) => simple_http::get_json_authenticated(&url, Some(token)),
            None => simple_http::get_json(&url),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HubuVersion {
    pub product_version: String,
    pub executor_contract: String,
    #[serde(default)]
    pub source_commit: Option<String>,
}

/// Production Hubu activity bridge. It only connects to the operator-provided
/// Hubu endpoint; it contains no installation, provisioning, or lifecycle code.
pub struct ProductionHubuActivities {
    client: HubuClient,
    repository: crate::execution::Repository,
}

pub trait SpendAuthorizationResolver {
    fn resolve_authorization(
        &self,
        spend_auth_token_id: &str,
    ) -> Result<ExecutorSpendResponse, HttpClientError>;
}

impl SpendAuthorizationResolver for HubuClient {
    fn resolve_authorization(
        &self,
        spend_auth_token_id: &str,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        self.resolve(&ExecutorSpendResolveRequest {
            spend_auth_token_id: spend_auth_token_id.into(),
        })
    }
}

impl SpendAuthorizationResolver for ProductionHubuActivities {
    fn resolve_authorization(
        &self,
        spend_auth_token_id: &str,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        self.client.resolve_authorization(spend_auth_token_id)
    }
}

impl ProductionHubuActivities {
    pub fn new(client: HubuClient, repository: crate::execution::Repository) -> Self {
        Self { client, repository }
    }

    pub(crate) fn spend(&self, execution: &Execution) -> ExecutorSpendRequest {
        let (merchant, execution_scope) = match &execution.execution_scope {
            Some(scope) => (None, Some(scope.clone())),
            None => (Some("gongbu.execution".into()), None),
        };
        ExecutorSpendRequest {
            spend_auth_token_id: execution.hubu_token_reference.as_str().into(),
            agent_id: None,
            account_id: Some(execution.account_id.clone()),
            amount_cents: execution.authorized_minor,
            merchant,
            execution_scope,
            // Hubu owns task correlation in the authorization snapshot. Gongbu
            // omits the untrusted duplicate and lets Hubu return the stored value.
            task_id: None,
        }
    }
}

impl ProductionHubuActivities {
    /// The Hubu claim to finalize. An execution that never recorded its
    /// claim, such as after a lost claim response, recovers it by replaying
    /// the identical claim: Hubu returns the existing claim, including its
    /// terminal state, without claiming twice.
    fn finalization_claim_id(&self, execution: &Execution) -> Result<String, ActivityError> {
        match execution.hubu_claim_id.as_deref() {
            Some(claim_id) => Ok(claim_id.to_owned()),
            None => self.claim(execution),
        }
    }
}

impl HubuActivities for ProductionHubuActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.client
            .validate(&self.spend(execution))
            .map(|_| ())
            .map_err(map_activity_error)
    }

    fn claim(&self, execution: &Execution) -> Result<String, ActivityError> {
        self.client
            .claim(&ExecutorSpendClaimRequest {
                spend: self.spend(execution),
            })
            .map(|claim| claim.claim_id)
            .map_err(map_activity_error)
    }

    fn validate_claim(&self, execution: &Execution) -> Result<(), ActivityError> {
        let claim_id = execution
            .hubu_claim_id
            .as_deref()
            .ok_or_else(|| ActivityError::Proven("hubu_claim_missing".into()))?;
        let claim = self
            .client
            .inspect_claim(claim_id)
            .map_err(map_activity_error)?;
        if matches!(claim.status.as_str(), "claimed" | "active")
            && claim.operation_key == execution.operation_key
            && claim.spend.account_id == execution.account_id
        {
            Ok(())
        } else {
            Err(ActivityError::Proven("hubu_claim_not_active".into()))
        }
    }

    fn settle(
        &self,
        execution: &Execution,
        receipt_id: &str,
        amount_minor: i64,
    ) -> Result<String, ActivityError> {
        let receipt = self
            .repository
            .get_receipt_for_execution(&execution.execution_id)
            .map_err(|_| ActivityError::Proven("gongbu_receipt_missing".into()))?;
        if receipt.receipt_id != receipt_id || receipt.settlement_minor != amount_minor {
            return Err(ActivityError::Proven("gongbu_receipt_mismatch".into()));
        }
        self.client
            .settle(&ExecutorSpendFinalizationRequest {
                claim_id: self.finalization_claim_id(execution)?,
                receipt: Some(ProviderReceipt {
                    actual_vendor_cost: receipt.actual_vendor_cost,
                    provider_request_id: receipt.provider_request_id,
                    price_model_snapshot: receipt.price_model_snapshot,
                    artifact_reference: format!("gongbu://execution/{}", execution.execution_id),
                }),
            })
            .map(|settlement| settlement.settlement_id)
            .map_err(map_activity_error)
    }

    fn release(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.client
            .release(&ExecutorSpendFinalizationRequest {
                claim_id: self.finalization_claim_id(execution)?,
                receipt: None,
            })
            .map(|_| ())
            .map_err(map_activity_error)
    }
}

fn map_activity_error(error: HttpClientError) -> ActivityError {
    match error {
        HttpClientError::Status { status, .. } if (400..500).contains(&status) => {
            ActivityError::Proven("hubu_request_rejected".into())
        }
        _ => ActivityError::Ambiguous("hubu_transport_ambiguous".into()),
    }
}

#[cfg(test)]
mod rejection_tests {
    use super::*;

    #[test]
    fn request_level_hubu_rejections_are_proven_and_redacted() {
        for (status, body) in [
            (401, "token=secret-value"),
            (403, "authorization scope account-private"),
            (410, "expired bearer credential"),
            (422, "provider rejected private prompt"),
            (429, "rate-limit account-private"),
        ] {
            assert_eq!(
                map_activity_error(HttpClientError::Status {
                    status,
                    body: body.into(),
                }),
                ActivityError::Proven("hubu_request_rejected".into())
            );
        }
    }

    /// Gongbu consumes the executor-neutral Hubu conformance corpus: every
    /// retry decision Hubu can return must map to the activity class the
    /// corpus assigns, so Gongbu never retries a rejected request as if it
    /// were ambiguous, or abandons an ambiguous one as if it were rejected.
    #[test]
    fn hubu_conformance_retry_decisions_map_to_gongbu_activity_classes() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/hubu-executor-conformance-v4.3.json"
        ))
        .expect("parse Hubu executor conformance corpus");
        assert_eq!(corpus["protocol_version"], "hubu-spend-executor-v4.3");
        let decisions = corpus["retry_decisions"]
            .as_object()
            .expect("retry decision table");
        let mut checked = 0;
        for (name, decision) in decisions {
            let class = decision["gongbu_activity_class"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: missing gongbu_activity_class"));
            let error = match (&decision["observed"], class) {
                (_, "not_applicable") => continue,
                (serde_json::Value::Null, _) => HttpClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "response lost",
                )),
                (observed, _) => HttpClientError::Status {
                    status: observed["status"].as_u64().unwrap() as u16,
                    body: observed["error_contains"].as_str().unwrap_or("").into(),
                },
            };
            let expected = match class {
                "proven" => ActivityError::Proven("hubu_request_rejected".into()),
                "ambiguous" => ActivityError::Ambiguous("hubu_transport_ambiguous".into()),
                other => panic!("{name}: unknown Gongbu activity class {other}"),
            };
            assert_eq!(map_activity_error(error), expected, "retry decision {name}");
            checked += 1;
        }
        assert!(checked >= 10, "corpus retry decisions were not exercised");
    }

    #[test]
    fn dependency_transport_loss_remains_ambiguous() {
        let error = HttpClientError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "endpoint unavailable",
        ));
        assert_eq!(
            map_activity_error(error),
            ActivityError::Ambiguous("hubu_transport_ambiguous".into())
        );
    }
}

fn percent_encode_query(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn trim_trailing_slash(mut value: String) -> String {
    while value.ends_with('/') {
        value.pop();
    }
    value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutorSpendRequest {
    pub spend_auth_token_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub amount_cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merchant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_scope: Option<ExecutionScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutorSpendResolveRequest {
    pub spend_auth_token_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// v4.4 claim: the authorization token plus Gongbu's account, amount and
/// scope assertions. Hubu derives the operation key from the stored decision.
pub struct ExecutorSpendClaimRequest {
    #[serde(flatten)]
    pub spend: ExecutorSpendRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutorSpendResponse {
    pub operation_key: String,
    pub reason: String,
    pub spend_auth_token_id: String,
    pub decision_id: String,
    pub account_id: String,
    pub agent_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub merchant: Option<String>,
    pub execution_scope: Option<ExecutionScope>,
    pub task_id: Option<String>,
    pub lease_profile: String,
    pub status: String,
    pub expires_at: String,
    pub budget_hold: BudgetHold,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutorSpendClaimResponse {
    pub operation_key: String,
    pub claim_id: String,
    pub lease_profile: String,
    pub status: String,
    pub claimed_at: String,
    pub claim_expires_at: String,
    pub finalized_at: Option<String>,
    pub settlement_id: Option<String>,
    pub reconciliation_required: bool,
    pub spend: ExecutorSpendResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// v4.4 settle/release, identified by the Hubu claim.
pub struct ExecutorSpendFinalizationRequest {
    pub claim_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ProviderReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderReceipt {
    pub actual_vendor_cost: crate::provider_contract::ActualVendorCost,
    pub provider_request_id: String,
    pub price_model_snapshot: serde_json::Value,
    pub artifact_reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutorSpendSettlementResponse {
    pub operation_key: String,
    pub settlement_id: String,
    pub claim_id: String,
    pub status: String,
    pub receipt: serde_json::Value,
    pub spend: ExecutorSpendResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetHold {
    pub hold_id: String,
    pub budget_id: String,
    pub status: String,
    pub amount_cents: i64,
    pub consumed_amount_cents: i64,
    pub frozen_amount_cents: i64,
    pub remaining_amount_cents: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;
    use tempfile::tempdir;

    fn execution_params() -> crate::execution::CreateExecutionParams {
        let scope = crate::execution_scope::for_target("google", "gemini_developer_image").unwrap();
        crate::execution::CreateExecutionParams {
            account_id: "account-1".into(),
            operation_key: "operation-1".into(),
            hubu_authorization_id: "token-1".into(),
            hubu_claim_id: None,
            hubu_token_reference: crate::execution::HubuTokenReference::new("token-1").unwrap(),
            authorized_minor: 100,
            authorization_currency: "USD".into(),
            normalized_input: json!({"prompt":"cat","image_count":1}),
            input_hash: "sha256:input".into(),
            input_schema_version: 1,
            target: "image_generation/google/gemini_developer_image/gemini-image-v1".into(),
            config_version: "provider-v1".into(),
            workload_type: "image_generation".into(),
            provider: "google".into(),
            adapter: "gemini_developer_image".into(),
            model: "gemini-image-v1".into(),
            provider_config_version: "provider-v1".into(),
            provider_config_digest: format!("sha256:{}", "a".repeat(64)),
            pricing_snapshot: json!({
                "schema_version":2,"provider":"google","model":"gemini-image-v1",
                "catalog_version":"prices-v2","catalog_digest":format!("sha256:{}", "b".repeat(64)),
                "pricing_rule_id":"image","components":[{"unit":"image","rate_numerator_minor":100,"rate_denominator":1,"quantity":1}],
                "exact_estimate_numerator":"100","exact_estimate_denominator":"1",
                "estimated_amount_minor":100,"currency":"USD"
            }),
            pricing_schema_version: 2,
            execution_scope: Some(scope.clone()),
            created_at: "2026-08-25T00:00:00Z".into(),
        }
    }

    #[test]
    fn ambiguous_claim_is_returned_without_retry() {
        let (client, paths) = fake_hubu(vec![None]);
        client
            .claim(&claim_request())
            .expect_err("ambiguous claim must reach the durable workflow");
        assert_eq!(
            paths.lock().expect("paths").clone(),
            vec!["/spend/executor/claim"]
        );
    }

    #[test]
    fn claim_request_is_token_identified_and_omits_hubu_derived_identity() {
        let value = serde_json::to_value(claim_request()).unwrap();
        assert_eq!(value["spend_auth_token_id"], "token-1");
        assert!(value.get("task_id").is_none());
        assert!(value.get("operation_key").is_none());
    }

    #[test]
    fn settlement_wire_preserves_exact_cost_and_complete_frozen_snapshot() {
        let frozen = execution_params().pricing_snapshot;
        let request = ExecutorSpendFinalizationRequest {
            claim_id: "claim-1".into(),
            receipt: Some(ProviderReceipt {
                actual_vendor_cost: crate::provider_contract::ActualVendorCost::new(1, 4, "USD")
                    .unwrap(),
                provider_request_id: "provider-request-1".into(),
                price_model_snapshot: frozen.clone(),
                artifact_reference: "gongbu://execution/execution-1".into(),
            }),
        };
        let wire = serde_json::to_value(request).unwrap();
        assert_eq!(wire["claim_id"], "claim-1");
        assert!(wire.get("agent_id").is_none() && wire.get("operation_key").is_none());
        assert_eq!(
            wire["receipt"]["actual_vendor_cost"],
            json!({"amount":1,"scale":4,"currency":"USD"})
        );
        assert!(wire["receipt"].get("actual_vendor_cost_cents").is_none());
        assert_eq!(wire["receipt"]["price_model_snapshot"], frozen);
        assert_eq!(
            wire["receipt"]["price_model_snapshot"]["components"][0]["quantity"],
            1
        );
    }

    #[test]
    fn resolver_returns_authorization_without_a_configured_agent_binding() {
        let (client, _) = fake_hubu(vec![Some(json!({
            "operation_key":"op-1",
            "reason":"test",
            "spend_auth_token_id":"token-1",
            "decision_id":"decision-1",
            "account_id":"account-1",
            "agent_id":"another-agent",
            "amount_cents":100,
            "currency":"usd",
            "merchant":null,
            "execution_scope":null,
            "task_id":null,
            "lease_profile":"default",
            "status":"available",
            "expires_at":"2099-01-01T00:00:00Z",
            "budget_hold":{
                "hold_id":"hold-1","budget_id":"budget-1","status":"frozen",
                "amount_cents":100,"consumed_amount_cents":0,
                "frozen_amount_cents":100,"remaining_amount_cents":0
            }
        }))]);
        let root = tempdir().unwrap();
        let repository = crate::execution::Repository::open(
            root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let activities = ProductionHubuActivities::new(client, repository);
        let authorization = activities.resolve_authorization("token-1").unwrap();
        assert_eq!(authorization.agent_id, "another-agent");
    }

    #[test]
    fn migrated_lost_response_retries_the_exact_legacy_receipt_payload() {
        let root = tempdir().unwrap();
        let path = root.path().join("legacy-lost-response.sqlite3");
        let legacy = rusqlite::Connection::open(&path).unwrap();
        legacy
            .execute_batch(include_str!(
                "../../../../fixtures/gongbu-pre-authorization-snapshot.sql"
            ))
            .unwrap();
        legacy
            .execute(
                "UPDATE executions SET hubu_claim_id='legacy-claim' WHERE execution_id='legacy-reconciliation'",
                [],
            )
            .unwrap();
        legacy.execute("INSERT INTO provider_attempts(provider_attempt_id,execution_id,provider,provider_request_id,outcome,usage_json,usage_schema_version,provider_amount_minor,provider_currency,started_at,transmission_started_at,completed_at) VALUES('legacy-attempt','legacy-reconciliation','example','provider-before-upgrade','succeeded','{\"images\":1}',1,7,'usd','2026-08-05T20:00:10Z','2026-08-05T20:00:10Z','2026-08-05T20:00:20Z')", []).unwrap();
        legacy.execute("INSERT INTO receipts(receipt_id,execution_id,provider_attempt_id,settlement_minor,currency,pricing_catalog_version,created_at,transmission_started_at) VALUES('legacy-receipt','legacy-reconciliation','legacy-attempt',7,'usd','prices-v2','2026-08-05T20:00:30Z','2026-08-05T20:00:31Z')", []).unwrap();
        drop(legacy);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut settlement, _) = listener.accept().unwrap();
            let mut raw = String::new();
            settlement.read_to_string(&mut raw).unwrap();
            assert!(raw.starts_with("POST /spend/executor/settle "));
            serde_json::from_str::<serde_json::Value>(raw.split_once("\r\n\r\n").unwrap().1)
                .unwrap()
        });

        let repository =
            crate::execution::Repository::open(&path, crate::redaction::Redactor::default())
                .unwrap();
        let execution = repository.get_execution("legacy-reconciliation").unwrap();
        let activities =
            ProductionHubuActivities::new(HubuClient::new(format!("http://{address}")), repository);
        activities
            .settle(&execution, "legacy-receipt", 7)
            .expect_err("the fixture drops the retried settlement response");
        let wire = server.join().unwrap();
        // The pre-snapshot execution finalizes by its persisted token: no
        // principal lookup or claim inspection precedes the settlement.
        assert_eq!(wire["claim_id"], "legacy-claim");
        assert!(wire.get("agent_id").is_none() && wire.get("operation_key").is_none());
        assert_eq!(
            wire["receipt"],
            json!({
                "actual_vendor_cost":{"amount":7,"scale":2,"currency":"USD"},
                "provider_request_id":"legacy-receipt",
                "price_model_snapshot":{
                    "provider":"example","model":"image-v1","unit_price_cents":100,
                    "pricing_unit":"execution","currency":"usd"
                },
                "artifact_reference":"gongbu://execution/legacy-reconciliation"
            })
        );
    }

    #[test]
    fn ambiguous_settlement_is_returned_without_inspection_or_retry() {
        let (client, paths) = fake_hubu(vec![None]);
        client
            .settle(&ExecutorSpendFinalizationRequest {
                claim_id: "claim-1".to_string(),
                receipt: Some(ProviderReceipt {
                    actual_vendor_cost:
                        crate::provider_contract::ActualVendorCost::new(500, 2, "USD").unwrap(),
                    provider_request_id: "provider-1".to_string(),
                    price_model_snapshot: json!({
                        "schema_version":2,"provider":"example","model":"image-v1",
                        "catalog_version":"prices-v2","catalog_digest":format!("sha256:{}", "b".repeat(64)),
                        "pricing_rule_id":"image","components":[{"unit":"image","rate_numerator_minor":500,"rate_denominator":1,"quantity":1}],
                        "exact_estimate_numerator":"500","exact_estimate_denominator":"1",
                        "estimated_amount_minor":500,"currency":"USD"
                    }),
                    artifact_reference: "artifact://image-1".to_string(),
                }),
            })
            .expect_err("ambiguous settlement must reach the durable workflow");
        assert_eq!(
            paths.lock().expect("paths").clone(),
            vec!["/spend/executor/settle"]
        );
    }

    fn claim_request() -> ExecutorSpendClaimRequest {
        ExecutorSpendClaimRequest {
            spend: ExecutorSpendRequest {
                spend_auth_token_id: "token-1".to_string(),
                agent_id: Some("agt_example".to_string()),
                account_id: None,
                amount_cents: 500,
                merchant: Some("gongbu.image".to_string()),
                execution_scope: None,
                task_id: None,
            },
        }
    }

    type RecordedRequests = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    /// Fake Hubu that records each request's path and JSON body and drops
    /// the connection without answering.
    fn recording_hubu(requests: usize) -> (HubuClient, RecordedRequests) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind recording Hubu");
        let addr = listener.local_addr().expect("recording Hubu address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let thread_seen = Arc::clone(&seen);
        thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut raw = String::new();
                stream.read_to_string(&mut raw).expect("read request");
                let path = raw
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .expect("request path")
                    .to_string();
                let body = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1)
                    .expect("JSON request body");
                thread_seen.lock().expect("seen").push((path, body));
            }
        });
        (HubuClient::new(format!("http://{addr}")), seen)
    }

    #[test]
    fn production_activities_claim_by_token_and_finalize_by_claim_id() {
        let (client, seen) = recording_hubu(3);
        let root = tempdir().unwrap();
        let repository = crate::execution::Repository::open(
            root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let unrecorded = repository.create_execution(&execution_params()).unwrap();
        let mut params = execution_params();
        params.hubu_claim_id = Some("claim-1".into());
        params.hubu_token_reference = crate::execution::HubuTokenReference::new("token-2").unwrap();
        params.hubu_authorization_id = "token-2".into();
        params.operation_key = "operation-2".into();
        let recorded = repository.create_execution(&params).unwrap();
        let activities = ProductionHubuActivities::new(client, repository);
        activities
            .claim(&unrecorded)
            .expect_err("the recording Hubu drops the claim response");
        // A lost claim response is recovered by replaying the claim, so the
        // dropped replay stops the release before it is sent.
        activities
            .release(&unrecorded)
            .expect_err("the recording Hubu drops the claim replay");
        activities
            .release(&recorded)
            .expect_err("the recording Hubu drops the release response");

        let seen = seen.lock().unwrap().clone();
        let paths: Vec<_> = seen.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/spend/executor/claim",
                "/spend/executor/claim",
                "/spend/executor/release"
            ]
        );
        for (path, body) in &seen {
            for private in ["operation_key", "agent_id", "executor_execution_id"] {
                assert!(body.get(private).is_none(), "{path} sent {private}: {body}");
            }
        }
        assert_eq!(seen[0].1["spend_auth_token_id"], "token-1");
        assert_eq!(seen[0].1["account_id"], "account-1");
        assert_eq!(seen[1].1, seen[0].1, "claim replay is identical");
        assert_eq!(seen[2].1, json!({"claim_id": "claim-1"}));
    }

    fn fake_hubu(
        responses: Vec<Option<serde_json::Value>>,
    ) -> (HubuClient, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Hubu");
        let addr = listener.local_addr().expect("fake Hubu address");
        let paths = Arc::new(Mutex::new(Vec::new()));
        let thread_paths = Arc::clone(&paths);
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut raw = String::new();
                stream.read_to_string(&mut raw).expect("read request");
                let path = raw
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .expect("request path")
                    .to_string();
                thread_paths.lock().expect("paths").push(path);
                if let Some(body) = response {
                    let body = body.to_string();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .expect("write response");
                }
            }
        });
        (HubuClient::new(format!("http://{addr}")), paths)
    }
}
