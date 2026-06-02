use std::{collections::HashMap, path::Path};

use chrono::Utc;
use hubu_common::{
    ids::UserId,
    models::{User, UserContext, UserStatus},
};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::{
    init_schema, user_from_row, user_status, StorageError, DEFAULT_USER_IDENTITY_KEY,
    SELECTED_DEFAULT_USER_ID_KEY,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UserError {
    #[error("default user is missing")]
    MissingDefaultUser,

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, Clone)]
pub struct CreateUserRequest {
    pub display_name: String,
    pub email: Option<String>,
}

pub struct UserManager {
    store: UserStore,
    default_user_id: Option<UserId>,
}

impl UserManager {
    pub fn new() -> Self {
        Self {
            store: UserStore::Memory(MemoryUserStore::new()),
            default_user_id: None,
        }
    }

    pub fn in_memory_sqlite() -> Result<Self, UserError> {
        Self::from_store(UserStore::Sqlite(SqliteUserStore::in_memory()?))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, UserError> {
        Self::from_store(UserStore::Sqlite(SqliteUserStore::open(path)?))
    }

    pub fn create_user(&mut self, request: CreateUserRequest) -> Result<User, UserError> {
        let identity_key = request
            .email
            .as_ref()
            .map(|email| format!("email:{}", email.trim().to_ascii_lowercase()));
        if let Some(identity_key) = identity_key.as_deref() {
            if let Some(existing) = self.store.user_by_identity_key(identity_key)? {
                self.select_default_user(&existing)?;
                return Ok(existing);
            }
        }

        let now = Utc::now();
        let (id, pub_id) = self.new_public_user_id()?;
        let user = User {
            id: id.clone(),
            pub_id,
            display_name: request.display_name,
            email: request.email,
            status: UserStatus::Active,
            created_at: now,
            updated_at: now,
        };

        self.store.insert_user(&user, identity_key.as_deref())?;
        self.select_default_user(&user)?;

        Ok(user)
    }

    pub fn ensure_default_user(&mut self) -> Result<User, UserError> {
        if let Some(user) = self.default_user()? {
            return Ok(user);
        }
        if let Some(user) = self.store.user_by_identity_key(DEFAULT_USER_IDENTITY_KEY)? {
            self.default_user_id = Some(user.id.clone());
            return Ok(user);
        }

        let now = Utc::now();
        let (id, pub_id) = self.new_public_user_id()?;
        let user = User {
            id: id.clone(),
            pub_id,
            display_name: "Hubu User".to_string(),
            email: None,
            status: UserStatus::Active,
            created_at: now,
            updated_at: now,
        };
        self.store
            .insert_user(&user, Some(DEFAULT_USER_IDENTITY_KEY))?;
        self.default_user_id = Some(id);
        Ok(user)
    }

    pub fn default_user(&self) -> Result<Option<User>, UserError> {
        match self.default_user_id.as_ref() {
            Some(id) => Ok(self.store.user_for_id(id)?),
            None => Ok(None),
        }
    }

    pub fn default_user_context(&self) -> Result<UserContext, UserError> {
        let user_id = self
            .default_user_id
            .as_ref()
            .ok_or(UserError::MissingDefaultUser)?
            .clone();
        Ok(UserContext::new(user_id))
    }

    pub fn user_id_for_pub_id(&self, pub_id: &str) -> Result<Option<UserId>, UserError> {
        Ok(self.store.user_by_pub_id(pub_id)?.map(|user| user.id))
    }

    pub fn user_for_id(&self, user_id: &UserId) -> Result<Option<User>, UserError> {
        Ok(self.store.user_for_id(user_id)?)
    }

    fn new_public_user_id(&self) -> Result<(UserId, String), UserError> {
        loop {
            let id = UserId::new();
            let pub_id = format!("usr_{}", id.public_suffix());
            if self.store.user_by_pub_id(&pub_id)?.is_none() {
                return Ok((id, pub_id));
            }
        }
    }

    fn from_store(store: UserStore) -> Result<Self, UserError> {
        let default_user_id = store.selected_default_user()?.map(|user| user.id);
        Ok(Self {
            store,
            default_user_id,
        })
    }

    fn select_default_user(&mut self, user: &User) -> Result<(), UserError> {
        self.store.set_selected_default_user(&user.id)?;
        self.default_user_id = Some(user.id.clone());
        Ok(())
    }
}

impl Default for UserManager {
    fn default() -> Self {
        Self::new()
    }
}

enum UserStore {
    Memory(MemoryUserStore),
    Sqlite(SqliteUserStore),
}

impl UserStore {
    fn insert_user(&mut self, user: &User, identity_key: Option<&str>) -> Result<(), StorageError> {
        match self {
            UserStore::Memory(store) => store.insert_user(user, identity_key),
            UserStore::Sqlite(store) => store.insert_user(user, identity_key),
        }
    }

    fn user_for_id(&self, user_id: &UserId) -> Result<Option<User>, StorageError> {
        match self {
            UserStore::Memory(store) => store.user_for_id(user_id),
            UserStore::Sqlite(store) => store.user_for_id(user_id),
        }
    }

    fn user_by_pub_id(&self, pub_id: &str) -> Result<Option<User>, StorageError> {
        match self {
            UserStore::Memory(store) => store.user_by_pub_id(pub_id),
            UserStore::Sqlite(store) => store.user_by_pub_id(pub_id),
        }
    }

    fn user_by_identity_key(&self, identity_key: &str) -> Result<Option<User>, StorageError> {
        match self {
            UserStore::Memory(store) => store.user_by_identity_key(identity_key),
            UserStore::Sqlite(store) => store.user_by_identity_key(identity_key),
        }
    }

    fn selected_default_user(&self) -> Result<Option<User>, StorageError> {
        match self {
            UserStore::Memory(store) => store.selected_default_user(),
            UserStore::Sqlite(store) => store.selected_default_user(),
        }
    }

    fn set_selected_default_user(&mut self, user_id: &UserId) -> Result<(), StorageError> {
        match self {
            UserStore::Memory(store) => store.set_selected_default_user(user_id),
            UserStore::Sqlite(store) => store.set_selected_default_user(user_id),
        }
    }
}

struct MemoryUserStore {
    users: HashMap<UserId, User>,
    user_by_pub_id: HashMap<String, UserId>,
    user_by_identity_key: HashMap<String, UserId>,
    selected_default_user_id: Option<UserId>,
}

impl MemoryUserStore {
    fn new() -> Self {
        Self {
            users: HashMap::new(),
            user_by_pub_id: HashMap::new(),
            user_by_identity_key: HashMap::new(),
            selected_default_user_id: None,
        }
    }

    fn insert_user(&mut self, user: &User, identity_key: Option<&str>) -> Result<(), StorageError> {
        self.user_by_pub_id
            .insert(user.pub_id.clone(), user.id.clone());
        if let Some(identity_key) = identity_key {
            self.user_by_identity_key
                .insert(identity_key.to_string(), user.id.clone());
        }
        self.users.insert(user.id.clone(), user.clone());
        Ok(())
    }

    fn user_for_id(&self, user_id: &UserId) -> Result<Option<User>, StorageError> {
        Ok(self.users.get(user_id).cloned())
    }

    fn user_by_pub_id(&self, pub_id: &str) -> Result<Option<User>, StorageError> {
        Ok(self
            .user_by_pub_id
            .get(pub_id)
            .and_then(|id| self.users.get(id))
            .cloned())
    }

    fn user_by_identity_key(&self, identity_key: &str) -> Result<Option<User>, StorageError> {
        Ok(self
            .user_by_identity_key
            .get(identity_key)
            .and_then(|id| self.users.get(id))
            .cloned())
    }

    fn selected_default_user(&self) -> Result<Option<User>, StorageError> {
        Ok(self
            .selected_default_user_id
            .as_ref()
            .and_then(|id| self.users.get(id))
            .cloned())
    }

    fn set_selected_default_user(&mut self, user_id: &UserId) -> Result<(), StorageError> {
        self.selected_default_user_id = Some(user_id.clone());
        Ok(())
    }
}

pub struct SqliteUserStore {
    conn: Connection,
}

impl SqliteUserStore {
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

    fn insert_user(&mut self, user: &User, identity_key: Option<&str>) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO users
             (id, pub_id, identity_key, display_name, email, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                user.id.to_string(),
                user.pub_id,
                identity_key,
                user.display_name,
                user.email,
                user_status(&user.status),
                user.created_at.to_rfc3339(),
                user.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn user_for_id(&self, user_id: &UserId) -> Result<Option<User>, StorageError> {
        let user = self
            .conn
            .query_row(
                "SELECT id, pub_id, display_name, email, status, created_at, updated_at
                 FROM users
                 WHERE id = ?1",
                params![user_id.to_string()],
                user_from_row,
            )
            .optional()?;
        Ok(user)
    }

    fn user_by_pub_id(&self, pub_id: &str) -> Result<Option<User>, StorageError> {
        let user = self
            .conn
            .query_row(
                "SELECT id, pub_id, display_name, email, status, created_at, updated_at
                 FROM users
                 WHERE pub_id = ?1",
                params![pub_id],
                user_from_row,
            )
            .optional()?;
        Ok(user)
    }

    fn user_by_identity_key(&self, identity_key: &str) -> Result<Option<User>, StorageError> {
        let user = self
            .conn
            .query_row(
                "SELECT id, pub_id, display_name, email, status, created_at, updated_at
                 FROM users
                 WHERE identity_key = ?1",
                params![identity_key],
                user_from_row,
            )
            .optional()?;
        Ok(user)
    }

    fn selected_default_user(&self) -> Result<Option<User>, StorageError> {
        let selected_user_id = self
            .conn
            .query_row(
                "SELECT value FROM app_state WHERE key = ?1",
                params![SELECTED_DEFAULT_USER_ID_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match selected_user_id {
            Some(user_id) => {
                let user_id = user_id
                    .parse()
                    .map_err(|_| StorageError::InvalidData("selected default user id".into()))?;
                self.user_for_id(&user_id)
            }
            None => Ok(None),
        }
    }

    fn set_selected_default_user(&mut self, user_id: &UserId) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO app_state (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![
                SELECTED_DEFAULT_USER_ID_KEY,
                user_id.to_string(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_default_user_once_with_public_id() {
        let mut manager = UserManager::new();

        let first = manager.ensure_default_user().unwrap();
        let second = manager.ensure_default_user().unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(first.display_name, "Hubu User");
        assert!(first.pub_id.starts_with("usr_"));
        assert_eq!(first.pub_id.len(), "usr_".len() + 12);
        assert_eq!(
            manager.user_id_for_pub_id(&first.pub_id).unwrap(),
            Some(first.id)
        );
        assert_eq!(manager.default_user_context().unwrap().user_id, second.id);
    }

    #[test]
    fn explicit_user_creation_adds_new_default_user() {
        let mut manager = UserManager::new();
        let fallback = manager.ensure_default_user().unwrap();

        let explicit = manager
            .create_user(CreateUserRequest {
                display_name: "Demo User".to_string(),
                email: Some("demo@example.com".to_string()),
            })
            .unwrap();

        assert_ne!(fallback.id, explicit.id);
        assert_eq!(explicit.display_name, "Demo User");
        assert_eq!(explicit.email.as_deref(), Some("demo@example.com"));
        assert_eq!(
            manager.user_id_for_pub_id(&explicit.pub_id).unwrap(),
            Some(explicit.id.clone())
        );
        assert_eq!(
            manager.user_for_id(&explicit.id).unwrap(),
            Some(explicit.clone())
        );
        assert_eq!(manager.default_user_context().unwrap().user_id, explicit.id);
    }

    #[test]
    fn sqlite_user_is_persisted_and_available_after_restart() {
        let path = std::env::temp_dir().join(format!("hubu-user-{}.sqlite", UserId::new()));
        let explicit = {
            let mut manager = UserManager::open(&path).unwrap();
            manager
                .create_user(CreateUserRequest {
                    display_name: "Persisted User".to_string(),
                    email: Some("persisted@example.com".to_string()),
                })
                .unwrap()
        };

        let manager = UserManager::open(&path).unwrap();
        assert_eq!(
            manager.user_for_id(&explicit.id).unwrap(),
            Some(explicit.clone())
        );
        assert_eq!(
            manager.user_id_for_pub_id(&explicit.pub_id).unwrap(),
            Some(explicit.id)
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn sqlite_startup_default_user_is_created_once() {
        let path = std::env::temp_dir().join(format!("hubu-default-{}.sqlite", UserId::new()));
        let first = {
            let mut manager = UserManager::open(&path).unwrap();
            manager.ensure_default_user().unwrap()
        };
        let second = {
            let mut manager = UserManager::open(&path).unwrap();
            manager.ensure_default_user().unwrap()
        };

        assert_eq!(first.id, second.id);
        assert_eq!(first.pub_id, second.pub_id);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn sqlite_explicit_init_new_email_creates_new_user_and_same_email_reuses() {
        let mut manager = UserManager::in_memory_sqlite().unwrap();
        let default = manager.ensure_default_user().unwrap();
        let first = manager
            .create_user(CreateUserRequest {
                display_name: "Alice".to_string(),
                email: Some("Alice@Example.com".to_string()),
            })
            .unwrap();
        let second = manager
            .create_user(CreateUserRequest {
                display_name: "Alice Again".to_string(),
                email: Some("alice@example.com".to_string()),
            })
            .unwrap();

        assert_ne!(default.id, first.id);
        assert_eq!(first.id, second.id);
        assert_eq!(first.display_name, "Alice");
    }

    #[test]
    fn sqlite_explicit_user_selection_is_persisted_after_restart() {
        let path = std::env::temp_dir().join(format!("hubu-selected-{}.sqlite", UserId::new()));
        let explicit = {
            let mut manager = UserManager::open(&path).unwrap();
            manager.ensure_default_user().unwrap();
            manager
                .create_user(CreateUserRequest {
                    display_name: "Selected User".to_string(),
                    email: Some("selected@example.com".to_string()),
                })
                .unwrap()
        };

        let manager = UserManager::open(&path).unwrap();
        assert_eq!(manager.default_user_context().unwrap().user_id, explicit.id);
        assert_eq!(manager.default_user().unwrap(), Some(explicit));
        std::fs::remove_file(path).ok();
    }
}
