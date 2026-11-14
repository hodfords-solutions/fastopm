//! nginx-style intervals: `2m`, `30s`, `1m 30s`, `1.5h`, bare numbers are seconds.
//!
//! Verdaccio reference: `parseInterval` in `packages/proxy/src/proxy-utils.ts`.
//!
//! Grammar: whitespace-separated tokens, each `<number><unit>` where `<number>` is
//! `0|[1-9][0-9]*` with an optional `.[0-9]+` fraction and `<unit>` is one of
//! `ms s m h d w M y` (`M` = 30 days, `y` = 365 days) or empty (= seconds). Units must
//! strictly decrease from left to right, and a bare number is only allowed as the first token.
//! An empty or blank string is zero.

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserialize, Deserializer, Visitor};

/// Error for a string that is not a valid interval. Display text matches Verdaccio:
/// `invalid interval: <input>`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid interval: {0}")]
pub struct IntervalError(pub String);

const MS: f64 = 1.0;
const SECOND: f64 = 1000.0;
const MINUTE: f64 = 60.0 * SECOND;
const HOUR: f64 = 60.0 * MINUTE;
const DAY: f64 = 86_400_000.0;

fn unit_ms(unit: &str) -> Option<f64> {
    Some(match unit {
        "" | "s" => SECOND,
        "ms" => MS,
        "m" => MINUTE,
        "h" => HOUR,
        "d" => DAY,
        "w" => 7.0 * DAY,
        "M" => 30.0 * DAY,
        "y" => 365.0 * DAY,
        _ => return None,
    })
}

/// JavaScript `\s` also matches U+FEFF, which `char::is_whitespace` does not.
fn is_js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

/// Split `token` into its numeric part and unit, checking the number grammar.
fn split_number(token: &str) -> Option<(&str, &str)> {
    let bytes = token.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || (i > 1 && bytes[0] == b'0') {
        return None;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            return None;
        }
        i = j;
    }
    Some((&token[..i], &token[i..]))
}

/// Parse an interval string into milliseconds (may be fractional).
pub fn parse_interval_ms(input: &str) -> Result<f64, IntervalError> {
    let invalid = || IntervalError(input.to_owned());
    let mut total = 0.0;
    let mut last_unit = f64::INFINITY;
    for token in input.split(is_js_whitespace).filter(|t| !t.is_empty()) {
        let (number, unit) = split_number(token).ok_or_else(invalid)?;
        let unit_value = unit_ms(unit).ok_or_else(invalid)?;
        if unit_value >= last_unit || (unit.is_empty() && last_unit != f64::INFINITY) {
            return Err(invalid());
        }
        last_unit = unit_value;
        let number: f64 = number.parse().map_err(|_| invalid())?;
        total += number * unit_value;
    }
    Ok(total)
}

/// Parse an interval string into a [`Duration`].
///
/// Errors when the string does not follow the grammar or the value does not fit a `Duration`.
pub fn parse_interval(input: &str) -> Result<Duration, IntervalError> {
    let ms = parse_interval_ms(input)?;
    Duration::try_from_secs_f64(ms / 1000.0).map_err(|_| IntervalError(input.to_owned()))
}

/// A config value that is either a number of seconds or an interval string.
///
/// Deserializes from a YAML/JSON number (seconds, fractional allowed; negative values clamp to
/// zero, which Verdaccio treats the same as zero) or from a string parsed with
/// [`parse_interval`]. Any other type is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Interval(pub Duration);

impl Interval {
    /// The wrapped duration.
    pub fn duration(self) -> Duration {
        self.0
    }

    /// Interval from a number of seconds (Verdaccio: `interval * 1000` ms).
    pub fn from_secs_f64(secs: f64) -> Option<Self> {
        if secs.is_nan() {
            return None;
        }
        Duration::try_from_secs_f64(secs.max(0.0)).ok().map(Interval)
    }
}

impl std::str::FromStr for Interval {
    type Err = IntervalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_interval(s).map(Interval)
    }
}

impl From<Interval> for Duration {
    fn from(value: Interval) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for Interval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IntervalVisitor;

        impl Visitor<'_> for IntervalVisitor {
            type Value = Interval;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a number of seconds or an interval string such as \"2m\"")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Interval, E> {
                Interval::from_secs_f64(v)
                    .ok_or_else(|| E::custom(format!("invalid interval: {v}")))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Interval, E> {
                self.visit_f64(v as f64)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Interval, E> {
                self.visit_f64(v as f64)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Interval, E> {
                parse_interval(v).map(Interval).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(IntervalVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> f64 {
        parse_interval_ms(s).unwrap()
    }

    #[test]
    fn single_units() {
        assert_eq!(ms("2m"), 120_000.0);
        assert_eq!(ms("30s"), 30_000.0);
        assert_eq!(ms("30"), 30_000.0);
        assert_eq!(ms("500ms"), 500.0);
        assert_eq!(ms("5h"), 5.0 * 3_600_000.0);
        assert_eq!(ms("1d"), 86_400_000.0);
        assert_eq!(ms("1w"), 7.0 * 86_400_000.0);
        assert_eq!(ms("1M"), 30.0 * 86_400_000.0);
        assert_eq!(ms("1y"), 365.0 * 86_400_000.0);
        assert_eq!(ms("0"), 0.0);
        assert_eq!(ms("1.5s"), 1500.0);
    }

    #[test]
    fn combined_tokens() {
        assert_eq!(ms("1m 30s"), 90_000.0);
        assert_eq!(ms("1h 2m 3s 4ms"), 3_723_004.0);
        assert_eq!(ms("  1m\t30s  "), 90_000.0);
        assert_eq!(ms("1 500ms"), 1500.0);
    }

    #[test]
    fn blank_is_zero() {
        assert_eq!(ms(""), 0.0);
        assert_eq!(ms("   "), 0.0);
    }

    #[test]
    fn rejects_bad_grammar() {
        for bad in [
            "abc", "1x", "01s", "1.s", ".5s", "-1s", "1m 2m", "30s 1m", "1m 5", "5 5", "1S", "1 s",
            "1e3", "+1s",
        ] {
            assert!(parse_interval_ms(bad).is_err(), "{bad} should be rejected");
        }
        assert_eq!(parse_interval_ms("1x").unwrap_err().to_string(), "invalid interval: 1x");
    }

    #[test]
    fn duration_conversion() {
        assert_eq!(parse_interval("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_interval("1.5ms").unwrap(), Duration::from_micros(1500));
        assert!(parse_interval("99999999999999999999y").is_err());
    }

    #[test]
    fn deserialize_number_and_string() {
        let v: Interval = serde_json::from_str("30").unwrap();
        assert_eq!(v.duration(), Duration::from_secs(30));
        let v: Interval = serde_json::from_str("0.5").unwrap();
        assert_eq!(v.duration(), Duration::from_millis(500));
        let v: Interval = serde_json::from_str("-5").unwrap();
        assert_eq!(v.duration(), Duration::ZERO);
        let v: Interval = serde_json::from_str("\"1m 30s\"").unwrap();
        assert_eq!(v.duration(), Duration::from_secs(90));
        assert!(serde_json::from_str::<Interval>("\"1x\"").is_err());
        assert!(serde_json::from_str::<Interval>("true").is_err());
    }
}
