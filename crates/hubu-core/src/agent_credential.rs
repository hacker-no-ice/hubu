//! Per-agent and registration credentials (v0.3.1 spec, sections 4 and 5).
//!
//! The secret is minted by the client (the Hubu MCP server or the owner CLI)
//! and never reaches Hubu: a registration carries only the credential id and
//! the verification hash, SHA-256 of the secret. Hubu stores that hash, looks
//! a presented credential up by its id, and compares hashes in constant time.
//!
//! Format: `hubu_agent_<credential_id>_<secret>` for an agent credential and
//! `hubu_reg_<credential_id>_<secret>` for a registration credential. The id is
//! 16 random bytes and the secret 32 random bytes, both lowercase base32
//! without padding. The fixed prefixes let logs and secret scanners redact
//! them.
//!
//! An agent has at most one live (pending or active) credential. A pending
//! credential cannot spend until the owner confirms it, and it expires after
//! [`PENDING_CREDENTIAL_TTL_DAYS`]. Revocation takes effect on the next
//! request because every request looks the credential up again.
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use hubu_common::ids::{AgentAccountId, AgentId, UserId};
use rusqlite::{params, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};

use crate::storage::StorageError;

pub const AGENT_CREDENTIAL_PREFIX: &str = "hubu_agent_";
pub const REGISTRATION_CREDENTIAL_PREFIX: &str = "hubu_reg_";
pub const VERIFICATION_HASH_PREFIX: &str = "sha256:";
/// At most this many registrations may wait for the owner at once.
pub const MAX_PENDING_CREDENTIALS: usize = 10;
/// A registration the owner has not confirmed expires after this many days.
pub const PENDING_CREDENTIAL_TTL_DAYS: i64 = 7;
const CREDENTIAL_ID_BYTES: usize = 16;
const SECRET_BYTES: usize = 32;
/// `last_used_at` is refreshed at most this often, so a busy agent does not
/// write on every request.
const LAST_USED_REFRESH_SECONDS: i64 = 60;
const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Which class a presented credential claims by its prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    Agent,
    Registration,
}

impl CredentialKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Agent => AGENT_CREDENTIAL_PREFIX,
            Self::Registration => REGISTRATION_CREDENTIAL_PREFIX,
        }
    }
}

/// A presented credential split into its id and secret. The secret is kept
/// only long enough to hash it and is never formatted by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ParsedCredential {
    pub kind: CredentialKind,
    pub credential_id: String,
    secret: String,
}

impl std::fmt::Debug for ParsedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedCredential")
            .field("kind", &self.kind)
            .field("credential_id", &self.credential_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl ParsedCredential {
    pub fn verification_hash(&self) -> String {
        verification_hash(&self.secret)
    }
}

/// Splits a bearer into kind, id and secret. Returns `None` for anything that
/// is not exactly the per-agent or registration format, including the shared
/// v0.3.0 agent credential.
pub fn parse_credential(token: &str) -> Option<ParsedCredential> {
    let (kind, rest) = [CredentialKind::Agent, CredentialKind::Registration]
        .into_iter()
        .find_map(|kind| token.strip_prefix(kind.prefix()).map(|rest| (kind, rest)))?;
    let (credential_id, secret) = rest.split_once('_')?;
    if !is_base32(credential_id, CREDENTIAL_ID_BYTES) || !is_base32(secret, SECRET_BYTES) {
        return None;
    }
    Some(ParsedCredential {
        kind,
        credential_id: credential_id.to_string(),
        secret: secret.to_string(),
    })
}

/// Formats a credential from caller-supplied random bytes: the first 16 bytes
/// become the id and the last 32 the secret. The caller owns the entropy
/// source, so this module needs no random number generator.
pub fn format_credential(kind: CredentialKind, entropy: &[u8; 48]) -> String {
    format!(
        "{}{}_{}",
        kind.prefix(),
        base32(&entropy[..CREDENTIAL_ID_BYTES]),
        base32(&entropy[CREDENTIAL_ID_BYTES..])
    )
}

/// `sha256:<hex>` of the secret part of a credential.
pub fn verification_hash(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(VERIFICATION_HASH_PREFIX.len() + 64);
    out.push_str(VERIFICATION_HASH_PREFIX);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn is_valid_credential_id(credential_id: &str) -> bool {
    is_base32(credential_id, CREDENTIAL_ID_BYTES)
}

pub fn is_valid_verification_hash(hash: &str) -> bool {
    hash.strip_prefix(VERIFICATION_HASH_PREFIX)
        .is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn base32_len(bytes: usize) -> usize {
    (bytes * 8).div_ceil(5)
}

fn is_base32(value: &str, bytes: usize) -> bool {
    value.len() == base32_len(bytes) && value.bytes().all(|b| BASE32_ALPHABET.contains(&b))
}

fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(base32_len(bytes.len()));
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32_ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentCredentialStatus {
    Pending,
    Active,
    Revoked,
}

impl AgentCredentialStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }

    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "revoked" => Ok(Self::Revoked),
            other => Err(StorageError::InvalidData(format!(
                "unknown agent credential status `{other}`"
            ))),
        }
    }
}

/// One stored agent credential. Holds the verification hash, never the
/// secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCredential {
    pub credential_id: String,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub account_id: AgentAccountId,
    pub status: AgentCredentialStatus,
    /// Harness the registering client reported. Self-reported until caller
    /// attestation ships.
    pub harness: Option<String>,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    verification_hash: String,
}

impl AgentCredential {
    /// A pending credential the owner did not confirm within the TTL.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.status == AgentCredentialStatus::Pending
            && now - self.created_at >= Duration::days(PENDING_CREDENTIAL_TTL_DAYS)
    }

    /// `pending`, `active`, `revoked`, or `expired` for an unconfirmed pending
    /// credential past its TTL.
    pub fn display_status(&self, now: DateTime<Utc>) -> &'static str {
        if self.is_expired(now) {
            "expired"
        } else {
            self.status.as_str()
        }
    }

    fn hash_matches(&self, hash: &str) -> bool {
        constant_time_eq(self.verification_hash.as_bytes(), hash.as_bytes())
    }
}

/// What a registration asks to record for its agent.
#[derive(Clone, Debug)]
pub struct NewAgentCredential {
    pub credential_id: String,
    pub verification_hash: String,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub account_id: AgentAccountId,
    pub harness: Option<String>,
}

/// How a registration's credential was recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordedCredential {
    /// A new pending credential waiting for the owner.
    Created(AgentCredential),
    /// The same credential was already recorded for this agent; nothing new.
    Existing(AgentCredential),
}

impl RecordedCredential {
    pub fn credential(&self) -> &AgentCredential {
        match self {
            Self::Created(credential) | Self::Existing(credential) => credential,
        }
    }
}

/// The result of checking a presented agent credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentCredentialCheck {
    Active(AgentCredential),
    Pending(AgentCredential),
    Revoked,
    Expired,
    /// Unknown id or a hash that does not match.
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationCredential {
    pub credential_id: String,
    pub owner_user_id: UserId,
    pub harness: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    verification_hash: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgentCredentialError {
    #[error("credential_id must be 26 lowercase base32 characters (16 bytes)")]
    InvalidCredentialId,
    #[error("verification_hash must be `sha256:` followed by 64 lowercase hex characters")]
    InvalidVerificationHash,
    #[error("credential id is already in use")]
    CredentialIdConflict,
    #[error(
        "this agent already has a credential; ask the owner to run `hubu agent rotate` to replace it"
    )]
    AlreadyRegistered,
    #[error("more than {MAX_PENDING_CREDENTIALS} registrations are waiting for the owner")]
    TooManyPending,
    #[error("the agent has no pending registration to confirm")]
    NotPending,
    #[error("the agent has no active credential")]
    NotActive,
    #[error("the agent has no credential")]
    NoCredential,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for AgentCredentialError {
    fn from(error: rusqlite::Error) -> Self {
        StorageError::from(error).into()
    }
}

/// SQLite store for agent and registration credentials. It opens its own
/// connection to the Hubu database, like the other managers.
pub struct AgentCredentialManager {
    conn: Connection,
}

const CREDENTIAL_COLUMNS: &str = "credential_id, owner_user_id, agent_id, account_id, status, harness, created_at, activated_at, revoked_at, last_used_at, verification_hash";

impl AgentCredentialManager {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AgentCredentialError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, AgentCredentialError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, AgentCredentialError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS agent_credentials (
                credential_id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'revoked')),
                harness TEXT,
                created_at TEXT NOT NULL,
                activated_at TEXT,
                revoked_at TEXT,
                last_used_at TEXT,
                verification_hash TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS agent_credentials_one_live_per_agent
            ON agent_credentials(agent_id)
            WHERE status IN ('pending', 'active');
            CREATE INDEX IF NOT EXISTS agent_credentials_owner_status
            ON agent_credentials(owner_user_id, status);

            CREATE TABLE IF NOT EXISTS registration_credentials (
                credential_id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                harness TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
                created_at TEXT NOT NULL,
                revoked_at TEXT,
                last_used_at TEXT,
                verification_hash TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS registration_credentials_one_per_harness
            ON registration_credentials(owner_user_id, harness)
            WHERE status = 'active';
            ",
        )?;
        Ok(Self { conn })
    }

    /// Records the credential a registration brought (spec 5.2 and 5.7).
    ///
    /// The same id and hash for the same agent is idempotent. A different
    /// credential while one is pending or active is refused, so a second
    /// client cannot replace a live credential; the owner rotates or revokes
    /// instead. An expired pending credential is revoked and replaced.
    pub fn record_pending(
        &mut self,
        new: NewAgentCredential,
        now: DateTime<Utc>,
    ) -> Result<RecordedCredential, AgentCredentialError> {
        validate_new(&new.credential_id, &new.verification_hash)?;
        let tx = self.conn.transaction()?;
        if let Some(live) = live_for_agent(&tx, &new.agent_id)? {
            if live.is_expired(now) {
                revoke_row(&tx, &live.credential_id, now)?;
            } else if live.credential_id == new.credential_id
                && live.hash_matches(&new.verification_hash)
            {
                return Ok(RecordedCredential::Existing(live));
            } else {
                return Err(AgentCredentialError::AlreadyRegistered);
            }
        }
        let cutoff = (now - Duration::days(PENDING_CREDENTIAL_TTL_DAYS)).to_rfc3339();
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM agent_credentials
             WHERE owner_user_id = ?1 AND status = 'pending' AND created_at > ?2",
            params![new.owner_user_id.to_string(), cutoff],
            |row| row.get(0),
        )?;
        if usize::try_from(pending).unwrap_or(usize::MAX) >= MAX_PENDING_CREDENTIALS {
            return Err(AgentCredentialError::TooManyPending);
        }
        if credential_id_taken(&tx, &new.credential_id)? {
            return Err(AgentCredentialError::CredentialIdConflict);
        }
        tx.execute(
            "INSERT INTO agent_credentials
             (credential_id, owner_user_id, agent_id, account_id, status, harness, created_at, verification_hash)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
            params![
                new.credential_id,
                new.owner_user_id.to_string(),
                new.agent_id.to_string(),
                new.account_id.to_string(),
                new.harness,
                now.to_rfc3339(),
                new.verification_hash,
            ],
        )?;
        let created = credential_by_id(&tx, &new.credential_id)?
            .ok_or_else(|| StorageError::InvalidData("inserted credential missing".into()))?;
        tx.commit()?;
        Ok(RecordedCredential::Created(created))
    }

    /// Runs the checks of [`Self::record_pending`] without writing, so a caller
    /// can refuse a registration before it creates an agent. `agent_id` is
    /// `None` for an identity that does not exist yet.
    pub fn can_record(
        &self,
        owner_user_id: &UserId,
        agent_id: Option<&AgentId>,
        credential_id: &str,
        verification_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AgentCredentialError> {
        validate_new(credential_id, verification_hash)?;
        if let Some(agent_id) = agent_id {
            if let Some(live) = live_for_agent(&self.conn, agent_id)? {
                if live.is_expired(now) {
                    // Replaced on record.
                } else if live.credential_id == credential_id
                    && live.hash_matches(verification_hash)
                {
                    return Ok(());
                } else {
                    return Err(AgentCredentialError::AlreadyRegistered);
                }
            }
        }
        let cutoff = (now - Duration::days(PENDING_CREDENTIAL_TTL_DAYS)).to_rfc3339();
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM agent_credentials
             WHERE owner_user_id = ?1 AND status = 'pending' AND created_at > ?2",
            params![owner_user_id.to_string(), cutoff],
            |row| row.get(0),
        )?;
        if usize::try_from(pending).unwrap_or(usize::MAX) >= MAX_PENDING_CREDENTIALS {
            return Err(AgentCredentialError::TooManyPending);
        }
        if credential_id_taken(&self.conn, credential_id)? {
            return Err(AgentCredentialError::CredentialIdConflict);
        }
        Ok(())
    }

    /// The agent's pending or active credential, if any.
    pub fn live_for_agent(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCredential>, AgentCredentialError> {
        Ok(live_for_agent(&self.conn, agent_id)?)
    }

    /// The agent's newest credential in any status, for display.
    pub fn latest_for_agent(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCredential>, AgentCredentialError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CREDENTIAL_COLUMNS} FROM agent_credentials WHERE agent_id = ?1
                     ORDER BY CASE status WHEN 'revoked' THEN 1 ELSE 0 END, created_at DESC LIMIT 1"
                ),
                params![agent_id.to_string()],
                credential_from_row,
            )
            .optional()?
            .transpose()?)
    }

    /// Activates the pending credentials of every listed agent, all or none
    /// (spec 5.5; one owner confirmation can cover the migration list).
    pub fn confirm(
        &mut self,
        owner_user_id: &UserId,
        agent_ids: &[AgentId],
        now: DateTime<Utc>,
    ) -> Result<Vec<AgentCredential>, AgentCredentialError> {
        let tx = self.conn.transaction()?;
        let mut confirmed = Vec::with_capacity(agent_ids.len());
        for agent_id in agent_ids {
            let live = live_for_agent(&tx, agent_id)?
                .filter(|credential| &credential.owner_user_id == owner_user_id)
                .filter(|credential| credential.status == AgentCredentialStatus::Pending)
                .filter(|credential| !credential.is_expired(now))
                .ok_or(AgentCredentialError::NotPending)?;
            tx.execute(
                "UPDATE agent_credentials SET status = 'active', activated_at = ?2
                 WHERE credential_id = ?1 AND status = 'pending'",
                params![live.credential_id, now.to_rfc3339()],
            )?;
            confirmed.push(
                credential_by_id(&tx, &live.credential_id)?.ok_or_else(|| {
                    StorageError::InvalidData("confirmed credential missing".into())
                })?,
            );
        }
        tx.commit()?;
        Ok(confirmed)
    }

    /// Revokes the agent's pending or active credential (spec 5.6). The
    /// budget, ledger and ids stay.
    pub fn revoke(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        now: DateTime<Utc>,
    ) -> Result<AgentCredential, AgentCredentialError> {
        let tx = self.conn.transaction()?;
        let live = live_for_agent(&tx, agent_id)?
            .filter(|credential| &credential.owner_user_id == owner_user_id)
            .ok_or(AgentCredentialError::NoCredential)?;
        revoke_row(&tx, &live.credential_id, now)?;
        let revoked = credential_by_id(&tx, &live.credential_id)?
            .ok_or_else(|| StorageError::InvalidData("revoked credential missing".into()))?;
        tx.commit()?;
        Ok(revoked)
    }

    /// Activates a new credential for an agent and revokes its active one in
    /// one transaction (spec 5.6). The owner CLI minted the new secret and
    /// sends only its hash.
    pub fn rotate(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        credential_id: &str,
        verification_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<AgentCredential, AgentCredentialError> {
        validate_new(credential_id, verification_hash)?;
        let tx = self.conn.transaction()?;
        let live = live_for_agent(&tx, agent_id)?
            .filter(|credential| &credential.owner_user_id == owner_user_id)
            .filter(|credential| credential.status == AgentCredentialStatus::Active)
            .ok_or(AgentCredentialError::NotActive)?;
        if credential_id_taken(&tx, credential_id)? {
            return Err(AgentCredentialError::CredentialIdConflict);
        }
        revoke_row(&tx, &live.credential_id, now)?;
        tx.execute(
            "INSERT INTO agent_credentials
             (credential_id, owner_user_id, agent_id, account_id, status, harness, created_at, activated_at, verification_hash)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?6, ?7)",
            params![
                credential_id,
                owner_user_id.to_string(),
                agent_id.to_string(),
                live.account_id.to_string(),
                live.harness,
                now.to_rfc3339(),
                verification_hash,
            ],
        )?;
        let rotated = credential_by_id(&tx, credential_id)?
            .ok_or_else(|| StorageError::InvalidData("rotated credential missing".into()))?;
        tx.commit()?;
        Ok(rotated)
    }

    /// Looks up a presented agent credential. Refreshes `last_used_at` on an
    /// active credential at most once a minute.
    pub fn check_agent(
        &mut self,
        presented: &ParsedCredential,
        now: DateTime<Utc>,
    ) -> Result<AgentCredentialCheck, AgentCredentialError> {
        if presented.kind != CredentialKind::Agent {
            return Ok(AgentCredentialCheck::Invalid);
        }
        let Some(credential) = credential_by_id(&self.conn, &presented.credential_id)? else {
            return Ok(AgentCredentialCheck::Invalid);
        };
        if !credential.hash_matches(&presented.verification_hash()) {
            return Ok(AgentCredentialCheck::Invalid);
        }
        Ok(match credential.status {
            AgentCredentialStatus::Revoked => AgentCredentialCheck::Revoked,
            AgentCredentialStatus::Pending if credential.is_expired(now) => {
                AgentCredentialCheck::Expired
            }
            AgentCredentialStatus::Pending => AgentCredentialCheck::Pending(credential),
            AgentCredentialStatus::Active => {
                let stale = credential
                    .last_used_at
                    .is_none_or(|last| now - last >= Duration::seconds(LAST_USED_REFRESH_SECONDS));
                if stale {
                    self.conn.execute(
                        "UPDATE agent_credentials SET last_used_at = ?2 WHERE credential_id = ?1",
                        params![credential.credential_id, now.to_rfc3339()],
                    )?;
                }
                AgentCredentialCheck::Active(credential)
            }
        })
    }

    /// Records a registration credential for one harness install (spec 5.4).
    /// Recording a new one for the same harness revokes the previous one, so
    /// running `hubu init <harness>` again replaces it.
    pub fn record_registration_credential(
        &mut self,
        owner_user_id: &UserId,
        harness: &str,
        credential_id: &str,
        verification_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<RegistrationCredential, AgentCredentialError> {
        validate_new(credential_id, verification_hash)?;
        let tx = self.conn.transaction()?;
        if credential_id_taken(&tx, credential_id)? {
            return Err(AgentCredentialError::CredentialIdConflict);
        }
        tx.execute(
            "UPDATE registration_credentials SET status = 'revoked', revoked_at = ?3
             WHERE owner_user_id = ?1 AND harness = ?2 AND status = 'active'",
            params![owner_user_id.to_string(), harness, now.to_rfc3339()],
        )?;
        tx.execute(
            "INSERT INTO registration_credentials
             (credential_id, owner_user_id, harness, status, created_at, verification_hash)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
            params![
                credential_id,
                owner_user_id.to_string(),
                harness,
                now.to_rfc3339(),
                verification_hash,
            ],
        )?;
        tx.commit()?;
        Ok(RegistrationCredential {
            credential_id: credential_id.to_string(),
            owner_user_id: owner_user_id.clone(),
            harness: harness.to_string(),
            created_at: now,
            revoked_at: None,
            verification_hash: verification_hash.to_string(),
        })
    }

    /// Looks up a presented registration credential; `None` unless it is
    /// known, matches, and is active.
    pub fn check_registration(
        &mut self,
        presented: &ParsedCredential,
        now: DateTime<Utc>,
    ) -> Result<Option<RegistrationCredential>, AgentCredentialError> {
        if presented.kind != CredentialKind::Registration {
            return Ok(None);
        }
        let row = self
            .conn
            .query_row(
                "SELECT credential_id, owner_user_id, harness, status, created_at, revoked_at, verification_hash
                 FROM registration_credentials WHERE credential_id = ?1",
                params![presented.credential_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((credential_id, owner, harness, status, created_at, revoked_at, hash)) = row
        else {
            return Ok(None);
        };
        if status != "active"
            || !constant_time_eq(hash.as_bytes(), presented.verification_hash().as_bytes())
        {
            return Ok(None);
        }
        self.conn.execute(
            "UPDATE registration_credentials SET last_used_at = ?2 WHERE credential_id = ?1",
            params![credential_id, now.to_rfc3339()],
        )?;
        Ok(Some(RegistrationCredential {
            credential_id,
            owner_user_id: parse_id(&owner)?,
            harness,
            created_at: parse_timestamp(&created_at)?,
            revoked_at: revoked_at.as_deref().map(parse_timestamp).transpose()?,
            verification_hash: hash,
        }))
    }
}

fn validate_new(credential_id: &str, verification_hash: &str) -> Result<(), AgentCredentialError> {
    if !is_valid_credential_id(credential_id) {
        return Err(AgentCredentialError::InvalidCredentialId);
    }
    if !is_valid_verification_hash(verification_hash) {
        return Err(AgentCredentialError::InvalidVerificationHash);
    }
    Ok(())
}

/// Ids are unique across both tables, so an id names one credential.
fn credential_id_taken(conn: &Connection, credential_id: &str) -> Result<bool, StorageError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_credentials WHERE credential_id = ?1)
             OR EXISTS(SELECT 1 FROM registration_credentials WHERE credential_id = ?1)",
        params![credential_id],
        |row| row.get(0),
    )?)
}

fn revoke_row(
    conn: &Connection,
    credential_id: &str,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE agent_credentials SET status = 'revoked', revoked_at = ?2
         WHERE credential_id = ?1 AND status != 'revoked'",
        params![credential_id, now.to_rfc3339()],
    )?;
    Ok(())
}

fn live_for_agent(
    conn: &Connection,
    agent_id: &AgentId,
) -> Result<Option<AgentCredential>, StorageError> {
    conn.query_row(
        &format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM agent_credentials
             WHERE agent_id = ?1 AND status IN ('pending', 'active')"
        ),
        params![agent_id.to_string()],
        credential_from_row,
    )
    .optional()?
    .transpose()
}

fn credential_by_id(
    conn: &Connection,
    credential_id: &str,
) -> Result<Option<AgentCredential>, StorageError> {
    conn.query_row(
        &format!("SELECT {CREDENTIAL_COLUMNS} FROM agent_credentials WHERE credential_id = ?1"),
        params![credential_id],
        credential_from_row,
    )
    .optional()?
    .transpose()
}

fn parse_id<T: std::str::FromStr>(value: &str) -> Result<T, StorageError> {
    value
        .parse()
        .map_err(|_| StorageError::InvalidData(format!("invalid stored id `{value}`")))
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| StorageError::InvalidData(format!("invalid stored timestamp `{value}`")))
}

fn optional_timestamp(value: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
    value.as_deref().map(parse_timestamp).transpose()
}

#[allow(clippy::type_complexity)]
fn credential_from_row(row: &Row<'_>) -> rusqlite::Result<Result<AgentCredential, StorageError>> {
    let credential_id: String = row.get(0)?;
    let owner: String = row.get(1)?;
    let agent: String = row.get(2)?;
    let account: String = row.get(3)?;
    let status: String = row.get(4)?;
    let harness: Option<String> = row.get(5)?;
    let created_at: String = row.get(6)?;
    let activated_at: Option<String> = row.get(7)?;
    let revoked_at: Option<String> = row.get(8)?;
    let last_used_at: Option<String> = row.get(9)?;
    let verification_hash: String = row.get(10)?;
    Ok((|| {
        Ok(AgentCredential {
            credential_id,
            owner_user_id: parse_id(&owner)?,
            agent_id: parse_id(&agent)?,
            account_id: parse_id(&account)?,
            status: AgentCredentialStatus::parse(&status)?,
            harness,
            created_at: parse_timestamp(&created_at)?,
            activated_at: optional_timestamp(activated_at)?,
            revoked_at: optional_timestamp(revoked_at)?,
            last_used_at: optional_timestamp(last_used_at)?,
            verification_hash,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entropy(seed: u8) -> [u8; 48] {
        let mut bytes = [0_u8; 48];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = seed.wrapping_mul(31).wrapping_add(index as u8);
        }
        bytes
    }

    fn minted(kind: CredentialKind, seed: u8) -> (String, ParsedCredential) {
        let token = format_credential(kind, &entropy(seed));
        let parsed = parse_credential(&token).expect("minted credential parses");
        (token, parsed)
    }

    fn new_for(agent: &AgentId, owner: &UserId, parsed: &ParsedCredential) -> NewAgentCredential {
        NewAgentCredential {
            credential_id: parsed.credential_id.clone(),
            verification_hash: parsed.verification_hash(),
            owner_user_id: owner.clone(),
            agent_id: agent.clone(),
            account_id: AgentAccountId::new(),
            harness: Some("claude".to_string()),
        }
    }

    #[test]
    fn format_and_parse_round_trip() {
        let (token, parsed) = minted(CredentialKind::Agent, 1);
        assert!(token.starts_with(AGENT_CREDENTIAL_PREFIX));
        assert_eq!(parsed.credential_id.len(), 26);
        assert!(is_valid_verification_hash(&parsed.verification_hash()));
        assert!(!format!("{parsed:?}").contains(&token[AGENT_CREDENTIAL_PREFIX.len() + 27..]));
        let (reg, parsed) = minted(CredentialKind::Registration, 2);
        assert!(reg.starts_with(REGISTRATION_CREDENTIAL_PREFIX));
        assert_eq!(parsed.kind, CredentialKind::Registration);
    }

    #[test]
    fn parse_rejects_other_formats() {
        for token in [
            "hubu_agent_",
            "hubu_agent_00000000-0000-0000-0000-000000000000",
            "hubu_agent_short_secret",
            "hubu_owner_aaaaaaaaaaaaaaaaaaaaaaaaaa_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "test-local-agent-token",
        ] {
            assert!(parse_credential(token).is_none(), "{token} must not parse");
        }
        let (token, _) = minted(CredentialKind::Agent, 3);
        assert!(parse_credential(&token.to_uppercase()).is_none());
    }

    #[test]
    fn pending_then_confirm_then_revoke() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let (_, parsed) = minted(CredentialKind::Agent, 4);
        let now = Utc::now();
        let recorded = manager
            .record_pending(new_for(&agent, &owner, &parsed), now)
            .unwrap();
        assert!(matches!(recorded, RecordedCredential::Created(_)));
        assert!(matches!(
            manager.check_agent(&parsed, now).unwrap(),
            AgentCredentialCheck::Pending(_)
        ));

        // The same registration again is idempotent.
        assert!(matches!(
            manager
                .record_pending(new_for(&agent, &owner, &parsed), now)
                .unwrap(),
            RecordedCredential::Existing(_)
        ));

        manager
            .confirm(&owner, std::slice::from_ref(&agent), now)
            .unwrap();
        assert!(matches!(
            manager.check_agent(&parsed, now).unwrap(),
            AgentCredentialCheck::Active(_)
        ));

        manager.revoke(&owner, &agent, now).unwrap();
        assert_eq!(
            manager.check_agent(&parsed, now).unwrap(),
            AgentCredentialCheck::Revoked
        );
    }

    #[test]
    fn a_second_credential_for_a_live_agent_is_refused() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let now = Utc::now();
        let (_, first) = minted(CredentialKind::Agent, 5);
        let (_, second) = minted(CredentialKind::Agent, 6);
        manager
            .record_pending(new_for(&agent, &owner, &first), now)
            .unwrap();
        assert_eq!(
            manager.record_pending(new_for(&agent, &owner, &second), now),
            Err(AgentCredentialError::AlreadyRegistered)
        );
        manager
            .confirm(&owner, std::slice::from_ref(&agent), now)
            .unwrap();
        assert_eq!(
            manager.record_pending(new_for(&agent, &owner, &second), now),
            Err(AgentCredentialError::AlreadyRegistered)
        );
        // After revocation the agent may register a new credential, which
        // waits for the owner again.
        manager.revoke(&owner, &agent, now).unwrap();
        assert!(matches!(
            manager.record_pending(new_for(&agent, &owner, &second), now),
            Ok(RecordedCredential::Created(_))
        ));
    }

    #[test]
    fn wrong_secret_and_unknown_id_are_invalid() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let now = Utc::now();
        let (token, parsed) = minted(CredentialKind::Agent, 7);
        manager
            .record_pending(new_for(&agent, &owner, &parsed), now)
            .unwrap();
        let mut tampered = token.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'a' { 'b' } else { 'a' });
        let tampered = parse_credential(&tampered).unwrap();
        assert_eq!(
            manager.check_agent(&tampered, now).unwrap(),
            AgentCredentialCheck::Invalid
        );
        let (_, unknown) = minted(CredentialKind::Agent, 8);
        assert_eq!(
            manager.check_agent(&unknown, now).unwrap(),
            AgentCredentialCheck::Invalid
        );
    }

    #[test]
    fn pending_expires_and_can_be_replaced() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let then = Utc::now() - Duration::days(PENDING_CREDENTIAL_TTL_DAYS);
        let now = Utc::now();
        let (_, old) = minted(CredentialKind::Agent, 9);
        manager
            .record_pending(new_for(&agent, &owner, &old), then)
            .unwrap();
        assert_eq!(
            manager.check_agent(&old, now).unwrap(),
            AgentCredentialCheck::Expired
        );
        assert_eq!(
            manager.confirm(&owner, std::slice::from_ref(&agent), now),
            Err(AgentCredentialError::NotPending)
        );
        let (_, new) = minted(CredentialKind::Agent, 10);
        assert!(matches!(
            manager.record_pending(new_for(&agent, &owner, &new), now),
            Ok(RecordedCredential::Created(_))
        ));
        assert_eq!(
            manager.check_agent(&old, now).unwrap(),
            AgentCredentialCheck::Revoked
        );
    }

    #[test]
    fn pending_registrations_are_capped() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let now = Utc::now();
        for seed in 0..MAX_PENDING_CREDENTIALS as u8 {
            let (_, parsed) = minted(CredentialKind::Agent, 20 + seed);
            manager
                .record_pending(new_for(&AgentId::new(), &owner, &parsed), now)
                .unwrap();
        }
        let (_, parsed) = minted(CredentialKind::Agent, 90);
        assert_eq!(
            manager.record_pending(new_for(&AgentId::new(), &owner, &parsed), now),
            Err(AgentCredentialError::TooManyPending)
        );
    }

    #[test]
    fn rotate_replaces_the_active_credential() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let agent = AgentId::new();
        let now = Utc::now();
        let (_, old) = minted(CredentialKind::Agent, 11);
        let (_, new) = minted(CredentialKind::Agent, 12);
        manager
            .record_pending(new_for(&agent, &owner, &old), now)
            .unwrap();
        assert_eq!(
            manager.rotate(
                &owner,
                &agent,
                &new.credential_id,
                &new.verification_hash(),
                now
            ),
            Err(AgentCredentialError::NotActive)
        );
        manager
            .confirm(&owner, std::slice::from_ref(&agent), now)
            .unwrap();
        let rotated = manager
            .rotate(
                &owner,
                &agent,
                &new.credential_id,
                &new.verification_hash(),
                now,
            )
            .unwrap();
        assert_eq!(rotated.status, AgentCredentialStatus::Active);
        assert_eq!(
            manager.check_agent(&old, now).unwrap(),
            AgentCredentialCheck::Revoked
        );
        assert!(matches!(
            manager.check_agent(&new, now).unwrap(),
            AgentCredentialCheck::Active(_)
        ));
    }

    #[test]
    fn another_owner_cannot_confirm_or_revoke() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let other = UserId::new();
        let agent = AgentId::new();
        let now = Utc::now();
        let (_, parsed) = minted(CredentialKind::Agent, 13);
        manager
            .record_pending(new_for(&agent, &owner, &parsed), now)
            .unwrap();
        assert_eq!(
            manager.confirm(&other, std::slice::from_ref(&agent), now),
            Err(AgentCredentialError::NotPending)
        );
        assert_eq!(
            manager.revoke(&other, &agent, now),
            Err(AgentCredentialError::NoCredential)
        );
    }

    #[test]
    fn registration_credential_is_replaced_per_harness() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let now = Utc::now();
        let (_, first) = minted(CredentialKind::Registration, 14);
        let (_, second) = minted(CredentialKind::Registration, 15);
        manager
            .record_registration_credential(
                &owner,
                "claude",
                &first.credential_id,
                &first.verification_hash(),
                now,
            )
            .unwrap();
        assert!(manager.check_registration(&first, now).unwrap().is_some());
        manager
            .record_registration_credential(
                &owner,
                "claude",
                &second.credential_id,
                &second.verification_hash(),
                now,
            )
            .unwrap();
        assert!(manager.check_registration(&first, now).unwrap().is_none());
        assert!(manager.check_registration(&second, now).unwrap().is_some());
        // A registration credential is never an agent credential.
        assert_eq!(
            manager.check_agent(&second, now).unwrap(),
            AgentCredentialCheck::Invalid
        );
    }

    #[test]
    fn ids_are_unique_across_both_tables() {
        let mut manager = AgentCredentialManager::in_memory().unwrap();
        let owner = UserId::new();
        let now = Utc::now();
        let (_, reg) = minted(CredentialKind::Registration, 16);
        manager
            .record_registration_credential(
                &owner,
                "codex",
                &reg.credential_id,
                &reg.verification_hash(),
                now,
            )
            .unwrap();
        let mut new = new_for(&AgentId::new(), &owner, &reg);
        new.verification_hash = reg.verification_hash();
        assert_eq!(
            manager.record_pending(new, now),
            Err(AgentCredentialError::CredentialIdConflict)
        );
    }
}
