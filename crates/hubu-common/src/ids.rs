use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentId(Uuid);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentVersionId(Uuid);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentAccountId(Uuid);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionId(Uuid);
