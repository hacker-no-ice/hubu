//! Exact money input and display for the CLI.
//!
//! Hubu responses carry exact money objects (`{units, scale, currency,
//! display}`) beside legacy `*_cents` projections. The CLI reads the exact
//! object and falls back to the cent field from older servers. By default it
//! shows the exact adaptive form (`$0.005`, `$10.00`); `--round cents` shows
//! cent-rounded values instead, marked with `≈` when rounding changed them.

use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use hubu_common::money::{Amount, Currency, ACCOUNTING_SCALE};
use serde_json::Value;

/// How the CLI shows amounts, chosen by the global `--round` option.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Rounding {
    /// The exact adaptive form: 2 to 6 decimal places, never rounded.
    #[default]
    Exact,
    /// Whole cents, rounded conservatively and marked with `≈`.
    Cents,
}

impl std::str::FromStr for Rounding {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "exact" => Ok(Self::Exact),
            "cents" => Ok(Self::Cents),
            _ => Err("expected `exact` or `cents`".to_string()),
        }
    }
}

static ROUNDING: OnceLock<Rounding> = OnceLock::new();

pub(crate) fn configure(rounding: Rounding) {
    let _ = ROUNDING.set(rounding);
}

fn rounding() -> Rounding {
    ROUNDING.get().copied().unwrap_or_default()
}

/// Which way a cent-rounded amount moves. Amounts that use budget (consumed,
/// frozen, holds, charges, overruns) round up; amounts that permit spend
/// (limits, remaining, released) round down, so a rounded view never shows
/// more available budget than exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Up,
    Down,
}

impl Direction {
    /// The conservative direction for a response field, by its name.
    pub(crate) fn for_field(field: &str) -> Self {
        let field = field.trim_end_matches("_cents");
        if [
            "remaining",
            "available",
            "limit",
            "released",
            "target_amount",
        ]
        .iter()
        .any(|permits_spend| field.contains(permits_spend))
        {
            Self::Down
        } else {
            Self::Up
        }
    }
}

/// An exact amount and, when the server reported it, its currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Money {
    pub(crate) amount: Amount,
    pub(crate) currency: Option<Currency>,
}

impl Money {
    pub(crate) fn new(amount: Amount, currency: Option<Currency>) -> Self {
        Self { amount, currency }
    }
}

/// Read one amount from a response object: the exact money object named by
/// `legacy_field` without its `_cents` suffix, or the legacy cent field from
/// an older server.
pub(crate) fn money_field(value: &Value, legacy_field: &str) -> Result<Money> {
    let exact_field = legacy_field
        .strip_suffix("_cents")
        .ok_or_else(|| anyhow!("`{legacy_field}` is not a cent field"))?;
    let currency = value
        .get("currency")
        .and_then(Value::as_str)
        .and_then(|currency| currency.to_ascii_lowercase().parse().ok());
    if let Some(exact) = value.get(exact_field).filter(|exact| exact.is_object()) {
        return money_object(exact)
            .map(|money| Money {
                currency: money.currency.or(currency),
                ..money
            })
            .with_context(|| format!("server response has an invalid `{exact_field}`"));
    }
    let cents = value
        .get(legacy_field)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("server response missing `{legacy_field}`"))?;
    Ok(Money::new(Amount::from_legacy_cents(cents)?, currency))
}

/// Parse an exact money object `{units, scale, currency}`.
pub(crate) fn money_object(value: &Value) -> Result<Money> {
    let units = value
        .get("units")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("money object is missing `units`"))?;
    let scale = value
        .get("scale")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("money object is missing `scale`"))?;
    if scale != u64::from(ACCOUNTING_SCALE) {
        bail!("money object has unsupported scale {scale}");
    }
    let units = units
        .parse::<i64>()
        .map_err(|_| anyhow!("money object has invalid units `{units}`"))?;
    let currency = value
        .get("currency")
        .and_then(Value::as_str)
        .map(|currency| currency.to_ascii_lowercase().parse::<Currency>())
        .transpose()
        .map_err(|error| anyhow!(error))?;
    Ok(Money::new(Amount::from_units(units), currency))
}

/// Format a response amount for people, honoring `--round`.
pub(crate) fn format_money(money: Money, direction: Direction) -> String {
    format_money_with(money, direction, rounding())
}

pub(crate) fn format_money_with(money: Money, direction: Direction, rounding: Rounding) -> String {
    let (approximate, text) = match rounding {
        Rounding::Exact => (false, money.amount.display()),
        Rounding::Cents => {
            let cents = match direction {
                Direction::Up => money.amount.legacy_cents_ceil(),
                Direction::Down => money.amount.legacy_cents_floor(),
            };
            let rounded = Amount::from_legacy_cents(cents).unwrap_or(money.amount);
            (rounded != money.amount, rounded.display())
        }
    };
    let (sign, magnitude) = match text.strip_prefix('-') {
        Some(magnitude) => ("-", magnitude),
        None => ("", text.as_str()),
    };
    let approximate = if approximate { "≈" } else { "" };
    match money.currency {
        Some(Currency::Usdc) => format!("{approximate}{sign}{magnitude} USDC"),
        _ => format!("{approximate}{sign}${magnitude}"),
    }
}

/// Parse an operator-typed decimal amount, such as `5`, `10.00` or `0.005`.
pub(crate) fn parse_amount(value: &str, flag: &str) -> Result<Amount> {
    value
        .trim()
        .parse::<Amount>()
        .map_err(|error| anyhow!("{flag}: {error}"))
}

/// Parse a positive operator-typed decimal amount.
pub(crate) fn parse_positive_amount(value: &str, flag: &str) -> Result<Amount> {
    let amount = parse_amount(value, flag)?;
    if amount.units() <= 0 {
        bail!("{flag} must be greater than zero");
    }
    Ok(amount)
}

/// Parse an exact provider cost such as `0.0031` into an integer coefficient
/// and its decimal scale (at most 18), without rounding.
pub(crate) fn parse_exact_cost(value: &str, flag: &str) -> Result<(i64, u32)> {
    let value = value.trim();
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty()
        || !digits(whole)
        || !digits(fraction)
        || (value.contains('.') && fraction.is_empty())
    {
        bail!("{flag} must be a non-negative decimal such as 0.0031");
    }
    let scale = u32::try_from(fraction.len()).unwrap_or(u32::MAX);
    if scale > 18 {
        bail!("{flag} supports at most 18 decimal places");
    }
    let coefficient = format!("{whole}{fraction}")
        .parse::<i64>()
        .map_err(|_| anyhow!("{flag} is too large"))?;
    Ok((coefficient, scale))
}

/// Format an exact decimal of any scale (such as a provider cost with
/// `{amount, scale}`) adaptively: at least two decimal places, trailing zeros
/// trimmed, never rounded.
pub(crate) fn format_exact_decimal(coefficient: &str, scale: u32) -> Option<String> {
    if coefficient.is_empty() || !coefficient.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let places = usize::try_from(scale).ok()?;
    let padded = format!("{coefficient:0>width$}", width = places + 1);
    let split = padded.len() - places;
    let whole = padded[..split].trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = padded[split..].trim_end_matches('0');
    Some(format!("{whole}.{fraction:0<2}"))
}

/// Pad amounts so their decimal points line up in a column.
pub(crate) fn align_decimals(values: &[String]) -> Vec<String> {
    let split = |value: &str| -> (usize, usize) {
        let point = value.find('.').unwrap_or(value.len());
        let whole = value[..point].chars().count();
        (whole, value.chars().count() - whole)
    };
    let whole_width = values.iter().map(|v| split(v).0).max().unwrap_or(0);
    let rest_width = values.iter().map(|v| split(v).1).max().unwrap_or(0);
    values
        .iter()
        .map(|value| {
            let (whole, rest) = split(value);
            format!(
                "{}{value}{}",
                " ".repeat(whole_width - whole),
                " ".repeat(rest_width - rest)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usd(value: &str) -> Money {
        let amount = match value.strip_prefix('-') {
            Some(magnitude) => Amount::from_units(-magnitude.parse::<Amount>().unwrap().units()),
            None => value.parse().unwrap(),
        };
        Money::new(amount, Some(Currency::Usd))
    }

    #[test]
    fn exact_display_is_adaptive_and_never_rounded() {
        for (value, shown) in [
            ("0.035", "$0.035"),
            ("10", "$10.00"),
            ("0.000001", "$0.000001"),
            ("-0.0031", "-$0.0031"),
        ] {
            assert_eq!(
                format_money_with(usd(value), Direction::Up, Rounding::Exact),
                shown
            );
        }
        assert_eq!(
            format_money_with(
                Money::new("0.005".parse().unwrap(), Some(Currency::Usdc)),
                Direction::Up,
                Rounding::Exact
            ),
            "0.005 USDC"
        );
    }

    #[test]
    fn cent_rounding_is_conservative_and_marked() {
        let rounded = |value, direction| format_money_with(usd(value), direction, Rounding::Cents);
        assert_eq!(rounded("0.035", Direction::Up), "≈$0.04");
        assert_eq!(rounded("0.035", Direction::Down), "≈$0.03");
        assert_eq!(rounded("10", Direction::Down), "$10.00");
        // A negative overrun remainder rounds toward the conservative side too.
        assert_eq!(rounded("-0.0131", Direction::Up), "≈-$0.01");
        assert_eq!(rounded("-0.0131", Direction::Down), "≈-$0.02");
        assert_eq!(
            Direction::for_field("remaining_amount_cents"),
            Direction::Down
        );
        assert_eq!(Direction::for_field("amount_limit_cents"), Direction::Down);
        assert_eq!(Direction::for_field("consumed_amount_cents"), Direction::Up);
        assert_eq!(Direction::for_field("frozen_amount_cents"), Direction::Up);
    }

    #[test]
    fn reads_exact_objects_and_falls_back_to_legacy_cents() {
        let current = json!({
            "consumed_amount": {"units": "3100", "scale": 6, "currency": "usdc", "display": "0.0031"},
            "consumed_amount_cents": 1
        });
        assert_eq!(
            money_field(&current, "consumed_amount_cents").unwrap(),
            Money::new("0.0031".parse().unwrap(), Some(Currency::Usdc))
        );
        let older = json!({"consumed_amount_cents": 125, "currency": "usd"});
        assert_eq!(
            money_field(&older, "consumed_amount_cents").unwrap(),
            usd("1.25")
        );
        assert!(money_field(&json!({}), "consumed_amount_cents").is_err());
        assert!(money_field(
            &json!({"consumed_amount": {"units": "1", "scale": 2, "currency": "usd"}}),
            "consumed_amount_cents"
        )
        .is_err());
    }

    #[test]
    fn parses_operator_amounts_and_exact_costs() {
        assert_eq!(
            parse_amount("0.005", "--amount").unwrap(),
            "0.005".parse().unwrap()
        );
        assert!(parse_amount("0.0000001", "--amount").is_err());
        assert!(parse_positive_amount("0", "--amount").is_err());
        assert_eq!(parse_exact_cost("0.0031", "--cost").unwrap(), (31, 4));
        assert_eq!(parse_exact_cost("4", "--cost").unwrap(), (4, 0));
        for invalid in ["", ".5", "5.", "-1", "1e3", "0.0000000000000000001"] {
            assert!(parse_exact_cost(invalid, "--cost").is_err(), "{invalid}");
        }
        assert_eq!(format_exact_decimal("58", 3).as_deref(), Some("0.058"));
        assert_eq!(format_exact_decimal("1000", 2).as_deref(), Some("10.00"));
        assert_eq!(format_exact_decimal("x", 2), None);
    }

    #[test]
    fn aligns_decimal_points() {
        let aligned = align_decimals(&[
            "$0.035".to_string(),
            "$10.00".to_string(),
            "$0.000001".to_string(),
        ]);
        assert_eq!(aligned, vec![" $0.035   ", "$10.00    ", " $0.000001"]);
    }
}
