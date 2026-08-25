use crate::{
    execution::Execution,
    execution_scope::ExecutionScope,
    workflow::{ActivityError, HubuActivities},
};
use serde::{Deserialize, Serialize};

mod transport;

use self::transport as simple_http;
pub use self::transport::HttpClientError;

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

    fn agent_id_for(&self, execution: &Execution) -> Result<String, ActivityError> {
        match self
            .repository
            .get_hubu_authorization_snapshot(&execution.execution_id)
        {
            Ok(authorization)
                if authorization.account_id == execution.account_id
                    && authorization.operation_key == execution.operation_key
                    && authorization.spend_auth_token_id
                        == execution.hubu_token_reference.as_str() =>
            {
                return Ok(authorization.agent_id);
            }
            Ok(_) => {
                return Err(ActivityError::Proven(
                    "persisted_hubu_authorization_identity_mismatch".into(),
                ));
            }
            Err(crate::execution::Error::NotFound) => {}
            Err(_) => {
                return Err(ActivityError::Proven(
                    "persisted_hubu_authorization_unavailable".into(),
                ));
            }
        }
        let claim_id = execution.hubu_claim_id.as_deref().ok_or_else(|| {
            ActivityError::Proven("legacy_finalization_principal_unavailable".into())
        })?;
        let claim = self
            .client
            .inspect_claim(claim_id)
            .map_err(map_activity_error)?;
        if claim.operation_key != execution.operation_key
            || claim.spend.account_id != execution.account_id
            || claim.spend.spend_auth_token_id != execution.hubu_token_reference.as_str()
            || !matches!(
                claim.status.as_str(),
                "claimed" | "active" | "settled" | "released"
            )
        {
            return Err(ActivityError::Proven(
                "legacy_claim_identity_mismatch".into(),
            ));
        }
        Ok(claim.spend.agent_id)
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
                operation_key: execution.operation_key.clone(),
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
        let snapshot: crate::provider_contract::PricingSnapshot =
            serde_json::from_value(execution.pricing_snapshot.clone())
                .map_err(|_| ActivityError::Proven("pricing_snapshot_invalid".into()))?;
        self.client
            .settle(&ExecutorSpendFinalizationRequest {
                agent_id: self.agent_id_for(execution)?,
                operation_key: execution.operation_key.clone(),
                receipt: Some(ProviderReceipt {
                    actual_vendor_cost_cents: amount_minor,
                    provider_request_id: receipt_id.into(),
                    price_model_snapshot: PriceModelSnapshot {
                        provider: execution.provider.clone(),
                        model: execution.model.clone(),
                        unit_price_cents: snapshot.estimated_amount_minor,
                        pricing_unit: "execution".into(),
                        currency: execution.authorization_currency.to_ascii_lowercase(),
                    },
                    artifact_reference: format!("gongbu://execution/{}", execution.execution_id),
                }),
            })
            .map(|settlement| settlement.settlement_id)
            .map_err(map_activity_error)
    }

    fn release(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.client
            .release(&ExecutorSpendFinalizationRequest {
                agent_id: self.agent_id_for(execution)?,
                operation_key: execution.operation_key.clone(),
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
pub struct ExecutorSpendClaimRequest {
    pub operation_key: String,
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
pub struct ExecutorSpendFinalizationRequest {
    pub agent_id: String,
    pub operation_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ProviderReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderReceipt {
    pub actual_vendor_cost_cents: i64,
    pub provider_request_id: String,
    pub price_model_snapshot: PriceModelSnapshot,
    pub artifact_reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PriceModelSnapshot {
    pub provider: String,
    pub model: String,
    pub unit_price_cents: i64,
    pub pricing_unit: String,
    pub currency: String,
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
        let scope = crate::execution_scope::for_target("google", "gemini_image").unwrap();
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
            target: "image_generation/google/gemini_image/gemini-image-v1".into(),
            config_version: "provider-v1".into(),
            workload_type: "image_generation".into(),
            provider: "google".into(),
            adapter: "gemini_image".into(),
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

    fn persisted_execution(
        repository: &crate::execution::Repository,
        agent_id: &str,
    ) -> crate::execution::Execution {
        let params = execution_params();
        let scope = params.execution_scope.clone().unwrap();
        repository
            .create_execution_with_authorization(
                &params,
                &crate::execution::HubuAuthorizationSnapshot {
                    account_id: "account-1".into(),
                    agent_id: agent_id.into(),
                    operation_key: "operation-1".into(),
                    decision_id: "decision-1".into(),
                    spend_auth_token_id: "token-1".into(),
                    amount_minor: 100,
                    currency: "USD".into(),
                    execution_scope: scope,
                    lease_profile: "default".into(),
                    expires_at: "2099-01-01T00:00:00Z".into(),
                    authorization_status: "available".into(),
                    task_id: None,
                    reason: "test".into(),
                },
            )
            .unwrap()
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
    fn claim_request_omits_task_identity_for_hubu_to_resolve() {
        let value = serde_json::to_value(claim_request()).unwrap();
        assert!(value.get("task_id").is_none());
        assert_eq!(value["operation_key"], "platform:op-1");
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
    fn finalization_agent_is_loaded_from_persisted_snapshot_after_restart() {
        let root = tempdir().unwrap();
        let path = root.path().join("gongbu.sqlite3");
        let repository =
            crate::execution::Repository::open(&path, crate::redaction::Redactor::default())
                .unwrap();
        let execution = persisted_execution(&repository, "agent-from-authorization");
        drop(repository);

        let restarted =
            crate::execution::Repository::open(&path, crate::redaction::Redactor::default())
                .unwrap();
        let activities =
            ProductionHubuActivities::new(HubuClient::new("http://127.0.0.1:1"), restarted);
        assert_eq!(
            activities.agent_id_for(&execution).unwrap(),
            "agent-from-authorization"
        );
    }

    #[test]
    fn legacy_finalization_agent_uses_immutable_claim_inspection_without_resolve() {
        let claim = json!({
            "operation_key":"operation-1","claim_id":"claim-1","lease_profile":"default",
            "status":"claimed","claimed_at":"2026-08-25T00:00:01Z",
            "claim_expires_at":"2099-01-01T00:00:00Z","finalized_at":null,
            "settlement_id":null,"reconciliation_required":false,
            "spend":{
                "operation_key":"operation-1","reason":"test","spend_auth_token_id":"token-1",
                "decision_id":"decision-1","account_id":"account-1","agent_id":"claim-agent",
                "amount_cents":100,"currency":"USD","merchant":null,
                "execution_scope":crate::execution_scope::for_target("google", "gemini_image"),
                "task_id":null,"lease_profile":"default","status":"claimed",
                "expires_at":"2099-01-01T00:00:00Z",
                "budget_hold":{"hold_id":"hold-1","budget_id":"budget-1","status":"frozen",
                    "amount_cents":100,"consumed_amount_cents":0,"frozen_amount_cents":100,
                    "remaining_amount_cents":0}
            }
        });
        let (client, paths) = fake_hubu(vec![Some(claim)]);
        let root = tempdir().unwrap();
        let repository = crate::execution::Repository::open(
            root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let mut params = execution_params();
        params.hubu_claim_id = Some("claim-1".into());
        let execution = repository.create_execution(&params).unwrap();
        let activities = ProductionHubuActivities::new(client, repository);

        assert_eq!(activities.agent_id_for(&execution).unwrap(), "claim-agent");
        assert_eq!(
            paths.lock().unwrap().as_slice(),
            ["/spend/executor/claim?claim_id=claim-1"]
        );
    }

    #[test]
    fn ambiguous_settlement_is_returned_without_inspection_or_retry() {
        let (client, paths) = fake_hubu(vec![None]);
        client
            .settle(&ExecutorSpendFinalizationRequest {
                agent_id: "agt_example".to_string(),
                operation_key: "platform:op-1".to_string(),
                receipt: Some(ProviderReceipt {
                    actual_vendor_cost_cents: 500,
                    provider_request_id: "provider-1".to_string(),
                    price_model_snapshot: PriceModelSnapshot {
                        provider: "example".to_string(),
                        model: "image-v1".to_string(),
                        unit_price_cents: 500,
                        pricing_unit: "image".to_string(),
                        currency: "usd".to_string(),
                    },
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
            operation_key: "platform:op-1".to_string(),
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
