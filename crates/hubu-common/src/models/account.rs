/// Agent Account is used for tracking agent spending, balance and other financial operations
use crate::ids::{AgentAccountId, AgentId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccount {
    pub id: AgentAccountId,
    pub pub_id: String, // External opaque ID, example "aga_..."

    pub agent_id: AgentId,

    pub account_status: AccountStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountStatus {
    Active,
    Suspended,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_id() -> AgentId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000001\"").unwrap()
    }

    fn agent_account_id() -> AgentAccountId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000002\"").unwrap()
    }

    fn timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn agent_account_can_be_constructed_and_round_tripped() {
        let account = AgentAccount {
            id: agent_account_id(),
            pub_id: "aga_123".to_string(),
            agent_id: agent_id(),
            account_status: AccountStatus::Active,
            created_at: timestamp(),
            updated_at: timestamp(),
        };

        let encoded = serde_json::to_string(&account).unwrap();
        let decoded: AgentAccount = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, account);
    }
}

