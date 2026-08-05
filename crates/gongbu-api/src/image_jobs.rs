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
    provider_targets::ProviderTargetConfig,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageJobRequest {
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
    pub prompt: String,
    pub operation_key: String,
    pub spend_auth_token_id: String,
    pub agent_id: Option<String>,
    pub account_id: Option<String>,
    pub amount_cents: i64,
    pub merchant: String,
    pub task_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImageJobResponse {
    pub job_id: String,
    pub provider: String,
    pub model: String,
    pub adapter: String,
    pub provider_config_version: String,
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
    targets: &ProviderTargetConfig,
) -> Result<ImageJobResponse> {
    request.validate(config)?;
    if request.workload_type != "image_generation" {
        return Err(anyhow!(
            "image jobs require workload_type 'image_generation'"
        ));
    }
    let resolved = targets.resolve(
        &request.workload_type,
        &request.provider,
        &request.adapter,
        &request.model,
    )?;
    if request.provider != config.provider
        || request.adapter != config.adapter_kind.label()
        || request.model != config.model
    {
        return Err(anyhow!(
            "requested provider target is not available in this Gongbu process"
        ));
    }
    config.adapter().map_err(|error| {
        let failure = redact_image_provider_error_message(&error.to_string(), config);
        anyhow!("image provider configuration invalid: {failure}")
    })?;
    let provider = request.provider.clone();
    let model = request.model.clone();
    let adapter_name = request.adapter.clone();
    let provider_config_version = resolved.provider_config_version.clone();
    let spend_request = request.spend_request();
    let claim = hubu
        .claim(&ExecutorSpendClaimRequest {
            operation_key: request.operation_key.clone(),
            spend: spend_request,
        })
        .context("claim Hubu spend authorization")?;
    ensure_claim_executable(&claim, &request.operation_key)?;
    if claim.workload_profile != request.workload_type {
        release_after_pre_work_failure(hubu, &claim)?;
        return Err(anyhow!(
            "Hubu claim workload_profile does not match the requested workload_type"
        ));
    }

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
    let artifact_id = artifact_id_from_operation_key(&request.operation_key);
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
        adapter: adapter_name,
        provider_config_version,
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

fn ensure_claim_executable(claim: &ExecutorSpendClaimResponse, operation_key: &str) -> Result<()> {
    if claim.operation_key != operation_key {
        return Err(anyhow!(
            "Hubu claim operation_key does not match the work request"
        ));
    }
    if claim.reconciliation_required {
        return Err(anyhow!(
            "Hubu claim requires reconciliation and is not executable"
        ));
    }
    if claim.status != "claimed" || claim.finalized_at.is_some() || claim.settlement_id.is_some() {
        return Err(anyhow!(
            "Hubu claim is not executable in status '{}'",
            claim.status
        ));
    }
    Ok(())
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

fn artifact_id_from_operation_key(operation_key: &str) -> String {
    let mut id = String::with_capacity(3 + operation_key.len() * 2);
    id.push_str("op-");
    for byte in operation_key.as_bytes() {
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::{artifact_id_from_operation_key, ImageJobRequest};

    #[test]
    fn artifact_ids_preserve_operation_key_uniqueness() {
        let punctuation = artifact_id_from_operation_key("a:b");
        let underscore = artifact_id_from_operation_key("a_b");

        assert_eq!(punctuation, "op-613a62");
        assert_eq!(underscore, "op-615f62");
        assert_ne!(punctuation, underscore);
        assert_eq!(
            artifact_id_from_operation_key("模型:一"),
            artifact_id_from_operation_key("模型:一")
        );
    }

    #[test]
    fn caller_cannot_supply_operator_controlled_provider_fields() {
        let error = serde_json::from_str::<ImageJobRequest>(r#"{
          "workload_type":"image_generation","provider":"vendor","adapter":"http-json","model":"image-v1",
          "prompt":"logo","operation_key":"op","spend_auth_token_id":"ref","agent_id":"agent",
          "amount_cents":1,"merchant":"gongbu.image","endpoint":"https://attacker.example","headers":{"x":"y"}
        }"#).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}
