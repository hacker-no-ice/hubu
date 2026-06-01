use serde::{de, Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const PUBLIC_ID_SUFFIX_LEN: usize = 16;
const PUBLIC_ID_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AgentId(Uuid);

impl AgentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AgentVersionId(Uuid);

impl AgentVersionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AgentAccountId(Uuid);

impl AgentAccountId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AgentSessionId(Uuid);

impl AgentSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SpendDecisionId(Uuid);

impl SpendDecisionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SpendAuthTokenId(Uuid);

impl SpendAuthTokenId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PaymentId(Uuid);

impl PaymentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LedgerAccountId(Uuid);

impl LedgerAccountId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LedgerTransactionId(Uuid);

impl LedgerTransactionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LedgerEntryId(Uuid);

impl LedgerEntryId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct BudgetId(Uuid);

impl BudgetId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct UserId(Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TaskId(Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct BudgetHoldId(Uuid);

impl BudgetHoldId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

macro_rules! public_uuid_suffix {
    ($($id:ty),+ $(,)?) => {
        $(
            impl $id {
                pub fn public_suffix(&self) -> String {
                    public_suffix_from_uuid(self.0)
                }
            }
        )+
    };
}

public_uuid_suffix!(AgentId, AgentVersionId, AgentAccountId, AgentSessionId,);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentPubId(String);

impl AgentPubId {
    pub fn from_agent_id(id: &AgentId) -> Self {
        Self(format!("agt_{}", id.public_suffix()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentVersionPubId(String);

impl AgentVersionPubId {
    pub fn from_agent_version_id(id: &AgentVersionId) -> Self {
        Self(format!("agv_{}", id.public_suffix()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentAccountPubId(String);

impl AgentAccountPubId {
    pub fn from_agent_account_id(id: &AgentAccountId) -> Self {
        Self(format!("aga_{}", id.public_suffix()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentSessionPubId(String);

impl AgentSessionPubId {
    pub fn from_agent_session_id(id: &AgentSessionId) -> Self {
        Self(format!("ags_{}", id.public_suffix()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn public_suffix_from_uuid(uuid: Uuid) -> String {
    let mut value = uuid.as_u128();
    let mut suffix = [0_u8; PUBLIC_ID_SUFFIX_LEN];

    // Encode the low 80 UUID bits as 16 base-32 characters.
    for character in suffix.iter_mut().rev() {
        *character = PUBLIC_ID_ALPHABET[(value & 0b11111) as usize];
        value >>= 5;
    }

    String::from_utf8(suffix.to_vec()).expect("public ID alphabet is ASCII")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsePublicIdError {
    expected_prefix: &'static str,
    value: String,
}

impl fmt::Display for ParsePublicIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid public ID `{}`; expected {}_<{} base-32 chars>",
            self.value, self.expected_prefix, PUBLIC_ID_SUFFIX_LEN
        )
    }
}

impl std::error::Error for ParsePublicIdError {}

macro_rules! parse_public_id {
    ($id:ty, $prefix:literal) => {
        impl FromStr for $id {
            type Err = ParsePublicIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let expected_start = concat!($prefix, "_");
                let suffix =
                    value
                        .strip_prefix(expected_start)
                        .ok_or_else(|| ParsePublicIdError {
                            expected_prefix: $prefix,
                            value: value.to_string(),
                        })?;

                if suffix.len() != PUBLIC_ID_SUFFIX_LEN
                    || !suffix
                        .bytes()
                        .all(|byte| PUBLIC_ID_ALPHABET.contains(&byte))
                {
                    return Err(ParsePublicIdError {
                        expected_prefix: $prefix,
                        value: value.to_string(),
                    });
                }

                Ok(Self(value.to_string()))
            }
        }

        impl<'de> Deserialize<'de> for $id {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(de::Error::custom)
            }
        }
    };
}

parse_public_id!(AgentPubId, "agt");
parse_public_id!(AgentVersionPubId, "agv");
parse_public_id!(AgentAccountPubId, "aga");
parse_public_id!(AgentSessionPubId, "ags");

macro_rules! display_uuid_id {
    ($($id:ty),+ $(,)?) => {
        $(
            impl fmt::Display for $id {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{}", self.0)
                }
            }
        )+
    };
}

display_uuid_id!(
    AgentId,
    AgentVersionId,
    AgentAccountId,
    AgentSessionId,
    BudgetId,
    SpendDecisionId,
    SpendAuthTokenId,
    PaymentId,
    LedgerAccountId,
    LedgerTransactionId,
    LedgerEntryId,
    TaskId,
    UserId,
    BudgetHoldId,
);

display_uuid_id!(
    AgentPubId,
    AgentVersionPubId,
    AgentAccountPubId,
    AgentSessionPubId,
);

macro_rules! parse_uuid_id {
    ($($id:ty),+ $(,)?) => {
        $(
            impl FromStr for $id {
                type Err = uuid::Error;

                fn from_str(value: &str) -> Result<Self, Self::Err> {
                    Ok(Self(Uuid::parse_str(value)?))
                }
            }
        )+
    };
}

parse_uuid_id!(
    AgentId,
    AgentVersionId,
    AgentAccountId,
    AgentSessionId,
    BudgetId,
    SpendDecisionId,
    SpendAuthTokenId,
    PaymentId,
    LedgerAccountId,
    LedgerTransactionId,
    LedgerEntryId,
    TaskId,
    UserId,
    BudgetHoldId,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ids_parse_and_serialize_as_strings() {
        let agent_pub_id: AgentPubId = "agt_123456789abcdefg".parse().unwrap();

        assert_eq!(agent_pub_id.to_string(), "agt_123456789abcdefg");
        assert_eq!(
            serde_json::to_string(&agent_pub_id).unwrap(),
            "\"agt_123456789abcdefg\""
        );
        assert_eq!(
            serde_json::from_str::<AgentPubId>("\"agt_123456789abcdefg\"").unwrap(),
            agent_pub_id
        );
    }

    #[test]
    fn public_ids_reject_wrong_prefix_length_or_alphabet() {
        assert!("agv_123456789abcdefg".parse::<AgentPubId>().is_err());
        assert!("agt_123".parse::<AgentPubId>().is_err());
        assert!("agt_123456789abcdefi".parse::<AgentPubId>().is_err());
    }
}
