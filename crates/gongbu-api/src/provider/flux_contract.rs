//! The frozen FLUX.2 Pro contract facts that outlive the adapter: target
//! identity and certified output dimensions per size preset. The verified FLUX pack runs on the async engine; these helpers keep
//! the create path, legacy-row recovery and attestation exact.

use super::contract::{ContractError, OutputDimensions, Result};
use serde_json::{json, Value};

pub const PROVIDER_ID: &str = "flux";
pub const ADAPTER_ID: &str = "flux2_api";
pub const MODEL_ID: &str = "flux-2-pro";
pub const MIN_DIMENSION: u32 = 64;
pub const DIMENSION_MULTIPLE: u32 = 16;
pub const MAX_OUTPUT_PIXELS: u64 = 2048 * 2048;
pub const SUPPORTED_PRESETS: &[&str] = &["1k", "2k", "4k"];

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

fn provider_error(code: &str) -> ContractError {
    ContractError::Provider { code: code.into() }
}
