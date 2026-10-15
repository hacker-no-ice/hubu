use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Fractional digits of every budget-plane [`Amount`]: one unit is 10⁻⁶ of the
/// budget currency (µUSD).
pub const ACCOUNTING_SCALE: u32 = 6;

/// Largest scale an [`ExactMoney`] fact may carry, matching the ledger.
pub const MAX_EXACT_SCALE: u32 = 18;

const UNITS_PER_MAJOR: i64 = 1_000_000;
const UNITS_PER_LEGACY_CENT: i64 = 10_000;

/// A decimal major-unit amount parsed without floating-point arithmetic.
#[deprecated(note = "parses at most two decimals into cents; use `Amount` instead")]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct DecimalMajorAmount {
    minor_units: i64,
}

#[allow(deprecated)]
impl DecimalMajorAmount {
    pub fn minor_units(self) -> i64 {
        self.minor_units
    }
}

#[allow(deprecated)]
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

/// The currency a budget is denominated in. Budgets are per currency: spend
/// in a currency only counts against a budget in that same currency, with no
/// conversion between currencies.
#[derive(Debug, Copy, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Usd,
    Usdc,
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Currency::Usd => f.write_str("usd"),
            Currency::Usdc => f.write_str("usdc"),
        }
    }
}

impl FromStr for Currency {
    type Err = ParseCurrencyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "usd" => Ok(Currency::Usd),
            "usdc" => Ok(Currency::Usdc),
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

/// An asset an exact external amount (receipt, quote, payment requirement) can
/// be denominated in.
#[derive(Debug, Copy, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Asset {
    Usd,
    Usdc,
}

impl Asset {
    /// Smallest unit the asset itself moves in: cents for USD, atomic units
    /// for USDC.
    pub const fn native_scale(self) -> u32 {
        match self {
            Asset::Usd => 2,
            Asset::Usdc => 6,
        }
    }
}

impl From<Currency> for Asset {
    fn from(currency: Currency) -> Self {
        match currency {
            Currency::Usd => Asset::Usd,
            Currency::Usdc => Asset::Usdc,
        }
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Asset::Usd => f.write_str("usd"),
            Asset::Usdc => f.write_str("usdc"),
        }
    }
}

impl FromStr for Asset {
    type Err = ParseAssetError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "usd" => Ok(Asset::Usd),
            "usdc" => Ok(Asset::Usdc),
            _ => Err(ParseAssetError {
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unsupported asset `{value}`")]
pub struct ParseAssetError {
    value: String,
}

/// A budget-plane amount in units of 10⁻[`ACCOUNTING_SCALE`] of the budget
/// currency. Arithmetic is checked only; there is deliberately no `Add` impl.
/// Negative values occur only as a remaining balance after a reconciled
/// overrun.
#[derive(Debug, Copy, Clone, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Amount(i64);

impl Amount {
    pub const ZERO: Self = Self(0);

    pub const fn from_units(units: i64) -> Self {
        Self(units)
    }

    pub const fn units(self) -> i64 {
        self.0
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    /// Converts a legacy whole-cent amount exactly.
    pub fn from_legacy_cents(cents: i64) -> Result<Self, MoneyError> {
        cents
            .checked_mul(UNITS_PER_LEGACY_CENT)
            .map(Self)
            .ok_or(MoneyError::Overflow)
    }

    /// Cent projection rounded toward +∞, for values that consume budget
    /// (consumed, frozen, holds, charges).
    pub const fn legacy_cents_ceil(self) -> i64 {
        let cents = self.0.div_euclid(UNITS_PER_LEGACY_CENT);
        if self.0.rem_euclid(UNITS_PER_LEGACY_CENT) == 0 {
            cents
        } else {
            cents + 1
        }
    }

    /// Cent projection rounded toward −∞, for values that permit spend
    /// (limits, remaining).
    pub const fn legacy_cents_floor(self) -> i64 {
        self.0.div_euclid(UNITS_PER_LEGACY_CENT)
    }

    /// Exact human-readable form: at least two and at most six fractional
    /// digits, trailing zeros trimmed, never rounded. No currency symbol.
    pub fn display(self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        let per_major = UNITS_PER_MAJOR as u64;
        let fraction = format!("{:06}", magnitude % per_major);
        let fraction = fraction.trim_end_matches('0');
        write!(
            f,
            "{sign}{}.{fraction:0<2}",
            magnitude / per_major,
            fraction = fraction
        )
    }
}

/// Serializes as the exact decimal string (`"0.005"`), never a float.
impl Serialize for Amount {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl FromStr for Amount {
    type Err = ParseAmountError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let input = value;
        let value = value.trim();
        let error = |reason| ParseAmountError {
            input: input.to_string(),
            reason,
        };
        if value.is_empty() {
            return Err(error(ParseAmountErrorReason::Empty));
        }
        if value.starts_with(['-', '+']) {
            return Err(error(ParseAmountErrorReason::Signed));
        }
        let (major, fractional) = match value.split_once('.') {
            Some((major, fractional)) => (major, Some(fractional)),
            None => (value, None),
        };
        let is_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if major.is_empty() || fractional.is_some_and(str::is_empty) {
            return Err(error(ParseAmountErrorReason::MissingDigits));
        }
        if !is_digits(major) || !fractional.is_none_or(is_digits) {
            return Err(error(ParseAmountErrorReason::InvalidCharacter));
        }
        let fractional = fractional.unwrap_or_default();
        if fractional.len() > ACCOUNTING_SCALE as usize {
            return Err(error(ParseAmountErrorReason::TooManyFractionalDigits));
        }
        let fractional_units = if fractional.is_empty() {
            0
        } else {
            format!("{fractional:0<6}")
                .parse::<i64>()
                .map_err(|_| error(ParseAmountErrorReason::InvalidCharacter))?
        };
        major
            .parse::<i64>()
            .ok()
            .and_then(|major| major.checked_mul(UNITS_PER_MAJOR))
            .and_then(|units| units.checked_add(fractional_units))
            .map(Self)
            .ok_or_else(|| error(ParseAmountErrorReason::Overflow))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("invalid amount `{input}`: {reason}")]
pub struct ParseAmountError {
    input: String,
    reason: ParseAmountErrorReason,
}

impl ParseAmountError {
    pub fn reason(&self) -> ParseAmountErrorReason {
        self.reason
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ParseAmountErrorReason {
    #[error("enter a decimal such as 5, 0.10 or 0.005")]
    Empty,
    #[error("amounts are non-negative and must not carry a sign")]
    Signed,
    #[error("write digits on both sides of the decimal point, such as 0.5 instead of .5")]
    MissingDigits,
    #[error(
        "use only the ASCII digits 0-9 and at most one decimal point; exponents are not accepted"
    )]
    InvalidCharacter,
    #[error("use at most 6 decimal places")]
    TooManyFractionalDigits,
    #[error("the amount is too large")]
    Overflow,
}

/// An exact external amount (provider receipt, executor quote, payment
/// requirement) at its own scale, before it enters the budget plane. Costs are
/// never negative, and the fields are private so that invariant and the scale
/// bound hold for every value.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ExactMoney {
    amount: i128,
    scale: u32,
    asset: Asset,
}

impl ExactMoney {
    pub fn new(amount: i128, scale: u32, asset: Asset) -> Result<Self, MoneyError> {
        if amount < 0 {
            return Err(MoneyError::Negative);
        }
        if scale > MAX_EXACT_SCALE {
            return Err(MoneyError::ScaleTooLarge { scale });
        }
        Ok(Self {
            amount,
            scale,
            asset,
        })
    }

    pub fn amount(&self) -> i128 {
        self.amount
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn asset(&self) -> Asset {
        self.asset
    }

    /// Converts into the budget currency through `rate` and rounds up to the
    /// accounting scale. This is the single rounding step of a crossing into
    /// the budget plane.
    pub fn to_amount_ceil(&self, rate: &ConversionRate) -> Result<Amount, MoneyError> {
        // units = amount × num × 10^6 / (den × 10^scale), rounded toward +∞.
        let numerator = i128::try_from(rate.numerator)
            .ok()
            .and_then(|rate_numerator| self.amount.checked_mul(rate_numerator))
            .and_then(|value| value.checked_mul(10_i128.pow(ACCOUNTING_SCALE)))
            .ok_or(MoneyError::Overflow)?;
        let denominator = i128::try_from(rate.denominator)
            .ok()
            .and_then(|rate_denominator| rate_denominator.checked_mul(10_i128.pow(self.scale)))
            .ok_or(MoneyError::Overflow)?;
        // Both operands are non-negative, so truncating division is floor.
        let quotient = numerator / denominator;
        let units = if numerator % denominator == 0 {
            quotient
        } else {
            quotient + 1
        };
        i64::try_from(units)
            .map(Amount)
            .map_err(|_| MoneyError::Overflow)
    }
}

/// A rate converting an asset into the budget currency, recorded with its
/// provenance so it can be frozen on a hold.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ConversionRate {
    numerator: u128,
    denominator: u128,
    source: String,
    observed_at: DateTime<Utc>,
}

impl ConversionRate {
    pub fn new(
        numerator: u128,
        denominator: u128,
        source: impl Into<String>,
        observed_at: DateTime<Utc>,
    ) -> Result<Self, MoneyError> {
        if denominator == 0 {
            return Err(MoneyError::ZeroDenominator);
        }
        Ok(Self {
            numerator,
            denominator,
            source: source.into(),
            observed_at,
        })
    }

    /// The 1:1 rate for an amount already in the budget currency.
    pub fn identity(source: impl Into<String>, observed_at: DateTime<Utc>) -> Self {
        Self {
            numerator: 1,
            denominator: 1,
            source: source.into(),
            observed_at,
        }
    }

    pub fn numerator(&self) -> u128 {
        self.numerator
    }

    pub fn denominator(&self) -> u128 {
        self.denominator
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }
}

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum MoneyError {
    #[error("amount overflows the supported range")]
    Overflow,
    #[error("exact amounts must not be negative")]
    Negative,
    #[error("scale {scale} exceeds the maximum exact scale of {MAX_EXACT_SCALE}")]
    ScaleTooLarge { scale: u32 },
    #[error("conversion rate denominator must be non-zero")]
    ZeroDenominator,
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn amount(value: &str) -> Amount {
        value.parse().unwrap()
    }

    fn identity() -> ConversionRate {
        ConversionRate::identity("test", DateTime::<Utc>::UNIX_EPOCH)
    }

    fn exact(amount: i128, scale: u32, asset: Asset) -> ExactMoney {
        ExactMoney::new(amount, scale, asset).unwrap()
    }

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

    #[test]
    fn asset_native_scales_and_names() {
        assert_eq!(Asset::Usd.native_scale(), 2);
        assert_eq!(Asset::Usdc.native_scale(), 6);
        assert_eq!(Asset::from(Currency::Usd), Asset::Usd);
        assert_eq!("usdc".parse::<Asset>().unwrap(), Asset::Usdc);
        assert_eq!(Asset::Usdc.to_string(), "usdc");
        assert_eq!(serde_json::to_string(&Asset::Usdc).unwrap(), "\"usdc\"");
        assert!("eth".parse::<Asset>().is_err());
    }

    #[test]
    fn currencies_parse_and_map_to_assets() {
        assert_eq!(serde_json::to_string(&Currency::Usd).unwrap(), "\"usd\"");
        assert_eq!(serde_json::to_string(&Currency::Usdc).unwrap(), "\"usdc\"");
        assert_eq!("usdc".parse::<Currency>().unwrap(), Currency::Usdc);
        assert_eq!(Asset::from(Currency::Usdc), Asset::Usdc);
        assert_eq!(Asset::from(Currency::Usdc).native_scale(), 6);
        assert!("eth".parse::<Currency>().is_err());
        assert!(serde_json::from_str::<Currency>("\"eth\"").is_err());
    }

    #[test]
    fn amount_parser_accepts_exact_decimals() {
        assert_eq!(amount("5").units(), 5_000_000);
        assert_eq!(amount("5.1").units(), 5_100_000);
        assert_eq!(amount("0.005").units(), 5_000);
        assert_eq!(amount("0.0010").units(), 1_000);
        assert_eq!(amount("0.000001").units(), 1);
        assert_eq!(amount("0").units(), 0);
        assert_eq!(amount(" 10.00 ").units(), 10_000_000);
        assert_eq!(
            amount("9223372036854.775807").units(),
            i64::MAX,
            "largest representable amount"
        );
    }

    #[test]
    fn amount_parser_rejects_unsafe_values_with_reasons() {
        use ParseAmountErrorReason::*;
        for (value, reason) in [
            ("", Empty),
            ("   ", Empty),
            ("0.0000001", TooManyFractionalDigits),
            ("0.0050000", TooManyFractionalDigits),
            ("1e-3", InvalidCharacter),
            ("1E3", InvalidCharacter),
            ("-0.001", Signed),
            ("+1", Signed),
            (".5", MissingDigits),
            ("5.", MissingDigits),
            ("1.2.3", InvalidCharacter),
            ("1,000", InvalidCharacter),
            ("$5", InvalidCharacter),
            ("١٢", InvalidCharacter),
            ("５", InvalidCharacter),
            ("9223372036854.775808", Overflow),
            ("99999999999999999999", Overflow),
        ] {
            let err = value.parse::<Amount>().unwrap_err();
            assert_eq!(err.reason(), reason, "{value:?}");
        }
    }

    #[test]
    fn amount_parser_errors_explain_the_fix() {
        let message = "0.0000001".parse::<Amount>().unwrap_err().to_string();
        assert_eq!(
            message,
            "invalid amount `0.0000001`: use at most 6 decimal places"
        );
        let message = "1e-3".parse::<Amount>().unwrap_err().to_string();
        assert!(message.contains("exponents are not accepted"), "{message}");
    }

    #[test]
    fn amount_display_is_adaptive_and_exact() {
        for (units, expected) in [
            (0, "0.00"),
            (10_000_000, "10.00"),
            (35_000, "0.035"),
            (1, "0.000001"),
            (12_500_000, "12.50"),
            (12_345_600, "12.3456"),
            (100_000, "0.10"),
            (999_999, "0.999999"),
            (-1_000, "-0.001"),
            (-10_000_000, "-10.00"),
            (i64::MAX, "9223372036854.775807"),
            (i64::MIN, "-9223372036854.775808"),
        ] {
            assert_eq!(Amount::from_units(units).display(), expected, "{units}");
        }
    }

    #[test]
    fn amount_serializes_as_an_exact_decimal_string() {
        assert_eq!(
            serde_json::to_value(Amount::from_units(5_000)).unwrap(),
            serde_json::json!("0.005")
        );
    }

    #[test]
    fn amount_arithmetic_is_checked() {
        let one = Amount::from_units(1);
        assert_eq!(one.checked_add(one), Some(Amount::from_units(2)));
        assert_eq!(Amount::ZERO.checked_sub(one), Some(Amount::from_units(-1)));
        assert!(Amount::from_units(-1).is_negative());
        assert_eq!(Amount::from_units(i64::MAX).checked_add(one), None);
        assert_eq!(Amount::from_units(i64::MIN).checked_sub(one), None);
    }

    #[test]
    fn legacy_cents_conversions() {
        assert_eq!(Amount::from_legacy_cents(17).unwrap().units(), 170_000);
        assert_eq!(Amount::from_legacy_cents(0).unwrap(), Amount::ZERO);
        assert_eq!(
            Amount::from_legacy_cents(i64::MAX),
            Err(MoneyError::Overflow)
        );
        assert_eq!(
            Amount::from_legacy_cents(922_337_203_685_477)
                .unwrap()
                .legacy_cents_floor(),
            922_337_203_685_477
        );

        let one = Amount::from_units(1);
        assert_eq!(one.legacy_cents_ceil(), 1);
        assert_eq!(one.legacy_cents_floor(), 0);
        let cent = Amount::from_units(10_000);
        assert_eq!(cent.legacy_cents_ceil(), 1);
        assert_eq!(cent.legacy_cents_floor(), 1);
        let overrun = Amount::from_units(-1);
        assert_eq!(overrun.legacy_cents_ceil(), 0);
        assert_eq!(overrun.legacy_cents_floor(), -1);
        assert_eq!(
            Amount::from_units(i64::MAX).legacy_cents_ceil(),
            922_337_203_685_478
        );
        assert_eq!(
            Amount::from_units(i64::MIN).legacy_cents_floor(),
            -922_337_203_685_478
        );
    }

    #[test]
    fn exact_money_converts_with_a_single_ceiling() {
        let rate = identity();
        for (value, scale, units) in [
            (1, 3, 1_000),
            (1, 7, 1),
            (10, 7, 1),
            (11, 7, 2),
            (0, 18, 0),
            (31, 4, 3_100),
            (1, 2, 10_000),
            (1, 18, 1),
            (1_000_000_000_000, 18, 1),
            (1_000_000_000_001, 18, 2),
        ] {
            assert_eq!(
                exact(value, scale, Asset::Usd)
                    .to_amount_ceil(&rate)
                    .unwrap(),
                Amount::from_units(units),
                "({value}, {scale})"
            );
        }
        assert_eq!(
            exact(5_000, 6, Asset::Usdc).to_amount_ceil(&rate).unwrap(),
            Amount::from_units(5_000)
        );
    }

    #[test]
    fn exact_money_applies_a_conversion_rate() {
        let rate =
            ConversionRate::new(998, 1_000, "static-config", DateTime::<Utc>::UNIX_EPOCH).unwrap();
        assert_eq!(rate.source(), "static-config");
        assert_eq!(
            exact(5_000, 6, Asset::Usdc).to_amount_ceil(&rate).unwrap(),
            Amount::from_units(4_990)
        );
        // 0.0050001 USDC × 0.998 = 0.0049900998 → ceil 0.004991
        assert_eq!(
            exact(50_001, 7, Asset::Usdc).to_amount_ceil(&rate).unwrap(),
            Amount::from_units(4_991)
        );
    }

    #[test]
    fn exact_money_rejects_invalid_inputs() {
        assert_eq!(
            ExactMoney::new(1, 19, Asset::Usd),
            Err(MoneyError::ScaleTooLarge { scale: 19 })
        );
        assert_eq!(
            ExactMoney::new(-1, 6, Asset::Usdc),
            Err(MoneyError::Negative)
        );
        assert_eq!(
            ExactMoney::new(i128::MIN, 0, Asset::Usd),
            Err(MoneyError::Negative)
        );
        let fact = exact(31, 4, Asset::Usd);
        assert_eq!(
            (fact.amount(), fact.scale(), fact.asset()),
            (31, 4, Asset::Usd)
        );
        assert_eq!(
            exact(i128::MAX, 0, Asset::Usd).to_amount_ceil(&identity()),
            Err(MoneyError::Overflow)
        );
        assert_eq!(
            exact(9_223_372_036_855, 0, Asset::Usd).to_amount_ceil(&identity()),
            Err(MoneyError::Overflow),
            "fits i128 but not an i64 Amount"
        );
        assert_eq!(
            ConversionRate::new(1, 0, "test", DateTime::<Utc>::UNIX_EPOCH),
            Err(MoneyError::ZeroDenominator)
        );
        let huge_rate =
            ConversionRate::new(u128::MAX, 1, "test", DateTime::<Utc>::UNIX_EPOCH).unwrap();
        assert_eq!(
            exact(1, 0, Asset::Usd).to_amount_ceil(&huge_rate),
            Err(MoneyError::Overflow)
        );
    }

    proptest! {
        #[test]
        fn ceiling_sums_bound_the_exact_sum(
            costs in prop::collection::vec((0_i128..1_000_000_000_000, 0_u32..=MAX_EXACT_SCALE), 1..64)
        ) {
            let rate = identity();
            let mut ceil_sum: i128 = 0;
            // Σ exact, held at scale 18.
            let mut exact_sum_e18: i128 = 0;
            for (value, scale) in &costs {
                let units = exact(*value, *scale, Asset::Usd).to_amount_ceil(&rate).unwrap();
                ceil_sum += i128::from(units.units());
                exact_sum_e18 += value * 10_i128.pow(MAX_EXACT_SCALE - scale);
            }
            let ceil_sum_e18 = ceil_sum * 10_i128.pow(MAX_EXACT_SCALE - ACCOUNTING_SCALE);
            let one_unit_e18 = 10_i128.pow(MAX_EXACT_SCALE - ACCOUNTING_SCALE);
            prop_assert!(ceil_sum_e18 >= exact_sum_e18);
            prop_assert!(ceil_sum_e18 - exact_sum_e18 < costs.len() as i128 * one_unit_e18);
        }

        #[test]
        fn display_round_trips_through_the_parser(units in 0_i64..=i64::MAX) {
            let value = Amount::from_units(units);
            prop_assert_eq!(value.display().parse::<Amount>().unwrap(), value);
        }

        #[test]
        fn display_never_rounds(units in i64::MIN..=i64::MAX) {
            let shown = Amount::from_units(units).display();
            let digits: String = shown.chars().filter(char::is_ascii_digit).collect();
            let fraction = shown.split_once('.').unwrap().1;
            prop_assert!((2..=6).contains(&fraction.len()));
            let rebuilt = digits.parse::<i128>().unwrap()
                * 10_i128.pow(ACCOUNTING_SCALE - fraction.len() as u32);
            prop_assert_eq!(rebuilt, i128::from(units).abs());
        }

        #[test]
        fn legacy_projections_bracket_the_value(units in i64::MIN..=i64::MAX) {
            let value = Amount::from_units(units);
            let floor = i128::from(value.legacy_cents_floor()) * 10_000;
            let ceil = i128::from(value.legacy_cents_ceil()) * 10_000;
            prop_assert!(floor <= i128::from(units) && i128::from(units) <= ceil);
            prop_assert!(ceil - floor <= 10_000);
        }
    }
}
