//! Owner-only agent identity relabeling (HUB-250).
//!
//! A rename keeps the stable `agt_...` ID and every record keyed by it, derives
//! the relabeled identity payload's fingerprint with the unchanged v1
//! canonicalization, and records both fingerprints as aliases so old and new
//! clients resolve to the same agent. The route requires the human approval
//! capability and is intentionally absent from the unified MCP route allowlist.
use super::*;
use hubu_core::registration::{
    AgentIdentityAlias, AgentIdentityRevision, IdentityFieldChange, RegistrationError,
    RenameAgentRequest,
};

pub(super) const AGENT_RENAME_ROUTE: &str = "/agents/rename";
pub(super) const AGENT_IDENTITY_HISTORY_ROUTE: &str = "/agents/history";
pub(super) const STALE_AGENT_IDENTITY_WARNING: &str = "stale_agent_identity";
const MAX_AGENT_NAME_CHARS: usize = 128;
const MAX_RENAME_REASON_CHARS: usize = 1000;

/// Only labels are accepted. Owner, agent kind, and version fields are not
/// part of the request so they cannot be edited; unknown fields are rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameAgentHttpRequest {
    agent_id: String,
    name: String,
    reason: String,
}

#[derive(Debug, Serialize)]
pub(super) struct RegistrationWarningHttpResponse {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) submitted_identity_fingerprint: String,
    pub(super) current_identity_fingerprint: String,
    pub(super) current_display_name: String,
}

pub(super) fn stale_identity_warning(
    agent_pub_id: &str,
    submitted_fingerprint: &str,
    current_fingerprint: &str,
    current_display_name: &str,
) -> RegistrationWarningHttpResponse {
    RegistrationWarningHttpResponse {
        code: STALE_AGENT_IDENTITY_WARNING,
        message: format!(
            "identity fingerprint resolved through a pre-rename alias of {agent_pub_id}; \
             the owner renamed this agent to `{current_display_name}`. Update the client's \
             agent name so it registers with the current identity."
        ),
        submitted_identity_fingerprint: submitted_fingerprint.to_string(),
        current_identity_fingerprint: current_fingerprint.to_string(),
        current_display_name: current_display_name.to_string(),
    }
}

pub(super) fn rename_agent_http(
    body: &str,
    approval_capability: Option<&str>,
    state: &ServerState,
) -> HttpResponse {
    if let Err(error) = authenticate_approval_capability(approval_capability, state) {
        return rename_error(
            403,
            &format!("agent rename is human-owner only: {error}"),
            "agent_rename_requires_human_owner",
        );
    }
    match rename_agent(body, state) {
        Ok(body) => HttpResponse { status: 200, body },
        Err(RenameFailure::Http(response)) => response,
        Err(RenameFailure::Internal(error)) => {
            log_event(
                "error",
                "agent_rename_failed",
                json!({ "error": error.to_string() }),
            );
            rename_error(
                500,
                "agent rename could not be completed",
                "agent_rename_storage_error",
            )
        }
    }
}

enum RenameFailure {
    Http(HttpResponse),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for RenameFailure {
    fn from(error: E) -> Self {
        RenameFailure::Internal(error.into())
    }
}

fn bad_request(message: impl Into<String>) -> RenameFailure {
    RenameFailure::Http(rename_error(
        400,
        &message.into(),
        "agent_rename_invalid_request",
    ))
}

fn rename_error(status: u16, message: &str, error_code: &str) -> HttpResponse {
    HttpResponse {
        status,
        body: json!({ "error": message, "error_code": error_code }),
    }
}

fn validate_label(value: &str, field: &str, max_chars: usize) -> Result<String, RenameFailure> {
    let value = value.trim();
    if value.is_empty() {
        return Err(bad_request(format!("{field} must not be empty")));
    }
    if value.chars().count() > max_chars {
        return Err(bad_request(format!(
            "{field} must be at most {max_chars} characters"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(bad_request(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(value.to_string())
}

fn rename_agent(body: &str, state: &ServerState) -> Result<Value, RenameFailure> {
    let request: RenameAgentHttpRequest = serde_json::from_str(body).map_err(|error| {
        bad_request(format!(
            "agent rename accepts only agent_id, name, and reason: {error}"
        ))
    })?;
    let new_name = validate_label(&request.name, "name", MAX_AGENT_NAME_CHARS)?;
    let reason = validate_label(&request.reason, "reason", MAX_RENAME_REASON_CHARS)?;
    let user = authenticated_user(state)?;

    // Hold the registration lock for the whole read-derive-write sequence so
    // the expected fingerprint cannot change underneath this request.
    let mut registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    let agent = registration
        .agent_id_for_pub_id(&request.agent_id)?
        .map(|agent_id| registration.agent_for_id(&agent_id))
        .transpose()?
        .flatten()
        .filter(|agent| agent.owner_user_id == user.id)
        .ok_or_else(|| {
            RenameFailure::Http(rename_error(404, "agent not found", "agent_not_found"))
        })?;

    let current_payload = match registration.identity_payload_for_agent(&agent.id)? {
        Some(payload) => payload,
        None => legacy_identity_payload(&agent.display_name, &user.pub_id, &agent.fingerprint)
            .ok_or_else(|| {
                RenameFailure::Http(rename_error(
                    409,
                    "the agent's original identity payload is unavailable, so the renamed fingerprint cannot be derived",
                    "agent_rename_identity_payload_unavailable",
                ))
            })?,
    };
    let previous_agent_name = current_payload
        .get("agent_name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let reserved_previous_name = previous_agent_name
        .clone()
        .unwrap_or_else(|| agent.display_name.clone());
    let mut new_payload = current_payload.clone();
    new_payload["agent_name"] = json!(new_name);
    let new_fingerprint = fingerprint_payload(&new_payload);

    let mut changes = Vec::new();
    if previous_agent_name.as_deref() != Some(new_name.as_str()) {
        changes.push(IdentityFieldChange {
            field: "agent_name".to_string(),
            old_value: previous_agent_name,
            new_value: Some(new_name.clone()),
        });
    }
    if agent.display_name != new_name {
        changes.push(IdentityFieldChange {
            field: "display_name".to_string(),
            old_value: Some(agent.display_name.clone()),
            new_value: Some(new_name.clone()),
        });
    }

    let renamed = registration
        .rename_agent(RenameAgentRequest {
            agent_id: agent.id.clone(),
            owner_user_id: user.id.clone(),
            expected_fingerprint: agent.fingerprint.clone(),
            previous_agent_name: reserved_previous_name,
            new_display_name: new_name,
            new_fingerprint,
            new_identity_payload: new_payload,
            changes,
            actor: user.pub_id.clone(),
            reason,
        })
        .map_err(|error| match error {
            RegistrationError::IdentityFingerprintCollision => RenameFailure::Http(rename_error(
                409,
                "the renamed identity fingerprint already belongs to a different agent; agents are never merged",
                "agent_rename_identity_collision",
            )),
            RegistrationError::AgentNameConflict => RenameFailure::Http(rename_error(
                409,
                "another agent of this owner already uses that name (names compare trimmed and case-insensitively)",
                "agent_rename_name_conflict",
            )),
            RegistrationError::AgentNameReserved => RenameFailure::Http(rename_error(
                409,
                "that name is reserved as a previous name of another agent of this owner",
                "agent_rename_name_reserved",
            )),
            RegistrationError::NoIdentityChange => {
                bad_request("rename does not change the agent identity")
            }
            RegistrationError::RenameConflict => RenameFailure::Http(rename_error(
                409,
                "agent identity changed concurrently; review the current name and retry",
                "agent_rename_conflict",
            )),
            RegistrationError::AgentNotFound => {
                RenameFailure::Http(rename_error(404, "agent not found", "agent_not_found"))
            }
            RegistrationError::InvalidRename(message) => bad_request(message),
            other => RenameFailure::Internal(other.into()),
        })?;
    let account_id = registration
        .account_for_agent(&renamed.agent.id)?
        .map(|account| account.pub_id);

    Ok(json!({
        "agent_id": renamed.agent.pub_id,
        "account_id": account_id,
        "display_name": renamed.agent.display_name,
        "previous_display_name": agent.display_name,
        "identity_fingerprint": renamed.agent.fingerprint,
        "previous_identity_fingerprint": agent.fingerprint,
        "revision": revision_json(&renamed.revision),
    }))
}

/// Rebuild the v1 payload Hubu's own clients sent for agents registered before
/// payloads were stored, and only trust it when it reproduces the stored
/// fingerprint exactly.
fn legacy_identity_payload(
    display_name: &str,
    owner_pub_id: &str,
    fingerprint: &str,
) -> Option<Value> {
    let payload = json!({
        "protocol_version": REGISTRATION_PROTOCOL_VERSION,
        "owner": {
            "type": "hubu_user",
            "pub_id": owner_pub_id
        },
        "agent_name": display_name,
        "agent_kind": "codex_agent"
    });
    (fingerprint_payload(&payload) == fingerprint).then_some(payload)
}

pub(super) fn agent_identity_history(request: &HttpRequest, state: &ServerState) -> Result<Value> {
    let agent_pub_id = request
        .query
        .get("agent_id")
        .map(|value| history::decode(value))
        .transpose()?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("agent identity history requires agent_id"))?;
    let user = authenticated_user_context(state)?;
    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    let agent = registration
        .agent_id_for_pub_id(&agent_pub_id)?
        .map(|agent_id| registration.agent_for_id(&agent_id))
        .transpose()?
        .flatten()
        .filter(|agent| agent.owner_user_id == user.user_id)
        .ok_or_else(|| anyhow!("agent not found"))?;
    let revisions = registration.identity_revisions(&agent.id)?;
    let aliases = registration.identity_aliases(&agent.id)?;

    Ok(json!({
        "agent_id": agent.pub_id,
        "display_name": agent.display_name,
        "identity_fingerprint": agent.fingerprint,
        "current_revision": revisions.last().map_or(0, |revision| revision.revision),
        "registered_at": agent.created_at.to_rfc3339(),
        "revisions": revisions.iter().map(revision_json).collect::<Vec<_>>(),
        "fingerprint_aliases": aliases.iter().map(alias_json).collect::<Vec<_>>(),
    }))
}

fn revision_json(revision: &AgentIdentityRevision) -> Value {
    json!({
        "revision": revision.revision,
        "changes": revision.changes,
        "previous_identity_fingerprint": revision.previous_fingerprint,
        "identity_fingerprint": revision.fingerprint,
        "actor": revision.actor,
        "reason": revision.reason,
        "created_at": revision.created_at.to_rfc3339(),
    })
}

fn alias_json(alias: &AgentIdentityAlias) -> Value {
    json!({
        "identity_fingerprint": alias.fingerprint,
        "source": alias.source,
        "revision": alias.revision,
        "created_at": alias.created_at.to_rfc3339(),
    })
}
