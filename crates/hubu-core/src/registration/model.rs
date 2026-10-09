use chrono::{DateTime, Utc};
use hubu_common::{
    ids::{AgentId, UserId},
    models::{
        account::AgentAccount,
        identity::{
            AgentIdentity, AgentType, AgentVersion, CodeReference, ModelIdentity, RuntimeIdentity,
        },
        session::AgentSession,
    },
};

/// Input needed to register or resume an agent connection.
///
/// The identity fingerprint resolves the logical agent. The version fingerprint
/// resolves the code/model/runtime version within that agent's lineage.
#[derive(Debug)]
pub struct RegisterAgentRequest {
    pub display_name: String,
    pub description: Option<String>,
    pub owner_user_id: UserId,
    pub agent_type: AgentType,

    pub identity_fingerprint: String,
    pub version_fingerprint: String,
    pub code_ref: Option<CodeReference>,
    pub model: Option<ModelIdentity>,
    pub runtime: Option<RuntimeIdentity>,

    pub mcp_client_name: Option<String>,
    pub mcp_client_version: Option<String>,

    /// Verified canonical identity payload, kept so an owner rename can derive
    /// the fingerprint of the relabeled payload without the original client.
    pub identity_payload: Option<serde_json::Value>,
}

/// Fully resolved registration output.
///
/// A successful registration always returns an identity, version, account, and
/// newly created session. Existing identity/version/account records may be
/// reused when their fingerprints and key fields match the request.
#[derive(Debug)]
pub struct RegisterAgentResponse {
    pub agent: AgentIdentity,
    pub version: AgentVersion,
    pub account: AgentAccount,
    pub session: AgentSession,
    /// True when the submitted identity fingerprint resolved through the
    /// fingerprint alias table recorded by an owner rename.
    pub resolved_via_alias: bool,
}

/// Agent identity plus its spend account.
#[derive(Debug)]
pub struct AgentWithAccount {
    pub agent: AgentIdentity,
    pub account: AgentAccount,
}

/// How a submitted identity fingerprint maps to an existing agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFingerprintResolution {
    pub agent: AgentIdentity,
    pub via_alias: bool,
}

/// One owner-facing label change inside an identity revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdentityFieldChange {
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

/// Owner-initiated relabel of an existing agent identity.
///
/// The caller computes `new_fingerprint` from `new_identity_payload` with the
/// unchanged registration protocol v1 canonicalization. `expected_fingerprint`
/// pins the identity the relabel was derived from so a concurrent rename fails
/// instead of silently stacking on a different payload.
#[derive(Debug, Clone)]
pub struct RenameAgentRequest {
    pub agent_id: AgentId,
    pub owner_user_id: UserId,
    pub expected_fingerprint: String,
    /// Identity payload `agent_name` the agent is being renamed away from; it
    /// stays reserved for this agent through its fingerprint alias.
    pub previous_agent_name: String,
    pub new_display_name: String,
    pub new_fingerprint: String,
    pub new_identity_payload: serde_json::Value,
    pub changes: Vec<IdentityFieldChange>,
    pub actor: String,
    pub reason: String,
}

/// Append-only audit record for one identity relabel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentityRevision {
    pub agent_id: AgentId,
    pub revision: u64,
    pub changes: Vec<IdentityFieldChange>,
    pub previous_fingerprint: String,
    pub fingerprint: String,
    pub actor: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

/// Identity fingerprint that resolves to an agent after a rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentityAlias {
    pub agent_id: AgentId,
    pub fingerprint: String,
    /// `registration` for the pre-rename fingerprint, `rename` for one added
    /// by an owner relabel.
    pub source: String,
    /// [`normalize_agent_name`] of the name this fingerprint was derived from.
    pub normalized_name: String,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RenameAgentResponse {
    pub agent: AgentIdentity,
    pub revision: AgentIdentityRevision,
}

/// Canonical comparison key for owner-facing agent names: trimmed and
/// case-insensitive. Shared by rename reservation checks and per-owner name
/// uniqueness (HUB-251) so both agree on what counts as "the same name".
pub fn normalize_agent_name(name: &str) -> String {
    name.trim().to_lowercase()
}
