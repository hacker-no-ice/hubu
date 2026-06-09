use crate::ids::UserId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub pub_id: String,
    pub username: Option<String>,
    pub display_name: String,
    pub email: Option<String>,
    pub status: UserStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserStatus {
    Active,
    Suspended,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserContext {
    pub user_id: UserId,
}

impl UserContext {
    pub fn new(user_id: UserId) -> Self {
        Self { user_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_id() -> UserId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000001\"").unwrap()
    }

    fn timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn user_can_be_constructed_and_round_tripped() {
        let user = User {
            id: user_id(),
            pub_id: "usr_123".to_string(),
            username: Some("demo-user".to_string()),
            display_name: "Demo User".to_string(),
            email: Some("demo@example.com".to_string()),
            status: UserStatus::Active,
            created_at: timestamp(),
            updated_at: timestamp(),
        };

        let encoded = serde_json::to_string(&user).unwrap();
        let decoded: User = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, user);
    }
}
