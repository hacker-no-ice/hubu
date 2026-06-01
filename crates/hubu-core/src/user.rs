use std::collections::HashMap;

use chrono::Utc;
use hubu_common::{
    ids::UserId,
    models::{User, UserContext, UserStatus},
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UserError {
    #[error("default user is missing")]
    MissingDefaultUser,
}

#[derive(Debug, Clone)]
pub struct CreateUserRequest {
    pub display_name: String,
    pub email: Option<String>,
}

pub struct UserManager {
    users: HashMap<UserId, User>,
    user_by_pub_id: HashMap<String, UserId>,
    default_user_id: Option<UserId>,
}

impl UserManager {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
            user_by_pub_id: HashMap::new(),
            default_user_id: None,
        }
    }

    pub fn create_user(&mut self, request: CreateUserRequest) -> User {
        let now = Utc::now();
        let (id, pub_id) = self.new_public_user_id();
        let user = User {
            id: id.clone(),
            pub_id,
            display_name: request.display_name,
            email: request.email,
            status: UserStatus::Active,
            created_at: now,
            updated_at: now,
        };

        self.user_by_pub_id.insert(user.pub_id.clone(), id.clone());
        self.users.insert(id.clone(), user.clone());
        self.default_user_id = Some(id);

        user
    }

    pub fn ensure_default_user(&mut self) -> User {
        self.default_user().unwrap_or_else(|| {
            self.create_user(CreateUserRequest {
                display_name: "Hubu User".to_string(),
                email: None,
            })
        })
    }

    pub fn default_user(&self) -> Option<User> {
        self.default_user_id
            .as_ref()
            .and_then(|id| self.users.get(id))
            .cloned()
    }

    pub fn default_user_context(&self) -> Result<UserContext, UserError> {
        let user_id = self
            .default_user_id
            .as_ref()
            .ok_or(UserError::MissingDefaultUser)?
            .clone();
        Ok(UserContext::new(user_id))
    }

    pub fn user_id_for_pub_id(&self, pub_id: &str) -> Option<UserId> {
        self.user_by_pub_id.get(pub_id).cloned()
    }

    fn new_public_user_id(&self) -> (UserId, String) {
        loop {
            let id = UserId::new();
            let pub_id = format!("usr_{}", id.public_suffix());
            if !self.user_by_pub_id.contains_key(&pub_id) {
                return (id, pub_id);
            }
        }
    }
}

impl Default for UserManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_default_user_once_with_public_id() {
        let mut manager = UserManager::new();

        let first = manager.ensure_default_user();
        let second = manager.ensure_default_user();

        assert_eq!(first.id, second.id);
        assert_eq!(first.display_name, "Hubu User");
        assert!(first.pub_id.starts_with("usr_"));
        assert_eq!(first.pub_id.len(), "usr_".len() + 12);
        assert_eq!(manager.user_id_for_pub_id(&first.pub_id), Some(first.id));
        assert_eq!(manager.default_user_context().unwrap().user_id, second.id);
    }

    #[test]
    fn explicit_user_creation_adds_new_default_user() {
        let mut manager = UserManager::new();
        let fallback = manager.ensure_default_user();

        let explicit = manager.create_user(CreateUserRequest {
            display_name: "Demo User".to_string(),
            email: Some("demo@example.com".to_string()),
        });

        assert_ne!(fallback.id, explicit.id);
        assert_eq!(explicit.display_name, "Demo User");
        assert_eq!(explicit.email.as_deref(), Some("demo@example.com"));
        assert_eq!(
            manager.user_id_for_pub_id(&explicit.pub_id),
            Some(explicit.id.clone())
        );
        assert_eq!(manager.default_user_context().unwrap().user_id, explicit.id);
    }
}
