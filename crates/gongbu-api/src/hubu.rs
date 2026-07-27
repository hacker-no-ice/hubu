use serde::{Deserialize, Serialize};

use crate::simple_http::{self, HttpClientError};

#[derive(Debug, Clone)]
pub struct HubuClient {
    base_url: String,
}

impl HubuClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: trim_trailing_slash(base_url.into()),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn validate(
        &self,
        request: &ExecutorSpendRequest,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        self.post_json("/spend/executor/validate", request)
    }

    pub fn claim(
        &self,
        request: &ExecutorSpendClaimRequest,
    ) -> Result<ExecutorSpendClaimResponse, HttpClientError> {
        self.retry_ambiguous_post("/spend/executor/claim", request)
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
        simple_http::get_json(&url)
    }

    pub fn settle(
        &self,
        request: &ExecutorSpendFinalizationRequest,
        claim_id: &str,
    ) -> Result<ExecutorSpendSettlementResponse, HttpClientError> {
        self.retry_ambiguous_finalization("/spend/executor/settle", request, claim_id)
    }

    pub fn release(
        &self,
        request: &ExecutorSpendFinalizationRequest,
        claim_id: &str,
    ) -> Result<ExecutorSpendClaimResponse, HttpClientError> {
        self.retry_ambiguous_finalization("/spend/executor/release", request, claim_id)
    }

    fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R, HttpClientError>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.base_url, path);
        simple_http::post_json(&url, body)
    }

    fn retry_ambiguous_post<T, R>(&self, path: &str, body: &T) -> Result<R, HttpClientError>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        match self.post_json(path, body) {
            Err(error) if is_ambiguous(&error) => self.post_json(path, body),
            result => result,
        }
    }

    fn retry_ambiguous_finalization<R>(
        &self,
        path: &str,
        request: &ExecutorSpendFinalizationRequest,
        claim_id: &str,
    ) -> Result<R, HttpClientError>
    where
        R: for<'de> Deserialize<'de>,
    {
        match self.post_json(path, request) {
            Err(error) if is_ambiguous(&error) => {
                // Hubu is authoritative. Inspect the durable claim before making
                // the same idempotent finalization request again.
                self.inspect_claim(claim_id)?;
                self.post_json(path, request)
            }
            result => result,
        }
    }
}

fn is_ambiguous(error: &HttpClientError) -> bool {
    matches!(
        error,
        HttpClientError::Io(_) | HttpClientError::Json(_) | HttpClientError::InvalidUrl { .. }
    )
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
    pub task_id: Option<String>,
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
    pub spend_auth_token_id: String,
    pub decision_id: String,
    pub account_id: String,
    pub agent_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub merchant: Option<String>,
    pub task_id: Option<String>,
    pub expires_at: String,
    pub budget_hold: BudgetHold,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutorSpendClaimResponse {
    pub operation_key: String,
    pub claim_id: String,
    pub workload_profile: String,
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
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use serde_json::json;

    use super::*;

    #[test]
    fn claim_retries_same_operation_after_ambiguous_response() {
        let (client, paths) = fake_hubu(vec![None, Some(claim_response("claimed"))]);
        let response = client.claim(&claim_request()).expect("claim retry");

        assert_eq!(response.claim_id, "claim-1");
        assert_eq!(
            paths.lock().expect("paths").clone(),
            vec!["/spend/executor/claim", "/spend/executor/claim"]
        );
    }

    #[test]
    fn settlement_inspects_claim_before_idempotent_retry() {
        let (client, paths) = fake_hubu(vec![
            None,
            Some(claim_response("claimed")),
            Some(json!({
                "operation_key": "platform:op-1",
                "settlement_id": "settlement-1",
                "claim_id": "claim-1",
                "status": "settled",
                "receipt": {},
                "spend": spend_response("settled"),
            })),
        ]);
        let response = client
            .settle(
                &ExecutorSpendFinalizationRequest {
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
                },
                "claim-1",
            )
            .expect("settlement recovery");

        assert_eq!(response.settlement_id, "settlement-1");
        assert_eq!(
            paths.lock().expect("paths").clone(),
            vec![
                "/spend/executor/settle",
                "/spend/executor/claim?claim_id=claim-1",
                "/spend/executor/settle",
            ]
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
                task_id: Some("task-1".to_string()),
            },
        }
    }

    fn claim_response(status: &str) -> serde_json::Value {
        json!({
            "operation_key": "platform:op-1",
            "claim_id": "claim-1",
            "workload_profile": "image_generation",
            "status": status,
            "claimed_at": "2026-07-27T00:00:00Z",
            "claim_expires_at": "2026-07-27T00:15:00Z",
            "finalized_at": null,
            "settlement_id": null,
            "reconciliation_required": false,
            "spend": spend_response(status),
        })
    }

    fn spend_response(status: &str) -> serde_json::Value {
        json!({
            "operation_key": "platform:op-1",
            "spend_auth_token_id": "token-1",
            "decision_id": "decision-1",
            "account_id": "aga_example",
            "agent_id": "agt_example",
            "amount_cents": 500,
            "currency": "usd",
            "merchant": "gongbu.image",
            "task_id": "task-1",
            "expires_at": "2026-07-27T00:05:00Z",
            "budget_hold": {
                "hold_id": "hold-1",
                "budget_id": "budget-1",
                "status": status,
                "amount_cents": 500,
                "consumed_amount_cents": 0,
                "frozen_amount_cents": 500,
                "remaining_amount_cents": 0
            }
        })
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
