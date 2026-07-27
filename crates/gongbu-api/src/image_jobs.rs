use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    hubu::{
        ExecutorSpendClaimRequest, ExecutorSpendClaimResponse, ExecutorSpendFinalizationRequest,
        ExecutorSpendRequest, ExecutorSpendSettlementResponse, HubuClient, PriceModelSnapshot,
        ProviderReceipt,
    },
    image_provider::{
        ensure_image_output_dir_ready, redact_image_provider_error_message, ImageGenerationOutput,
        ImageGenerationRequest, ImageProviderConfig,
    },
};

#[derive(Debug, Deserialize)]
pub struct ImageJobRequest {
    pub prompt: String,
    pub operation_key: String,
    pub spend_auth_token_id: String,
    pub agent_id: Option<String>,
    pub account_id: Option<String>,
    pub amount_cents: i64,
    pub merchant: String,
    pub task_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImageJobResponse {
    pub job_id: String,
    pub provider: String,
    pub model: String,
    pub output_ref: String,
    pub claim: ExecutorSpendClaimResponse,
    pub settlement: ExecutorSpendSettlementResponse,
}

#[derive(Debug, Serialize)]
pub struct ImageJobGuidanceResponse {
    pub provider: String,
    pub model: String,
    pub required_spend: RequiredSpend,
    pub provider_ready: bool,
    pub provider_api_key_configured: bool,
    pub provider_endpoint_configured: bool,
    pub provider_adapter: String,
    pub provider_adapter_supported: bool,
    pub output_dir: String,
    pub missing_configuration: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RequiredSpend {
    pub merchant: String,
    pub amount_cents: i64,
    pub currency: String,
}

pub fn image_job_guidance(config: &ImageProviderConfig) -> ImageJobGuidanceResponse {
    let readiness = config.readiness();
    ImageJobGuidanceResponse {
        provider: config.provider.clone(),
        model: config.model.clone(),
        required_spend: RequiredSpend {
            merchant: config.merchant.clone(),
            amount_cents: config.price_cents,
            currency: "USD".to_string(),
        },
        provider_ready: readiness.ready,
        provider_api_key_configured: config.has_api_key(),
        provider_endpoint_configured: config
            .endpoint
            .as_ref()
            .is_some_and(|endpoint| !endpoint.trim().is_empty()),
        provider_adapter: config.adapter_kind.label().to_string(),
        provider_adapter_supported: config.adapter_kind.is_supported(),
        output_dir: config.output_dir.display().to_string(),
        missing_configuration: readiness.missing_configuration,
    }
}

pub fn create_image_job(
    request: ImageJobRequest,
    hubu: &HubuClient,
    config: &ImageProviderConfig,
) -> Result<ImageJobResponse> {
    request.validate(config)?;
    let (provider, model) = config.resolve(request.provider.clone(), request.model.clone())?;
    let spend_request = request.spend_request();
    let claim = hubu
        .claim(&ExecutorSpendClaimRequest {
            operation_key: request.operation_key.clone(),
            spend: spend_request,
        })
        .context("claim Hubu spend authorization")?;

    let adapter = match config.adapter() {
        Ok(adapter) => adapter,
        Err(error) => {
            release_after_pre_work_failure(hubu, &claim)?;
            let failure = redact_image_provider_error_message(&error.to_string(), config);
            return Err(anyhow!("image provider configuration invalid: {failure}"));
        }
    };

    // The platform operation key is safe to reuse across retries. The
    // authorization token is deliberately excluded from artifact names.
    let artifact_id = safe_artifact_id(&request.operation_key);
    if config.adapter_kind.writes_local_artifact() {
        if let Err(error) = ensure_image_output_dir_ready(&config.output_dir, &artifact_id) {
            release_after_pre_work_failure(hubu, &claim)?;
            let failure = redact_image_provider_error_message(&error.to_string(), config);
            return Err(anyhow!("image provider generation failed: {failure}"));
        }
    }

    let output: ImageGenerationOutput = match adapter.generate(ImageGenerationRequest {
        provider: &provider,
        model: &model,
        prompt: &request.prompt,
        artifact_id: &artifact_id,
    }) {
        Ok(output) => output,
        Err(error) => {
            release_after_pre_work_failure(hubu, &claim)?;
            let failure = redact_image_provider_error_message(&error.to_string(), config);
            return Err(anyhow!("image provider generation failed: {failure}"));
        }
    };

    let settlement = hubu
        .settle(
            &ExecutorSpendFinalizationRequest {
                agent_id: claim.spend.agent_id.clone(),
                operation_key: request.operation_key,
                receipt: Some(ProviderReceipt {
                    actual_vendor_cost_cents: request.amount_cents,
                    provider_request_id: format!("gongbu:{artifact_id}"),
                    price_model_snapshot: PriceModelSnapshot {
                        provider: provider.clone(),
                        model: model.clone(),
                        unit_price_cents: request.amount_cents,
                        pricing_unit: "image".to_string(),
                        currency: "usd".to_string(),
                    },
                    artifact_reference: output.output_ref.clone(),
                }),
            },
            &claim.claim_id,
        )
        .context("settle Hubu spend authorization after image artifact write")?;
    Ok(ImageJobResponse {
        job_id: format!("img-{artifact_id}"),
        provider,
        model,
        output_ref: output.output_ref,
        claim,
        settlement,
    })
}

impl ImageJobRequest {
    fn validate(&self, config: &ImageProviderConfig) -> Result<()> {
        if self.prompt.trim().is_empty() {
            return Err(anyhow!("image prompt cannot be empty"));
        }
        match (&self.agent_id, &self.account_id) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => {
                return Err(anyhow!(
                    "request must include exactly one of agent_id or account_id"
                ))
            }
        }
        if self.spend_auth_token_id.trim().is_empty() {
            return Err(anyhow!("spend_auth_token_id is required"));
        }
        if self.operation_key.trim().is_empty() {
            return Err(anyhow!("operation_key is required"));
        }
        if self.amount_cents != config.price_cents {
            return Err(anyhow!(
                "image job amount_cents must match configured provider price"
            ));
        }
        if self.merchant != config.merchant {
            return Err(anyhow!(
                "image job merchant must match configured provider merchant"
            ));
        }
        Ok(())
    }

    fn spend_request(&self) -> ExecutorSpendRequest {
        ExecutorSpendRequest {
            spend_auth_token_id: self.spend_auth_token_id.clone(),
            agent_id: self.agent_id.clone(),
            account_id: self.account_id.clone(),
            amount_cents: self.amount_cents,
            merchant: Some(self.merchant.clone()),
            task_id: self.task_id.clone(),
        }
    }
}

fn release_after_pre_work_failure(
    hubu: &HubuClient,
    claim: &ExecutorSpendClaimResponse,
) -> Result<()> {
    hubu.release(
        &ExecutorSpendFinalizationRequest {
            agent_id: claim.spend.agent_id.clone(),
            operation_key: claim.operation_key.clone(),
            receipt: None,
        },
        &claim.claim_id,
    )
    .context("release Hubu spend authorization after pre-work failure")?;
    Ok(())
}

fn safe_artifact_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}
