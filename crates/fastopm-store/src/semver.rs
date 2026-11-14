//! The part of node-semver 7 the store needs: `parse(v, loose)`, `compare`, `lte` and
//! `compareLoose`.
//!
//! Written out instead of using `nodejs-semver` because that crate's parser is neither node's
//! strict nor its loose grammar (it accepts `V1.2.3`, `v 1.2.3` in strict mode...), and
//! `semver.lte` (strict) versus `semver.parse(v, true)` (loose) decide what Verdaccio keeps.
//!
//! Grammar (node-semver `internal/re.js`), applied to the trimmed input of at most 256 UTF-16
//! units:
//!
//! - strict: `^v?N.N.N(-PRE)?(+BUILD)?$`, `N` = `0|[1-9]\d*`, prerelease identifiers numeric
//!   without leading zero or alphanumeric with at least one letter or `-`;
//! - loose: `^[v=\s]*N.N.N(-?PRE)?(+BUILD)?$` with `N` = `\d+` and numeric prerelease
//!   identifiers allowed to have leading zeros.
//!
//! Major, minor and patch above `Number.MAX_SAFE_INTEGER` are invalid.

use std::cmp::Ordering;

use crate::js::{is_js_whitespace, js_trim};

const MAX_LENGTH: usize = 256;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// A parsed version (build metadata dropped: it never takes part in comparisons).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Vec<String>,
}

/// `semver.parse(version, loose)`: `None` where node returns `null`.
pub(crate) fn parse(version: &str, loose: bool) -> Option<SemVer> {
    if version.encode_utf16().count() > MAX_LENGTH {
        return None;
    }
    let mut s = js_trim(version);
    if loose {
        s = s.trim_start_matches(|c: char| c == 'v' || c == '=' || is_js_whitespace(c));
    } else if let Some(rest) = s.strip_prefix('v') {
        s = rest;
    }

    let (major, s) = number(s, loose)?;
    let s = s.strip_prefix('.')?;
    let (minor, s) = number(s, loose)?;
    let s = s.strip_prefix('.')?;
    let (patch, rest) = number(s, loose)?;

    let (pre_part, build_part) = match rest.find('+') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    if let Some(build) = build_part {
        if !identifiers(build, |id| !id.is_empty() && id.bytes().all(is_ident_byte)) {
            return None;
        }
    }
    let prerelease = if pre_part.is_empty() {
        Vec::new()
    } else if let Some(p) = pre_part.strip_prefix('-').filter(|p| prerelease_ok(p, loose)) {
        split_ids(p)
    } else if loose && prerelease_ok(pre_part, loose) {
        // `-?` is optional in the loose grammar, and `-` alone is a valid identifier.
        split_ids(pre_part)
    } else {
        return None;
    };

    Some(SemVer { major, minor, patch, prerelease })
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

fn number(s: &str, loose: bool) -> Option<(u64, &str)> {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let digits = &s[..end];
    if !loose && digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    // Numbers beyond 2^53 - 1 are rejected; parse as f64 like `+m[1]`.
    let value: f64 = digits.parse().ok()?;
    if value > MAX_SAFE_INTEGER as f64 {
        return None;
    }
    Some((value as u64, &s[end..]))
}

fn identifiers(s: &str, ok: impl Fn(&str) -> bool) -> bool {
    s.split('.').all(ok)
}

fn prerelease_ok(s: &str, loose: bool) -> bool {
    identifiers(s, |id| {
        if id.is_empty() || !id.bytes().all(is_ident_byte) {
            return false;
        }
        let numeric = id.bytes().all(|b| b.is_ascii_digit());
        !numeric || loose || id == "0" || !id.starts_with('0')
    })
}

fn split_ids(s: &str) -> Vec<String> {
    s.split('.').map(str::to_owned).collect()
}

/// `compareIdentifiers`: numeric identifiers compare as numbers and sort before alphanumeric
/// ones; others compare as strings.
fn compare_identifiers(a: &str, b: &str) -> Ordering {
    let anum = !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit());
    let bnum = !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit());
    match (anum, bnum) {
        (true, true) => {
            let (x, y) = (a.parse::<f64>().unwrap_or(0.0), b.parse::<f64>().unwrap_or(0.0));
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.cmp(b),
    }
}

impl SemVer {
    /// `SemVer.compare`: main version, then prerelease (a version without prerelease is
    /// greater).
    pub(crate) fn compare(&self, other: &SemVer) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| self.compare_pre(other))
    }

    fn compare_pre(&self, other: &SemVer) -> Ordering {
        match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
            (true, true) => return Ordering::Equal,
            (false, true) => return Ordering::Less,
            (true, false) => return Ordering::Greater,
            (false, false) => {}
        }
        for i in 0.. {
            match (self.prerelease.get(i), other.prerelease.get(i)) {
                (None, None) => return Ordering::Equal,
                (Some(_), None) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (Some(a), Some(b)) if a == b => continue,
                (Some(a), Some(b)) => return compare_identifiers(a, b),
            }
        }
        unreachable!("the loop returns once both lists are exhausted")
    }
}

/// `semver.lte(a, b)` (strict parsing). `None` where node throws `TypeError: Invalid Version`.
pub(crate) fn lte(a: &str, b: &str) -> Option<bool> {
    let (a, b) = (parse(a, false)?, parse(b, false)?);
    Some(a.compare(&b) != Ordering::Greater)
}

/// `sortVersionsAndFilterInvalid`: keep the strings `semver.parse(v, true)` accepts, sorted
/// ascending with `compareLoose` (stable, like V8's sort).
pub(crate) fn sort_versions_and_filter_invalid<'a>(
    versions: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut parsed: Vec<(&str, SemVer)> =
        versions.into_iter().filter_map(|v| Some((v, parse(v, true)?))).collect();
    parsed.sort_by(|a, b| a.1.compare(&b.1));
    parsed.into_iter().map(|(v, _)| v).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_and_loose_grammar() {
        for v in ["1.2.3", "v1.2.3", " 1.2.3 ", "1.2.3-beta.1", "1.2.3+build.5", "0.0.0-0"] {
            assert!(parse(v, false).is_some(), "strict {v}");
            assert!(parse(v, true).is_some(), "loose {v}");
        }
        for v in ["=1.2.3", "v 1.2.3", "01.2.3", "1.2.3beta", "1.2.3-01", "1.2.3-"] {
            assert!(parse(v, false).is_none(), "strict rejects {v}");
            assert!(parse(v, true).is_some(), "loose accepts {v}");
        }
        for v in ["", "1.2", "1.2.3.4", "V1.2.3", "latest", "1.2.3+", "1.2.3-a..b", "x1.2.3"] {
            assert!(parse(v, false).is_none(), "strict rejects {v}");
            assert!(parse(v, true).is_none(), "loose rejects {v}");
        }
        assert!(parse("9007199254740992.0.0", true).is_none());
        assert!(parse("9007199254740991.0.0", true).is_some());
        assert!(parse(&format!("1.0.0-{}", "a".repeat(251)), true).is_none());
    }

    #[test]
    fn ordering() {
        let ordered = [
            "0.0.1",
            "1.0.0-0",
            "1.0.0-1",
            "1.0.0-2",
            "1.0.0-10",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for w in ordered.windows(2) {
            let (a, b) = (parse(w[0], true).unwrap(), parse(w[1], true).unwrap());
            assert_eq!(a.compare(&b), Ordering::Less, "{} < {}", w[0], w[1]);
        }
        assert_eq!(lte("1.0.0", "1.0.0+build"), Some(true));
        assert_eq!(lte("2.0.0", "1.0.0"), Some(false));
        assert_eq!(lte("1.0.0", "next"), None);
        assert_eq!(lte("=1.0.0", "2.0.0"), None);
    }

    #[test]
    fn sort_filters_and_keeps_stable_order() {
        let sorted =
            sort_versions_and_filter_invalid(["2.0.0", "bad", "1.0.0", "v1.0.0", "1.0.0-rc.1"]);
        assert_eq!(sorted, ["1.0.0-rc.1", "1.0.0", "v1.0.0", "2.0.0"]);
    }
}
