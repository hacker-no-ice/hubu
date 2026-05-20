use std::collections::HashMap;

use crate::errors::RegistrationError;
use chrono::Utc;
use hubu_common::{
    actor::OwnerRef,
    ids::{AgentAccountId, AgentId, AgentSessionId, AgentVersionId},
    models::{
        account::{AccountStatus, AgentAccount},
        identity::{
            AgentIdentity, AgentStatus, AgentType, AgentVersion, CodeReference, ModelIdentity,
            RuntimeIdentity,
        },
        session::AgentSession,
    },
};

pub struct RegistrationManager {
    agents: HashMap<AgentId, AgentIdentity>,
    versions: HashMap<AgentVersionId, AgentVersion>,
    accounts: HashMap<AgentAccountId, AgentAccount>,
    sessions: HashMap<AgentSessionId, AgentSession>,

    // lookup agent by identity fingerprint, identity fingerprint is global unique for an agent
    agent_by_identity_fingerprint: HashMap<String, AgentId>,
    // lookup account by agent ID, one agent can only have one account for now
    account_by_agent: HashMap<AgentId, AgentAccountId>,
    // lookup agent version by agent id and version fingerprint, version fingerprint is only unique
    // in the context of an agent
    version_by_agent_and_fingerprint: HashMap<(AgentId, String), AgentVersionId>,
}

#[derive(Debug)]
pub struct RegisterAgentRequest {
    pub display_name: String,
    pub description: Option<String>,
    pub owner: OwnerRef,
    pub agent_type: AgentType,

    pub identity_fingerprint: String,
    pub version_fingerprint: String,
    pub code_ref: Option<CodeReference>,
    pub model: Option<ModelIdentity>,
    pub runtime: Option<RuntimeIdentity>,

    pub mcp_client_name: Option<String>,
    pub mcp_client_version: Option<String>,
}

#[derive(Debug)]
pub struct RegisterAgentResponse {
    pub agent: AgentIdentity,
    pub version: AgentVersion,
    pub account: AgentAccount,
    pub session: AgentSession,
}

impl RegistrationManager {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            versions: HashMap::new(),
            accounts: HashMap::new(),
            sessions: HashMap::new(),
            agent_by_identity_fingerprint: HashMap::new(),
            account_by_agent: HashMap::new(),
            version_by_agent_and_fingerprint: HashMap::new(),
        }
    }

    fn validate_request(&self, request: &RegisterAgentRequest) -> Result<(), RegistrationError> {
        if request.identity_fingerprint.is_empty() || request.version_fingerprint.is_empty() {
            return Err(RegistrationError::MissingFingerprint);
        }
        Ok(())
    }

    pub fn register_agent(
        &mut self,
        request: RegisterAgentRequest,
    ) -> Result<RegisterAgentResponse, RegistrationError> {
        self.validate_request(&request)?;

        let agent = self.resolve_or_create_agent(&request)?;
        let version = self.resolve_or_create_agent_version(&agent, &request)?;
        let account = self.resolve_or_create_account(&agent);
        let session = self.create_session(&agent, &request);

        Ok(RegisterAgentResponse {
            agent,
            version,
            account,
            session,
        })
    }

    fn resolve_or_create_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<AgentIdentity, RegistrationError> {
        if let Some(agent_id) = self
            .agent_by_identity_fingerprint
            .get(&request.identity_fingerprint)
        {
            let agent = self
                .agents
                .get(agent_id)
                .expect("agent index is stale")
                .clone();
            // conflict check
            if agent.owner != request.owner || agent.agent_type != request.agent_type {
                return Err(RegistrationError::IdentityConflict);
            }
            return Ok(agent);
        }

        let now = Utc::now();
        let id = AgentId::new();
        let agent = AgentIdentity {
            id: id.clone(),
            pub_id: format!("agt_{}", request.identity_fingerprint),
            fingerprint: request.identity_fingerprint.clone(),
            display_name: request.display_name.clone(),
            description: request.description.clone(),
            owner: request.owner.clone(),
            agent_type: request.agent_type.clone(),
            agent_status: AgentStatus::Active,
            created_at: now,
            updated_at: now,
        };

        self.agent_by_identity_fingerprint
            .insert(request.identity_fingerprint.clone(), id.clone());
        self.agents.insert(id, agent.clone());

        Ok(agent)
    }

    fn resolve_or_create_agent_version(
        &mut self,
        agent: &AgentIdentity,
        request: &RegisterAgentRequest,
    ) -> Result<AgentVersion, RegistrationError> {
        let key = (agent.id.clone(), request.version_fingerprint.clone());
        if let Some(agent_version_id) = self.version_by_agent_and_fingerprint.get(&key) {
            let version = self
                .versions
                .get(agent_version_id)
                .expect("agent version index is stale");
            // version conflict check here
            if version.code_ref != request.code_ref
                || version.model != request.model
                || version.runtime != request.runtime
            {
                return Err(RegistrationError::VersionConflict);
            }
            return Ok(version.clone());
        }
        let id = AgentVersionId::new();
        let version = AgentVersion {
            id: id.clone(),
            pub_id: format!("agv_{}", request.version_fingerprint),
            agent_id: agent.id.clone(),
            fingerprint: request.version_fingerprint.clone(),
            code_ref: request.code_ref.clone(),
            model: request.model.clone(),
            runtime: request.runtime.clone(),
            created_at: Utc::now(),
        };

        self.version_by_agent_and_fingerprint
            .insert(key, id.clone());
        self.versions.insert(id, version.clone());

        Ok(version)
    }

    fn resolve_or_create_account(&mut self, agent: &AgentIdentity) -> AgentAccount {
        if let Some(account_id) = self.account_by_agent.get(&agent.id) {
            return self
                .accounts
                .get(account_id)
                .expect("account index is stale")
                .clone();
        }

        let now = Utc::now();
        let id = AgentAccountId::new();
        let account = AgentAccount {
            id: id.clone(),
            pub_id: format!("aga_{}", agent.fingerprint),
            agent_id: agent.id.clone(),
            account_status: AccountStatus::Active,
            created_at: now,
            updated_at: now,
        };

        self.account_by_agent.insert(agent.id.clone(), id.clone());
        self.accounts.insert(id, account.clone());

        account
    }

    fn create_session(
        &mut self,
        agent: &AgentIdentity,
        request: &RegisterAgentRequest,
    ) -> AgentSession {
        let id = AgentSessionId::new();
        let session = AgentSession {
            id: id.clone(),
            pub_id: format!("ags_{}", self.sessions.len() + 1),
            agent_id: agent.id.clone(),
            mcp_client_name: request.mcp_client_name.clone(),
            mcp_client_version: request.mcp_client_version.clone(),
            created_at: Utc::now(),
        };

        self.sessions.insert(id, session.clone());

        session
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubu_common::actor::OwnerType;
    use hubu_common::models::identity::RuntimeEnvironment;

    fn test_request(identity_fingerprint: &str, version_fingerprint: &str) -> RegisterAgentRequest {
        RegisterAgentRequest {
            display_name: "Test Agent".to_string(),
            description: Some("MVE Agent to make money".to_string()),
            owner: OwnerRef {
                owner_type: OwnerType::Human,
                owner_id: "user_123".to_string(),
            },
            agent_type: AgentType::AutonomousAgent,
            identity_fingerprint: identity_fingerprint.to_string(),
            version_fingerprint: version_fingerprint.to_string(),
            code_ref: Some(CodeReference {
                repository_url: Some("https://github.com/example/hubu-agent".to_string()),
                commit_sha: Some("abc123".to_string()),
            }),
            model: Some(ModelIdentity {
                provider: "openai".to_string(),
                model: "gpt-5.5".to_string(),
                version: Some("2026-05-15".to_string()),
            }),
            runtime: Some(RuntimeIdentity {
                runtime_provider: "codex".to_string(),
                environment: RuntimeEnvironment::Production,
            }),
            mcp_client_name: Some("codex-cli".to_string()),
            mcp_client_version: Some("0.12.3".to_string()),
        }
    }

    #[test]
    fn registering_new_agent_creates_identity_version_account_and_session() {
        let mut manager = RegistrationManager::new();
        let response = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();

        assert_eq!(response.agent.display_name, "Test Agent");
        assert_eq!(response.agent.fingerprint, "sha256:agent-a");
        assert_eq!(response.version.fingerprint, "sha256:version-a");
        assert_eq!(response.version.agent_id, response.agent.id);
        assert_eq!(response.account.agent_id, response.agent.id);
        assert_eq!(response.session.agent_id, response.agent.id);
    }

    #[test]
    fn registering_same_identity_fingerprint_reuses_agent_and_account_but_create_new_session() {
        let mut manager = RegistrationManager::new();
        let first = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();
        let second = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-b"))
            .unwrap();

        assert_eq!(first.agent.id, second.agent.id);
        assert_eq!(first.account.id, second.account.id);
        assert_ne!(first.version.id, second.version.id);
        assert_ne!(first.session.id, second.session.id);
    }

    #[test]
    fn registering_without_fingerprint_fails() {
        let mut manager = RegistrationManager::new();

        let request1 = test_request("", "sha256:version-a");
        let error1 = manager.register_agent(request1).unwrap_err();
        assert_eq!(error1, RegistrationError::MissingFingerprint);

        let request2 = test_request("sha256:agent-a", "");
        let error2 = manager.register_agent(request2).unwrap_err();
        assert_eq!(error2, RegistrationError::MissingFingerprint);

        let request3 = test_request("", "");
        let error3 = manager.register_agent(request3).unwrap_err();
        assert_eq!(error3, RegistrationError::MissingFingerprint);
    }

    #[test]
    fn registering_same_agent_and_same_version_fingerprint_reuses_version() {
        let mut manager = RegistrationManager::new();
        let first = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();
        let second = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();

        assert_eq!(first.agent.id, second.agent.id);
        assert_eq!(first.account.id, second.account.id);
        assert_eq!(first.version.id, second.version.id);
        assert_ne!(first.session.id, second.session.id);
    }

    #[test]
    fn registering_identity_fingerprint_with_different_owner_fails() {
        let mut manager = RegistrationManager::new();
        let request = test_request("sha256:agent-a", "sha256:version-a");
        // calls unwrap to make sure register agent is successful
        manager.register_agent(request).unwrap();

        let mut conflicting_request = test_request("sha256:agent-a", "sha256:version-a");
        conflicting_request.owner.owner_id = "user_456".to_string();

        let error = manager.register_agent(conflicting_request).unwrap_err();
        assert_eq!(error, RegistrationError::IdentityConflict);
    }

    #[test]
    fn registering_same_agent_and_version_finderprint_with_different_config_fails() {
        let mut manager = RegistrationManager::new();
        let request = test_request("sha256:agent-a", "sha256:version-a");
        manager.register_agent(request).unwrap();

        let mut conflicting_request = test_request("sha256:agent-a", "sha256:version-a");
        conflicting_request.model = Some(ModelIdentity {
            provider: "anthropic".to_string(),
            model: "claude".to_string(),
            version: None,
        });

        let error = manager.register_agent(conflicting_request).unwrap_err();
        assert_eq!(error, RegistrationError::VersionConflict);
    }
}
