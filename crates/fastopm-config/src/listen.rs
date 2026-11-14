//! Listen address parsing (`parseAddress` in `packages/config/src/address.ts`).

use std::fmt;
use std::path::PathBuf;

/// Protocol of a TCP listen address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// `http`
    Http,
    /// `https` (needs TLS settings fastopm does not read; the binary refuses to start with it).
    Https,
}

/// A parsed listen address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenAddress {
    /// TCP socket.
    Tcp {
        /// Protocol, default `http`.
        scheme: Scheme,
        /// Host name or IP; an IPv6 address is stored without brackets. Default `localhost`.
        host: String,
        /// Port, default `4873`.
        port: u16,
    },
    /// Unix domain socket.
    Unix {
        /// Socket path (absolute, starts with `/`).
        path: PathBuf,
    },
}

impl fmt::Display for ListenAddress {
    /// `http://host:port` (IPv6 hosts in brackets) or `unix:/path`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListenAddress::Tcp { scheme, host, port } => {
                let scheme = match scheme {
                    Scheme::Http => "http",
                    Scheme::Https => "https",
                };
                if host.contains(':') {
                    write!(f, "{scheme}://[{host}]:{port}")
                } else {
                    write!(f, "{scheme}://{host}:{port}")
                }
            }
            ListenAddress::Unix { path } => write!(f, "unix:{}", path.display()),
        }
    }
}

/// Parse one listen address. Accepted forms (same as Verdaccio):
///
/// - `4873` (port), `localhost:4873`, `0.0.0.0:4873`, `[::1]:4873`
/// - `http::4873` (protocol + port), `https:localhost:4873`
/// - `http://localhost:4873/`, `http://[::1]:4873`
/// - `unix:/tmp/fastopm.sock`, `http://unix:/tmp/fastopm.sock`
///
/// Regex for TCP: `^((https?):(//)?)?((([^/:]*)|\[([^\[\]]+)\]):)?(\d+)/?$`; an empty host means
/// `localhost`. Returns `None` when neither the TCP nor the unix form matches, or the port does
/// not fit in `u16`.
pub fn parse_address(input: &str) -> Option<ListenAddress> {
    parse_tcp(input).or_else(|| parse_unix(input))
}

/// JavaScript line terminators, which `.` does not match.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The TCP regex, trying its alternatives in the order a backtracking engine does:
/// protocol present (`https` before `http`) before absent, `//` present before absent, host
/// present (plain before bracketed) before absent.
fn parse_tcp(input: &str) -> Option<ListenAddress> {
    let protos: [(Option<Scheme>, &str); 3] =
        [(Some(Scheme::Https), "https:"), (Some(Scheme::Http), "http:"), (None, "")];
    for (scheme, prefix) in protos {
        let Some(after_proto) = input.strip_prefix(prefix) else {
            continue;
        };
        let slash_options: &[&str] = if scheme.is_some() { &["//", ""] } else { &[""] };
        for slashes in slash_options {
            let Some(rest) = after_proto.strip_prefix(slashes) else {
                continue;
            };
            for (host, after_host) in host_options(rest) {
                if let Some(port) = port_part(after_host) {
                    let host = if host.is_empty() { "localhost" } else { host };
                    return Some(ListenAddress::Tcp {
                        scheme: scheme.unwrap_or(Scheme::Http),
                        host: host.to_owned(),
                        port,
                    });
                }
            }
        }
    }
    None
}

/// `((([^/:]*)|\[([^\[\]]+)\]):)?` matched at the start of `rest`: (host, remainder) pairs in
/// regex priority order; the last one is the absent group.
fn host_options(rest: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let plain_end = rest.find(['/', ':']).unwrap_or(rest.len());
    if rest[plain_end..].starts_with(':') {
        out.push((&rest[..plain_end], &rest[plain_end + 1..]));
    }
    if let Some(inner) = rest.strip_prefix('[')
        && let Some(close) = inner.find(['[', ']'])
        && close > 0
        && inner[close..].starts_with("]:")
    {
        out.push((&inner[..close], &inner[close + 2..]));
    }
    out.push(("", rest));
    out
}

/// `(\d+)\/?$`: the port, when it fits in a `u16`.
fn port_part(s: &str) -> Option<u16> {
    let digits = s.strip_suffix('/').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    if trimmed.len() > 5 {
        return None;
    }
    if trimmed.is_empty() { Some(0) } else { trimmed.parse().ok() }
}

/// `^(?:(https?):\/\/)?unix:(\/.*)$`
fn parse_unix(input: &str) -> Option<ListenAddress> {
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
        .and_then(|r| r.strip_prefix("unix:"))
        .or_else(|| input.strip_prefix("unix:"))?;
    if !rest.starts_with('/') || rest.chars().any(is_line_terminator) {
        return None;
    }
    Some(ListenAddress::Unix { path: PathBuf::from(rest) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp(scheme: Scheme, host: &str, port: u16) -> Option<ListenAddress> {
        Some(ListenAddress::Tcp { scheme, host: host.to_owned(), port })
    }

    #[test]
    fn verdaccio_forms() {
        assert_eq!(parse_address("4873"), tcp(Scheme::Http, "localhost", 4873));
        assert_eq!(parse_address("localhost:4873"), tcp(Scheme::Http, "localhost", 4873));
        assert_eq!(parse_address("0.0.0.0:4873"), tcp(Scheme::Http, "0.0.0.0", 4873));
        assert_eq!(parse_address("[::1]:4873"), tcp(Scheme::Http, "::1", 4873));
        assert_eq!(parse_address("http::4873"), tcp(Scheme::Http, "localhost", 4873));
        assert_eq!(parse_address("https:localhost:1234"), tcp(Scheme::Https, "localhost", 1234));
        assert_eq!(parse_address("http://localhost:4873/"), tcp(Scheme::Http, "localhost", 4873));
        assert_eq!(parse_address("https://localhost:443/"), tcp(Scheme::Https, "localhost", 443));
        assert_eq!(parse_address("http://[::1]:443/"), tcp(Scheme::Http, "::1", 443));
        assert_eq!(parse_address("http:4873"), tcp(Scheme::Http, "localhost", 4873));
        assert_eq!(parse_address("httpx:80"), tcp(Scheme::Http, "httpx", 80));
        assert_eq!(parse_address(":4873"), tcp(Scheme::Http, "localhost", 4873));
        assert_eq!(parse_address("04873"), tcp(Scheme::Http, "localhost", 4873));
        // `[^/:]*` accepts brackets, so this is a plain host for the regex too
        assert_eq!(parse_address("[]:80"), tcp(Scheme::Http, "[]", 80));
        assert_eq!(
            parse_address("unix:/tmp/http.sock"),
            Some(ListenAddress::Unix { path: PathBuf::from("/tmp/http.sock") })
        );
        assert_eq!(
            parse_address("https://unix:/tmp/http.sock"),
            Some(ListenAddress::Unix { path: PathBuf::from("/tmp/http.sock") })
        );
    }

    #[test]
    fn rejects() {
        for bad in [
            "",
            "localhost",
            "http://localhost",
            "65536",
            "a:b:4873",
            "http:///4873",
            "[::1]4873",
            "unix:tmp/sock",
            "unix:/a\nb",
            "ftp://localhost:21",
            "localhost:4873/x",
            "4873//",
        ] {
            assert_eq!(parse_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn display() {
        assert_eq!(parse_address("4873").unwrap().to_string(), "http://localhost:4873");
        assert_eq!(parse_address("https://[::1]:443").unwrap().to_string(), "https://[::1]:443");
        assert_eq!(parse_address("unix:/tmp/s.sock").unwrap().to_string(), "unix:/tmp/s.sock");
    }
}
