use crate::actor::OwnerRef;
use crate::ids::{AgentId, AgentVersionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Logical agent identity, answers the question "Who is this agent?"
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub id: AgentId,
    pub pub_id: String, // External opaque ID, example "agt_..."

    pub display_name: String,
    pub description: Option<String>,

    pub owner: OwnerRef,

    pub agent_type: AgentType,
    pub agent_status: AgentStatus,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Specific config for an agent, answers the question "What exact code/model/config the agent is running?"
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentVersion {
    pub id: AgentVersionId,
    pub pub_id: String, // External opaque ID, example "agv_..."

    pub agent_id: AgentId,

    pub fingerprint: String, // hash of key fields to identify the agent version

    pub code_ref: Option<CodeReference>,
    pub model: Option<ModelIdentity>,
    pub runtime: Option<RuntimeIdentity>,

    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentType {
    InteractiveAgent,
    AutonomousAgent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Active,
    Suspended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeReference {
    pub repository_url: Option<String>,
    pub commit_sha: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub provider: String,
    pub model: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub runtime_provider: String,
    pub environment: RuntimeEnvironment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEnvironment {
    Production,
    Staging,
    Development,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{OwnerRef, OwnerType};

    fn agent_id() -> AgentId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000001\"").unwrap()
    }

    fn agent_version_id() -> AgentVersionId {
        serde_json::from_str("\"00000000-0000-4000-8000-000000000002\"").unwrap()
    }

    fn timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn agent_identity_can_be_constructed_and_round_tripped() {
        let identity = AgentIdentity {
            id: agent_id(),
            pub_id: "agt_123".to_string(),
            display_name: "Settlement Agent".to_string(),
            description: Some("Handles settlement approvals".to_string()),
            owner: OwnerRef {
                owner_type: OwnerType::Organization,
                owner_id: "org_123".to_string(),
            },
            agent_type: AgentType::AutonomousAgent,
            agent_status: AgentStatus::Active,
            created_at: timestamp(),
            updated_at: timestamp(),
        };

        let encoded = serde_json::to_string(&identity).unwrap();
        let decoded: AgentIdentity = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, identity);
    }

    #[test]
    fn agent_version_can_be_constructed_and_round_tripped() {
        let version = AgentVersion {
            id: agent_version_id(),
            pub_id: "agv_123".to_string(),
            agent_id: agent_id(),
            fingerprint: "sha256:abc123".to_string(),
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
            created_at: timestamp(),
        };

        let encoded = serde_json::to_string(&version).unwrap();
        let decoded: AgentVersion = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, version);
    }
}
