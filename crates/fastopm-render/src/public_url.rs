//! `@verdaccio/url`: `getPublicUrl`, `wrapPrefix`, `getWebProtocol`, `isHost`.

use std::net::Ipv6Addr;

use crate::semver::is_js_whitespace;

/// `wrapPrefix`: `p`, `/p`, `p/` → `/p/`; empty → empty.
pub(crate) fn wrap_prefix(prefix: Option<&str>) -> String {
    match prefix {
        None | Some("") => String::new(),
        Some(p) => match (p.starts_with('/'), p.ends_with('/')) {
            (false, true) => format!("/{p}"),
            (false, false) => format!("/{p}/"),
            (true, false) => format!("{p}/"),
            (true, true) => p.to_owned(),
        },
    }
}

/// `getWebProtocol`: the forwarded value (cut at a `,` that is not the first character) when
/// non-empty, else the connection scheme; anything but `http`/`https` becomes `http`.
pub(crate) fn web_protocol<'a>(header: Option<&'a str>, scheme: &'a str) -> &'a str {
    let proto = match header {
        Some(h) if !h.is_empty() => match h.find(',') {
            Some(i) if i > 0 => &h[..i],
            _ => h,
        },
        _ => scheme,
    };
    if proto == "http" || proto == "https" { proto } else { "http" }
}

/// WHATWG `new URL(input, base).href`.
pub(crate) fn resolve(input: &str, base: &str) -> Option<String> {
    url::Url::parse(base).ok()?.join(input).ok().map(String::from)
}

/// `isHost`: validator 13 `isURL(host, { require_host: true, allow_trailing_dot: false,
/// require_valid_protocol: false, require_port: false, require_tld: false })`.
pub(crate) fn is_host(input: &str) -> bool {
    if input.is_empty() || input.chars().any(|c| is_js_whitespace(c) || c == '<' || c == '>') {
        return false;
    }
    if input.starts_with("mailto:") || input.encode_utf16().count() > 2084 {
        return false;
    }
    let mut url = input.split('#').next().unwrap_or_default();
    url = url.split('?').next().unwrap_or_default();

    // `^([a-z][a-z0-9+\-.]*):` (case-insensitive)
    let mut had_explicit_protocol = false;
    let scheme_len = url
        .bytes()
        .enumerate()
        .take_while(|(i, b)| {
            if *i == 0 {
                b.is_ascii_alphabetic()
            } else {
                b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
            }
        })
        .count();
    if scheme_len > 0 && url.as_bytes().get(scheme_len) == Some(&b':') {
        let after_colon = &url[scheme_len + 1..];
        let consume = if after_colon.starts_with("//") {
            true
        } else {
            let before_slash = after_colon.split('/').next().unwrap_or_default();
            if let Some(at) = before_slash.find('@') {
                let before_at = &before_slash[..at];
                let valid_auth = before_at.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'%' | b':')
                });
                let encoded = before_at
                    .as_bytes()
                    .windows(3)
                    .any(|w| w[0] == b'%' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit());
                !(valid_auth && !encoded)
            } else {
                // Looks like `host:port`.
                !after_colon.starts_with(|c: char| c.is_ascii_digit())
            }
        };
        if consume {
            had_explicit_protocol = true;
            url = after_colon;
        }
    }

    if let Some(rest) = url.strip_prefix("//") {
        if !had_explicit_protocol {
            return false;
        }
        url = rest;
    }
    if url.is_empty() {
        return false;
    }
    url = url.split('/').next().unwrap_or_default();

    let parts: Vec<&str> = url.split('@').collect();
    let hostname = if parts.len() > 1 {
        if parts[0].is_empty() {
            return false;
        }
        let auth = parts[0];
        if auth.split(':').count() > 2 {
            return false;
        }
        if auth == ":" {
            return false;
        }
        parts[1..].join("@")
    } else {
        url.to_owned()
    };

    let (host, ipv6, port) = match wrapped_ipv6(&hostname) {
        Some((ip, port)) => ("", Some(ip), port),
        None => match hostname.split_once(':') {
            Some((h, port)) => (h, None, Some(port)),
            None => (hostname.as_str(), None, None),
        },
    };
    if let Some(port) = port
        && !port.is_empty()
    {
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let significant = port.trim_start_matches('0');
        if significant.is_empty() || significant.len() > 5 {
            return false;
        }
        let n: u32 = significant.parse().unwrap_or(u32::MAX);
        if n > 65535 {
            return false;
        }
    }
    is_ipv4(host) || is_fqdn(host) || ipv6.is_some_and(is_ipv6)
}

/// `^\[([^\]]+)\](?::([0-9]+))?$`
fn wrapped_ipv6(s: &str) -> Option<(&str, Option<&str>)> {
    let inner = s.strip_prefix('[')?;
    let close = inner.find(']')?;
    let ip = &inner[..close];
    if ip.is_empty() {
        return None;
    }
    let rest = &inner[close + 1..];
    if rest.is_empty() {
        return Some((ip, None));
    }
    let port = rest.strip_prefix(':')?;
    (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then_some((ip, Some(port)))
}

/// Dotted quad without leading zeros.
fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// IPv6 with an optional `%zone` (`[0-9a-zA-Z.]+`).
fn is_ipv6(s: &str) -> bool {
    let addr = match s.split_once('%') {
        Some((addr, zone)) => {
            if zone.is_empty() || !zone.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.') {
                return false;
            }
            addr
        }
        None => s,
    };
    addr.parse::<Ipv6Addr>().is_ok()
}

/// validator `isFQDN` with `require_tld: false`, no underscores, no trailing dot, no numeric
/// TLD.
fn is_fqdn(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    let tld = parts[parts.len() - 1];
    if !tld.is_empty() && tld.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    parts.iter().all(|part| {
        part.encode_utf16().count() <= 63
            && !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || u32::from(c) >= 0xa1)
            && !part.chars().any(|c| ('\u{ff01}'..='\u{ff5e}').contains(&c))
            && !part.starts_with('-')
            && !part.ends_with('-')
            && !part.contains('_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_prefix_forms() {
        assert_eq!(wrap_prefix(None), "");
        assert_eq!(wrap_prefix(Some("")), "");
        assert_eq!(wrap_prefix(Some("p")), "/p/");
        assert_eq!(wrap_prefix(Some("/p")), "/p/");
        assert_eq!(wrap_prefix(Some("p/")), "/p/");
        assert_eq!(wrap_prefix(Some("/a/b/")), "/a/b/");
    }

    #[test]
    fn web_protocol_rules() {
        assert_eq!(web_protocol(Some("https"), "http"), "https");
        assert_eq!(web_protocol(Some("https,http"), "http"), "https");
        assert_eq!(web_protocol(Some(",https"), "https"), "http");
        assert_eq!(web_protocol(Some(""), "https"), "https");
        assert_eq!(web_protocol(None, "https"), "https");
        assert_eq!(web_protocol(Some("ftp"), "https"), "http");
        assert_eq!(web_protocol(Some("HTTPS"), "https"), "http");
    }

    #[test]
    fn host_validation() {
        for ok in [
            "localhost",
            "localhost:4873",
            "127.0.0.1:4873",
            "registry.example.com",
            "[::1]:4873",
            "[fe80::1%eth0]",
            "user:pw@host",
            "host/path",
            "a-b.c",
            "exämple.com",
            "example.com:abc",
        ] {
            assert!(is_host(ok), "{ok} should be a host");
        }
        for bad in [
            "",
            "localhost:",
            "a b",
            "foo_bar",
            "example.com.",
            "a..b",
            "-a.com",
            "01.2.3.4",
            "1.2.3.256",
            "host:0",
            "host:65536",
            "host:12a",
            "//host",
            "@host",
            ":@host",
            "a:b:c@host",
            "[::1]x",
            "<host>",
            "mailto:x",
            "\u{a0}host",
        ] {
            assert!(!is_host(bad), "{bad} should not be a host");
        }
    }
    /// validator `isURL` vectors generated by `tests/fixtures/gen.cjs`.
    #[test]
    fn hosts_match_validator() {
        let cases: Vec<(String, bool)> =
            serde_json::from_str(include_str!("../tests/fixtures/expected/hosts.json")).unwrap();
        for (host, want) in cases {
            assert_eq!(is_host(&host), want, "{host:?}");
        }
    }
}
