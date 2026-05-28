use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Usd,
}
