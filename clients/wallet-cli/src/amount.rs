//! ATC amounts: decimal strings with up to 18 fractional digits ↔ smallest units.

use anyhow::{Context, Result, bail};

/// Fractional digits of ATC.
pub const DECIMALS: u32 = 18;

/// Parses a decimal ATC amount such as `12`, `0.5` or `1.000000000000000001`.
///
/// # Errors
///
/// Rejects empty strings, signs, more than 18 fractional digits and overflow.
pub fn parse_atc(text: &str) -> Result<u128> {
    let text = text.trim();
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && frac.is_empty() {
        bail!("empty amount");
    }
    if !whole
        .chars()
        .chain(frac.chars())
        .all(|c| c.is_ascii_digit())
    {
        bail!("amount must be a non-negative decimal number: {text}");
    }
    if frac.len() > usize::try_from(DECIMALS)? {
        bail!("at most {DECIMALS} decimal places");
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().context("amount too large")?
    };
    let mut frac_units: u128 = if frac.is_empty() {
        0
    } else {
        frac.parse().context("bad fraction")?
    };
    for _ in frac.len()..usize::try_from(DECIMALS)? {
        frac_units = frac_units.checked_mul(10).context("amount too large")?;
    }
    whole
        .checked_mul(ac_runtime::ATC)
        .and_then(|w| w.checked_add(frac_units))
        .context("amount too large")
}

/// Formats smallest units as a decimal ATC string without trailing zeros.
#[must_use]
pub fn format_atc(units: u128) -> String {
    let whole = units / ac_runtime::ATC;
    let frac = units % ac_runtime::ATC;
    if frac == 0 {
        return format!("{whole} ATC");
    }
    let digits = format!("{frac:018}");
    format!("{whole}.{} ATC", digits.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        assert_eq!(parse_atc("1").unwrap(), ac_runtime::ATC);
        assert_eq!(parse_atc("0.5").unwrap(), ac_runtime::ATC / 2);
        assert_eq!(parse_atc(".000000000000000001").unwrap(), 1);
        assert!(parse_atc("-1").is_err());
        assert!(parse_atc("1.0000000000000000001").is_err());
        assert!(parse_atc("").is_err());
        assert_eq!(format_atc(ac_runtime::ATC * 3 / 2), "1.5 ATC");
        assert_eq!(format_atc(ac_runtime::ATC), "1 ATC");
    }
}
