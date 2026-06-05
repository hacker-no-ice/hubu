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

    pub fn settle(
        &self,
        request: &ExecutorSpendRequest,
    ) -> Result<ExecutorSpendSettlementResponse, HttpClientError> {
        self.post_json("/spend/executor/settle", request)
    }

    pub fn release(
        &self,
        request: &ExecutorSpendRequest,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        self.post_json("/spend/executor/release", request)
    }

    fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R, HttpClientError>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.base_url, path);
        simple_http::post_json(&url, body)
    }
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
pub struct ExecutorSpendResponse {
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
pub struct ExecutorSpendSettlementResponse {
    pub settlement_id: String,
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
