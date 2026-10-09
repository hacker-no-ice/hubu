use std::{collections::HashMap, path::Path};

use chrono::Utc;
use hubu_common::{
    ids::{AgentAccountId, AgentId, AgentSessionId, AgentVersionId, UserId},
    models::{
        account::{AccountStatus, AgentAccount},
        identity::{AgentIdentity, AgentStatus, AgentVersion},
        session::AgentSession,
    },
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;

use crate::registration::error::RegistrationError;
use crate::registration::model::{
    normalize_agent_name, AgentIdentityAlias, AgentIdentityRevision, AgentWithAccount,
    IdentityFieldChange, IdentityFingerprintResolution, RegisterAgentRequest,
    RegisterAgentResponse, RenameAgentRequest, RenameAgentResponse,
};
use crate::storage::{
    account_from_row, account_status, agent_from_row, agent_status, agent_type, init_schema,
    parse_timestamp, session_from_row, version_from_row, StorageError,
};
use crate::telemetry::log_event;

/// Registration coordinator backed by a replaceable storage layer.
pub struct RegistrationManager {
    store: RegistrationStore,
}

impl RegistrationManager {
    pub fn new() -> Self {
        Self {
            store: RegistrationStore::Memory(Box::new(MemoryRegistrationStore::new())),
        }
    }

    pub fn in_memory_sqlite() -> Result<Self, RegistrationError> {
        Ok(Self {
            store: RegistrationStore::Sqlite(SqliteRegistrationStore::in_memory()?),
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, RegistrationError> {
        Ok(Self {
            store: RegistrationStore::Sqlite(SqliteRegistrationStore::open(path)?),
        })
    }

    pub fn agent_id_for_pub_id(&self, pub_id: &str) -> Result<Option<AgentId>, RegistrationError> {
        Ok(self.store.agent_by_pub_id(pub_id)?.map(|agent| agent.id))
    }

    pub fn agent_for_id(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentIdentity>, RegistrationError> {
        Ok(self.store.agent_for_id(agent_id)?)
    }

    pub fn version_for_id(
        &self,
        version_id: &AgentVersionId,
    ) -> Result<Option<AgentVersion>, RegistrationError> {
        Ok(self.store.version_for_id(version_id)?)
    }

    pub fn account_for_agent(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentAccount>, RegistrationError> {
        Ok(self.store.account_for_agent(agent_id)?)
    }

    pub fn account_for_pub_id(
        &self,
        pub_id: &str,
    ) -> Result<Option<AgentAccount>, RegistrationError> {
        Ok(self.store.account_by_pub_id(pub_id)?)
    }

    pub fn agents_for_user(
        &self,
        owner_user_id: &UserId,
    ) -> Result<Vec<AgentWithAccount>, RegistrationError> {
        Ok(self.store.agents_for_user(owner_user_id)?)
    }

    pub fn session_for_id(
        &self,
        session_id: &AgentSessionId,
    ) -> Result<Option<AgentSession>, RegistrationError> {
        Ok(self.store.session_for_id(session_id)?)
    }

    /// Resolve an identity fingerprint to an existing agent for one owner.
    ///
    /// Fingerprints recorded by an owner rename resolve through the alias
    /// table first (`via_alias = true`); otherwise the agent's current
    /// identity fingerprint is matched directly.
    pub fn resolve_identity_fingerprint(
        &self,
        owner_user_id: &UserId,
        fingerprint: &str,
    ) -> Result<Option<IdentityFingerprintResolution>, RegistrationError> {
        Ok(self
            .store
            .resolve_identity_fingerprint(owner_user_id, fingerprint)?)
    }

    /// Verified identity payload stored at registration or by the last rename.
    /// Agents registered before HUB-250 return `None`.
    pub fn identity_payload_for_agent(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<serde_json::Value>, RegistrationError> {
        Ok(self.store.identity_payload_for_agent(agent_id)?)
    }

    /// Append-only identity revision history, oldest first.
    pub fn identity_revisions(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<AgentIdentityRevision>, RegistrationError> {
        Ok(self.store.identity_revisions(agent_id)?)
    }

    /// Fingerprint aliases that resolve to this agent, oldest first.
    pub fn identity_aliases(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<AgentIdentityAlias>, RegistrationError> {
        Ok(self.store.identity_aliases(agent_id)?)
    }

    /// Relabel an agent's owner-facing identity while keeping its stable ID.
    ///
    /// Only the display name, identity payload, and current fingerprint
    /// change. Versions, accounts, budgets, holds, policy assignments, and
    /// ledger rows keep referencing the same internal agent ID. The previous
    /// and new fingerprints are recorded as aliases so either resolves to this
    /// agent on registration. A new fingerprint already owned by a different
    /// agent is rejected; agents are never merged.
    pub fn rename_agent(
        &mut self,
        request: RenameAgentRequest,
    ) -> Result<RenameAgentResponse, RegistrationError> {
        if request.new_display_name.trim().is_empty() {
            return Err(RegistrationError::InvalidRename("name must not be empty"));
        }
        if request.reason.trim().is_empty() {
            return Err(RegistrationError::InvalidRename("reason must not be empty"));
        }
        if request.actor.trim().is_empty() {
            return Err(RegistrationError::InvalidRename("actor must not be empty"));
        }
        if request.previous_agent_name.trim().is_empty() {
            return Err(RegistrationError::InvalidRename(
                "previous agent name must not be empty",
            ));
        }
        if request.new_fingerprint.is_empty() || request.expected_fingerprint.is_empty() {
            return Err(RegistrationError::MissingFingerprint);
        }
        let result = self.store.rename_agent(&request);
        match &result {
            Ok(response) => log_event(
                "info",
                "agent_identity_renamed",
                json!({
                    "agent_id": response.agent.id.to_string(),
                    "agent_pub_id": response.agent.pub_id,
                    "owner_user_id": response.agent.owner_user_id.to_string(),
                    "revision": response.revision.revision,
                    "previous_fingerprint": response.revision.previous_fingerprint,
                    "fingerprint": response.revision.fingerprint,
                    "actor": response.revision.actor,
                }),
            ),
            Err(error) => log_event(
                "warn",
                "agent_identity_rename_rejected",
                json!({
                    "agent_id": request.agent_id.to_string(),
                    "owner_user_id": request.owner_user_id.to_string(),
                    "new_fingerprint": request.new_fingerprint,
                    "error": error.to_string(),
                }),
            ),
        }
        result
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
        log_event(
            "info",
            "registration_started",
            json!({
                "owner_user_id": request.owner_user_id.to_string(),
                "identity_fingerprint": request.identity_fingerprint,
                "version_fingerprint": request.version_fingerprint,
                "display_name": request.display_name,
                "agent_type": agent_type_name(&request.agent_type),
                "mcp_client_name": request.mcp_client_name,
                "mcp_client_version": request.mcp_client_version,
            }),
        );
        let response = self.store.register_agent(&request)?;
        log_event(
            "info",
            "registration_completed",
            json!({
                "owner_user_id": response.agent.owner_user_id.to_string(),
                "agent_id": response.agent.id.to_string(),
                "agent_pub_id": response.agent.pub_id,
                "version_id": response.version.id.to_string(),
                "version_pub_id": response.version.pub_id,
                "account_id": response.account.id.to_string(),
                "account_pub_id": response.account.pub_id,
                "session_id": response.session.id.to_string(),
                "session_pub_id": response.session.pub_id,
            }),
        );
        Ok(response)
    }

    fn validate_request(&self, request: &RegisterAgentRequest) -> Result<(), RegistrationError> {
        if request.identity_fingerprint.is_empty() || request.version_fingerprint.is_empty() {
            log_event(
                "warn",
                "registration_rejected",
                json!({
                    "reason": "missing_fingerprint",
                    "owner_user_id": request.owner_user_id.to_string(),
                    "display_name": request.display_name,
                }),
            );
            return Err(RegistrationError::MissingFingerprint);
        }
        Ok(())
    }
}

impl Default for RegistrationManager {
    fn default() -> Self {
        Self::new()
    }
}

enum RegistrationStore {
    Memory(Box<MemoryRegistrationStore>),
    Sqlite(SqliteRegistrationStore),
}

impl RegistrationStore {
    fn register_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<RegisterAgentResponse, RegistrationError> {
        match self {
            RegistrationStore::Memory(store) => store.register_agent(request),
            RegistrationStore::Sqlite(store) => store.register_agent(request),
        }
    }

    fn agent_by_pub_id(&self, pub_id: &str) -> Result<Option<AgentIdentity>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.agent_by_pub_id(pub_id),
            RegistrationStore::Sqlite(store) => store.agent_by_pub_id(pub_id),
        }
    }

    fn resolve_identity_fingerprint(
        &self,
        owner_user_id: &UserId,
        fingerprint: &str,
    ) -> Result<Option<IdentityFingerprintResolution>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => {
                Ok(store.resolve_identity_fingerprint(owner_user_id, fingerprint))
            }
            RegistrationStore::Sqlite(store) => {
                resolve_identity_fingerprint_sqlite(&store.conn, owner_user_id, fingerprint)
            }
        }
    }

    fn identity_payload_for_agent(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<serde_json::Value>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => Ok(store.identity_payloads.get(agent_id).cloned()),
            RegistrationStore::Sqlite(store) => {
                query_identity_payload_for_agent(&store.conn, agent_id)
            }
        }
    }

    fn identity_revisions(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<AgentIdentityRevision>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => {
                Ok(store.revisions.get(agent_id).cloned().unwrap_or_default())
            }
            RegistrationStore::Sqlite(store) => query_identity_revisions(&store.conn, agent_id),
        }
    }

    fn identity_aliases(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<AgentIdentityAlias>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => {
                let mut aliases = store
                    .aliases
                    .values()
                    .filter(|alias| &alias.agent_id == agent_id)
                    .cloned()
                    .collect::<Vec<_>>();
                aliases.sort_by_key(|alias| alias.revision);
                Ok(aliases)
            }
            RegistrationStore::Sqlite(store) => query_identity_aliases(&store.conn, agent_id),
        }
    }

    fn rename_agent(
        &mut self,
        request: &RenameAgentRequest,
    ) -> Result<RenameAgentResponse, RegistrationError> {
        match self {
            RegistrationStore::Memory(store) => store.rename_agent(request),
            RegistrationStore::Sqlite(store) => store.rename_agent(request),
        }
    }

    fn agent_for_id(&self, agent_id: &AgentId) -> Result<Option<AgentIdentity>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.agent_for_id(agent_id),
            RegistrationStore::Sqlite(store) => store.agent_for_id(agent_id),
        }
    }

    fn version_for_id(
        &self,
        version_id: &AgentVersionId,
    ) -> Result<Option<AgentVersion>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.version_for_id(version_id),
            RegistrationStore::Sqlite(store) => store.version_for_id(version_id),
        }
    }

    fn account_for_agent(&self, agent_id: &AgentId) -> Result<Option<AgentAccount>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.account_for_agent(agent_id),
            RegistrationStore::Sqlite(store) => store.account_for_agent(agent_id),
        }
    }

    fn account_by_pub_id(&self, pub_id: &str) -> Result<Option<AgentAccount>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.account_by_pub_id(pub_id),
            RegistrationStore::Sqlite(store) => store.account_by_pub_id(pub_id),
        }
    }

    fn agents_for_user(
        &self,
        owner_user_id: &UserId,
    ) -> Result<Vec<AgentWithAccount>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.agents_for_user(owner_user_id),
            RegistrationStore::Sqlite(store) => store.agents_for_user(owner_user_id),
        }
    }

    fn session_for_id(
        &self,
        session_id: &AgentSessionId,
    ) -> Result<Option<AgentSession>, StorageError> {
        match self {
            RegistrationStore::Memory(store) => store.session_for_id(session_id),
            RegistrationStore::Sqlite(store) => store.session_for_id(session_id),
        }
    }
}

struct MemoryRegistrationStore {
    agents: HashMap<AgentId, AgentIdentity>,
    versions: HashMap<AgentVersionId, AgentVersion>,
    accounts: HashMap<AgentAccountId, AgentAccount>,
    sessions: HashMap<AgentSessionId, AgentSession>,
    agent_by_owner_and_fingerprint: HashMap<(UserId, String), AgentId>,
    agent_by_pub_id: HashMap<String, AgentId>,
    account_by_agent: HashMap<AgentId, AgentAccountId>,
    account_by_pub_id: HashMap<String, AgentAccountId>,
    version_by_agent_and_fingerprint: HashMap<(AgentId, String), AgentVersionId>,
    aliases: HashMap<(UserId, String), AgentIdentityAlias>,
    revisions: HashMap<AgentId, Vec<AgentIdentityRevision>>,
    identity_payloads: HashMap<AgentId, serde_json::Value>,
}

impl MemoryRegistrationStore {
    fn new() -> Self {
        Self {
            agents: HashMap::new(),
            versions: HashMap::new(),
            accounts: HashMap::new(),
            sessions: HashMap::new(),
            agent_by_owner_and_fingerprint: HashMap::new(),
            agent_by_pub_id: HashMap::new(),
            account_by_agent: HashMap::new(),
            account_by_pub_id: HashMap::new(),
            version_by_agent_and_fingerprint: HashMap::new(),
            aliases: HashMap::new(),
            revisions: HashMap::new(),
            identity_payloads: HashMap::new(),
        }
    }

    fn register_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<RegisterAgentResponse, RegistrationError> {
        let (agent, resolved_via_alias) = self.resolve_or_create_agent(request)?;
        let version = self.resolve_or_create_agent_version(&agent, request)?;
        let account = self.resolve_or_create_account(&agent);
        let session = self.create_session(&agent, request);

        Ok(RegisterAgentResponse {
            agent,
            version,
            account,
            session,
            resolved_via_alias,
        })
    }

    fn resolve_identity_fingerprint(
        &self,
        owner_user_id: &UserId,
        fingerprint: &str,
    ) -> Option<IdentityFingerprintResolution> {
        let key = (owner_user_id.clone(), fingerprint.to_string());
        if let Some(alias) = self.aliases.get(&key) {
            return self.agents.get(&alias.agent_id).cloned().map(|agent| {
                IdentityFingerprintResolution {
                    agent,
                    via_alias: true,
                }
            });
        }
        self.agent_by_owner_and_fingerprint
            .get(&key)
            .and_then(|agent_id| self.agents.get(agent_id))
            .cloned()
            .map(|agent| IdentityFingerprintResolution {
                agent,
                via_alias: false,
            })
    }

    fn name_reserved_by_other_agent(
        &self,
        owner_user_id: &UserId,
        normalized_name: &str,
        except: Option<&AgentId>,
    ) -> bool {
        self.aliases.iter().any(|((owner, _), alias)| {
            owner == owner_user_id
                && alias.normalized_name == normalized_name
                && Some(&alias.agent_id) != except
        })
    }

    fn rename_agent(
        &mut self,
        request: &RenameAgentRequest,
    ) -> Result<RenameAgentResponse, RegistrationError> {
        let mut agent = self
            .agents
            .get(&request.agent_id)
            .filter(|agent| agent.owner_user_id == request.owner_user_id)
            .cloned()
            .ok_or(RegistrationError::AgentNotFound)?;
        if agent.fingerprint != request.expected_fingerprint {
            return Err(RegistrationError::RenameConflict);
        }
        if agent.fingerprint == request.new_fingerprint {
            return Err(RegistrationError::NoIdentityChange);
        }
        if let Some(existing) =
            self.resolve_identity_fingerprint(&request.owner_user_id, &request.new_fingerprint)
        {
            if existing.agent.id != agent.id {
                return Err(RegistrationError::IdentityFingerprintCollision);
            }
        }
        let new_name = normalize_agent_name(&request.new_display_name);
        if self.agents.values().any(|other| {
            other.id != agent.id
                && other.owner_user_id == request.owner_user_id
                && normalize_agent_name(&other.display_name) == new_name
        }) {
            return Err(RegistrationError::AgentNameConflict);
        }
        if self.name_reserved_by_other_agent(&request.owner_user_id, &new_name, Some(&agent.id)) {
            return Err(RegistrationError::AgentNameReserved);
        }

        let now = Utc::now();
        let revision_number = self.revisions.get(&agent.id).map_or(0, Vec::len) as u64 + 1;
        let owner = request.owner_user_id.clone();
        self.aliases
            .entry((owner.clone(), agent.fingerprint.clone()))
            .or_insert_with(|| AgentIdentityAlias {
                agent_id: agent.id.clone(),
                fingerprint: agent.fingerprint.clone(),
                source: "registration".to_string(),
                normalized_name: normalize_agent_name(&request.previous_agent_name),
                revision: 0,
                created_at: now,
            });
        self.aliases
            .entry((owner.clone(), request.new_fingerprint.clone()))
            .or_insert_with(|| AgentIdentityAlias {
                agent_id: agent.id.clone(),
                fingerprint: request.new_fingerprint.clone(),
                source: "rename".to_string(),
                normalized_name: new_name.clone(),
                revision: revision_number,
                created_at: now,
            });
        self.agent_by_owner_and_fingerprint
            .remove(&(owner.clone(), agent.fingerprint.clone()));
        self.agent_by_owner_and_fingerprint
            .insert((owner, request.new_fingerprint.clone()), agent.id.clone());

        let revision = AgentIdentityRevision {
            agent_id: agent.id.clone(),
            revision: revision_number,
            changes: request.changes.clone(),
            previous_fingerprint: agent.fingerprint.clone(),
            fingerprint: request.new_fingerprint.clone(),
            actor: request.actor.clone(),
            reason: request.reason.clone(),
            created_at: now,
        };
        agent.fingerprint = request.new_fingerprint.clone();
        agent.display_name = request.new_display_name.clone();
        agent.updated_at = now;
        self.agents.insert(agent.id.clone(), agent.clone());
        self.identity_payloads
            .insert(agent.id.clone(), request.new_identity_payload.clone());
        self.revisions
            .entry(agent.id.clone())
            .or_default()
            .push(revision.clone());
        Ok(RenameAgentResponse { agent, revision })
    }

    fn agent_by_pub_id(&self, pub_id: &str) -> Result<Option<AgentIdentity>, StorageError> {
        Ok(self
            .agent_by_pub_id
            .get(pub_id)
            .and_then(|id| self.agents.get(id))
            .cloned())
    }

    fn agent_for_id(&self, agent_id: &AgentId) -> Result<Option<AgentIdentity>, StorageError> {
        Ok(self.agents.get(agent_id).cloned())
    }

    fn version_for_id(
        &self,
        version_id: &AgentVersionId,
    ) -> Result<Option<AgentVersion>, StorageError> {
        Ok(self.versions.get(version_id).cloned())
    }

    fn account_for_agent(&self, agent_id: &AgentId) -> Result<Option<AgentAccount>, StorageError> {
        Ok(self
            .account_by_agent
            .get(agent_id)
            .and_then(|id| self.accounts.get(id))
            .cloned())
    }

    fn account_by_pub_id(&self, pub_id: &str) -> Result<Option<AgentAccount>, StorageError> {
        Ok(self
            .account_by_pub_id
            .get(pub_id)
            .and_then(|id| self.accounts.get(id))
            .cloned())
    }

    fn agents_for_user(
        &self,
        owner_user_id: &UserId,
    ) -> Result<Vec<AgentWithAccount>, StorageError> {
        let mut agents = self
            .agents
            .values()
            .filter(|agent| &agent.owner_user_id == owner_user_id)
            .filter_map(|agent| {
                self.account_by_agent
                    .get(&agent.id)
                    .and_then(|account_id| self.accounts.get(account_id))
                    .map(|account| AgentWithAccount {
                        agent: agent.clone(),
                        account: account.clone(),
                    })
            })
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| left.agent.created_at.cmp(&right.agent.created_at));
        Ok(agents)
    }

    fn session_for_id(
        &self,
        session_id: &AgentSessionId,
    ) -> Result<Option<AgentSession>, StorageError> {
        Ok(self.sessions.get(session_id).cloned())
    }

    fn resolve_or_create_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<(AgentIdentity, bool), RegistrationError> {
        let key = (
            request.owner_user_id.clone(),
            request.identity_fingerprint.clone(),
        );
        if let Some(resolution) =
            self.resolve_identity_fingerprint(&request.owner_user_id, &request.identity_fingerprint)
        {
            let agent = resolution.agent;

            if agent.agent_type != request.agent_type {
                log_event(
                    "warn",
                    "registration_identity_conflict",
                    json!({
                        "store": "memory",
                        "agent_id": agent.id.to_string(),
                        "agent_pub_id": agent.pub_id,
                        "owner_user_id": agent.owner_user_id.to_string(),
                        "identity_fingerprint": request.identity_fingerprint,
                    }),
                );
                return Err(RegistrationError::IdentityConflict);
            }

            log_event(
                "info",
                "registration_agent_reused",
                json!({
                    "store": "memory",
                    "agent_id": agent.id.to_string(),
                    "agent_pub_id": agent.pub_id,
                    "owner_user_id": agent.owner_user_id.to_string(),
                    "identity_fingerprint": request.identity_fingerprint,
                }),
            );
            return Ok((agent, resolution.via_alias));
        }

        if self.name_reserved_by_other_agent(
            &request.owner_user_id,
            &normalize_agent_name(requested_agent_name(request)),
            None,
        ) {
            return Err(RegistrationError::AgentNameReserved);
        }

        let now = Utc::now();
        let (id, pub_id) = self.new_public_agent_id();
        let agent = AgentIdentity {
            id: id.clone(),
            pub_id,
            fingerprint: request.identity_fingerprint.clone(),
            display_name: request.display_name.clone(),
            description: request.description.clone(),
            owner_user_id: request.owner_user_id.clone(),
            agent_type: request.agent_type.clone(),
            agent_status: AgentStatus::Active,
            created_at: now,
            updated_at: now,
        };

        self.agent_by_owner_and_fingerprint.insert(key, id.clone());
        self.agent_by_pub_id
            .insert(agent.pub_id.clone(), id.clone());
        self.agents.insert(id, agent.clone());

        log_event(
            "info",
            "registration_agent_created",
            json!({
                "store": "memory",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "owner_user_id": agent.owner_user_id.to_string(),
                "identity_fingerprint": agent.fingerprint,
                "agent_type": agent_type_name(&agent.agent_type),
            }),
        );
        if let Some(payload) = &request.identity_payload {
            self.identity_payloads
                .insert(agent.id.clone(), payload.clone());
        }
        Ok((agent, false))
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
                log_event(
                    "warn",
                    "registration_version_conflict",
                    json!({
                        "store": "memory",
                        "agent_id": agent.id.to_string(),
                        "agent_pub_id": agent.pub_id,
                        "version_id": version.id.to_string(),
                        "version_pub_id": version.pub_id,
                        "version_fingerprint": request.version_fingerprint,
                    }),
                );
                return Err(RegistrationError::VersionConflict);
            }

            log_event(
                "info",
                "registration_version_reused",
                json!({
                    "store": "memory",
                    "agent_id": agent.id.to_string(),
                    "agent_pub_id": agent.pub_id,
                    "version_id": version.id.to_string(),
                    "version_pub_id": version.pub_id,
                    "version_fingerprint": request.version_fingerprint,
                }),
            );
            return Ok(version.clone());
        }

        let id = AgentVersionId::new();
        let version = AgentVersion {
            id: id.clone(),
            pub_id: format!("agv_{}", id.public_suffix()),
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

        log_event(
            "info",
            "registration_version_created",
            json!({
                "store": "memory",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "version_id": version.id.to_string(),
                "version_pub_id": version.pub_id,
                "version_fingerprint": version.fingerprint,
            }),
        );
        Ok(version)
    }

    fn resolve_or_create_account(&mut self, agent: &AgentIdentity) -> AgentAccount {
        if let Some(account_id) = self.account_by_agent.get(&agent.id) {
            let account = self
                .accounts
                .get(account_id)
                .expect("account index is stale")
                .clone();
            log_event(
                "info",
                "registration_account_reused",
                json!({
                    "store": "memory",
                    "agent_id": agent.id.to_string(),
                    "agent_pub_id": agent.pub_id,
                    "account_id": account.id.to_string(),
                    "account_pub_id": account.pub_id,
                }),
            );
            return account;
        }

        let now = Utc::now();
        let id = AgentAccountId::new();
        let account = AgentAccount {
            id: id.clone(),
            pub_id: format!("aga_{}", id.public_suffix()),
            agent_id: agent.id.clone(),
            owner_user_id: agent.owner_user_id.clone(),
            account_status: AccountStatus::Active,
            created_at: now,
            updated_at: now,
        };

        self.account_by_agent.insert(agent.id.clone(), id.clone());
        self.account_by_pub_id
            .insert(account.pub_id.clone(), id.clone());
        self.accounts.insert(id, account.clone());

        log_event(
            "info",
            "registration_account_created",
            json!({
                "store": "memory",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "account_id": account.id.to_string(),
                "account_pub_id": account.pub_id,
            }),
        );
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
            pub_id: format!("ags_{}", id.public_suffix()),
            agent_id: agent.id.clone(),
            owner_user_id: agent.owner_user_id.clone(),
            mcp_client_name: request.mcp_client_name.clone(),
            mcp_client_version: request.mcp_client_version.clone(),
            created_at: Utc::now(),
        };

        self.sessions.insert(id, session.clone());

        log_event(
            "info",
            "registration_session_created",
            json!({
                "store": "memory",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "session_id": session.id.to_string(),
                "session_pub_id": session.pub_id,
                "mcp_client_name": session.mcp_client_name,
                "mcp_client_version": session.mcp_client_version,
            }),
        );
        session
    }

    fn new_public_agent_id(&self) -> (AgentId, String) {
        loop {
            let id = AgentId::new();
            let pub_id = format!("agt_{}", id.public_suffix());
            if !self.agent_by_pub_id.contains_key(&pub_id) {
                return (id, pub_id);
            }
        }
    }
}

pub struct SqliteRegistrationStore {
    conn: Connection,
}

impl SqliteRegistrationStore {
    fn in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn register_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<RegisterAgentResponse, RegistrationError> {
        let sqlite_tx = self.conn.transaction()?;

        let (agent, resolved_via_alias) = resolve_or_create_agent_sqlite(&sqlite_tx, request)?;
        let version = resolve_or_create_agent_version_sqlite(&sqlite_tx, &agent, request)?;
        let account = resolve_or_create_account_sqlite(&sqlite_tx, &agent)?;
        let session = create_session_sqlite(&sqlite_tx, &agent, &version, request)?;

        sqlite_tx.commit()?;

        Ok(RegisterAgentResponse {
            agent,
            version,
            account,
            session,
            resolved_via_alias,
        })
    }

    fn rename_agent(
        &mut self,
        request: &RenameAgentRequest,
    ) -> Result<RenameAgentResponse, RegistrationError> {
        let tx = self.conn.transaction()?;
        let mut agent = query_agent_for_id(&tx, &request.agent_id)?
            .filter(|agent| agent.owner_user_id == request.owner_user_id)
            .ok_or(RegistrationError::AgentNotFound)?;
        if agent.fingerprint != request.expected_fingerprint {
            return Err(RegistrationError::RenameConflict);
        }
        if agent.fingerprint == request.new_fingerprint {
            return Err(RegistrationError::NoIdentityChange);
        }
        if let Some(existing) = resolve_identity_fingerprint_sqlite(
            &tx,
            &request.owner_user_id,
            &request.new_fingerprint,
        )? {
            if existing.agent.id != agent.id {
                return Err(RegistrationError::IdentityFingerprintCollision);
            }
        }
        let new_name = normalize_agent_name(&request.new_display_name);
        if current_name_used_by_other_agent_sqlite(&tx, &agent, &new_name)? {
            return Err(RegistrationError::AgentNameConflict);
        }
        if name_reserved_by_other_agent_sqlite(
            &tx,
            &request.owner_user_id,
            &new_name,
            Some(&agent.id),
        )? {
            return Err(RegistrationError::AgentNameReserved);
        }

        let now = Utc::now();
        let revision_number: i64 = tx.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM agent_identity_revisions WHERE agent_id = ?1",
            params![agent.id.to_string()],
            |row| row.get(0),
        )?;
        insert_alias_if_missing_sqlite(
            &tx,
            &agent,
            &agent.fingerprint,
            "registration",
            &normalize_agent_name(&request.previous_agent_name),
            0,
            &now,
        )?;
        insert_alias_if_missing_sqlite(
            &tx,
            &agent,
            &request.new_fingerprint,
            "rename",
            &new_name,
            revision_number,
            &now,
        )?;
        tx.execute(
            "UPDATE agent_identities
             SET fingerprint = ?1, display_name = ?2, identity_payload_json = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                &request.new_fingerprint,
                &request.new_display_name,
                serde_json::to_string(&request.new_identity_payload)?,
                now.to_rfc3339(),
                agent.id.to_string(),
            ],
        )?;
        tx.execute(
            "INSERT INTO agent_identity_revisions
             (agent_id, revision, previous_fingerprint, fingerprint, changes_json, actor, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                agent.id.to_string(),
                revision_number,
                &agent.fingerprint,
                &request.new_fingerprint,
                serde_json::to_string(&request.changes)?,
                &request.actor,
                &request.reason,
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;

        let revision = AgentIdentityRevision {
            agent_id: agent.id.clone(),
            revision: revision_number as u64,
            changes: request.changes.clone(),
            previous_fingerprint: agent.fingerprint.clone(),
            fingerprint: request.new_fingerprint.clone(),
            actor: request.actor.clone(),
            reason: request.reason.clone(),
            created_at: now,
        };
        agent.fingerprint = request.new_fingerprint.clone();
        agent.display_name = request.new_display_name.clone();
        agent.updated_at = now;
        Ok(RenameAgentResponse { agent, revision })
    }

    fn agent_by_pub_id(&self, pub_id: &str) -> Result<Option<AgentIdentity>, StorageError> {
        query_agent_by_pub_id(&self.conn, pub_id)
    }

    fn agent_for_id(&self, agent_id: &AgentId) -> Result<Option<AgentIdentity>, StorageError> {
        query_agent_for_id(&self.conn, agent_id)
    }

    fn version_for_id(
        &self,
        version_id: &AgentVersionId,
    ) -> Result<Option<AgentVersion>, StorageError> {
        let version = self
            .conn
            .query_row(
                "SELECT id, pub_id, agent_id, fingerprint, code_ref_json, model_json, runtime_json, created_at
                 FROM agent_versions
                 WHERE id = ?1",
                params![version_id.to_string()],
                version_from_row,
            )
            .optional()?;
        Ok(version)
    }

    fn account_for_agent(&self, agent_id: &AgentId) -> Result<Option<AgentAccount>, StorageError> {
        query_account_for_agent(&self.conn, agent_id)
    }

    fn account_by_pub_id(&self, pub_id: &str) -> Result<Option<AgentAccount>, StorageError> {
        query_account_by_pub_id(&self.conn, pub_id)
    }

    fn agents_for_user(
        &self,
        owner_user_id: &UserId,
    ) -> Result<Vec<AgentWithAccount>, StorageError> {
        query_agents_for_user(&self.conn, owner_user_id)
    }

    fn session_for_id(
        &self,
        session_id: &AgentSessionId,
    ) -> Result<Option<AgentSession>, StorageError> {
        let session = self
            .conn
            .query_row(
                "SELECT id, pub_id, agent_id, owner_user_id, mcp_client_name, mcp_client_version, created_at
                 FROM agent_sessions
                 WHERE id = ?1",
                params![session_id.to_string()],
                session_from_row,
            )
            .optional()?;
        Ok(session)
    }

    #[cfg(test)]
    fn raw_connection(&self) -> &Connection {
        &self.conn
    }
}

fn resolve_or_create_agent_sqlite(
    tx: &Transaction<'_>,
    request: &RegisterAgentRequest,
) -> Result<(AgentIdentity, bool), RegistrationError> {
    if let Some(IdentityFingerprintResolution { agent, via_alias }) =
        resolve_identity_fingerprint_sqlite(
            tx,
            &request.owner_user_id,
            &request.identity_fingerprint,
        )?
    {
        if agent.agent_type != request.agent_type {
            log_event(
                "warn",
                "registration_identity_conflict",
                json!({
                    "store": "sqlite",
                    "agent_id": agent.id.to_string(),
                    "agent_pub_id": agent.pub_id,
                    "owner_user_id": agent.owner_user_id.to_string(),
                    "identity_fingerprint": request.identity_fingerprint,
                }),
            );
            return Err(RegistrationError::IdentityConflict);
        }
        log_event(
            "info",
            "registration_agent_reused",
            json!({
                "store": "sqlite",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "owner_user_id": agent.owner_user_id.to_string(),
                "identity_fingerprint": request.identity_fingerprint,
            }),
        );
        return Ok((agent, via_alias));
    }

    if name_reserved_by_other_agent_sqlite(
        tx,
        &request.owner_user_id,
        &normalize_agent_name(requested_agent_name(request)),
        None,
    )? {
        return Err(RegistrationError::AgentNameReserved);
    }

    let now = Utc::now();
    let id = AgentId::new();
    let agent = AgentIdentity {
        pub_id: format!("agt_{}", id.public_suffix()),
        id,
        fingerprint: request.identity_fingerprint.clone(),
        display_name: request.display_name.clone(),
        description: request.description.clone(),
        owner_user_id: request.owner_user_id.clone(),
        agent_type: request.agent_type.clone(),
        agent_status: AgentStatus::Active,
        created_at: now,
        updated_at: now,
    };

    tx.execute(
        "INSERT INTO agent_identities
         (id, pub_id, fingerprint, display_name, description, owner_user_id, agent_type, agent_status, created_at, updated_at, identity_payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            agent.id.to_string(),
            &agent.pub_id,
            &agent.fingerprint,
            &agent.display_name,
            &agent.description,
            agent.owner_user_id.to_string(),
            agent_type(&agent.agent_type),
            agent_status(&agent.agent_status),
            agent.created_at.to_rfc3339(),
            agent.updated_at.to_rfc3339(),
            request
                .identity_payload
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        ],
    )?;

    log_event(
        "info",
        "registration_agent_created",
        json!({
            "store": "sqlite",
            "agent_id": agent.id.to_string(),
            "agent_pub_id": agent.pub_id,
            "owner_user_id": agent.owner_user_id.to_string(),
            "identity_fingerprint": agent.fingerprint,
            "agent_type": agent_type_name(&agent.agent_type),
        }),
    );
    Ok((agent, false))
}

fn resolve_or_create_agent_version_sqlite(
    tx: &Transaction<'_>,
    agent: &AgentIdentity,
    request: &RegisterAgentRequest,
) -> Result<AgentVersion, RegistrationError> {
    if let Some(version) =
        query_version_by_agent_and_fingerprint(tx, &agent.id, &request.version_fingerprint)?
    {
        if version.code_ref != request.code_ref
            || version.model != request.model
            || version.runtime != request.runtime
        {
            log_event(
                "warn",
                "registration_version_conflict",
                json!({
                    "store": "sqlite",
                    "agent_id": agent.id.to_string(),
                    "agent_pub_id": agent.pub_id,
                    "version_id": version.id.to_string(),
                    "version_pub_id": version.pub_id,
                    "version_fingerprint": request.version_fingerprint,
                }),
            );
            return Err(RegistrationError::VersionConflict);
        }
        log_event(
            "info",
            "registration_version_reused",
            json!({
                "store": "sqlite",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "version_id": version.id.to_string(),
                "version_pub_id": version.pub_id,
                "version_fingerprint": request.version_fingerprint,
            }),
        );
        return Ok(version);
    }

    let id = AgentVersionId::new();
    let version = AgentVersion {
        pub_id: format!("agv_{}", id.public_suffix()),
        id,
        agent_id: agent.id.clone(),
        fingerprint: request.version_fingerprint.clone(),
        code_ref: request.code_ref.clone(),
        model: request.model.clone(),
        runtime: request.runtime.clone(),
        created_at: Utc::now(),
    };

    tx.execute(
        "INSERT INTO agent_versions
         (id, pub_id, agent_id, fingerprint, code_ref_json, model_json, runtime_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            version.id.to_string(),
            &version.pub_id,
            version.agent_id.to_string(),
            &version.fingerprint,
            json_string(&version.code_ref)?,
            json_string(&version.model)?,
            json_string(&version.runtime)?,
            version.created_at.to_rfc3339(),
        ],
    )?;

    log_event(
        "info",
        "registration_version_created",
        json!({
            "store": "sqlite",
            "agent_id": agent.id.to_string(),
            "agent_pub_id": agent.pub_id,
            "version_id": version.id.to_string(),
            "version_pub_id": version.pub_id,
            "version_fingerprint": version.fingerprint,
        }),
    );
    Ok(version)
}

fn resolve_or_create_account_sqlite(
    tx: &Transaction<'_>,
    agent: &AgentIdentity,
) -> Result<AgentAccount, RegistrationError> {
    if let Some(account) = query_account_for_agent(tx, &agent.id)? {
        log_event(
            "info",
            "registration_account_reused",
            json!({
                "store": "sqlite",
                "agent_id": agent.id.to_string(),
                "agent_pub_id": agent.pub_id,
                "account_id": account.id.to_string(),
                "account_pub_id": account.pub_id,
            }),
        );
        return Ok(account);
    }

    let now = Utc::now();
    let id = AgentAccountId::new();
    let account = AgentAccount {
        pub_id: format!("aga_{}", id.public_suffix()),
        id,
        agent_id: agent.id.clone(),
        owner_user_id: agent.owner_user_id.clone(),
        account_status: AccountStatus::Active,
        created_at: now,
        updated_at: now,
    };

    tx.execute(
        "INSERT INTO agent_accounts
         (id, pub_id, agent_id, owner_user_id, account_status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            account.id.to_string(),
            &account.pub_id,
            account.agent_id.to_string(),
            account.owner_user_id.to_string(),
            account_status(&account.account_status),
            account.created_at.to_rfc3339(),
            account.updated_at.to_rfc3339(),
        ],
    )?;

    log_event(
        "info",
        "registration_account_created",
        json!({
            "store": "sqlite",
            "agent_id": agent.id.to_string(),
            "agent_pub_id": agent.pub_id,
            "account_id": account.id.to_string(),
            "account_pub_id": account.pub_id,
        }),
    );
    Ok(account)
}

fn create_session_sqlite(
    tx: &Transaction<'_>,
    agent: &AgentIdentity,
    version: &AgentVersion,
    request: &RegisterAgentRequest,
) -> Result<AgentSession, RegistrationError> {
    let id = AgentSessionId::new();
    let session = AgentSession {
        pub_id: format!("ags_{}", id.public_suffix()),
        id,
        agent_id: agent.id.clone(),
        owner_user_id: agent.owner_user_id.clone(),
        mcp_client_name: request.mcp_client_name.clone(),
        mcp_client_version: request.mcp_client_version.clone(),
        created_at: Utc::now(),
    };

    tx.execute(
        "INSERT INTO agent_sessions
         (id, pub_id, agent_id, owner_user_id, agent_version_id, mcp_client_name, mcp_client_version, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session.id.to_string(),
            &session.pub_id,
            session.agent_id.to_string(),
            session.owner_user_id.to_string(),
            version.id.to_string(),
            &session.mcp_client_name,
            &session.mcp_client_version,
            session.created_at.to_rfc3339(),
        ],
    )?;

    log_event(
        "info",
        "registration_session_created",
        json!({
            "store": "sqlite",
            "agent_id": agent.id.to_string(),
            "agent_pub_id": agent.pub_id,
            "version_id": version.id.to_string(),
            "version_pub_id": version.pub_id,
            "session_id": session.id.to_string(),
            "session_pub_id": session.pub_id,
            "mcp_client_name": session.mcp_client_name,
            "mcp_client_version": session.mcp_client_version,
        }),
    );
    Ok(session)
}

fn resolve_identity_fingerprint_sqlite(
    conn: &impl Queryable,
    owner_user_id: &UserId,
    fingerprint: &str,
) -> Result<Option<IdentityFingerprintResolution>, StorageError> {
    if let Some(agent) = conn.query_agent(
        "SELECT a.id, a.pub_id, a.fingerprint, a.display_name, a.description, a.owner_user_id, a.agent_type, a.agent_status, a.created_at, a.updated_at
         FROM agent_identity_aliases alias
         JOIN agent_identities a ON a.id = alias.agent_id
         WHERE alias.owner_user_id = ?1 AND alias.fingerprint = ?2",
        &[owner_user_id.to_string(), fingerprint.to_string()],
    )? {
        return Ok(Some(IdentityFingerprintResolution {
            agent,
            via_alias: true,
        }));
    }
    Ok(
        query_agent_by_owner_and_fingerprint(conn, owner_user_id, fingerprint)?.map(|agent| {
            IdentityFingerprintResolution {
                agent,
                via_alias: false,
            }
        }),
    )
}

/// Name the registration request asks for: the identity payload's
/// `agent_name` when present, otherwise the reviewed display name.
fn requested_agent_name(request: &RegisterAgentRequest) -> &str {
    request
        .identity_payload
        .as_ref()
        .and_then(|payload| payload.get("agent_name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&request.display_name)
}

fn current_name_used_by_other_agent_sqlite(
    tx: &Transaction<'_>,
    agent: &AgentIdentity,
    normalized_name: &str,
) -> Result<bool, StorageError> {
    // Normalization is Unicode-aware, so compare in Rust instead of SQL lower().
    let mut stmt = tx.prepare(
        "SELECT display_name FROM agent_identities WHERE owner_user_id = ?1 AND id != ?2",
    )?;
    let names = stmt.query_map(
        params![agent.owner_user_id.to_string(), agent.id.to_string()],
        |row| row.get::<_, String>(0),
    )?;
    for name in names {
        if normalize_agent_name(&name?) == normalized_name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn name_reserved_by_other_agent_sqlite(
    conn: &impl Queryable,
    owner_user_id: &UserId,
    normalized_name: &str,
    except: Option<&AgentId>,
) -> Result<bool, StorageError> {
    conn.exists(
        "SELECT 1 FROM agent_identity_aliases
         WHERE owner_user_id = ?1 AND normalized_name = ?2 AND agent_id != ?3",
        &[
            owner_user_id.to_string(),
            normalized_name.to_string(),
            except.map(ToString::to_string).unwrap_or_default(),
        ],
    )
}

fn insert_alias_if_missing_sqlite(
    tx: &Transaction<'_>,
    agent: &AgentIdentity,
    fingerprint: &str,
    source: &str,
    normalized_name: &str,
    revision: i64,
    now: &chrono::DateTime<Utc>,
) -> Result<(), RegistrationError> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT agent_id FROM agent_identity_aliases WHERE owner_user_id = ?1 AND fingerprint = ?2",
            params![agent.owner_user_id.to_string(), fingerprint],
            |row| row.get(0),
        )
        .optional()?;
    match existing {
        Some(agent_id) if agent_id == agent.id.to_string() => Ok(()),
        Some(_) => Err(RegistrationError::IdentityFingerprintCollision),
        None => {
            tx.execute(
                "INSERT INTO agent_identity_aliases
                 (owner_user_id, fingerprint, agent_id, source, normalized_name, revision, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent.owner_user_id.to_string(),
                    fingerprint,
                    agent.id.to_string(),
                    source,
                    normalized_name,
                    revision,
                    now.to_rfc3339(),
                ],
            )?;
            Ok(())
        }
    }
}

fn query_identity_payload_for_agent(
    conn: &Connection,
    agent_id: &AgentId,
) -> Result<Option<serde_json::Value>, StorageError> {
    let payload: Option<Option<String>> = conn
        .query_row(
            "SELECT identity_payload_json FROM agent_identities WHERE id = ?1",
            params![agent_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    payload
        .flatten()
        .map(|payload| serde_json::from_str(&payload))
        .transpose()
        .map_err(Into::into)
}

fn query_identity_revisions(
    conn: &Connection,
    agent_id: &AgentId,
) -> Result<Vec<AgentIdentityRevision>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT revision, previous_fingerprint, fingerprint, changes_json, actor, reason, created_at
         FROM agent_identity_revisions
         WHERE agent_id = ?1
         ORDER BY revision ASC",
    )?;
    let rows = stmt.query_map(params![agent_id.to_string()], |row| {
        let revision: i64 = row.get(0)?;
        let changes_json: String = row.get(3)?;
        let created_at: String = row.get(6)?;
        Ok(AgentIdentityRevision {
            agent_id: agent_id.clone(),
            revision: revision as u64,
            previous_fingerprint: row.get(1)?,
            fingerprint: row.get(2)?,
            changes: serde_json::from_str::<Vec<IdentityFieldChange>>(&changes_json)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            actor: row.get(4)?,
            reason: row.get(5)?,
            created_at: parse_timestamp(&created_at)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn query_identity_aliases(
    conn: &Connection,
    agent_id: &AgentId,
) -> Result<Vec<AgentIdentityAlias>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint, source, revision, created_at, normalized_name
         FROM agent_identity_aliases
         WHERE agent_id = ?1
         ORDER BY revision ASC, created_at ASC",
    )?;
    let rows = stmt.query_map(params![agent_id.to_string()], |row| {
        let revision: i64 = row.get(2)?;
        let created_at: String = row.get(3)?;
        Ok(AgentIdentityAlias {
            agent_id: agent_id.clone(),
            fingerprint: row.get(0)?,
            source: row.get(1)?,
            normalized_name: row.get(4)?,
            revision: revision as u64,
            created_at: parse_timestamp(&created_at)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn agent_type_name(agent_type: &hubu_common::models::identity::AgentType) -> &'static str {
    match agent_type {
        hubu_common::models::identity::AgentType::InteractiveAgent => "interactive_agent",
        hubu_common::models::identity::AgentType::AutonomousAgent => "autonomous_agent",
    }
}

fn query_agent_by_owner_and_fingerprint(
    conn: &impl Queryable,
    owner_user_id: &UserId,
    fingerprint: &str,
) -> Result<Option<AgentIdentity>, StorageError> {
    conn.query_agent(
        "SELECT id, pub_id, fingerprint, display_name, description, owner_user_id, agent_type, agent_status, created_at, updated_at
         FROM agent_identities
         WHERE owner_user_id = ?1 AND fingerprint = ?2",
        &[owner_user_id.to_string(), fingerprint.to_string()],
    )
}

fn query_agent_by_pub_id(
    conn: &impl Queryable,
    pub_id: &str,
) -> Result<Option<AgentIdentity>, StorageError> {
    conn.query_agent(
        "SELECT id, pub_id, fingerprint, display_name, description, owner_user_id, agent_type, agent_status, created_at, updated_at
         FROM agent_identities
         WHERE pub_id = ?1",
        &[pub_id.to_string()],
    )
}

fn query_agent_for_id(
    conn: &impl Queryable,
    agent_id: &AgentId,
) -> Result<Option<AgentIdentity>, StorageError> {
    conn.query_agent(
        "SELECT id, pub_id, fingerprint, display_name, description, owner_user_id, agent_type, agent_status, created_at, updated_at
         FROM agent_identities
         WHERE id = ?1",
        &[agent_id.to_string()],
    )
}

fn query_version_by_agent_and_fingerprint(
    conn: &impl Queryable,
    agent_id: &AgentId,
    fingerprint: &str,
) -> Result<Option<AgentVersion>, StorageError> {
    conn.query_version(
        "SELECT id, pub_id, agent_id, fingerprint, code_ref_json, model_json, runtime_json, created_at
         FROM agent_versions
         WHERE agent_id = ?1 AND fingerprint = ?2",
        &[agent_id.to_string(), fingerprint.to_string()],
    )
}

fn query_account_for_agent(
    conn: &impl Queryable,
    agent_id: &AgentId,
) -> Result<Option<AgentAccount>, StorageError> {
    conn.query_account(
        "SELECT id, pub_id, agent_id, owner_user_id, account_status, created_at, updated_at
         FROM agent_accounts
         WHERE agent_id = ?1",
        &[agent_id.to_string()],
    )
}

fn query_account_by_pub_id(
    conn: &impl Queryable,
    pub_id: &str,
) -> Result<Option<AgentAccount>, StorageError> {
    conn.query_account(
        "SELECT id, pub_id, agent_id, owner_user_id, account_status, created_at, updated_at
         FROM agent_accounts
         WHERE pub_id = ?1",
        &[pub_id.to_string()],
    )
}

fn query_agents_for_user(
    conn: &Connection,
    owner_user_id: &UserId,
) -> Result<Vec<AgentWithAccount>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT
             a.id, a.pub_id, a.fingerprint, a.display_name, a.description, a.owner_user_id,
             a.agent_type, a.agent_status, a.created_at, a.updated_at,
             acct.id, acct.pub_id, acct.agent_id, acct.owner_user_id, acct.account_status,
             acct.created_at, acct.updated_at
         FROM agent_identities a
         JOIN agent_accounts acct ON acct.agent_id = a.id
         WHERE a.owner_user_id = ?1
         ORDER BY a.created_at ASC, a.id ASC",
    )?;
    let rows = stmt.query_map(params![owner_user_id.to_string()], |row| {
        Ok(AgentWithAccount {
            agent: agent_from_row(row)?,
            account: account_from_row_offset(row, 10)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn account_from_row_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<AgentAccount> {
    use std::str::FromStr;

    let id: String = row.get(offset)?;
    let agent_id: String = row.get(offset + 2)?;
    let owner_user_id: String = row.get(offset + 3)?;
    let created_at: String = row.get(offset + 5)?;
    let updated_at: String = row.get(offset + 6)?;

    Ok(AgentAccount {
        id: AgentAccountId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        pub_id: row.get(offset + 1)?,
        agent_id: AgentId::from_str(&agent_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        owner_user_id: UserId::from_str(&owner_user_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_status: match row.get::<_, String>(offset + 4)?.as_str() {
            "active" => AccountStatus::Active,
            "suspended" => AccountStatus::Suspended,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
            .map_err(|_| rusqlite::Error::InvalidQuery)?
            .with_timezone(&Utc),
        updated_at: chrono::DateTime::parse_from_rfc3339(&updated_at)
            .map_err(|_| rusqlite::Error::InvalidQuery)?
            .with_timezone(&Utc),
    })
}

trait Queryable {
    fn query_agent(
        &self,
        sql: &str,
        params: &[String],
    ) -> Result<Option<AgentIdentity>, StorageError>;
    fn query_version(
        &self,
        sql: &str,
        params: &[String],
    ) -> Result<Option<AgentVersion>, StorageError>;
    fn query_account(
        &self,
        sql: &str,
        params: &[String],
    ) -> Result<Option<AgentAccount>, StorageError>;
    fn exists(&self, sql: &str, params: &[String]) -> Result<bool, StorageError>;
}

impl Queryable for Connection {
    fn query_agent(
        &self,
        sql: &str,
        values: &[String],
    ) -> Result<Option<AgentIdentity>, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), agent_from_row)
            .optional()?)
    }

    fn query_version(
        &self,
        sql: &str,
        values: &[String],
    ) -> Result<Option<AgentVersion>, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), version_from_row)
            .optional()?)
    }

    fn query_account(
        &self,
        sql: &str,
        values: &[String],
    ) -> Result<Option<AgentAccount>, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), account_from_row)
            .optional()?)
    }
    fn exists(&self, sql: &str, values: &[String]) -> Result<bool, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), |_| Ok(()))
            .optional()?
            .is_some())
    }
}

impl Queryable for Transaction<'_> {
    fn query_agent(
        &self,
        sql: &str,
        values: &[String],
    ) -> Result<Option<AgentIdentity>, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), agent_from_row)
            .optional()?)
    }

    fn query_version(
        &self,
        sql: &str,
        values: &[String],
    ) -> Result<Option<AgentVersion>, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), version_from_row)
            .optional()?)
    }

    fn query_account(
        &self,
        sql: &str,
        values: &[String],
    ) -> Result<Option<AgentAccount>, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), account_from_row)
            .optional()?)
    }
    fn exists(&self, sql: &str, values: &[String]) -> Result<bool, StorageError> {
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self
            .query_row(sql, rusqlite::params_from_iter(refs), |_| Ok(()))
            .optional()?
            .is_some())
    }
}

fn json_string<T: serde::Serialize>(value: &Option<T>) -> Result<Option<String>, StorageError> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubu_common::models::identity::{
        AgentType, CodeReference, ModelIdentity, RuntimeEnvironment, RuntimeIdentity,
    };

    fn user_id(value: &str) -> UserId {
        value.parse().unwrap()
    }

    fn test_request(identity_fingerprint: &str, version_fingerprint: &str) -> RegisterAgentRequest {
        RegisterAgentRequest {
            display_name: "Test Agent".to_string(),
            description: Some("MVE Agent to make money".to_string()),
            owner_user_id: user_id("00000000-0000-4000-8000-000000000123"),
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
            identity_payload: None,
        }
    }

    fn assert_public_id(pub_id: &str, prefix: &str) {
        let expected_prefix = format!("{prefix}_");
        assert!(pub_id.starts_with(&expected_prefix));
        assert_eq!(pub_id.len(), expected_prefix.len() + 12);
        assert!(pub_id[expected_prefix.len()..]
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()));
    }

    fn seed_sqlite_owner(manager: &RegistrationManager) {
        if let RegistrationStore::Sqlite(store) = &manager.store {
            let owner_user_id = user_id("00000000-0000-4000-8000-000000000123");
            let now = Utc::now().to_rfc3339();
            store
                .raw_connection()
                .execute(
                    "INSERT OR IGNORE INTO users
                     (id, pub_id, identity_key, display_name, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        owner_user_id.to_string(),
                        "usr_testowner",
                        "test:owner",
                        "Test Owner",
                        "active",
                        now,
                        now,
                    ],
                )
                .unwrap();
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
        assert_eq!(
            response.agent.pub_id,
            format!("agt_{}", response.agent.id.public_suffix())
        );
        assert_eq!(
            response.version.pub_id,
            format!("agv_{}", response.version.id.public_suffix())
        );
        assert_eq!(
            response.account.pub_id,
            format!("aga_{}", response.account.id.public_suffix())
        );
        assert_eq!(
            response.session.pub_id,
            format!("ags_{}", response.session.id.public_suffix())
        );
        assert_public_id(&response.agent.pub_id, "agt");
        assert_public_id(&response.version.pub_id, "agv");
        assert_public_id(&response.account.pub_id, "aga");
        assert_public_id(&response.session.pub_id, "ags");
        assert_eq!(response.version.agent_id, response.agent.id);
        assert_eq!(response.account.agent_id, response.agent.id);
        assert_eq!(response.session.agent_id, response.agent.id);
        assert_eq!(response.agent.owner_user_id, response.account.owner_user_id);
        assert_eq!(response.agent.owner_user_id, response.session.owner_user_id);
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

        assert_public_id(&first.agent.pub_id, "agt");
        assert_public_id(&second.agent.pub_id, "agt");
        assert_ne!(first.agent.pub_id, second.agent.pub_id);
        assert_eq!(
            manager.agent_id_for_pub_id(&first.agent.pub_id).unwrap(),
            Some(first.agent.id)
        );
        assert_eq!(
            manager.account_for_pub_id(&first.account.pub_id).unwrap(),
            Some(first.account)
        );
        assert_eq!(
            manager.agent_id_for_pub_id(&second.agent.pub_id).unwrap(),
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
    fn same_identity_fingerprint_can_belong_to_different_owners() {
        let mut manager = RegistrationManager::new();
        let first = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();

        let mut other_owner = test_request("sha256:agent-a", "sha256:version-a");
        other_owner.owner_user_id = user_id("00000000-0000-4000-8000-000000000456");
        let second = manager.register_agent(other_owner).unwrap();

        assert_ne!(first.agent.id, second.agent.id);
        assert_eq!(first.agent.fingerprint, second.agent.fingerprint);
        assert_ne!(first.agent.owner_user_id, second.agent.owner_user_id);
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

    #[test]
    fn sqlite_agent_identity_is_persisted_and_available_after_restart() {
        let path = std::env::temp_dir().join(format!("hubu-agent-{}.sqlite", AgentId::new()));
        let registered = {
            let mut manager = RegistrationManager::open(&path).unwrap();
            seed_sqlite_owner(&manager);
            manager
                .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
                .unwrap()
        };

        let manager = RegistrationManager::open(&path).unwrap();
        assert_eq!(
            manager.agent_for_id(&registered.agent.id).unwrap(),
            Some(registered.agent.clone())
        );
        assert_eq!(
            manager.version_for_id(&registered.version.id).unwrap(),
            Some(registered.version.clone())
        );
        assert_eq!(
            manager.account_for_agent(&registered.agent.id).unwrap(),
            Some(registered.account.clone())
        );
        assert_eq!(
            manager.session_for_id(&registered.session.id).unwrap(),
            Some(registered.session)
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn sqlite_reuses_version_for_same_fingerprint_and_creates_for_new_fingerprint() {
        let mut manager = RegistrationManager::in_memory_sqlite().unwrap();
        seed_sqlite_owner(&manager);
        let first = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();
        let same = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap();
        let new = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-b"))
            .unwrap();

        assert_eq!(first.version.id, same.version.id);
        assert_ne!(first.version.id, new.version.id);
        assert_eq!(first.account.id, new.account.id);
        assert_ne!(first.session.id, same.session.id);
    }

    #[test]
    fn sqlite_registration_rolls_back_when_session_insert_fails() {
        let mut manager = RegistrationManager::in_memory_sqlite().unwrap();
        seed_sqlite_owner(&manager);
        if let RegistrationStore::Sqlite(store) = &manager.store {
            store
                .raw_connection()
                .execute_batch(
                    "
                    CREATE TRIGGER fail_agent_sessions
                    BEFORE INSERT ON agent_sessions
                    BEGIN
                        SELECT RAISE(ABORT, 'forced session failure');
                    END;
                    ",
                )
                .unwrap();
        }

        let error = manager
            .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
            .unwrap_err();
        assert!(matches!(error, RegistrationError::Storage(_)));

        assert!(manager
            .agent_id_for_pub_id("agt_missing")
            .unwrap()
            .is_none());
        if let RegistrationStore::Sqlite(store) = &manager.store {
            let count: i64 = store
                .raw_connection()
                .query_row("SELECT COUNT(*) FROM agent_identities", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    fn rename_request(
        agent: &AgentIdentity,
        new_name: &str,
        new_fingerprint: &str,
    ) -> RenameAgentRequest {
        RenameAgentRequest {
            agent_id: agent.id.clone(),
            owner_user_id: agent.owner_user_id.clone(),
            expected_fingerprint: agent.fingerprint.clone(),
            previous_agent_name: agent.display_name.clone(),
            new_display_name: new_name.to_string(),
            new_fingerprint: new_fingerprint.to_string(),
            new_identity_payload: serde_json::json!({ "agent_name": new_name }),
            changes: vec![IdentityFieldChange {
                field: "agent_name".to_string(),
                old_value: Some(agent.display_name.clone()),
                new_value: Some(new_name.to_string()),
            }],
            actor: "usr_testowner".to_string(),
            reason: "fix typo".to_string(),
        }
    }

    fn rename_managers() -> Vec<RegistrationManager> {
        let sqlite = RegistrationManager::in_memory_sqlite().unwrap();
        seed_sqlite_owner(&sqlite);
        vec![RegistrationManager::new(), sqlite]
    }

    #[test]
    fn rename_keeps_agent_account_and_versions_and_records_revision() {
        for mut manager in rename_managers() {
            let first = manager
                .register_agent(test_request("sha256:old-name", "sha256:version-a"))
                .unwrap();

            let renamed = manager
                .rename_agent(rename_request(
                    &first.agent,
                    "Fixed Agent",
                    "sha256:new-name",
                ))
                .unwrap();

            assert_eq!(renamed.agent.id, first.agent.id);
            assert_eq!(renamed.agent.pub_id, first.agent.pub_id);
            assert_eq!(renamed.agent.display_name, "Fixed Agent");
            assert_eq!(renamed.agent.fingerprint, "sha256:new-name");
            assert_eq!(renamed.revision.revision, 1);
            assert_eq!(renamed.revision.previous_fingerprint, "sha256:old-name");
            assert_eq!(renamed.revision.actor, "usr_testowner");
            assert_eq!(renamed.revision.reason, "fix typo");
            assert_eq!(
                manager.agent_for_id(&first.agent.id).unwrap().unwrap(),
                renamed.agent
            );
            assert_eq!(
                manager.account_for_agent(&first.agent.id).unwrap(),
                Some(first.account.clone())
            );
            assert_eq!(
                manager.version_for_id(&first.version.id).unwrap(),
                Some(first.version.clone())
            );
            assert_eq!(
                manager.identity_revisions(&first.agent.id).unwrap(),
                vec![renamed.revision.clone()]
            );
            let aliases = manager.identity_aliases(&first.agent.id).unwrap();
            assert_eq!(
                aliases
                    .iter()
                    .map(|alias| (
                        alias.fingerprint.as_str(),
                        alias.source.as_str(),
                        alias.revision
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    ("sha256:old-name", "registration", 0),
                    ("sha256:new-name", "rename", 1)
                ]
            );
            assert_eq!(
                manager.identity_payload_for_agent(&first.agent.id).unwrap(),
                Some(serde_json::json!({ "agent_name": "Fixed Agent" }))
            );
        }
    }

    #[test]
    fn registration_resolves_old_and_new_fingerprints_through_aliases() {
        for mut manager in rename_managers() {
            let first = manager
                .register_agent(test_request("sha256:old-name", "sha256:version-a"))
                .unwrap();
            assert!(!first.resolved_via_alias);
            manager
                .rename_agent(rename_request(
                    &first.agent,
                    "Fixed Agent",
                    "sha256:new-name",
                ))
                .unwrap();

            let with_new = manager
                .register_agent(test_request("sha256:new-name", "sha256:version-new"))
                .unwrap();
            let with_old = manager
                .register_agent(test_request("sha256:old-name", "sha256:version-a"))
                .unwrap();

            for resumed in [&with_new, &with_old] {
                assert!(resumed.resolved_via_alias);
                assert_eq!(resumed.agent.id, first.agent.id);
                assert_eq!(resumed.agent.display_name, "Fixed Agent");
                assert_eq!(resumed.account.id, first.account.id);
                assert_ne!(resumed.session.id, first.session.id);
            }
            assert_eq!(with_old.version.id, first.version.id);
            assert_ne!(with_new.version.id, first.version.id);
            let owner = first.agent.owner_user_id.clone();
            assert_eq!(manager.agents_for_user(&owner).unwrap().len(), 1);
        }
    }

    #[test]
    fn rename_rejects_fingerprint_owned_by_another_agent() {
        for mut manager in rename_managers() {
            let first = manager
                .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
                .unwrap();
            let second = manager
                .register_agent(test_request("sha256:agent-b", "sha256:version-a"))
                .unwrap();

            let error = manager
                .rename_agent(rename_request(&first.agent, "Agent B", "sha256:agent-b"))
                .unwrap_err();
            assert_eq!(error, RegistrationError::IdentityFingerprintCollision);

            // An alias held by another agent is also never reassigned.
            let renamed_b = manager
                .rename_agent(rename_request(&second.agent, "Agent C", "sha256:agent-c"))
                .unwrap();
            let error = manager
                .rename_agent(rename_request(&first.agent, "Agent B", "sha256:agent-b"))
                .unwrap_err();
            assert_eq!(error, RegistrationError::IdentityFingerprintCollision);

            assert_eq!(
                manager.agent_for_id(&first.agent.id).unwrap().unwrap(),
                first.agent
            );
            assert_eq!(
                manager.agent_for_id(&second.agent.id).unwrap().unwrap(),
                renamed_b.agent
            );
            assert!(manager
                .identity_revisions(&first.agent.id)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn rename_rejects_stale_expected_fingerprint_no_op_and_foreign_owner() {
        for mut manager in rename_managers() {
            let first = manager
                .register_agent(test_request("sha256:agent-a", "sha256:version-a"))
                .unwrap();

            let mut stale = rename_request(&first.agent, "Renamed", "sha256:renamed");
            stale.expected_fingerprint = "sha256:something-else".to_string();
            assert_eq!(
                manager.rename_agent(stale).unwrap_err(),
                RegistrationError::RenameConflict
            );

            assert_eq!(
                manager
                    .rename_agent(rename_request(&first.agent, "Same", "sha256:agent-a"))
                    .unwrap_err(),
                RegistrationError::NoIdentityChange
            );

            let mut foreign = rename_request(&first.agent, "Renamed", "sha256:renamed");
            foreign.owner_user_id = user_id("00000000-0000-4000-8000-000000000456");
            assert_eq!(
                manager.rename_agent(foreign).unwrap_err(),
                RegistrationError::AgentNotFound
            );

            let mut no_reason = rename_request(&first.agent, "Renamed", "sha256:renamed");
            no_reason.reason = "  ".to_string();
            assert!(matches!(
                manager.rename_agent(no_reason).unwrap_err(),
                RegistrationError::InvalidRename(_)
            ));
        }
    }

    #[test]
    fn renaming_back_to_a_previous_name_reuses_its_alias() {
        for mut manager in rename_managers() {
            let first = manager
                .register_agent(test_request("sha256:old-name", "sha256:version-a"))
                .unwrap();
            let renamed = manager
                .rename_agent(rename_request(&first.agent, "New", "sha256:new-name"))
                .unwrap();
            let reverted = manager
                .rename_agent(rename_request(
                    &renamed.agent,
                    "Test Agent",
                    "sha256:old-name",
                ))
                .unwrap();

            assert_eq!(reverted.agent.id, first.agent.id);
            assert_eq!(reverted.revision.revision, 2);
            assert_eq!(manager.identity_aliases(&first.agent.id).unwrap().len(), 2);
            assert_eq!(
                manager.identity_revisions(&first.agent.id).unwrap().len(),
                2
            );
        }
    }

    #[test]
    fn sqlite_identity_aliases_and_revisions_are_append_only() {
        let mut manager = RegistrationManager::in_memory_sqlite().unwrap();
        seed_sqlite_owner(&manager);
        let first = manager
            .register_agent(test_request("sha256:old-name", "sha256:version-a"))
            .unwrap();
        manager
            .rename_agent(rename_request(&first.agent, "New", "sha256:new-name"))
            .unwrap();

        if let RegistrationStore::Sqlite(store) = &manager.store {
            let conn = store.raw_connection();
            for statement in [
                "UPDATE agent_identity_aliases SET agent_id = 'other'",
                "DELETE FROM agent_identity_aliases",
                "UPDATE agent_identity_revisions SET reason = 'rewritten'",
                "DELETE FROM agent_identity_revisions",
            ] {
                let error = conn.execute(statement, []).unwrap_err();
                assert!(
                    error.to_string().contains("immutable"),
                    "{statement}: {error}"
                );
            }
        }
    }

    #[test]
    fn sqlite_rename_survives_restart() {
        let path =
            std::env::temp_dir().join(format!("hubu-agent-rename-{}.sqlite", AgentId::new()));
        let first = {
            let mut manager = RegistrationManager::open(&path).unwrap();
            seed_sqlite_owner(&manager);
            let first = manager
                .register_agent(test_request("sha256:old-name", "sha256:version-a"))
                .unwrap();
            manager
                .rename_agent(rename_request(&first.agent, "New", "sha256:new-name"))
                .unwrap();
            first
        };

        let mut manager = RegistrationManager::open(&path).unwrap();
        assert_eq!(
            manager.identity_revisions(&first.agent.id).unwrap().len(),
            1
        );
        let resumed = manager
            .register_agent(test_request("sha256:old-name", "sha256:version-a"))
            .unwrap();
        assert!(resumed.resolved_via_alias);
        assert_eq!(resumed.agent.id, first.agent.id);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn pre_rename_schema_upgrades_and_legacy_agents_can_be_renamed() {
        let path =
            std::env::temp_dir().join(format!("hubu-agent-legacy-{}.sqlite", AgentId::new()));
        let owner = user_id("00000000-0000-4000-8000-000000000123");
        let agent_id = AgentId::new();
        {
            let conn = Connection::open(&path).unwrap();
            let now = Utc::now().to_rfc3339();
            conn.execute_batch(
                "CREATE TABLE users (id TEXT PRIMARY KEY, pub_id TEXT NOT NULL UNIQUE,
                     identity_key TEXT UNIQUE, display_name TEXT NOT NULL, email TEXT,
                     status TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
                 CREATE TABLE agent_identities (id TEXT PRIMARY KEY, pub_id TEXT NOT NULL UNIQUE,
                     fingerprint TEXT NOT NULL, display_name TEXT NOT NULL, description TEXT,
                     owner_user_id TEXT NOT NULL, agent_type TEXT NOT NULL,
                     agent_status TEXT NOT NULL, created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL, UNIQUE(owner_user_id, fingerprint),
                     FOREIGN KEY(owner_user_id) REFERENCES users(id));",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO users VALUES (?1, 'usr_testowner', 'test:owner', 'Owner', NULL, 'active', ?2, ?2)",
                params![owner.to_string(), now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_identities VALUES (?1, 'agt_legacyagent1', 'sha256:legacy', 'Legacy', NULL, ?2, 'autonomous_agent', 'active', ?3, ?3)",
                params![agent_id.to_string(), owner.to_string(), now],
            )
            .unwrap();
        }

        let mut manager = RegistrationManager::open(&path).unwrap();
        assert_eq!(manager.identity_payload_for_agent(&agent_id).unwrap(), None);
        assert!(manager.identity_revisions(&agent_id).unwrap().is_empty());
        let legacy = manager.agent_for_id(&agent_id).unwrap().unwrap();
        let renamed = manager
            .rename_agent(rename_request(&legacy, "Modern", "sha256:modern"))
            .unwrap();
        assert_eq!(renamed.agent.pub_id, "agt_legacyagent1");
        assert_eq!(
            manager
                .resolve_identity_fingerprint(&owner, "sha256:legacy")
                .unwrap()
                .map(|resolution| (resolution.agent.id, resolution.via_alias)),
            Some((agent_id, true))
        );
        std::fs::remove_file(path).ok();
    }

    fn named_request(name: &str, identity_fingerprint: &str) -> RegisterAgentRequest {
        let mut request = test_request(identity_fingerprint, "sha256:version-a");
        request.display_name = name.to_string();
        request.identity_payload = Some(serde_json::json!({ "agent_name": name }));
        request
    }

    #[test]
    fn normalize_agent_name_trims_and_ignores_case() {
        assert_eq!(normalize_agent_name("  Reserch-Agent "), "reserch-agent");
        assert_eq!(normalize_agent_name("ÄGENT"), normalize_agent_name("ägent"));
    }

    #[test]
    fn rename_rejects_another_agents_current_or_reserved_name() {
        for mut manager in rename_managers() {
            let a = manager
                .register_agent(named_request("alpha", "sha256:alpha"))
                .unwrap();
            let b = manager
                .register_agent(named_request("beta", "sha256:beta"))
                .unwrap();

            // Current name of another agent, compared trimmed and case-insensitively.
            assert_eq!(
                manager
                    .rename_agent(rename_request(&a.agent, " BETA ", "sha256:beta-2"))
                    .unwrap_err(),
                RegistrationError::AgentNameConflict
            );

            // Previous name of another agent stays reserved for it.
            let b_renamed = manager
                .rename_agent(rename_request(&b.agent, "gamma", "sha256:gamma"))
                .unwrap();
            assert_eq!(
                manager
                    .rename_agent(rename_request(&a.agent, "Beta", "sha256:beta-3"))
                    .unwrap_err(),
                RegistrationError::AgentNameReserved
            );

            // An agent may reclaim its own previous name.
            let back = manager
                .rename_agent(rename_request(&b_renamed.agent, "Beta", "sha256:beta-4"))
                .unwrap();
            assert_eq!(back.agent.id, b.agent.id);
            assert_eq!(manager.identity_revisions(&a.agent.id).unwrap(), vec![]);
        }
    }

    #[test]
    fn registration_cannot_create_an_agent_under_a_reserved_previous_name() {
        for mut manager in rename_managers() {
            let a = manager
                .register_agent(named_request("reserch-agent", "sha256:typo"))
                .unwrap();
            manager
                .rename_agent(rename_request(&a.agent, "research-agent", "sha256:fixed"))
                .unwrap();

            // A differently cased previous name hashes to a new fingerprint but
            // is still reserved for the renamed agent.
            let error = manager
                .register_agent(named_request("Reserch-Agent", "sha256:typo-cased"))
                .unwrap_err();
            assert_eq!(error, RegistrationError::AgentNameReserved);
            let owner = a.agent.owner_user_id.clone();
            assert_eq!(manager.agents_for_user(&owner).unwrap().len(), 1);
        }
    }
}
