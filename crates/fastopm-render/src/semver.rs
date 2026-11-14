//! `semver.parse(x, true)` and `SemVer#compare` of node-semver 7 (the `LOOSE` regex), the subset
//! `getVersion` needs.
//!
//! Written by hand rather than with `nodejs-semver`: that crate's loose parser accepts trailing
//! text and its `MAX_SAFE_INTEGER` constant is wrong, both of which change which version a query
//! resolves to.

use std::cmp::Ordering;

const MAX_LENGTH: usize = 256;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A parsed version; build metadata is dropped (it never takes part in `compare`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SemVer {
    major: f64,
    minor: f64,
    patch: f64,
    prerelease: Vec<Identifier>,
}

#[derive(Debug, Clone, PartialEq)]
enum Identifier {
    /// All-digit identifier below `MAX_SAFE_INTEGER`, stored as the JavaScript number.
    Number(f64),
    Text(String),
}

/// JavaScript `WhiteSpace` + `LineTerminator` (what `String#trim` and `\s` match).
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// `IDENT(\.IDENT)*` where every identifier is `[0-9A-Za-z-]+` (in node-semver's loose grammar
/// `NONNUMERICIDENTIFIER | NUMERICIDENTIFIERLOOSE` matches exactly those strings).
fn is_dotted_identifiers(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(|part| !part.is_empty() && part.bytes().all(is_ident_char))
}

/// Matches `(?:-?(PRE))?(?:\+(BUILD))?$` against `tail` with the regex engine's priorities and
/// returns the prerelease capture: `Some(None)` = matched without prerelease.
fn match_tail(tail: &str) -> Option<Option<&str>> {
    let pre_end = tail.find('+').unwrap_or(tail.len());
    let build_ok = |at: usize| {
        at == tail.len() || (tail.as_bytes()[at] == b'+' && is_dotted_identifiers(&tail[at + 1..]))
    };
    // With a prerelease: the capture is greedy up to `+` or the end (no shorter capture can be
    // followed by a build or the end). `-?` is tried consuming the hyphen first.
    if let Some(rest) = tail.strip_prefix('-')
        && is_dotted_identifiers(&rest[..pre_end - 1])
        && build_ok(pre_end)
    {
        return Some(Some(&tail[1..pre_end]));
    }
    if is_dotted_identifiers(&tail[..pre_end]) && build_ok(pre_end) {
        return Some(Some(&tail[..pre_end]));
    }
    // Without a prerelease.
    (pre_end == 0 && build_ok(0)).then_some(None)
}

fn leading_digits(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_digit).count()
}

/// JavaScript `+digits`.
fn digits_value(digits: &str) -> f64 {
    digits.parse().unwrap_or(f64::INFINITY)
}

impl SemVer {
    /// `semver.parse(version, true)`; `None` where node-semver returns `null`.
    pub(crate) fn parse_loose(version: &str) -> Option<Self> {
        if version.encode_utf16().count() > MAX_LENGTH {
            return None;
        }
        let trimmed = version.trim_matches(is_js_whitespace);
        let s = trimmed.trim_start_matches(|c| c == 'v' || c == '=' || is_js_whitespace(c));

        let major_len = leading_digits(s);
        if major_len == 0 || s.as_bytes().get(major_len) != Some(&b'.') {
            return None;
        }
        let rest = &s[major_len + 1..];
        let minor_len = leading_digits(rest);
        if minor_len == 0 || rest.as_bytes().get(minor_len) != Some(&b'.') {
            return None;
        }
        let rest2 = &rest[minor_len + 1..];
        let patch_max = leading_digits(rest2);
        // The patch `\d+` is greedy but backtracks: `1.2.34.5` matches as `1.2.3-4.5`.
        let (patch_len, prerelease) = (1..=patch_max)
            .rev()
            .find_map(|len| match_tail(&rest2[len..]).map(|pre| (len, pre)))?;

        let major = digits_value(&s[..major_len]);
        let minor = digits_value(&rest[..minor_len]);
        let patch = digits_value(&rest2[..patch_len]);
        if major > MAX_SAFE_INTEGER || minor > MAX_SAFE_INTEGER || patch > MAX_SAFE_INTEGER {
            return None;
        }
        let prerelease = prerelease
            .map(|pre| {
                pre.split('.')
                    .map(|id| {
                        if id.bytes().all(|b| b.is_ascii_digit()) {
                            let n = digits_value(id);
                            if n < MAX_SAFE_INTEGER {
                                return Identifier::Number(n);
                            }
                        }
                        Identifier::Text(id.to_owned())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self { major, minor, patch, prerelease })
    }

    /// `SemVer#compare`: main version, then prerelease (build metadata ignored).
    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        let main = [self.major, self.minor, self.patch]
            .iter()
            .zip([other.major, other.minor, other.patch].iter())
            .map(|(a, b)| a.total_cmp(b))
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal);
        main.then_with(|| self.compare_pre(other))
    }

    fn compare_pre(&self, other: &Self) -> Ordering {
        match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
            (false, true) => return Ordering::Less,
            (true, false) => return Ordering::Greater,
            (true, true) => return Ordering::Equal,
            (false, false) => {}
        }
        let mut i = 0;
        loop {
            match (self.prerelease.get(i), other.prerelease.get(i)) {
                (None, None) => return Ordering::Equal,
                (Some(_), None) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (Some(a), Some(b)) if a == b => {}
                // `compareIdentifiers` decides, even when it says "equal" for two different
                // identifiers (two huge numeric strings that round to the same double).
                (Some(a), Some(b)) => return compare_identifiers(a, b),
            }
            i += 1;
        }
    }
}

fn numeric_value(id: &Identifier) -> Option<f64> {
    match id {
        Identifier::Number(n) => Some(*n),
        Identifier::Text(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            Some(digits_value(s))
        }
        Identifier::Text(_) => None,
    }
}

/// node-semver `compareIdentifiers`.
fn compare_identifiers(a: &Identifier, b: &Identifier) -> Ordering {
    match (numeric_value(a), numeric_value(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => {
            let (Identifier::Text(x), Identifier::Text(y)) = (a, b) else {
                unreachable!("non-numeric identifiers are text")
            };
            // Identifiers are ASCII, so byte order is UTF-16 code unit order.
            x.as_bytes().cmp(y.as_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<String> {
        SemVer::parse_loose(s).map(|v| {
            let mut out = format!("{}.{}.{}", v.major, v.minor, v.patch);
            if !v.prerelease.is_empty() {
                let ids: Vec<String> = v
                    .prerelease
                    .iter()
                    .map(|id| match id {
                        Identifier::Number(n) => format!("{n}"),
                        Identifier::Text(t) => t.clone(),
                    })
                    .collect();
                out.push('-');
                out.push_str(&ids.join("."));
            }
            out
        })
    }

    #[test]
    fn loose_forms() {
        assert_eq!(p("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(p(" v=1.2.3 ").as_deref(), Some("1.2.3"));
        assert_eq!(p("01.02.03").as_deref(), Some("1.2.3"));
        assert_eq!(p("1.2.3beta").as_deref(), Some("1.2.3-beta"));
        assert_eq!(p("1.2.3-beta.01+build.1").as_deref(), Some("1.2.3-beta.1"));
        assert_eq!(p("1.2.34.5").as_deref(), Some("1.2.3-4.5"));
        assert_eq!(p("1.2.3-").as_deref(), Some("1.2.3--"));
        assert_eq!(p("1.2.3+"), None);
        assert_eq!(p("1.2"), None);
        assert_eq!(p("latest"), None);
        assert_eq!(p("1.2.3 x"), None);
        assert_eq!(p("9007199254740992.0.0"), None);
        assert_eq!(p(&format!("1.2.3-{}", "a".repeat(260))), None);
    }

    #[test]
    fn compare_ignores_build_and_orders_prerelease() {
        let c = |a: &str, b: &str| {
            SemVer::parse_loose(a).unwrap().compare(&SemVer::parse_loose(b).unwrap())
        };
        assert_eq!(c("1.0.0+a", "v1.0.0+b"), Ordering::Equal);
        assert_eq!(c("1.0.0-alpha", "1.0.0"), Ordering::Less);
        assert_eq!(c("1.0.0-2", "1.0.0-10"), Ordering::Less);
        assert_eq!(c("1.0.0-1", "1.0.0-a"), Ordering::Less);
        assert_eq!(c("1.0.0-01", "1.0.0-1"), Ordering::Equal);
        assert_eq!(c("1.0.0-a.b", "1.0.0-a"), Ordering::Greater);
        assert_eq!(
            c("1.0.0-99999999999999999999.a", "1.0.0-99999999999999999998.b"),
            Ordering::Equal
        );
    }
    /// `semver.parse(x, true)` / `compare` vectors generated by `tests/fixtures/gen.cjs`.
    #[test]
    fn matches_node_semver() {
        type Vectors = (Vec<(String, Option<String>)>, Vec<(String, String, i8)>);
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/expected/semver.json")).unwrap();
        let (parsed, compares): Vectors = (
            serde_json::from_value(v["parsed"].clone()).unwrap(),
            serde_json::from_value(v["compares"].clone()).unwrap(),
        );
        for (input, want) in &parsed {
            assert_eq!(p(input).as_deref(), want.as_deref(), "{input:?}");
        }
        for (a, b, want) in &compares {
            let got = SemVer::parse_loose(a).unwrap().compare(&SemVer::parse_loose(b).unwrap());
            assert_eq!(got as i8, *want, "{a:?} vs {b:?}");
        }
    }
}
