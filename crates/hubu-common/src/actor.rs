use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerRef {
    pub owner_type: OwnerType,
    pub owner_id: String, // todo, we need stronger type for UserId and OrgId in later PR
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerType {
    Human,
    Organization,
}
