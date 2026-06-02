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

use crate::registration::error::RegistrationError;
use crate::registration::model::{RegisterAgentRequest, RegisterAgentResponse};
use crate::storage::{
    account_from_row, account_status, agent_from_row, agent_status, agent_type, init_schema,
    session_from_row, version_from_row, StorageError,
};

/// Registration coordinator backed by a replaceable storage layer.
pub struct RegistrationManager {
    store: RegistrationStore,
}

impl RegistrationManager {
    pub fn new() -> Self {
        Self {
            store: RegistrationStore::Memory(MemoryRegistrationStore::new()),
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

    pub fn session_for_id(
        &self,
        session_id: &AgentSessionId,
    ) -> Result<Option<AgentSession>, RegistrationError> {
        Ok(self.store.session_for_id(session_id)?)
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
        Ok(self.store.register_agent(&request)?)
    }

    fn validate_request(&self, request: &RegisterAgentRequest) -> Result<(), RegistrationError> {
        if request.identity_fingerprint.is_empty() || request.version_fingerprint.is_empty() {
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
    Memory(MemoryRegistrationStore),
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
    version_by_agent_and_fingerprint: HashMap<(AgentId, String), AgentVersionId>,
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
            version_by_agent_and_fingerprint: HashMap::new(),
        }
    }

    fn register_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<RegisterAgentResponse, RegistrationError> {
        let agent = self.resolve_or_create_agent(request)?;
        let version = self.resolve_or_create_agent_version(&agent, request)?;
        let account = self.resolve_or_create_account(&agent);
        let session = self.create_session(&agent, request);

        Ok(RegisterAgentResponse {
            agent,
            version,
            account,
            session,
        })
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

    fn session_for_id(
        &self,
        session_id: &AgentSessionId,
    ) -> Result<Option<AgentSession>, StorageError> {
        Ok(self.sessions.get(session_id).cloned())
    }

    fn resolve_or_create_agent(
        &mut self,
        request: &RegisterAgentRequest,
    ) -> Result<AgentIdentity, RegistrationError> {
        let key = (
            request.owner_user_id.clone(),
            request.identity_fingerprint.clone(),
        );
        if let Some(agent_id) = self.agent_by_owner_and_fingerprint.get(&key) {
            let agent = self
                .agents
                .get(agent_id)
                .expect("agent index is stale")
                .clone();

            if agent.agent_type != request.agent_type {
                return Err(RegistrationError::IdentityConflict);
            }

            return Ok(agent);
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
            pub_id: format!("aga_{}", id.public_suffix()),
            agent_id: agent.id.clone(),
            owner_user_id: agent.owner_user_id.clone(),
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
            pub_id: format!("ags_{}", id.public_suffix()),
            agent_id: agent.id.clone(),
            owner_user_id: agent.owner_user_id.clone(),
            mcp_client_name: request.mcp_client_name.clone(),
            mcp_client_version: request.mcp_client_version.clone(),
            created_at: Utc::now(),
        };

        self.sessions.insert(id, session.clone());

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

        let agent = resolve_or_create_agent_sqlite(&sqlite_tx, request)?;
        let version = resolve_or_create_agent_version_sqlite(&sqlite_tx, &agent, request)?;
        let account = resolve_or_create_account_sqlite(&sqlite_tx, &agent)?;
        let session = create_session_sqlite(&sqlite_tx, &agent, &version, request)?;

        sqlite_tx.commit()?;

        Ok(RegisterAgentResponse {
            agent,
            version,
            account,
            session,
        })
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
) -> Result<AgentIdentity, RegistrationError> {
    if let Some(agent) = query_agent_by_owner_and_fingerprint(
        tx,
        &request.owner_user_id,
        &request.identity_fingerprint,
    )? {
        if agent.agent_type != request.agent_type {
            return Err(RegistrationError::IdentityConflict);
        }
        return Ok(agent);
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
         (id, pub_id, fingerprint, display_name, description, owner_user_id, agent_type, agent_status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        ],
    )?;

    Ok(agent)
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
            return Err(RegistrationError::VersionConflict);
        }
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

    Ok(version)
}

fn resolve_or_create_account_sqlite(
    tx: &Transaction<'_>,
    agent: &AgentIdentity,
) -> Result<AgentAccount, RegistrationError> {
    if let Some(account) = query_account_for_agent(tx, &agent.id)? {
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

    Ok(session)
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
}
