use hubu_common::{
    ids::UserId,
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
}

/// Agent identity plus its spend account.
#[derive(Debug)]
pub struct AgentWithAccount {
    pub agent: AgentIdentity,
    pub account: AgentAccount,
}
