use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// A decimal major-unit amount parsed without floating-point arithmetic.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct DecimalMajorAmount {
    minor_units: i64,
}

impl DecimalMajorAmount {
    pub fn minor_units(self) -> i64 {
        self.minor_units
    }
}

impl FromStr for DecimalMajorAmount {
    type Err = ParseDecimalMajorAmountError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
            return Err(ParseDecimalMajorAmountError(value.to_string()));
        }
        let mut parts = value.split('.');
        let major = parts.next().unwrap_or_default();
        let fractional = parts.next().unwrap_or("0");
        if parts.next().is_some()
            || major.is_empty()
            || fractional.is_empty()
            || fractional.len() > 2
            || !major.bytes().all(|byte| byte.is_ascii_digit())
            || !fractional.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ParseDecimalMajorAmountError(value.to_string()));
        }
        let major = major
            .parse::<i64>()
            .map_err(|_| ParseDecimalMajorAmountError(value.to_string()))?;
        let fractional = match fractional.len() {
            1 => {
                fractional
                    .parse::<i64>()
                    .map_err(|_| ParseDecimalMajorAmountError(value.to_string()))?
                    * 10
            }
            _ => fractional
                .parse::<i64>()
                .map_err(|_| ParseDecimalMajorAmountError(value.to_string()))?,
        };
        let minor_units = major
            .checked_mul(100)
            .and_then(|major| major.checked_add(fractional))
            .ok_or_else(|| ParseDecimalMajorAmountError(value.to_string()))?;
        Ok(Self { minor_units })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid decimal major-unit amount `{0}`; use a non-negative value with at most two decimal places")]
pub struct ParseDecimalMajorAmountError(String);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_major_amount_preserves_major_and_fractional_units() {
        assert_eq!(
            "5".parse::<DecimalMajorAmount>().unwrap().minor_units(),
            500
        );
        assert_eq!(
            "0.05".parse::<DecimalMajorAmount>().unwrap().minor_units(),
            5
        );
        assert_eq!(
            "5.1".parse::<DecimalMajorAmount>().unwrap().minor_units(),
            510
        );
    }

    #[test]
    fn decimal_major_amount_rejects_ambiguous_or_unsafe_values() {
        for value in ["", ".5", "5.", "-1", "+1", "1.001", "1e2"] {
            assert!(value.parse::<DecimalMajorAmount>().is_err(), "{value}");
        }
        assert!(format!("{}", i64::MAX)
            .parse::<DecimalMajorAmount>()
            .is_err());
    }
}
