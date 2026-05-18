/// Agent Session track session metadata foreach time an agent is connected to the MCP server
use crate::ids::{AgentId, AgentSessionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: AgentSessionId,
    pub pub_id: String, // External opaque ID, example "ags_..."

    pub agent_id: AgentId,

    pub mcp_client_name: Option<String>,
    pub mcp_client_version: Option<String>,

    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_id() -> AgentId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000002\"").unwrap()
    }

    fn agent_session_id() -> AgentSessionId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000003\"").unwrap()
    }

    fn timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn agent_session_can_be_constructed_and_round_tripped() {
        let agent_session = AgentSession {
            id: agent_session_id(),
            pub_id: "ags_123".to_string(),
            agent_id: agent_id(),
            mcp_client_name: Some("codex-cli".to_string()),
            mcp_client_version: Some("0.12.3".to_string()),
            created_at: timestamp(),
        };

        let encoded = serde_json::to_string(&agent_session).unwrap();
        let decoded: AgentSession = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, agent_session);
    }
}
