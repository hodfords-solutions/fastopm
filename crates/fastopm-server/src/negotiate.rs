//! Header negotiation as Verdaccio's dependencies do it: `getByQualityPriorityValue`
//! (`Accept`), `negotiator@0.6` as used by `compression@1.8` (`Accept-Encoding`) and
//! `fresh@2` (`If-None-Match`).

use crate::js;

/// `getByQualityPriorityValue` (`packages/core/core/src/string-utils.ts`), quirks included:
/// a single value is cut at its first `;` without trimming; in a list, entries without
/// parameters are not trimmed and rank as `q=0`, the "q" is whatever follows the first `=` of
/// the first parameter (whatever its name), and the sort is V8's with `b.q - a.q`.
pub(crate) fn quality_priority_value(header: Option<&str>) -> String {
    let Some(header) = header else {
        return String::new();
    };
    let split: Vec<&str> = header.split(',').collect();
    if split.len() <= 1 {
        return split[0].split(';').next().unwrap_or_default().to_owned();
    }
    enum Q<'a> {
        Zero,
        Str(&'a str),
    }
    let number = |q: &Q<'_>| match q {
        Q::Zero => 0.0,
        Q::Str(s) => js::string_to_number(s),
    };
    let mut entries: Vec<(&str, Q<'_>)> = split
        .iter()
        .map(|item| {
            let q_list: Vec<&str> = item.split(';').collect();
            if q_list.len() > 1 {
                let query = q_list[1].split('=').nth(1).filter(|q| !q.is_empty());
                (js::trim(q_list[0]), query.map_or(Q::Zero, Q::Str))
            } else {
                (q_list[0], Q::Zero)
            }
        })
        .collect();
    js::v8_sort(&mut entries, |a, b| number(&b.1) - number(&a.1));
    entries[0].0.to_owned()
}

/// Response content coding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Coding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
}

impl Coding {
    pub(crate) fn header_value(self) -> &'static str {
        match self {
            Coding::Identity => "identity",
            Coding::Gzip => "gzip",
            Coding::Deflate => "deflate",
            Coding::Brotli => "br",
        }
    }
}

/// What `compression@1.8` sends for this `Accept-Encoding` (Node.js has brotli):
/// `negotiator.encoding(['br', 'gzip', 'deflate', 'identity'], ['br', 'gzip'])`, and
/// `identity` when the header is absent or empty.
pub(crate) fn negotiate_coding(accept_encoding: Option<&str>) -> Coding {
    const PROVIDED: [(&str, Coding); 4] = [
        ("br", Coding::Brotli),
        ("gzip", Coding::Gzip),
        ("deflate", Coding::Deflate),
        ("identity", Coding::Identity),
    ];
    const PREFERRED: [&str; 2] = ["br", "gzip"];
    let Some(header) = accept_encoding.filter(|h| !h.is_empty()) else {
        return Coding::Identity;
    };

    struct Accepted<'a> {
        encoding: &'a str,
        q: f64,
        i: usize,
    }
    // parseAcceptEncoding
    let parts: Vec<&str> = header.split(',').collect();
    let mut accepts = Vec::new();
    let mut has_identity = false;
    let mut min_quality: f64 = 1.0;
    for (i, part) in parts.iter().enumerate() {
        let Some(spec) = parse_encoding(js::trim(part)) else { continue };
        let spec = Accepted { encoding: spec.0, q: spec.1, i };
        has_identity |= specify("identity", spec.encoding) || spec.encoding == "*";
        let q = if spec.q == 0.0 || spec.q.is_nan() { 1.0 } else { spec.q };
        min_quality = js_min(min_quality, q);
        accepts.push(spec);
    }
    if !has_identity {
        accepts.push(Accepted { encoding: "identity", q: min_quality, i: parts.len() });
    }

    // getEncodingPriority for each provided encoding
    #[derive(Clone, Copy)]
    struct Priority {
        index: usize,
        o: i64,
        q: f64,
        s: i64,
    }
    let mut priorities: Vec<Priority> = Vec::new();
    for (index, (name, _)) in PROVIDED.iter().enumerate() {
        let mut priority = Priority { index, o: -1, q: 0.0, s: 0 };
        for spec in &accepts {
            if !specify(name, spec.encoding) && spec.encoding != "*" {
                continue;
            }
            let candidate = Priority {
                index,
                o: spec.i as i64,
                q: spec.q,
                s: i64::from(spec.encoding.eq_ignore_ascii_case(name)),
            };
            // `a || b || c` on numbers: 0 and NaN fall through.
            let q_diff = priority.q - candidate.q;
            let diff = if priority.s != candidate.s {
                (priority.s - candidate.s) as f64
            } else if q_diff != 0.0 && !q_diff.is_nan() {
                q_diff
            } else {
                (priority.o - candidate.o) as f64
            };
            if diff < 0.0 {
                priority = candidate;
            }
        }
        priorities.push(priority);
    }
    priorities.retain(|p| p.q > 0.0);
    let preferred_index =
        |p: &Priority| PREFERRED.iter().position(|name| *name == PROVIDED[p.index].0);
    priorities.sort_by(|a, b| {
        use std::cmp::Ordering;
        if a.q != b.q {
            return b.q.partial_cmp(&a.q).unwrap_or(Ordering::Equal);
        }
        match (preferred_index(a), preferred_index(b)) {
            (None, None) => b.s.cmp(&a.s).then(a.o.cmp(&b.o)).then(a.index.cmp(&b.index)),
            (Some(x), Some(y)) => x.cmp(&y),
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
        }
    });
    priorities.first().map_or(Coding::Identity, |p| PROVIDED[p.index].1)
}

/// `Math.min` with NaN propagation.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// Whether `spec` names `encoding` (case-insensitive).
fn specify(encoding: &str, spec: &str) -> bool {
    spec.to_lowercase() == encoding.to_lowercase()
}

/// `parseEncoding`: `/^\s*([^\s;]+)\s*(?:;(.*))?$/`, `q` from the first `q=` parameter
/// (`parseFloat`), default 1.
fn parse_encoding(s: &str) -> Option<(&str, f64)> {
    let s = s.trim_start_matches(js::is_js_space);
    let name_end = s.find(|c: char| js::is_js_space(c) || c == ';').unwrap_or(s.len());
    if name_end == 0 {
        return None;
    }
    let name = &s[..name_end];
    let rest = s[name_end..].trim_start_matches(js::is_js_space);
    let params = if rest.is_empty() { None } else { Some(rest.strip_prefix(';')?) };
    let mut q = 1.0;
    if let Some(params) = params.filter(|p| !p.is_empty()) {
        for param in params.split(';') {
            let mut kv = js::trim(param).split('=');
            if kv.next() == Some("q") {
                q = kv.next().map_or(f64::NAN, js::parse_float);
                break;
            }
        }
    }
    Some((name, q))
}

/// Express `req.fresh` for a `GET`/`HEAD` with a 2xx status (`fresh@2`): no
/// `If-None-Match` and no `If-Modified-Since` → stale; `Cache-Control: no-cache` → stale;
/// `If-None-Match: *` → fresh; otherwise fresh when one listed tag equals `etag` (with or
/// without `W/` on either side). `If-Modified-Since` alone is always stale: responses carry no
/// `Last-Modified`.
pub(crate) fn is_fresh(
    if_none_match: Option<&str>,
    if_modified_since: Option<&str>,
    cache_control: Option<&str>,
    etag: &str,
) -> bool {
    let none_match = if_none_match.filter(|v| !v.is_empty());
    let modified_since = if_modified_since.filter(|v| !v.is_empty());
    if none_match.is_none() && modified_since.is_none() {
        return false;
    }
    if cache_control.is_some_and(has_no_cache) {
        return false;
    }
    match none_match {
        Some("*") => true,
        Some(list) => parse_token_list(list).into_iter().any(|tag| {
            tag == etag
                || tag.strip_prefix("W/") == Some(etag)
                || etag.strip_prefix("W/") == Some(tag)
        }),
        None => false,
    }
}

/// `/(?:^|,)\s*?no-cache\s*?(?:,|$)/`
fn has_no_cache(value: &str) -> bool {
    value.split(',').any(|part| js::trim(part) == "no-cache")
}

/// `parseTokenList` of `fresh`: split on `,`, skipping spaces before each token.
fn parse_token_list(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let (mut start, mut end) = (0, 0);
    let mut list = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b' ' => {
                if start == end {
                    start = i + 1;
                    end = i + 1;
                }
            }
            b',' => {
                list.push(&s[start..end]);
                start = i + 1;
                end = i + 1;
            }
            _ => end = i + 1,
        }
    }
    list.push(&s[start..end]);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    const NPM: &str = "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*";

    #[test]
    fn priority_value() {
        let q = |h| quality_priority_value(Some(h));
        assert_eq!(quality_priority_value(None), "");
        assert_eq!(q(NPM), "application/vnd.npm.install-v1+json");
        assert_eq!(q("application/vnd.npm.install-v1+json"), "application/vnd.npm.install-v1+json");
        assert_eq!(
            q("application/vnd.npm.install-v1+json; q=0.1"),
            "application/vnd.npm.install-v1+json"
        );
        // single value is not trimmed
        assert_eq!(q(" application/json"), " application/json");
        // entries without parameters count as q=0 and are not trimmed
        assert_eq!(q("application/json, application/vnd.npm.install-v1+json"), "application/json");
        assert_eq!(
            q("application/json, application/vnd.npm.install-v1+json;q=0.1"),
            "application/vnd.npm.install-v1+json"
        );
        assert_eq!(
            q("application/json; q=1.0, application/vnd.npm.install-v1+json; q=0.5"),
            "application/json"
        );
        // the first parameter's value is used whatever its name
        assert_eq!(q("a/b, c/d; charset=2"), "c/d");
        // equal q keeps the order
        assert_eq!(q("a/b; q=1, c/d; q=1"), "a/b");
        // a second "=" is ignored
        assert_eq!(q("a/b; q=0.1, c/d; q=0.5=9"), "c/d");
        assert_eq!(q("a/b; q=, c/d"), "a/b");
    }

    #[test]
    fn coding() {
        let n = |h| negotiate_coding(Some(h));
        assert_eq!(negotiate_coding(None), Coding::Identity);
        assert_eq!(n(""), Coding::Identity);
        assert_eq!(n("gzip"), Coding::Gzip);
        assert_eq!(n("gzip, deflate, br"), Coding::Brotli);
        assert_eq!(n("deflate, gzip"), Coding::Gzip);
        assert_eq!(n("deflate"), Coding::Deflate);
        assert_eq!(n("gzip;q=0.5, deflate"), Coding::Deflate);
        assert_eq!(n("gzip;q=0, deflate;q=0"), Coding::Identity);
        assert_eq!(n("identity"), Coding::Identity);
        assert_eq!(n("*"), Coding::Brotli);
        assert_eq!(n("GZIP"), Coding::Gzip);
        assert_eq!(n("compress"), Coding::Identity);
        assert_eq!(n("gzip, *;q=0"), Coding::Gzip);
        assert_eq!(n("br;q=0.9, gzip"), Coding::Gzip);
        assert_eq!(n("gzip;q=abc"), Coding::Identity);
    }

    #[test]
    fn freshness() {
        let e = "\"abc\"";
        assert!(!is_fresh(None, None, None, e));
        assert!(is_fresh(Some("\"abc\""), None, None, e));
        assert!(is_fresh(Some("W/\"abc\""), None, None, e));
        assert!(is_fresh(Some("\"x\", \"abc\""), None, None, e));
        assert!(is_fresh(Some("*"), None, None, e));
        assert!(!is_fresh(Some("\"x\""), None, None, e));
        assert!(!is_fresh(Some("\"abc\""), None, Some("max-age=0, no-cache"), e));
        assert!(is_fresh(Some("\"abc\""), None, Some("no-cache-x"), e));
        assert!(!is_fresh(None, Some("Sat, 01 Jan 2000 00:00:00 GMT"), None, e));
        assert!(is_fresh(Some("\"abc\""), Some("Sat, 01 Jan 2000 00:00:00 GMT"), None, e));
        assert!(is_fresh(Some("\"abc\""), None, None, "W/\"abc\""));
    }

    #[test]
    fn token_list() {
        assert_eq!(parse_token_list(" a , b,c"), vec!["a", "b", "c"]);
        assert_eq!(parse_token_list("a b"), vec!["a b"]);
        assert_eq!(parse_token_list(""), vec![""]);
    }
}
