use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnerRef {
    pub owner_type: OwnerType,
    pub owner_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OwnerType {
    Human,
    Organization,
}
