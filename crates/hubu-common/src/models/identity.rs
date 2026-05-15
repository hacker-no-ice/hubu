use crate::actor::OwnerRef;
use crate::ids::{AgentId, AgentVersionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Logical agent identity, answers the question "Who is this agent?"
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub id: AgentId,

    pub display_name: String,
    pub description: Option<String>,

    pub owner: OwnerRef,

    pub agent_type: AgentType,
    pub agent_status: AgentStatus,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Specific config for an agent, answers the question "What exact code/model/config the agent is running?"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentVersion {
    pub id: AgentVersionId,
    pub agent_id: AgentId,

    pub version: String,

    pub code_ref: Option<CodeReference>,
    pub model: Option<ModelIdentity>,
    pub runtime: Option<RuntimeIdentity>,

    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentType {
    InteractiveAgent,
    AutonomousAgent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Active,
    Suspended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeReference {
    pub repository_url: Option<String>,
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub provider: String,
    pub model: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub runtime_provider: String,
    pub environment: RuntimeEnvironment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

        assert_eq!(decoded.id, identity.id);
        assert_eq!(decoded.display_name, "Settlement Agent");
        assert_eq!(
            decoded.description.as_deref(),
            Some("Handles settlement approvals")
        );
        assert!(matches!(decoded.owner.owner_type, OwnerType::Organization));
        assert_eq!(decoded.owner.owner_id, "org_123");
        assert!(matches!(decoded.agent_type, AgentType::AutonomousAgent));
        assert!(matches!(decoded.agent_status, AgentStatus::Active));
        assert_eq!(decoded.created_at, timestamp());
        assert_eq!(decoded.updated_at, timestamp());
    }

    #[test]
    fn agent_version_can_be_constructed_and_round_tripped() {
        let version = AgentVersion {
            id: agent_version_id(),
            agent_id: agent_id(),
            version: "v1.0.0".to_string(),
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

        assert_eq!(decoded.id, version.id);
        assert_eq!(decoded.agent_id, version.agent_id);
        assert_eq!(decoded.version, "v1.0.0");

        let code_ref = decoded.code_ref.unwrap();
        assert_eq!(
            code_ref.repository_url.as_deref(),
            Some("https://github.com/example/hubu-agent")
        );
        assert_eq!(code_ref.commit_sha.as_deref(), Some("abc123"));

        let model = decoded.model.unwrap();
        assert_eq!(model.provider, "openai");
        assert_eq!(model.model, "gpt-5.5");
        assert_eq!(model.version.as_deref(), Some("2026-05-15"));

        let runtime = decoded.runtime.unwrap();
        assert_eq!(runtime.runtime_provider, "codex");
        assert!(matches!(
            runtime.environment,
            RuntimeEnvironment::Production
        ));
        assert_eq!(decoded.created_at, timestamp());
    }
}
