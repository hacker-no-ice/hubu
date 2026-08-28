//! One-call router orchestration for an already separated Hubu/Gongbu flow.
//!
//! This module owns no execution state machine. It records the same normalized
//! authorization and Gongbu continuation used by the primitive tools, wakes the
//! existing durable worker, and observes only the public durable projection.

use std::{thread, time::Duration};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    backend_error_response, error_response, gongbu, hubu, operation_registry, success_response,
    tool_availability, BackendOwner, CapabilitySnapshot, Server, ToolCall, ToolRejection,
    REQUEST_TIMEOUT,
};

pub(crate) const TOOL_NAME: &str = "hubu_submit_governed_execution";

const MAX_INLINE_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_INLINE_ARTIFACT_COUNT: usize = 16;
const MAX_PROCESSED_ARTIFACT_ENTRIES: usize = 64;
const MAX_RESUMABLE_ARTIFACT_IDS: usize = 64;
const RESPONSE_HEADROOM: Duration = Duration::from_secs(1);
const STATUS_POLL_CEILING: Duration = Duration::from_millis(50);
const STATUS_POLL_FLOOR: Duration = Duration::from_millis(10);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GovernedExecutionInput {
    authorization: Value,
    execution: Value,
    #[serde(default = "default_inline_artifact_bytes")]
    max_inline_artifact_bytes: u64,
}

fn default_inline_artifact_bytes() -> u64 {
    MAX_INLINE_ARTIFACT_BYTES
}

#[derive(Clone, Copy, Debug, Default)]
struct GongbuTiming {
    execution_total_ms: Option<u64>,
    provider_interaction_ms: Option<u64>,
    non_provider_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct ArtifactDelivery {
    images: Vec<Value>,
    artifacts: Vec<Value>,
    warnings: Vec<Value>,
    remaining_artifact_ids: Vec<Value>,
    inline_bytes: u64,
}

pub(super) fn backend_availability(
    snapshot: &CapabilitySnapshot,
) -> Result<(), (BackendOwner, ToolRejection)> {
    tool_availability("hubu_authorize_spend", BackendOwner::Hubu, snapshot)
        .map_err(|rejection| (BackendOwner::Hubu, rejection))?;
    tool_availability("gongbu_create_execution", BackendOwner::Gongbu, snapshot)
        .map_err(|rejection| (BackendOwner::Gongbu, rejection))
}

pub(super) fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Submit one governed execution request. Hubu evaluates policy first. If allowed, Gongbu executes and returns the result. If human approval is required, no provider work starts and the request remains resumable by exact redelivery after approval. A definitive denial is terminal; corrected work must be submitted as a new tool call.",
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "required": ["authorization", "execution"],
            "properties": {
                "authorization": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["account_id", "amount_cents", "reason"],
                    "properties": {
                        "account_id": {"type": "string"},
                        "amount_cents": {"type": "integer"},
                        "reason": {"type": "string"},
                        "merchant": {"type": "string"},
                        "execution_scope": hubu::execution_scope_input_schema(),
                        "lease_profile": {"type": "string"}
                    }
                },
                "execution": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "schema_version", "input", "input_schema_version", "workload_type",
                        "provider", "adapter", "model"
                    ],
                    "properties": {
                        "schema_version": {"type": "integer", "const": 2},
                        "input": {"type": "object"},
                        "input_schema_version": {"type": "integer", "minimum": 1},
                        "workload_type": {"type": "string", "minLength": 1},
                        "provider": {"type": "string", "minLength": 1},
                        "adapter": {"type": "string", "minLength": 1},
                        "model": {"type": "string", "minLength": 1}
                    }
                },
                "max_inline_artifact_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_INLINE_ARTIFACT_BYTES,
                    "default": MAX_INLINE_ARTIFACT_BYTES
                }
            }
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true,
            "x_hubu_human_approval": "conditional",
            "x_hubu_client_approval_mode": "auto",
            "x_hubu_runtime_approval": "hubu_policy_needs_approval"
        }
    })
}

pub(super) fn call_tool(
    server: &Server,
    id: Value,
    call: ToolCall,
    started: std::time::Instant,
    deadline: std::time::Instant,
) -> Value {
    if !server.operation_registry_available() {
        return error_response(
            id,
            -32000,
            "governed execution requires an available durable operation registry",
        );
    }
    if let Err((owner, rejection)) = backend_availability(&server.snapshot()) {
        return backend_error_response(id, TOOL_NAME, owner, rejection);
    }
    if operation_registry::validate_durable_request_size(&call.arguments).is_err() {
        return error_response(id, -32602, "Invalid params");
    }
    let input = match serde_json::from_value::<GovernedExecutionInput>(call.arguments.clone()) {
        Ok(input) if (1..=MAX_INLINE_ARTIFACT_BYTES).contains(&input.max_inline_artifact_bytes) => {
            input
        }
        _ => return error_response(id, -32602, "Invalid params"),
    };

    // Validate the complete execution intent before asking Hubu to reserve or
    // authorize spend. The placeholder never crosses a backend boundary.
    let maximum_length_token = "v".repeat(255);
    let validation_arguments =
        match gongbu::governed_execution_arguments(&input.execution, &maximum_length_token) {
            Ok(arguments) => arguments,
            Err(result) => return success_response(id, result),
        };
    if operation_registry::validate_gongbu_request_size(&validation_arguments).is_err() {
        return error_response(id, -32602, "Invalid params");
    }

    let authorization_started = std::time::Instant::now();
    let authorization_response = hubu::call_governed_authorization(
        server,
        id.clone(),
        input.authorization,
        &call.arguments,
        call.meta,
    );
    let hubu_authorization = authorization_started.elapsed();
    if authorization_response.get("error").is_some() {
        return authorization_response;
    }
    let Some(authorization) = authorization_response
        .pointer("/result/structuredContent")
        .cloned()
    else {
        return error_response(id, -32000, "Hubu returned an invalid authorization result");
    };
    let Some(operation_handle) = authorization
        .get("operation_handle")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return error_response(id, -32000, "Hubu returned an invalid authorization result");
    };
    let decision = authorization.get("decision").and_then(Value::as_str);

    if matches!(decision, Some("deny" | "needs_approval")) {
        let status = match server.durable_operation_status(&operation_handle) {
            Ok(status) => status,
            Err(_) => {
                return error_response(
                    id,
                    -32000,
                    "governed authorization has no durable public status",
                )
            }
        };
        let outcome = if decision == Some("deny") {
            "denied"
        } else {
            "approval_required"
        };
        return success_response(
            id,
            composite_result(
                outcome,
                &status,
                &authorization,
                Vec::new(),
                empty_artifact_delivery(input.max_inline_artifact_bytes, "not_started"),
                TimingInput {
                    started,
                    hubu_authorization,
                    execution_wait: Duration::ZERO,
                    artifact_delivery: Duration::ZERO,
                    gongbu: GongbuTiming::default(),
                },
            ),
        );
    }
    if decision != Some("allow") {
        return error_response(
            id,
            -32000,
            "Hubu returned an invalid authorization decision",
        );
    }
    let Some(auth_token_id) = authorization
        .get("auth_token_id")
        .or_else(|| authorization.get("spend_auth_token_id"))
        .and_then(Value::as_str)
    else {
        let status = match server.durable_operation_status(&operation_handle) {
            Ok(status) => status,
            Err(_) => {
                return error_response(
                    id,
                    -32000,
                    &format!(
                        "authorization continuation is unavailable; observe operation handle {operation_handle}"
                    ),
                )
            }
        };
        return success_response(
            id,
            composite_result(
                "failed",
                &status,
                &authorization,
                Vec::new(),
                empty_artifact_delivery(input.max_inline_artifact_bytes, "not_started"),
                TimingInput {
                    started,
                    hubu_authorization,
                    execution_wait: Duration::ZERO,
                    artifact_delivery: Duration::ZERO,
                    gongbu: GongbuTiming::default(),
                },
            ),
        );
    };
    let execution_arguments = match gongbu::governed_execution_arguments(
        &input.execution,
        auth_token_id,
    ) {
        Ok(arguments) => arguments,
        Err(_) => {
            let status = match server.fail_pre_execution_operation(
                    &operation_handle,
                    "authorization_continuation_unavailable",
                ) {
                    Ok(status) => status,
                    Err(_) => {
                        return error_response(
                            id,
                            -32000,
                            &format!(
                                "authorization continuation could not be bound; observe operation handle {operation_handle}"
                            ),
                        )
                    }
                };
            return success_response(
                id,
                composite_result(
                    "failed",
                    &status,
                    &authorization,
                    Vec::new(),
                    empty_artifact_delivery(input.max_inline_artifact_bytes, "not_started"),
                    TimingInput {
                        started,
                        hubu_authorization,
                        execution_wait: Duration::ZERO,
                        artifact_delivery: Duration::ZERO,
                        gongbu: GongbuTiming::default(),
                    },
                ),
            );
        }
    };
    let continuation = match server.resolve_gongbu_continuation(auth_token_id, &execution_arguments)
    {
        Ok(continuation) => continuation,
        Err(_) => {
            let status = match server.durable_operation_status(&operation_handle) {
                Ok(status) if status.terminal() => status,
                Ok(_) => match server.fail_pre_execution_operation(
                    &operation_handle,
                    "execution_intent_binding_failed",
                ) {
                    Ok(status) => status,
                    Err(_) => {
                        return error_response(
                            id,
                            -32000,
                            &format!(
                                "execution intent could not be bound; observe operation handle {operation_handle}"
                            ),
                        )
                    }
                },
                Err(_) => {
                    return error_response(
                        id,
                        -32000,
                        &format!(
                            "execution intent could not be bound; observe operation handle {operation_handle}"
                        ),
                    )
                }
            };
            return success_response(
                id,
                composite_result(
                    "failed",
                    &status,
                    &authorization,
                    Vec::new(),
                    empty_artifact_delivery(input.max_inline_artifact_bytes, "not_started"),
                    TimingInput {
                        started,
                        hubu_authorization,
                        execution_wait: Duration::ZERO,
                        artifact_delivery: Duration::ZERO,
                        gongbu: GongbuTiming::default(),
                    },
                ),
            );
        }
    };
    server.wake_operation_worker();

    let execution_started = std::time::Instant::now();
    let (status, timed_out) = match wait_for_terminal(server, &operation_handle, deadline) {
        Ok(result) => result,
        Err(error) => return error_response(id, -32000, &error),
    };
    let mut execution_wait = execution_started.elapsed();
    if timed_out {
        return success_response(
            id,
            composite_result(
                "in_progress",
                &status,
                &authorization,
                Vec::new(),
                empty_artifact_delivery(input.max_inline_artifact_bytes, "not_started"),
                TimingInput {
                    started,
                    hubu_authorization,
                    execution_wait,
                    artifact_delivery: Duration::ZERO,
                    gongbu: GongbuTiming::default(),
                },
            ),
        );
    }

    let gongbu_timing = status
        .execution_id
        .as_deref()
        .and_then(|execution_id| {
            enough_time_for_request(deadline).then(|| {
                server.backends.gongbu.as_ref().and_then(|client| {
                    gongbu::fetch_durable_execution_observation(client, execution_id, &continuation)
                        .ok()
                        .map(|observation| GongbuTiming {
                            execution_total_ms: observation.execution_total_ms,
                            provider_interaction_ms: observation.provider_interaction_ms,
                            non_provider_ms: observation.non_provider_ms,
                        })
                })
            })
        })
        .flatten()
        .unwrap_or_default();
    execution_wait = execution_started.elapsed();

    if status.state != "succeeded" {
        return success_response(
            id,
            composite_result(
                "failed",
                &status,
                &authorization,
                Vec::new(),
                empty_artifact_delivery(input.max_inline_artifact_bytes, "not_started"),
                TimingInput {
                    started,
                    hubu_authorization,
                    execution_wait,
                    artifact_delivery: Duration::ZERO,
                    gongbu: gongbu_timing,
                },
            ),
        );
    }

    let artifact_started = std::time::Instant::now();
    let artifact_delivery = status.execution_id.as_deref().map_or_else(
        || {
            let mut delivery = ArtifactDelivery::default();
            delivery.warnings.push(json!({
                "code": "execution_artifact_identity_unavailable",
                "message": "Execution succeeded, but artifact identity is not yet available. Observe the existing operation; do not submit a replacement."
            }));
            delivery
        },
        |execution_id| {
            deliver_artifacts(
                server,
                execution_id,
                input.max_inline_artifact_bytes,
                deadline,
            )
        },
    );
    let artifact_elapsed = artifact_started.elapsed();
    let ArtifactDelivery {
        images,
        artifacts,
        warnings,
        remaining_artifact_ids,
        inline_bytes,
    } = artifact_delivery;
    let delivery_status = if warnings.is_empty() {
        "complete"
    } else if artifacts.is_empty() {
        "warning"
    } else {
        "partial"
    };
    let artifact_projection = json!({
        "status": delivery_status,
        "inline_count": artifacts.len(),
        "inline_bytes": inline_bytes,
        "max_inline_bytes": input.max_inline_artifact_bytes,
        "warnings": warnings,
        "remaining_artifact_ids": remaining_artifact_ids,
        "guidance": if delivery_status == "complete" {
            "Inline artifact delivery is complete. Do not submit a replacement execution."
        } else {
            "Execution succeeded. Resume artifact delivery with gongbu_list_artifacts and gongbu_get_artifact for this execution; do not rerun the provider."
        }
    });
    success_response(
        id,
        composite_result(
            "succeeded",
            &status,
            &authorization,
            images,
            (artifacts, artifact_projection),
            TimingInput {
                started,
                hubu_authorization,
                execution_wait,
                artifact_delivery: artifact_elapsed,
                gongbu: gongbu_timing,
            },
        ),
    )
}

fn wait_for_terminal(
    server: &Server,
    operation_handle: &str,
    deadline: std::time::Instant,
) -> Result<(operation_registry::DurableOperationStatus, bool), String> {
    let poll = server
        .operation_tick
        .min(STATUS_POLL_CEILING)
        .max(STATUS_POLL_FLOOR);
    loop {
        let status = server
            .durable_operation_status(operation_handle)
            .map_err(|_| "durable operation status became unavailable".to_owned())?;
        if status.terminal() {
            return Ok((status, false));
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok((status, true));
        }
        thread::sleep(poll.min(deadline.saturating_duration_since(now)));
    }
}

fn enough_time_for_request(deadline: std::time::Instant) -> bool {
    deadline.saturating_duration_since(std::time::Instant::now())
        >= REQUEST_TIMEOUT + RESPONSE_HEADROOM
}

fn deliver_artifacts(
    server: &Server,
    execution_id: &str,
    max_inline_bytes: u64,
    deadline: std::time::Instant,
) -> ArtifactDelivery {
    let mut delivery = ArtifactDelivery::default();
    let Some(client) = server.backends.gongbu.as_ref() else {
        delivery.warnings.push(delivery_warning(
            "artifact_backend_unavailable",
            "Execution succeeded, but artifact delivery is temporarily unavailable.",
            None,
        ));
        return delivery;
    };
    if !enough_time_for_request(deadline) {
        delivery.warnings.push(delivery_warning(
            "artifact_delivery_budget_exhausted",
            "Execution succeeded, but the composite response budget left no time for artifact delivery.",
            None,
        ));
        return delivery;
    }
    let listed = gongbu::call_tool(
        client,
        "gongbu_list_artifacts",
        json!({"execution_id": execution_id}),
        None,
    )
    .result;
    let Some(list) = successful_text_json(&listed) else {
        delivery.warnings.push(delivery_warning(
            "artifact_list_unavailable",
            "Execution succeeded, but its artifact list could not be delivered.",
            None,
        ));
        return delivery;
    };
    let Some(artifacts) = list.get("artifacts").and_then(Value::as_array) else {
        delivery.warnings.push(delivery_warning(
            "artifact_list_invalid",
            "Execution succeeded, but its artifact list was invalid.",
            None,
        ));
        return delivery;
    };
    if list.get("execution_id").and_then(Value::as_str) != Some(execution_id) {
        delivery.warnings.push(delivery_warning(
            "artifact_execution_identity_conflict",
            "Execution succeeded, but the artifact list belonged to a different execution and was not delivered.",
            None,
        ));
        return delivery;
    }

    for artifact in artifacts.iter().take(MAX_PROCESSED_ARTIFACT_ENTRIES) {
        let Some(artifact_id) = bound_artifact_id(artifact, execution_id) else {
            delivery.warnings.push(delivery_warning(
                "artifact_execution_identity_conflict",
                "One artifact had invalid or conflicting execution identity and was not inlined.",
                None,
            ));
            continue;
        };
        let media_type = artifact.get("media_type").and_then(Value::as_str);
        let listed_size = artifact.get("size_bytes").and_then(Value::as_u64);
        if !matches!(media_type, Some("image/png" | "image/jpeg")) || listed_size.is_none() {
            delivery.warnings.push(delivery_warning(
                "artifact_not_inline_eligible",
                "An artifact is not an eligible PNG or JPEG and was not inlined.",
                Some(artifact_id),
            ));
            continue;
        }
        let listed_size = listed_size.expect("validated above");
        if delivery.images.len() >= MAX_INLINE_ARTIFACT_COUNT
            || listed_size > max_inline_bytes.saturating_sub(delivery.inline_bytes)
        {
            record_resumable(&mut delivery, artifact_id);
            delivery.warnings.push(delivery_warning(
                "artifact_inline_limit_reached",
                "Execution succeeded, but an eligible artifact exceeded the composite inline limit.",
                Some(artifact_id),
            ));
            continue;
        }
        if !enough_time_for_request(deadline) {
            record_resumable(&mut delivery, artifact_id);
            delivery.warnings.push(delivery_warning(
                "artifact_delivery_budget_exhausted",
                "Execution succeeded, but the composite response budget expired before all artifacts could be delivered.",
                Some(artifact_id),
            ));
            continue;
        }
        let remaining = max_inline_bytes.saturating_sub(delivery.inline_bytes);
        let fetched = gongbu::fetch_artifact_bounded(
            client,
            artifact_id,
            usize::try_from(remaining).unwrap_or(usize::MAX),
        );
        let Some(image) = successful_image(&fetched, artifact_id, media_type.unwrap()) else {
            record_resumable(&mut delivery, artifact_id);
            delivery.warnings.push(delivery_warning(
                "artifact_retrieval_failed",
                "Execution succeeded, but an eligible artifact could not be delivered.",
                Some(artifact_id),
            ));
            continue;
        };
        let Some(data) = image.get("data").and_then(Value::as_str) else {
            continue;
        };
        let Ok(decoded) = BASE64.decode(data) else {
            record_resumable(&mut delivery, artifact_id);
            delivery.warnings.push(delivery_warning(
                "artifact_retrieval_invalid",
                "Execution succeeded, but an eligible artifact response was invalid.",
                Some(artifact_id),
            ));
            continue;
        };
        let size = u64::try_from(decoded.len()).unwrap_or(u64::MAX);
        if size > max_inline_bytes.saturating_sub(delivery.inline_bytes) {
            record_resumable(&mut delivery, artifact_id);
            delivery.warnings.push(delivery_warning(
                "artifact_inline_limit_reached",
                "Execution succeeded, but an eligible artifact exceeded the composite inline limit.",
                Some(artifact_id),
            ));
            continue;
        }
        let fetched_metadata = successful_text_json(&fetched).unwrap_or_else(|| json!({}));
        delivery.inline_bytes = delivery.inline_bytes.saturating_add(size);
        delivery.images.push(image);
        delivery.artifacts.push(json!({
            "artifact_id": artifact_id,
            "media_type": media_type,
            "size_bytes": size,
            "sha256": fetched_metadata.get("sha256"),
            "metadata": artifact.get("metadata"),
            "created_at": artifact.get("created_at")
        }));
    }
    let omitted = artifacts
        .len()
        .saturating_sub(MAX_PROCESSED_ARTIFACT_ENTRIES);
    if omitted > 0 {
        delivery.warnings.push(json!({
            "code": "artifact_list_truncated",
            "message": "Execution succeeded, but additional artifact entries were omitted from the composite response. Resume with gongbu_list_artifacts.",
            "omitted_count": omitted
        }));
    }
    delivery
}

fn bound_artifact_id<'a>(artifact: &'a Value, execution_id: &str) -> Option<&'a str> {
    if artifact.get("execution_id").and_then(Value::as_str) != Some(execution_id) {
        return None;
    }
    let artifact_id = artifact.get("artifact_id").and_then(Value::as_str)?;
    (artifact_id.len() <= 255
        && !artifact_id.is_empty()
        && artifact_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then_some(artifact_id)
}

fn successful_text_json(result: &Value) -> Option<Value> {
    if result.get("isError").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    result
        .get("content")?
        .as_array()?
        .iter()
        .find(|content| content.get("type").and_then(Value::as_str) == Some("text"))?
        .get("text")?
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
}

fn successful_image(result: &Value, artifact_id: &str, media_type: &str) -> Option<Value> {
    let metadata = successful_text_json(result)?;
    if metadata.get("artifact_id").and_then(Value::as_str) != Some(artifact_id)
        || metadata.get("media_type").and_then(Value::as_str) != Some(media_type)
    {
        return None;
    }
    result
        .get("content")?
        .as_array()?
        .iter()
        .find(|content| {
            content.get("type").and_then(Value::as_str) == Some("image")
                && content.get("mimeType").and_then(Value::as_str) == Some(media_type)
        })
        .cloned()
}

fn record_resumable(delivery: &mut ArtifactDelivery, artifact_id: &str) {
    if delivery.remaining_artifact_ids.len() < MAX_RESUMABLE_ARTIFACT_IDS {
        delivery
            .remaining_artifact_ids
            .push(Value::String(artifact_id.to_owned()));
    }
}

fn delivery_warning(code: &str, message: &str, artifact_id: Option<&str>) -> Value {
    let mut warning = Map::from_iter([
        ("code".into(), Value::String(code.to_owned())),
        ("message".into(), Value::String(message.to_owned())),
    ]);
    if let Some(artifact_id) = artifact_id {
        warning.insert("artifact_id".into(), Value::String(artifact_id.to_owned()));
    }
    Value::Object(warning)
}

fn empty_artifact_delivery(max_inline_bytes: u64, status: &str) -> (Vec<Value>, Value) {
    (
        Vec::new(),
        json!({
            "status": status,
            "inline_count": 0,
            "inline_bytes": 0,
            "max_inline_bytes": max_inline_bytes,
            "warnings": [],
            "remaining_artifact_ids": [],
            "guidance": "No provider artifact delivery was started."
        }),
    )
}

struct TimingInput {
    started: std::time::Instant,
    hubu_authorization: Duration,
    execution_wait: Duration,
    artifact_delivery: Duration,
    gongbu: GongbuTiming,
}

fn composite_result(
    outcome: &str,
    status: &operation_registry::DurableOperationStatus,
    authorization: &Value,
    images: Vec<Value>,
    artifact_delivery: (Vec<Value>, Value),
    timing: TimingInput,
) -> Value {
    let total_ms = millis(timing.started.elapsed());
    let hubu_authorization_ms = millis(timing.hubu_authorization);
    let execution_wait_ms = millis(timing.execution_wait);
    let artifact_delivery_ms = millis(timing.artifact_delivery);
    let router_unattributed_ms = total_ms.saturating_sub(
        hubu_authorization_ms
            .saturating_add(execution_wait_ms)
            .saturating_add(artifact_delivery_ms),
    );
    let summary = format!(
        "total={total_ms}ms; hubu_authorization={hubu_authorization_ms}ms; execution_wait={execution_wait_ms}ms; gongbu_execution={}; provider_interaction={}; gongbu_non_provider={}; artifact_delivery={artifact_delivery_ms}ms; router_unattributed={router_unattributed_ms}ms",
        display_millis(timing.gongbu.execution_total_ms),
        display_millis(timing.gongbu.provider_interaction_ms),
        display_millis(timing.gongbu.non_provider_ms),
    );
    let (artifacts, artifact_delivery) = artifact_delivery;
    let structured = json!({
        "schema_version": 1,
        "outcome": outcome,
        "operation_handle": status.operation_handle,
        "state": status.state,
        "terminal": status.terminal(),
        "replacement_safe": status.state == "authorized",
        "execution_id": status.execution_id,
        "result": super::operation_result_projection(status.result_code.as_deref()),
        "authorization": authorization_projection(authorization),
        "artifacts": artifacts,
        "artifact_delivery": artifact_delivery,
        "timing": {
            "schema_version": 1,
            "scope": "composite_tool_server_observed",
            "total_ms": total_ms,
            "hubu_authorization_ms": hubu_authorization_ms,
            "execution_wait_ms": execution_wait_ms,
            "gongbu_execution_total_ms": timing.gongbu.execution_total_ms,
            "provider_interaction_ms": timing.gongbu.provider_interaction_ms,
            "gongbu_non_provider_ms": timing.gongbu.non_provider_ms,
            "artifact_delivery_ms": artifact_delivery_ms,
            "router_unattributed_ms": router_unattributed_ms,
            "human_approval_wait_ms": Value::Null,
            "summary": summary
        },
        "guidance": outcome_guidance(outcome)
    });
    let mut content = Vec::with_capacity(images.len() + 1);
    content.push(json!({
        "type": "text",
        "text": serde_json::to_string_pretty(&structured)
            .expect("governed execution result serializes")
    }));
    content.extend(images);
    json!({
        "content": content,
        "structuredContent": structured,
        "isError": false
    })
}

fn authorization_projection(authorization: &Value) -> Value {
    let private_values = ["auth_token_id", "spend_auth_token_id", "operation_key"]
        .into_iter()
        .filter_map(|field| authorization.get(field).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let mut projection = Map::new();
    for field in [
        "decision",
        "decision_id",
        "requires_human_approval",
        "approval_reason",
        "approval",
        "authorization_expires_at",
        "reasons",
        "policy_decision",
        "retry_guidance",
    ] {
        if let Some(value) = authorization.get(field) {
            let mut value = value.clone();
            redact_authorization_identity(&mut value, &private_values);
            projection.insert(field.into(), value);
        }
    }
    if projection.get("decision").and_then(Value::as_str) == Some("deny") {
        projection.insert("retry_guidance".into(), hubu::denied_retry_guidance());
    }
    Value::Object(projection)
}

fn redact_authorization_identity(value: &mut Value, private_values: &[&str]) {
    match value {
        Value::Array(values) => {
            for value in values {
                redact_authorization_identity(value, private_values);
            }
        }
        Value::Object(object) => {
            for field in [
                "auth_token_id",
                "spend_auth_token_id",
                "operation_key",
                "operation_handle",
                "task_id",
                "platform",
                "installation_id",
                "invocation_id",
                "call_id",
                "callId",
                "tool_use_id",
                "claudecode/toolUseId",
                "_meta",
                "authorization_header",
                "bearer_token",
                "api_key",
                "credential",
                "credentials",
            ] {
                object.remove(field);
            }
            for value in object.values_mut() {
                redact_authorization_identity(value, private_values);
            }
        }
        Value::String(text) => {
            for private in private_values {
                if text.contains(private) {
                    *text = text.replace(private, "<private authorization redacted>");
                }
            }
        }
        _ => {}
    }
}

fn outcome_guidance(outcome: &str) -> &'static str {
    match outcome {
        "approval_required" => {
            "Human approval is required and no provider work started. Resolve the existing approval, then redeliver this exact composite call with the same identity; do not submit a replacement."
        }
        "in_progress" => {
            "The durable worker continues this execution. Observe operation_handle with hubu_operation_status; do not submit a replacement."
        }
        "succeeded" => {
            "Execution is terminal and succeeded. Artifact delivery warnings may be resumed without rerunning the provider."
        }
        "denied" => hubu::DENIED_OPERATION_GUIDANCE,
        "failed" => {
            "This governed operation is terminal. Do not submit a replacement for this operation."
        }
        _ => "Observe the existing operation_handle; do not submit a replacement.",
    }
}

fn display_millis(value: Option<u64>) -> String {
    value.map_or_else(|| "unavailable".into(), |value| format!("{value}ms"))
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        time::Instant,
    };

    fn composite_arguments() -> Value {
        json!({
            "authorization": {
                "account_id":"account-1",
                "amount_cents":25,
                "reason":"generate an image"
            },
            "execution": {
                "schema_version":2,
                "input":{"prompt":"circle"},
                "input_schema_version":1,
                "workload_type":"image_generation",
                "provider":"fixture",
                "adapter":"fixture",
                "model":"v1"
            }
        })
    }

    #[test]
    fn catalog_contract_is_bounded_and_has_no_continuation_token_input() {
        let definition = tool_definition();
        assert_eq!(definition["name"], TOOL_NAME);
        assert!(
            definition["inputSchema"]["properties"]["execution"]["properties"]
                .get("spend_auth_token_id")
                .is_none()
        );
        assert_eq!(
            definition["inputSchema"]["properties"]["max_inline_artifact_bytes"]["maximum"],
            MAX_INLINE_ARTIFACT_BYTES
        );
    }

    #[test]
    fn terminal_projection_never_exposes_authorization_continuation() {
        let status = operation_registry::DurableOperationStatus {
            operation_handle: format!("hubu:public-operation:v1:{}", "a".repeat(32)),
            state: "failed".into(),
            execution_id: None,
            result_code: Some("authorization_denied".into()),
            updated_at: "now".into(),
        };
        let result = composite_result(
            "denied",
            &status,
            &json!({
                "decision":"deny",
                "auth_token_id":"private-continuation",
                "operation_key":"private-operation",
                "reasons":["merchant_not_allowed: private-continuation"],
                "policy_decision":{
                    "rule":"deny-provider",
                    "auth_token_id":"private-continuation",
                    "operation_key":"private-operation"
                },
                "retry_guidance":{
                    "action":"reuse_operation_key",
                    "operation_key":"private-operation",
                    "message":"reuse this operation key with corrected scope"
                }
            }),
            Vec::new(),
            empty_artifact_delivery(MAX_INLINE_ARTIFACT_BYTES, "not_started"),
            TimingInput {
                started: std::time::Instant::now(),
                hubu_authorization: Duration::ZERO,
                execution_wait: Duration::ZERO,
                artifact_delivery: Duration::ZERO,
                gongbu: GongbuTiming::default(),
            },
        );
        let serialized = result.to_string();
        assert!(!serialized.contains("private-continuation"));
        assert!(!serialized.contains("private-operation"));
        assert_eq!(result["structuredContent"]["outcome"], "denied");
        assert_eq!(result["structuredContent"]["terminal"], true);
        assert_eq!(
            result["structuredContent"]["authorization"]["retry_guidance"]["action"],
            "create_new_operation"
        );
        assert!(result["structuredContent"]["guidance"]
            .as_str()
            .unwrap()
            .contains("new logical operation"));
        assert!(!serialized.contains("reuse_operation_key"));
        assert!(!serialized.contains("reuse this operation key"));
        assert_eq!(
            result["structuredContent"]["authorization"]["reasons"],
            json!(["merchant_not_allowed: <private authorization redacted>"])
        );
        assert!(
            result["structuredContent"]["authorization"]["policy_decision"]
                .get("auth_token_id")
                .is_none()
        );
        assert_eq!(
            result["structuredContent"]["authorization"]["policy_decision"]["rule"],
            "deny-provider"
        );
    }

    #[test]
    fn artifact_failure_preserves_success_outcome_shape() {
        let status = operation_registry::DurableOperationStatus {
            operation_handle: format!("hubu:public-operation:v1:{}", "b".repeat(32)),
            state: "succeeded".into(),
            execution_id: Some("execution-1".into()),
            result_code: Some("execution_succeeded".into()),
            updated_at: "now".into(),
        };
        let result = composite_result(
            "succeeded",
            &status,
            &json!({"decision":"allow"}),
            Vec::new(),
            (
                Vec::new(),
                json!({
                    "status":"warning",
                    "warnings":[{"code":"artifact_retrieval_failed"}]
                }),
            ),
            TimingInput {
                started: std::time::Instant::now(),
                hubu_authorization: Duration::ZERO,
                execution_wait: Duration::ZERO,
                artifact_delivery: Duration::ZERO,
                gongbu: GongbuTiming::default(),
            },
        );
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["outcome"], "succeeded");
        assert_eq!(
            result["structuredContent"]["artifact_delivery"]["status"],
            "warning"
        );
    }

    #[test]
    fn passed_in_start_charges_pre_handler_time_to_router_total() {
        let status = operation_registry::DurableOperationStatus {
            operation_handle: format!("hubu:public-operation:v1:{}", "c".repeat(32)),
            state: "failed".into(),
            execution_id: None,
            result_code: Some("authorization_denied".into()),
            updated_at: "now".into(),
        };
        let result = composite_result(
            "denied",
            &status,
            &json!({"decision":"deny"}),
            Vec::new(),
            empty_artifact_delivery(MAX_INLINE_ARTIFACT_BYTES, "not_started"),
            TimingInput {
                started: std::time::Instant::now() - Duration::from_millis(20),
                hubu_authorization: Duration::ZERO,
                execution_wait: Duration::ZERO,
                artifact_delivery: Duration::ZERO,
                gongbu: GongbuTiming::default(),
            },
        );
        let timing = &result["structuredContent"]["timing"];
        let total_ms = timing["total_ms"].as_u64().unwrap();
        assert!(total_ms >= 20);
        assert_eq!(timing["router_unattributed_ms"], total_ms);
    }

    #[test]
    fn artifact_identity_must_bind_to_the_terminal_execution() {
        let artifact = json!({
            "artifact_id":"artifact-1",
            "execution_id":"execution-1"
        });
        assert_eq!(
            bound_artifact_id(&artifact, "execution-1"),
            Some("artifact-1")
        );
        assert_eq!(bound_artifact_id(&artifact, "execution-2"), None);
        assert_eq!(
            bound_artifact_id(
                &json!({"artifact_id":"../private","execution_id":"execution-1"}),
                "execution-1"
            ),
            None
        );
    }

    #[test]
    fn composite_identity_reuses_one_authorization_and_one_execution_intent() {
        let mut registry = operation_registry::OperationRegistry::open_in_memory().unwrap();
        let identity = operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
            "callId":"composite-replay-1"
        })))
        .unwrap();
        let request = composite_arguments();
        let operation = registry
            .resolve_or_allocate(&identity, TOOL_NAME, &request)
            .unwrap();
        registry
            .mark_dispatch_started(&operation.operation_handle)
            .unwrap();
        let authorized = registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"authorization-1",
                    "operation_handle":operation.operation_handle
                }),
            )
            .unwrap();
        let execution = gongbu::governed_execution_arguments(
            &request["execution"],
            authorized["auth_token_id"].as_str().unwrap(),
        )
        .unwrap();
        let continuation = registry
            .resolve_gongbu_continuation("authorization-1", &execution)
            .unwrap();

        let replay = registry
            .resolve_or_allocate(&identity, TOOL_NAME, &request)
            .unwrap();
        assert_eq!(replay.operation_handle, operation.operation_handle);
        assert_eq!(replay.recorded_result, Some(authorized));
        assert_eq!(continuation.operation_handle, operation.operation_handle);
        let replay_continuation = registry
            .resolve_gongbu_continuation("authorization-1", &execution)
            .unwrap();
        assert_eq!(replay_continuation, continuation);

        let mut changed = request;
        changed["max_inline_artifact_bytes"] = json!(1024);
        assert!(registry
            .resolve_or_allocate(&identity, TOOL_NAME, &changed)
            .unwrap_err()
            .to_string()
            .contains("different operation"));
    }

    #[test]
    fn approval_required_is_resumable_without_an_execution_continuation() {
        let mut registry = operation_registry::OperationRegistry::open_in_memory().unwrap();
        let identity = operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
            "callId":"composite-approval-1"
        })))
        .unwrap();
        let request = composite_arguments();
        let operation = registry
            .resolve_or_allocate(&identity, TOOL_NAME, &request)
            .unwrap();
        registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision":"needs_approval",
                    "decision_id":"decision-1",
                    "approval":{"approval_request_id":"approval-1"}
                }),
            )
            .unwrap();
        let status = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(status.state, "approval_required");
        assert!(!status.terminal());
        assert!(registry
            .resolve_gongbu_continuation(
                "authorization-that-does-not-exist",
                &gongbu::governed_execution_arguments(&request["execution"], "placeholder")
                    .unwrap(),
            )
            .is_err());
    }

    #[test]
    fn restart_recovers_composite_identity_and_bound_continuation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("composite-operations.sqlite3");
        let identity = operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
            "callId":"composite-restart-1"
        })))
        .unwrap();
        let request = composite_arguments();
        let execution =
            gongbu::governed_execution_arguments(&request["execution"], "restart-auth").unwrap();
        let original = {
            let mut registry = operation_registry::OperationRegistry::open(&path).unwrap();
            let operation = registry
                .resolve_or_allocate(&identity, TOOL_NAME, &request)
                .unwrap();
            registry
                .record_authorization_result(
                    &operation.operation_handle,
                    &json!({
                        "decision":"allow",
                        "auth_token_id":"restart-auth",
                        "operation_handle":operation.operation_handle
                    }),
                )
                .unwrap();
            let continuation = registry
                .resolve_gongbu_continuation("restart-auth", &execution)
                .unwrap();
            (operation, continuation)
        };

        let mut reopened = operation_registry::OperationRegistry::open(&path).unwrap();
        let replay = reopened
            .resolve_or_allocate(&identity, TOOL_NAME, &request)
            .unwrap();
        let continuation = reopened
            .resolve_gongbu_continuation("restart-auth", &execution)
            .unwrap();
        assert_eq!(replay.operation_handle, original.0.operation_handle);
        assert!(replay.recorded_result.is_some());
        assert_eq!(continuation, original.1);
        assert_eq!(
            reopened
                .durable_operation_status(&replay.operation_handle)
                .unwrap()
                .state,
            "accepted"
        );
    }

    #[test]
    fn bounded_wait_returns_in_progress_without_replacing_the_durable_operation() {
        let root = tempfile::tempdir().unwrap();
        let server = Server::new(crate::Config {
            operation_state_path: Some(root.path().join("bounded-wait.sqlite3")),
            governed_execution_wait: Duration::from_millis(10),
            ..crate::Config::default()
        })
        .unwrap();
        let identity = operation_registry::NormalizedHarnessIdentity::from_meta(Some(&json!({
            "callId":"bounded-wait-1"
        })))
        .unwrap();
        let request = composite_arguments();
        let operation_handle = {
            let crate::OperationRegistryCapability::Available(registry) =
                server.operation_registry.as_ref()
            else {
                panic!("test registry should be available");
            };
            let mut registry = registry.lock().unwrap();
            let operation = registry
                .resolve_or_allocate(&identity, TOOL_NAME, &request)
                .unwrap();
            registry
                .record_authorization_result(
                    &operation.operation_handle,
                    &json!({"decision":"allow","auth_token_id":"bounded-wait-auth"}),
                )
                .unwrap();
            let execution =
                gongbu::governed_execution_arguments(&request["execution"], "bounded-wait-auth")
                    .unwrap();
            registry
                .resolve_gongbu_continuation("bounded-wait-auth", &execution)
                .unwrap();
            operation.operation_handle
        };
        let (status, timed_out) = wait_for_terminal(
            &server,
            &operation_handle,
            Instant::now() + Duration::from_millis(10),
        )
        .unwrap();
        assert!(timed_out);
        assert!(!status.terminal());
        assert_eq!(status.operation_handle, operation_handle);
        assert_eq!(
            server
                .durable_operation_status(&operation_handle)
                .unwrap()
                .state,
            "accepted"
        );
    }

    fn spawn_http_backend(
        expected_requests: usize,
        handler: impl Fn(&str) -> (&'static str, Vec<u8>) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while captured.lock().unwrap().len() < expected_requests && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(accepted) => accepted,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("mock backend accept failed: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                let request = read_http_request(&mut stream);
                let (content_type, body) = handler(&request);
                captured.lock().unwrap().push(request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (endpoint, requests, handle)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                request.push_str(&line);
                break;
            }
            if let Some(length) = line
                .to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
            {
                content_length = length;
            }
            request.push_str(&line);
        }
        let mut body = vec![0_u8; content_length];
        reader.read_exact(&mut body).unwrap();
        request.push_str(&String::from_utf8(body).unwrap());
        request
    }

    fn use_available_capability_snapshot(server: &mut Server) {
        server.use_capability_snapshot_for_test = true;
        let mut snapshot = server
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.hubu.state = crate::capability::BackendState::Available;
        snapshot.hubu.reason_code = None;
        snapshot.gongbu.state = crate::capability::BackendState::Available;
        snapshot.gongbu.reason_code = None;
    }

    #[test]
    fn stdio_worker_completes_one_call_and_replay_never_repeats_provider_mutation() {
        let operation_key = Arc::new(Mutex::new(None::<String>));
        let hubu_operation_key = Arc::clone(&operation_key);
        let hubu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (hubu_endpoint, hubu_requests, hubu_handle) = spawn_http_backend(3, move |request| {
            let first_line = request.lines().next().unwrap_or_default();
            if first_line.starts_with("GET /health ") {
                ("application/json", br#"{"status":"ok"}"#.to_vec())
            } else if first_line.starts_with("GET /version ") {
                (
                    "application/json",
                    serde_json::to_vec(&hubu_version).unwrap(),
                )
            } else if first_line.starts_with("POST /spend/authorize ") {
                let body: Value =
                    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap_or_default())
                        .unwrap();
                *hubu_operation_key.lock().unwrap() = body
                    .get("operation_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                (
                    "application/json",
                    br#"{"decision":"allow","auth_token_id":"authorization-1"}"#.to_vec(),
                )
            } else {
                panic!("unexpected Hubu request: {first_line}");
            }
        });

        let gongbu_operation_key = Arc::clone(&operation_key);
        let gongbu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "api_schema_version": 2,
            "mcp_protocol_version": crate::MCP_PROTOCOL_VERSION,
            "mcp_schema_version": 2,
            "hubu_executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (gongbu_endpoint, gongbu_requests, gongbu_handle) =
            spawn_http_backend(11, move |request| {
                let first_line = request.lines().next().unwrap_or_default();
                if first_line.starts_with("GET /livez ") {
                    ("application/json", br#"{"status":"live"}"#.to_vec())
                } else if first_line.starts_with("GET /readyz ") {
                    ("application/json", br#"{"status":"ready"}"#.to_vec())
                } else if first_line.starts_with("GET /version ") {
                    (
                        "application/json",
                        serde_json::to_vec(&gongbu_version).unwrap(),
                    )
                } else if first_line.starts_with("POST /v2/executions ") {
                    let key = gongbu_operation_key.lock().unwrap().clone().unwrap();
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "schema_version":2,
                            "execution_id":"execution-1",
                            "operation_key":key,
                            "status":"pending",
                            "outcome":null,
                            "failure":null,
                            "authorization":{"amount_minor":25,"currency":"USD"},
                            "created_at":"2026-08-27T00:00:00Z",
                            "updated_at":"2026-08-27T00:00:00Z",
                            "started_at":null,
                            "completed_at":null
                        }))
                        .unwrap(),
                    )
                } else if first_line.starts_with("GET /v1/executions/execution-1/artifacts ") {
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "schema_version":1,
                            "execution_id":"execution-1",
                            "artifacts":[{
                                "artifact_id":"artifact-1",
                                "execution_id":"execution-1",
                                "kind":"image",
                                "media_type":"image/png",
                                "size_bytes":9,
                                "sha256":"sha256:fixture",
                                "metadata":{"width":1,"height":1},
                                "metadata_schema_version":1,
                                "created_at":"2026-08-27T00:00:00Z"
                            }]
                        }))
                        .unwrap(),
                    )
                } else if first_line.starts_with("GET /v1/artifacts/artifact-1 ") {
                    ("image/png", b"png-bytes".to_vec())
                } else if first_line.starts_with("GET /v1/executions/execution-1 ") {
                    let key = gongbu_operation_key.lock().unwrap().clone().unwrap();
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "schema_version":1,
                            "execution_id":"execution-1",
                            "operation_key":key,
                            "status":"succeeded",
                            "outcome":"completed",
                            "failure":null,
                            "authorization":{"amount_minor":25,"currency":"USD"},
                            "created_at":"2026-08-27T00:00:00Z",
                            "updated_at":"2026-08-27T00:00:00.004Z",
                            "started_at":"2026-08-27T00:00:00Z",
                            "completed_at":"2026-08-27T00:00:00.004Z",
                            "timing":{
                                "schema_version":1,
                                "scope":"gongbu_execution",
                                "execution_total_ms":4,
                                "provider_interaction_ms":3,
                                "non_provider_ms":1
                            }
                        }))
                        .unwrap(),
                    )
                } else {
                    panic!("unexpected Gongbu request: {first_line}");
                }
            });

        let state = tempfile::tempdir().unwrap();
        let mut server = Server::new(crate::Config {
            hubu: Some(
                crate::BackendConfig::new(BackendOwner::Hubu, &hubu_endpoint, "hubu-secret")
                    .unwrap(),
            ),
            gongbu: Some(
                crate::BackendConfig::new(BackendOwner::Gongbu, &gongbu_endpoint, "gongbu-secret")
                    .unwrap(),
            ),
            operation_state_path: Some(state.path().join("operations.sqlite3")),
            operation_tick: Duration::from_millis(10),
            governed_execution_wait: Duration::from_secs(10),
            ..crate::Config::default()
        })
        .unwrap();
        use_available_capability_snapshot(&mut server);
        let params = json!({
            "name":TOOL_NAME,
            "arguments":composite_arguments(),
            "_meta":{"callId":"stdio-composite-replay"}
        });
        let first = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params});
        let second = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":params});
        let input = format!("{first}\n{second}\n");
        let mut output = Vec::new();
        server
            .run(std::io::Cursor::new(input.into_bytes()), &mut output)
            .unwrap();

        hubu_handle.join().unwrap();
        gongbu_handle.join().unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        for response in &responses {
            let timing = &response["result"]["structuredContent"]["timing"];
            assert_eq!(
                response["result"]["structuredContent"]["outcome"],
                "succeeded"
            );
            assert_eq!(timing["provider_interaction_ms"], 3);
            assert_eq!(
                timing["total_ms"].as_u64().unwrap(),
                timing["hubu_authorization_ms"].as_u64().unwrap()
                    + timing["execution_wait_ms"].as_u64().unwrap()
                    + timing["artifact_delivery_ms"].as_u64().unwrap()
                    + timing["router_unattributed_ms"].as_u64().unwrap()
            );
            assert_eq!(
                timing["gongbu_execution_total_ms"].as_u64().unwrap(),
                timing["provider_interaction_ms"].as_u64().unwrap()
                    + timing["gongbu_non_provider_ms"].as_u64().unwrap()
            );
            assert!(timing["human_approval_wait_ms"].is_null());
            assert_eq!(
                response["result"]["structuredContent"]["artifact_delivery"]["inline_bytes"],
                9
            );
            assert_eq!(response["result"]["content"][1]["type"], "image");
        }
        let hubu_requests = hubu_requests.lock().unwrap();
        assert_eq!(
            hubu_requests
                .iter()
                .filter(|request| request.starts_with("POST /spend/authorize "))
                .count(),
            1
        );
        let gongbu_requests = gongbu_requests.lock().unwrap();
        assert_eq!(
            gongbu_requests
                .iter()
                .filter(|request| request.starts_with("POST /v2/executions "))
                .count(),
            1
        );
        let private_key = operation_key.lock().unwrap().clone().unwrap();
        assert!(!responses
            .iter()
            .any(|response| response.to_string().contains(&private_key)));
    }

    #[test]
    fn timed_out_composite_returns_in_progress_while_worker_finishes_same_execution() {
        let operation_key = Arc::new(Mutex::new(None::<String>));
        let hubu_operation_key = Arc::clone(&operation_key);
        let hubu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (hubu_endpoint, hubu_requests, hubu_handle) = spawn_http_backend(3, move |request| {
            let first_line = request.lines().next().unwrap_or_default();
            if first_line.starts_with("GET /health ") {
                ("application/json", br#"{"status":"ok"}"#.to_vec())
            } else if first_line.starts_with("GET /version ") {
                (
                    "application/json",
                    serde_json::to_vec(&hubu_version).unwrap(),
                )
            } else if first_line.starts_with("POST /spend/authorize ") {
                let body: Value =
                    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap_or_default())
                        .unwrap();
                *hubu_operation_key.lock().unwrap() = body
                    .get("operation_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                (
                    "application/json",
                    br#"{"decision":"allow","auth_token_id":"timeout-authorization"}"#.to_vec(),
                )
            } else {
                panic!("unexpected Hubu request: {first_line}");
            }
        });

        let gongbu_operation_key = Arc::clone(&operation_key);
        let gongbu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "api_schema_version": 2,
            "mcp_protocol_version": crate::MCP_PROTOCOL_VERSION,
            "mcp_schema_version": 2,
            "hubu_executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (gongbu_endpoint, gongbu_requests, gongbu_handle) =
            spawn_http_backend(5, move |request| {
                let first_line = request.lines().next().unwrap_or_default();
                if first_line.starts_with("GET /livez ") {
                    ("application/json", br#"{"status":"live"}"#.to_vec())
                } else if first_line.starts_with("GET /readyz ") {
                    ("application/json", br#"{"status":"ready"}"#.to_vec())
                } else if first_line.starts_with("GET /version ") {
                    (
                        "application/json",
                        serde_json::to_vec(&gongbu_version).unwrap(),
                    )
                } else if first_line.starts_with("POST /v2/executions ") {
                    let key = gongbu_operation_key.lock().unwrap().clone().unwrap();
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "schema_version":2,
                            "execution_id":"timeout-execution",
                            "operation_key":key,
                            "status":"pending",
                            "outcome":null,
                            "failure":null,
                            "authorization":{"amount_minor":25,"currency":"USD"},
                            "created_at":"2026-08-27T00:00:00Z",
                            "updated_at":"2026-08-27T00:00:00Z",
                            "started_at":null,
                            "completed_at":null
                        }))
                        .unwrap(),
                    )
                } else if first_line.starts_with("GET /v1/executions/timeout-execution ") {
                    let key = gongbu_operation_key.lock().unwrap().clone().unwrap();
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "schema_version":1,
                            "execution_id":"timeout-execution",
                            "operation_key":key,
                            "status":"succeeded",
                            "outcome":"completed",
                            "failure":null,
                            "authorization":{"amount_minor":25,"currency":"USD"},
                            "created_at":"2026-08-27T00:00:00Z",
                            "updated_at":"2026-08-27T00:00:00.004Z",
                            "started_at":"2026-08-27T00:00:00Z",
                            "completed_at":"2026-08-27T00:00:00.004Z"
                        }))
                        .unwrap(),
                    )
                } else {
                    panic!("unexpected Gongbu request: {first_line}");
                }
            });

        let state = tempfile::tempdir().unwrap();
        let mut server = Server::new(crate::Config {
            hubu: Some(
                crate::BackendConfig::new(BackendOwner::Hubu, &hubu_endpoint, "hubu-secret")
                    .unwrap(),
            ),
            gongbu: Some(
                crate::BackendConfig::new(BackendOwner::Gongbu, &gongbu_endpoint, "gongbu-secret")
                    .unwrap(),
            ),
            operation_state_path: Some(state.path().join("timeout-operations.sqlite3")),
            operation_tick: Duration::from_millis(100),
            governed_execution_wait: Duration::from_millis(15),
            ..crate::Config::default()
        })
        .unwrap();
        use_available_capability_snapshot(&mut server);

        let params = json!({
            "name":TOOL_NAME,
            "arguments":composite_arguments(),
            "_meta":{"callId":"timeout-composite-replay"}
        });
        let first = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params});
        let second = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":params});
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (reader, _) = listener.accept().unwrap();
        let input_handle = thread::spawn(move || {
            writeln!(writer, "{first}").unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(250));
            writeln!(writer, "{second}").unwrap();
            writer.flush().unwrap();
        });
        let mut output = Vec::new();
        server.run(BufReader::new(reader), &mut output).unwrap();
        input_handle.join().unwrap();
        hubu_handle.join().unwrap();
        gongbu_handle.join().unwrap();

        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(
            responses[0]["result"]["structuredContent"]["outcome"],
            "in_progress"
        );
        assert_eq!(
            responses[0]["result"]["structuredContent"]["replacement_safe"],
            false
        );
        assert_eq!(
            responses[1]["result"]["structuredContent"]["outcome"],
            "succeeded"
        );
        assert_eq!(
            responses[0]["result"]["structuredContent"]["operation_handle"],
            responses[1]["result"]["structuredContent"]["operation_handle"]
        );
        assert_eq!(
            hubu_requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.starts_with("POST /spend/authorize "))
                .count(),
            1
        );
        let gongbu_requests = gongbu_requests.lock().unwrap();
        assert_eq!(
            gongbu_requests
                .iter()
                .filter(|request| request.starts_with("POST /v2/executions "))
                .count(),
            1
        );
        assert_eq!(
            gongbu_requests
                .iter()
                .filter(|request| request.starts_with("GET /v1/executions/timeout-execution "))
                .count(),
            1
        );
    }

    #[test]
    fn terminal_pre_execution_outcomes_start_zero_gongbu_execution_requests() {
        let hubu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (hubu_endpoint, hubu_requests, hubu_handle) = spawn_http_backend(6, move |request| {
            let first_line = request.lines().next().unwrap_or_default();
            if first_line.starts_with("GET /health ") {
                ("application/json", br#"{"status":"ok"}"#.to_vec())
            } else if first_line.starts_with("GET /version ") {
                (
                    "application/json",
                    serde_json::to_vec(&hubu_version).unwrap(),
                )
            } else if first_line.starts_with("POST /spend/authorize ") {
                let body: Value =
                    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap_or_default())
                        .unwrap();
                if body["reason"] == "missing continuation" {
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "decision":"allow",
                            "decision_id":"decision-missing-continuation"
                        }))
                        .unwrap(),
                    )
                } else if body["reason"] == "deny execution" {
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "decision":"deny",
                            "decision_id":"decision-denied",
                            "reasons":["policy_limit_exceeded"],
                            "policy_decision":{"rule":"daily-limit"},
                            "retry_guidance":{
                                "action":"reuse_operation_key",
                                "operation_key":body["operation_key"],
                                "message":"reuse this operation key with corrected scope"
                            }
                        }))
                        .unwrap(),
                    )
                } else if body["reason"] == "expired continuation" {
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "decision":"allow",
                            "decision_id":"decision-expired-continuation",
                            "auth_token_id":"expired-authorization",
                            "authorization_expires_at":"2020-01-01T00:00:00Z"
                        }))
                        .unwrap(),
                    )
                } else {
                    (
                        "application/json",
                        serde_json::to_vec(&json!({
                            "decision":"needs_approval",
                            "decision_id":"decision-approval",
                            "reasons":["human_review_required"],
                            "approval":{"approval_request_id":"approval-1"}
                        }))
                        .unwrap(),
                    )
                }
            } else {
                panic!("unexpected Hubu request: {first_line}");
            }
        });
        let gongbu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "api_schema_version": 2,
            "mcp_protocol_version": crate::MCP_PROTOCOL_VERSION,
            "mcp_schema_version": 2,
            "hubu_executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (gongbu_endpoint, gongbu_requests, gongbu_handle) =
            spawn_http_backend(3, move |request| {
                let first_line = request.lines().next().unwrap_or_default();
                if first_line.starts_with("GET /livez ") {
                    ("application/json", br#"{"status":"live"}"#.to_vec())
                } else if first_line.starts_with("GET /readyz ") {
                    ("application/json", br#"{"status":"ready"}"#.to_vec())
                } else if first_line.starts_with("GET /version ") {
                    (
                        "application/json",
                        serde_json::to_vec(&gongbu_version).unwrap(),
                    )
                } else {
                    panic!("approval-required path called Gongbu: {first_line}");
                }
            });
        let state = tempfile::tempdir().unwrap();
        let mut server = Server::new(crate::Config {
            hubu: Some(
                crate::BackendConfig::new(BackendOwner::Hubu, &hubu_endpoint, "hubu-secret")
                    .unwrap(),
            ),
            gongbu: Some(
                crate::BackendConfig::new(BackendOwner::Gongbu, &gongbu_endpoint, "gongbu-secret")
                    .unwrap(),
            ),
            operation_state_path: Some(state.path().join("approval-operations.sqlite3")),
            operation_tick: Duration::from_millis(10),
            governed_execution_wait: Duration::from_secs(10),
            ..crate::Config::default()
        })
        .unwrap();
        use_available_capability_snapshot(&mut server);
        let approval_request = json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"tools/call",
            "params":{
                "name":TOOL_NAME,
                "arguments":composite_arguments(),
                "_meta":{"callId":"approval-composite"}
            }
        });
        let mut missing_continuation_arguments = composite_arguments();
        missing_continuation_arguments["authorization"]["reason"] = json!("missing continuation");
        let missing_continuation_request = json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":TOOL_NAME,
                "arguments":missing_continuation_arguments,
                "_meta":{"callId":"missing-continuation-composite"}
            }
        });
        let mut denied_arguments = composite_arguments();
        denied_arguments["authorization"]["reason"] = json!("deny execution");
        let denied_request = json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":TOOL_NAME,
                "arguments":denied_arguments,
                "_meta":{"callId":"denied-composite"}
            }
        });
        let mut expired_continuation_arguments = composite_arguments();
        expired_continuation_arguments["authorization"]["reason"] = json!("expired continuation");
        let expired_continuation_request = json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":TOOL_NAME,
                "arguments":expired_continuation_arguments,
                "_meta":{"callId":"expired-continuation-composite"}
            }
        });
        let mut output = Vec::new();
        server
            .run(
                std::io::Cursor::new(
                    format!(
                        "{approval_request}\n{missing_continuation_request}\n{denied_request}\n{expired_continuation_request}\n"
                    )
                    .into_bytes(),
                ),
                &mut output,
            )
            .unwrap();
        hubu_handle.join().unwrap();
        gongbu_handle.join().unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 4);
        let approval = &responses[0];
        assert_eq!(
            approval["result"]["structuredContent"]["outcome"],
            "approval_required"
        );
        assert_eq!(approval["result"]["structuredContent"]["terminal"], false);
        assert_eq!(
            approval["result"]["structuredContent"]["authorization"]["reasons"],
            json!(["human_review_required"])
        );
        let missing_continuation = &responses[1];
        assert_eq!(
            missing_continuation["result"]["structuredContent"]["outcome"],
            "failed"
        );
        assert_eq!(
            missing_continuation["result"]["structuredContent"]["terminal"],
            true
        );
        assert_eq!(
            missing_continuation["result"]["structuredContent"]["result"]["code"],
            "authorization_continuation_unavailable"
        );
        assert!(
            missing_continuation["result"]["structuredContent"]["operation_handle"]
                .as_str()
                .is_some_and(|handle| handle.starts_with("hubu:public-operation:v1:"))
        );
        let denied = &responses[2];
        assert_eq!(denied["result"]["structuredContent"]["outcome"], "denied");
        assert_eq!(denied["result"]["structuredContent"]["terminal"], true);
        assert_eq!(
            denied["result"]["structuredContent"]["authorization"]["retry_guidance"]["action"],
            "create_new_operation"
        );
        assert!(denied["result"]["structuredContent"]["guidance"]
            .as_str()
            .unwrap()
            .contains("new logical operation"));
        assert!(!denied.to_string().contains("reuse_operation_key"));
        assert_eq!(
            denied["result"]["structuredContent"]["authorization"]["reasons"],
            json!(["policy_limit_exceeded"])
        );
        assert_eq!(
            denied["result"]["structuredContent"]["authorization"]["policy_decision"]["rule"],
            "daily-limit"
        );
        let expired_continuation = &responses[3];
        assert_eq!(
            expired_continuation["result"]["structuredContent"]["outcome"],
            "failed"
        );
        assert_eq!(
            expired_continuation["result"]["structuredContent"]["terminal"],
            true
        );
        assert_eq!(
            expired_continuation["result"]["structuredContent"]["result"]["code"],
            "authorization_continuation_unavailable"
        );
        assert!(
            expired_continuation["result"]["structuredContent"]["operation_handle"]
                .as_str()
                .is_some_and(|handle| handle.starts_with("hubu:public-operation:v1:"))
        );
        assert_eq!(
            hubu_requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.starts_with("POST /spend/authorize "))
                .count(),
            4
        );
        assert!(gongbu_requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.starts_with("POST /v2/executions ")
                && !request.starts_with("GET /v1/executions/")));
    }

    #[test]
    fn invalid_or_oversized_execution_is_rejected_before_hubu_authorization() {
        let hubu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (hubu_endpoint, hubu_requests, hubu_handle) = spawn_http_backend(2, move |request| {
            let first_line = request.lines().next().unwrap_or_default();
            if first_line.starts_with("GET /health ") {
                ("application/json", br#"{"status":"ok"}"#.to_vec())
            } else if first_line.starts_with("GET /version ") {
                (
                    "application/json",
                    serde_json::to_vec(&hubu_version).unwrap(),
                )
            } else {
                panic!("invalid execution request reached Hubu: {first_line}");
            }
        });
        let gongbu_version = json!({
            "product_version": crate::product_version(),
            "source_commit": crate::source_commit(),
            "api_schema_version": 2,
            "mcp_protocol_version": crate::MCP_PROTOCOL_VERSION,
            "mcp_schema_version": 2,
            "hubu_executor_contract": crate::EXECUTOR_CONTRACT_VERSION
        });
        let (gongbu_endpoint, _gongbu_requests, gongbu_handle) =
            spawn_http_backend(3, move |request| {
                let first_line = request.lines().next().unwrap_or_default();
                if first_line.starts_with("GET /livez ") {
                    ("application/json", br#"{"status":"live"}"#.to_vec())
                } else if first_line.starts_with("GET /readyz ") {
                    ("application/json", br#"{"status":"ready"}"#.to_vec())
                } else if first_line.starts_with("GET /version ") {
                    (
                        "application/json",
                        serde_json::to_vec(&gongbu_version).unwrap(),
                    )
                } else {
                    panic!("invalid execution request reached Gongbu: {first_line}");
                }
            });
        let state = tempfile::tempdir().unwrap();
        let mut server = Server::new(crate::Config {
            hubu: Some(
                crate::BackendConfig::new(BackendOwner::Hubu, &hubu_endpoint, "hubu-secret")
                    .unwrap(),
            ),
            gongbu: Some(
                crate::BackendConfig::new(BackendOwner::Gongbu, &gongbu_endpoint, "gongbu-secret")
                    .unwrap(),
            ),
            operation_state_path: Some(state.path().join("oversized-operations.sqlite3")),
            ..crate::Config::default()
        })
        .unwrap();
        use_available_capability_snapshot(&mut server);
        let mut cases = Vec::new();
        let mut oversized = composite_arguments();
        oversized["execution"]["input"]["prompt"] = "x".repeat(1024 * 1024 + 1024).into();
        cases.push(oversized);
        let mut oversized_authorization = composite_arguments();
        oversized_authorization["authorization"]["reason"] = "x".repeat(1024 * 1024 + 1024).into();
        cases.push(oversized_authorization);
        let mut worst_case_token_boundary = composite_arguments();
        worst_case_token_boundary["execution"]["input"]["prompt"] = json!("");
        let one_byte_token_arguments =
            gongbu::governed_execution_arguments(&worst_case_token_boundary["execution"], "v")
                .unwrap();
        let fixed_request_bytes = serde_json::to_string(&one_byte_token_arguments)
            .unwrap()
            .len();
        let prompt_bytes = (1024_usize * 1024)
            .checked_sub(fixed_request_bytes)
            .unwrap();
        worst_case_token_boundary["execution"]["input"]["prompt"] = "x".repeat(prompt_bytes).into();
        let short_token_arguments =
            gongbu::governed_execution_arguments(&worst_case_token_boundary["execution"], "v")
                .unwrap();
        let maximum_token_arguments = gongbu::governed_execution_arguments(
            &worst_case_token_boundary["execution"],
            &"v".repeat(255),
        )
        .unwrap();
        assert!(operation_registry::validate_gongbu_request_size(&short_token_arguments).is_ok());
        assert!(
            operation_registry::validate_gongbu_request_size(&maximum_token_arguments).is_err()
        );
        cases.push(worst_case_token_boundary);
        let mut wrong_schema = composite_arguments();
        wrong_schema["execution"]["schema_version"] = json!(1);
        cases.push(wrong_schema);
        let mut non_object_input = composite_arguments();
        non_object_input["execution"]["input"] = json!("not-an-object");
        cases.push(non_object_input);
        let mut invalid_input_schema = composite_arguments();
        invalid_input_schema["execution"]["input_schema_version"] = json!(0);
        cases.push(invalid_input_schema);
        for field in ["workload_type", "provider", "adapter", "model"] {
            let mut empty_target = composite_arguments();
            empty_target["execution"][field] = json!("");
            cases.push(empty_target);
        }
        let input = cases
            .into_iter()
            .enumerate()
            .map(|(index, arguments)| {
                json!({
                    "jsonrpc":"2.0",
                    "id":index + 1,
                    "method":"tools/call",
                    "params":{
                        "name":TOOL_NAME,
                        "arguments":arguments,
                        "_meta":{"callId":format!("invalid-composite-{index}")}
                    }
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut output = Vec::new();
        server
            .run(
                std::io::Cursor::new(format!("{input}\n").into_bytes()),
                &mut output,
            )
            .unwrap();
        hubu_handle.join().unwrap();
        gongbu_handle.join().unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 10);
        for response in &responses[..3] {
            assert_eq!(response["error"]["code"], -32602);
        }
        for response in &responses[3..] {
            assert_eq!(response["result"]["isError"], true);
        }
        assert!(hubu_requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.starts_with("POST /spend/authorize ")));
    }

    #[test]
    fn composite_artifact_fetch_enforces_remaining_raw_byte_budget() {
        let (endpoint, _requests, handle) = spawn_http_backend(1, |request| {
            assert!(request.starts_with("GET /v1/artifacts/artifact-1 "));
            ("image/png", b"0123456789".to_vec())
        });
        let client = crate::BackendClients::new(crate::Config {
            gongbu: Some(
                crate::BackendConfig::new(BackendOwner::Gongbu, endpoint, "gongbu-secret").unwrap(),
            ),
            ..crate::Config::default()
        })
        .unwrap()
        .gongbu
        .unwrap();
        let result = gongbu::fetch_artifact_bounded(&client, "artifact-1", 8);
        handle.join().unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid_artifact"));
    }
}
