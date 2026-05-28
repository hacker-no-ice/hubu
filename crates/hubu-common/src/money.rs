use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Copy, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Usd,
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Currency::Usd => f.write_str("usd"),
        }
    }
}

impl FromStr for Currency {
    type Err = ParseCurrencyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "usd" => Ok(Currency::Usd),
            _ => Err(ParseCurrencyError {
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unsupported currency `{value}`")]
pub struct ParseCurrencyError {
    value: String,
}
