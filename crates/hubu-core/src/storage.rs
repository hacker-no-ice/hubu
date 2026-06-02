use std::str::FromStr;

use chrono::{DateTime, Utc};
use hubu_common::models::{
    account::{AccountStatus, AgentAccount},
    identity::{AgentIdentity, AgentStatus, AgentType, AgentVersion},
    session::AgentSession,
    User, UserStatus,
};
use rusqlite::{Connection, Row};

pub(crate) const DEFAULT_USER_IDENTITY_KEY: &str = "system:local-default";
pub(crate) const SELECTED_DEFAULT_USER_ID_KEY: &str = "selected_default_user_id";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(String),

    #[error("json error: {0}")]
    Json(String),

    #[error("stored value is invalid: {0}")]
    InvalidData(String),
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error.to_string())
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

pub(crate) fn init_schema(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            pub_id TEXT NOT NULL UNIQUE,
            identity_key TEXT UNIQUE,
            display_name TEXT NOT NULL,
            email TEXT,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS users_email_unique
        ON users(email)
        WHERE email IS NOT NULL;

        CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agent_identities (
            id TEXT PRIMARY KEY,
            pub_id TEXT NOT NULL UNIQUE,
            fingerprint TEXT NOT NULL,
            display_name TEXT NOT NULL,
            description TEXT,
            owner_user_id TEXT NOT NULL,
            agent_type TEXT NOT NULL,
            agent_status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(owner_user_id, fingerprint),
            FOREIGN KEY(owner_user_id) REFERENCES users(id)
        );

        CREATE TABLE IF NOT EXISTS agent_versions (
            id TEXT PRIMARY KEY,
            pub_id TEXT NOT NULL UNIQUE,
            agent_id TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            code_ref_json TEXT,
            model_json TEXT,
            runtime_json TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(agent_id, fingerprint),
            FOREIGN KEY(agent_id) REFERENCES agent_identities(id)
        );

        CREATE TABLE IF NOT EXISTS agent_accounts (
            id TEXT PRIMARY KEY,
            pub_id TEXT NOT NULL UNIQUE,
            agent_id TEXT NOT NULL UNIQUE,
            owner_user_id TEXT NOT NULL,
            account_status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(agent_id) REFERENCES agent_identities(id),
            FOREIGN KEY(owner_user_id) REFERENCES users(id)
        );

        CREATE TABLE IF NOT EXISTS agent_sessions (
            id TEXT PRIMARY KEY,
            pub_id TEXT NOT NULL UNIQUE,
            agent_id TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            agent_version_id TEXT NOT NULL,
            mcp_client_name TEXT,
            mcp_client_version TEXT,
            created_at TEXT NOT NULL,
            FOREIGN KEY(agent_id) REFERENCES agent_identities(id),
            FOREIGN KEY(owner_user_id) REFERENCES users(id),
            FOREIGN KEY(agent_version_id) REFERENCES agent_versions(id)
        );

        CREATE TRIGGER IF NOT EXISTS agent_versions_no_update
        BEFORE UPDATE ON agent_versions
        BEGIN
            SELECT RAISE(ABORT, 'agent versions are immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS agent_versions_no_delete
        BEFORE DELETE ON agent_versions
        BEGIN
            SELECT RAISE(ABORT, 'agent versions are immutable');
        END;
        ",
    )?;
    Ok(())
}

pub(crate) fn user_from_row(row: &Row<'_>) -> Result<User, rusqlite::Error> {
    let id: String = row.get(0)?;
    let status: String = row.get(4)?;
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;

    Ok(User {
        id: parse_id(&id)?,
        pub_id: row.get(1)?,
        display_name: row.get(2)?,
        email: row.get(3)?,
        status: parse_user_status(&status)?,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
    })
}

pub(crate) fn agent_from_row(row: &Row<'_>) -> Result<AgentIdentity, rusqlite::Error> {
    let id: String = row.get(0)?;
    let owner_user_id: String = row.get(5)?;
    let agent_type: String = row.get(6)?;
    let agent_status: String = row.get(7)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;

    Ok(AgentIdentity {
        id: parse_id(&id)?,
        pub_id: row.get(1)?,
        fingerprint: row.get(2)?,
        display_name: row.get(3)?,
        description: row.get(4)?,
        owner_user_id: parse_id(&owner_user_id)?,
        agent_type: parse_agent_type(&agent_type)?,
        agent_status: parse_agent_status(&agent_status)?,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
    })
}

pub(crate) fn version_from_row(row: &Row<'_>) -> Result<AgentVersion, rusqlite::Error> {
    let id: String = row.get(0)?;
    let agent_id: String = row.get(2)?;
    let created_at: String = row.get(7)?;
    let code_ref_json: Option<String> = row.get(4)?;
    let model_json: Option<String> = row.get(5)?;
    let runtime_json: Option<String> = row.get(6)?;

    Ok(AgentVersion {
        id: parse_id(&id)?,
        pub_id: row.get(1)?,
        agent_id: parse_id(&agent_id)?,
        fingerprint: row.get(3)?,
        code_ref: parse_json(code_ref_json)?,
        model: parse_json(model_json)?,
        runtime: parse_json(runtime_json)?,
        created_at: parse_timestamp(&created_at)?,
    })
}

pub(crate) fn account_from_row(row: &Row<'_>) -> Result<AgentAccount, rusqlite::Error> {
    let id: String = row.get(0)?;
    let agent_id: String = row.get(2)?;
    let owner_user_id: String = row.get(3)?;
    let status: String = row.get(4)?;
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;

    Ok(AgentAccount {
        id: parse_id(&id)?,
        pub_id: row.get(1)?,
        agent_id: parse_id(&agent_id)?,
        owner_user_id: parse_id(&owner_user_id)?,
        account_status: parse_account_status(&status)?,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
    })
}

pub(crate) fn session_from_row(row: &Row<'_>) -> Result<AgentSession, rusqlite::Error> {
    let id: String = row.get(0)?;
    let agent_id: String = row.get(2)?;
    let owner_user_id: String = row.get(3)?;
    let created_at: String = row.get(6)?;

    Ok(AgentSession {
        id: parse_id(&id)?,
        pub_id: row.get(1)?,
        agent_id: parse_id(&agent_id)?,
        owner_user_id: parse_id(&owner_user_id)?,
        mcp_client_name: row.get(4)?,
        mcp_client_version: row.get(5)?,
        created_at: parse_timestamp(&created_at)?,
    })
}

pub(crate) fn user_status(value: &UserStatus) -> &'static str {
    match value {
        UserStatus::Active => "active",
        UserStatus::Suspended => "suspended",
    }
}

pub(crate) fn agent_type(value: &AgentType) -> &'static str {
    match value {
        AgentType::InteractiveAgent => "interactive_agent",
        AgentType::AutonomousAgent => "autonomous_agent",
    }
}

pub(crate) fn agent_status(value: &AgentStatus) -> &'static str {
    match value {
        AgentStatus::Active => "active",
        AgentStatus::Suspended => "suspended",
    }
}

pub(crate) fn account_status(value: &AccountStatus) -> &'static str {
    match value {
        AccountStatus::Active => "active",
        AccountStatus::Suspended => "suspended",
    }
}

pub(crate) fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_id<T>(value: &str) -> Result<T, rusqlite::Error>
where
    T: FromStr,
{
    value.parse().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_json<T>(value: Option<String>) -> Result<Option<T>, rusqlite::Error>
where
    T: serde::de::DeserializeOwned,
{
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()
}

fn parse_user_status(value: &str) -> Result<UserStatus, rusqlite::Error> {
    match value {
        "active" => Ok(UserStatus::Active),
        "suspended" => Ok(UserStatus::Suspended),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_agent_type(value: &str) -> Result<AgentType, rusqlite::Error> {
    match value {
        "interactive_agent" => Ok(AgentType::InteractiveAgent),
        "autonomous_agent" => Ok(AgentType::AutonomousAgent),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_agent_status(value: &str) -> Result<AgentStatus, rusqlite::Error> {
    match value {
        "active" => Ok(AgentStatus::Active),
        "suspended" => Ok(AgentStatus::Suspended),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_account_status(value: &str) -> Result<AccountStatus, rusqlite::Error> {
    match value {
        "active" => Ok(AccountStatus::Active),
        "suspended" => Ok(AccountStatus::Suspended),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
