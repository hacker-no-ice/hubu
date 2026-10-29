use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const PUBLIC_ID_SUFFIX_LEN: usize = 12;
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
pub struct SpendExecutorClaimId(Uuid);

impl SpendExecutorClaimId {
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
pub struct BudgetVersionId(Uuid);

impl BudgetVersionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SpendingTargetId(Uuid);

impl SpendingTargetId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PolicyId(Uuid);

impl PolicyId {
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
pub struct BudgetHoldId(Uuid);

impl BudgetHoldId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

macro_rules! default_uuid_id {
    ($($id:ty),+ $(,)?) => {
        $(
            impl Default for $id {
                fn default() -> Self {
                    Self::new()
                }
            }
        )+
    };
}

default_uuid_id!(
    AgentId,
    AgentVersionId,
    AgentAccountId,
    AgentSessionId,
    SpendDecisionId,
    SpendAuthTokenId,
    SpendExecutorClaimId,
    PaymentId,
    LedgerAccountId,
    LedgerTransactionId,
    LedgerEntryId,
    BudgetId,
    BudgetVersionId,
    SpendingTargetId,
    PolicyId,
    UserId,
    BudgetHoldId,
);

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

public_uuid_suffix!(
    AgentId,
    AgentVersionId,
    AgentAccountId,
    AgentSessionId,
    BudgetId,
    BudgetVersionId,
    SpendingTargetId,
    PolicyId,
    UserId,
);

fn public_suffix_from_uuid(uuid: Uuid) -> String {
    let mut value = uuid.as_u128();
    let mut suffix = [0_u8; PUBLIC_ID_SUFFIX_LEN];

    for character in suffix.iter_mut().rev() {
        *character = PUBLIC_ID_ALPHABET[(value & 0b11111) as usize];
        value >>= 5;
    }

    String::from_utf8(suffix.to_vec()).expect("public ID alphabet is ASCII")
}

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
    BudgetVersionId,
    SpendingTargetId,
    PolicyId,
    SpendDecisionId,
    SpendAuthTokenId,
    SpendExecutorClaimId,
    PaymentId,
    LedgerAccountId,
    LedgerTransactionId,
    LedgerEntryId,
    UserId,
    BudgetHoldId,
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
    BudgetVersionId,
    SpendingTargetId,
    PolicyId,
    SpendDecisionId,
    SpendAuthTokenId,
    SpendExecutorClaimId,
    PaymentId,
    LedgerAccountId,
    LedgerTransactionId,
    LedgerEntryId,
    UserId,
    BudgetHoldId,
);
