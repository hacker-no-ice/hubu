use std::collections::HashMap;

use chrono::Utc;
use hubu_common::{
    ids::{AgentAccountId, AgentId, AgentSessionId, AgentVersionId},
    models::{
        account::{AccountStatus, AgentAccount},
        identity::{AgentIdentity, AgentStatus, AgentVersion},
        session::AgentSession,
    },
};

use crate::registration::error::RegistrationError;
use crate::registration::model::{RegisterAgentRequest, RegisterAgentResponse};

/// In-memory registration coordinator.
///
/// This prototype manager owns the current registration state directly. Later,
/// these maps can become repository/storage traits without changing the high
/// level registration flow.
pub struct RegistrationManager {
    agents: HashMap<AgentId, AgentIdentity>,
    versions: HashMap<AgentVersionId, AgentVersion>,
    accounts: HashMap<AgentAccountId, AgentAccount>,
    sessions: HashMap<AgentSessionId, AgentSession>,

    /// Lookup agent by globally unique identity fingerprint.
    agent_by_identity_fingerprint: HashMap<String, AgentId>,

    /// Lookup agent by public, human-readable ID.
    agent_by_pub_id: HashMap<String, AgentId>,

    /// Lookup account by agent ID. For now, each agent has exactly one account.
    account_by_agent: HashMap<AgentId, AgentAccountId>,

    /// Lookup version by agent ID and version fingerprint.
    ///
    /// Version fingerprints are unique within one agent lineage, not globally.
    version_by_agent_and_fingerprint: HashMap<(AgentId, String), AgentVersionId>,
}

impl RegistrationManager {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            versions: HashMap::new(),
            accounts: HashMap::new(),
            sessions: HashMap::new(),
            agent_by_identity_fingerprint: HashMap::new(),
            agent_by_pub_id: HashMap::new(),
            account_by_agent: HashMap::new(),
            version_by_agent_and_fingerprint: HashMap::new(),
        }
    }

    pub fn agent_id_for_pub_id(&self, pub_id: &str) -> Option<AgentId> {
        self.agent_by_pub_id.get(pub_id).cloned()
    }

    /// Register an agent connection.
    ///
    /// The happy path is idempotent for identity, version, and account, but
    /// intentionally creates a fresh session for every successful call.
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

    fn validate_request(&self, request: &RegisterAgentRequest) -> Result<(), RegistrationError> {
        if request.identity_fingerprint.is_empty() || request.version_fingerprint.is_empty() {
            return Err(RegistrationError::MissingFingerprint);
        }
        Ok(())
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

            if agent.owner != request.owner || agent.agent_type != request.agent_type {
                return Err(RegistrationError::IdentityConflict);
            }

            return Ok(agent);
        }

        let now = Utc::now();
        let id = AgentId::new();
        let pub_id = self.unique_agent_pub_id(&request.display_name);
        let agent = AgentIdentity {
            id: id.clone(),
            pub_id,
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
        self.agent_by_pub_id
            .insert(agent.pub_id.clone(), id.clone());
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
            pub_id: public_id("agv", &request.version_fingerprint),
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
            pub_id: public_id("aga", &agent.pub_id),
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
            pub_id: format!("ags_{:06}", self.sessions.len() + 1),
            agent_id: agent.id.clone(),
            mcp_client_name: request.mcp_client_name.clone(),
            mcp_client_version: request.mcp_client_version.clone(),
            created_at: Utc::now(),
        };

        self.sessions.insert(id, session.clone());

        session
    }

    fn unique_agent_pub_id(&self, display_name: &str) -> String {
        let base = public_id("agt", display_name);
        if !self.agent_by_pub_id.contains_key(&base) {
            return base;
        }

        for suffix in 2.. {
            let candidate = format!("{base}_{suffix}");
            if !self.agent_by_pub_id.contains_key(&candidate) {
                return candidate;
            }
        }

        unreachable!("unbounded suffix search should always find a public ID")
    }
}

fn public_id(prefix: &str, seed: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;

    for character in seed.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('_');
            last_was_separator = true;
        }
    }

    while slug.ends_with('_') {
        slug.pop();
    }

    if slug.is_empty() {
        slug.push_str("agent");
    }

    slug.truncate(40);
    format!("{prefix}_{slug}")
}

impl Default for RegistrationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubu_common::{
        actor::{OwnerRef, OwnerType},
        models::identity::{
            AgentType, CodeReference, ModelIdentity, RuntimeEnvironment, RuntimeIdentity,
        },
    };

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
        assert_eq!(response.agent.pub_id, "agt_test_agent");
        assert_eq!(response.version.fingerprint, "sha256:version-a");
        assert_eq!(response.version.pub_id, "agv_sha256_version_a");
        assert_eq!(response.account.pub_id, "aga_agt_test_agent");
        assert_eq!(response.session.pub_id, "ags_000001");
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
    fn public_agent_ids_are_unique_and_resolve_to_internal_ids() {
        let mut manager = RegistrationManager::new();
        let first = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();
        let second = manager
            .register_agent(test_request("sha256:agent-b", "sha256:version-a"))
            .unwrap();

        assert_eq!(first.agent.pub_id, "agt_test_agent");
        assert_eq!(second.agent.pub_id, "agt_test_agent_2");
        assert_eq!(
            manager.agent_id_for_pub_id(&first.agent.pub_id),
            Some(first.agent.id)
        );
        assert_eq!(
            manager.agent_id_for_pub_id(&second.agent.pub_id),
            Some(second.agent.id)
        );
    }

    #[test]
    fn registering_without_fingerprint_fails() {
        let mut manager = RegistrationManager::new();

        let error = manager
            .register_agent(test_request("", "sha256:version-a"))
            .unwrap_err();
        assert_eq!(error, RegistrationError::MissingFingerprint);

        let error = manager
            .register_agent(test_request("sha256:agent-a", ""))
            .unwrap_err();
        assert_eq!(error, RegistrationError::MissingFingerprint);

        let error = manager.register_agent(test_request("", "")).unwrap_err();
        assert_eq!(error, RegistrationError::MissingFingerprint);
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
        manager.register_agent(request).unwrap();

        let mut conflicting_request = test_request("sha256:agent-a", "sha256:version-a");
        conflicting_request.owner.owner_id = "user_456".to_string();

        let error = manager.register_agent(conflicting_request).unwrap_err();
        assert_eq!(error, RegistrationError::IdentityConflict);
    }

    #[test]
    fn registering_same_agent_and_version_fingerprint_with_different_config_fails() {
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
