//! Request target handling before the handlers: `dotfiles`, `makeURLrelative`,
//! `encodeScopePackage` and Express route matching with parameter decoding and validation.

use fastopm_core::{RegistryError, validate_name, validate_package};
use url::Url;

use crate::Route;

/// `dotfiles('ignore')`: a segment of the raw path that starts with `.` and is longer than one
/// character.
pub(crate) fn has_dotfile_segment(path: &str) -> bool {
    path.split('/').any(|segment| segment.starts_with('.') && segment.len() > 1)
}

/// `makeURLrelative`: `new URL(req.url, "http://<host>/")` reduced to
/// `pathname + search`. This normalises dot segments (also `%2e`), backslashes and
/// characters outside the path set, and drops the authority of absolute-form targets. A
/// missing `Host` reads as `undefined`, like the JavaScript template literal.
pub(crate) fn make_relative(target: &str, host: Option<&str>) -> Result<String, RegistryError> {
    let invalid =
        || RegistryError::bad_request(format!("Invalid URL: {target} (TypeError: Invalid URL)"));
    let base =
        Url::parse(&format!("http://{}/", host.unwrap_or("undefined"))).map_err(|_| invalid())?;
    let url = base.join(target).map_err(|_| invalid())?;
    let mut out = url.path().to_owned();
    if let Some(query) = url.query() {
        out.push('?');
        out.push_str(query);
    }
    Ok(out)
}

/// `encodeScopePackage`: replace the first `/%40` with `/@`, then encode as `%2F` the `/` that
/// follows the first `/@<chars other than / and %>` unless that `/` ends the string
/// (`/(\/@[^\/%]+)\/(?!$)/`).
pub(crate) fn encode_scope(url: &str) -> String {
    let mut url = url.replacen("/%40", "/@", 1);
    if let Some(slash) = scope_slash(url.as_bytes()) {
        url.replace_range(slash..=slash, "%2F");
    }
    url
}

/// Index of the `/` that [`encode_scope`] encodes, if any.
fn scope_slash(bytes: &[u8]) -> Option<usize> {
    let mut start = 0;
    while start + 1 < bytes.len() {
        if bytes[start] == b'/' && bytes[start + 1] == b'@' {
            let run = bytes[start + 2..].iter().take_while(|&&b| b != b'/' && b != b'%').count();
            let slash = start + 2 + run;
            if run > 0 && bytes.get(slash) == Some(&b'/') && slash + 1 < bytes.len() {
                return Some(slash);
            }
        }
        start += 1;
    }
    None
}

/// `decodeURIComponent` as Express' `decodeParam` applies it: a malformed escape or invalid
/// UTF-8 → `400 Failed to decode param '<raw>'`.
fn decode_param(raw: &str) -> Result<String, RegistryError> {
    let failed = || RegistryError::bad_request(format!("Failed to decode param '{raw}'"));
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).ok_or_else(failed)?;
            let hex = std::str::from_utf8(hex).map_err(|_| failed())?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(failed());
            }
            out.push(u8::from_str_radix(hex, 16).map_err(|_| failed())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| failed())
}

/// See [`crate::parse_route`].
pub(crate) fn parse(raw_path: &str) -> Result<Option<Route>, RegistryError> {
    let encoded = encode_scope(raw_path);
    let Some(path) = encoded.strip_prefix('/') else {
        return Ok(None);
    };
    // Express non-strict routing: one trailing slash is optional.
    let path = path.strip_suffix('/').unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Ok(None);
    }
    // Literal route parts match case-insensitively on the raw path.
    match segments.as_slice() {
        ["-", ping] if ping.eq_ignore_ascii_case("ping") => Ok(Some(Route::Ping)),
        [package] => {
            let name = decode_param(package)?;
            check_package(&name)?;
            Ok(Some(Route::Manifest { name }))
        }
        [package, version] => {
            let name = decode_param(package)?;
            let version = decode_param(version)?;
            check_package(&name)?;
            if !validate_name(&version) {
                return Err(RegistryError::invalid_param("version"));
            }
            Ok(Some(Route::Version { name, version }))
        }
        [package, "-", filename] => {
            let name = decode_param(package)?;
            let filename = decode_param(filename)?;
            check_package(&name)?;
            if !validate_name(&filename) {
                return Err(RegistryError::invalid_param("filename"));
            }
            Ok(Some(Route::Tarball { name, filename }))
        }
        _ => Ok(None),
    }
}

fn check_package(name: &str) -> Result<(), RegistryError> {
    if validate_package(name) { Ok(()) } else { Err(RegistryError::invalid_param("package")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str) -> Option<Route> {
        Some(Route::Manifest { name: name.to_owned() })
    }

    #[test]
    fn dotfiles() {
        assert!(has_dotfile_segment("/.env"));
        assert!(has_dotfile_segment("/a/../b"));
        assert!(has_dotfile_segment("/pkg/-/.hidden.tgz"));
        assert!(!has_dotfile_segment("/a/./b"));
        assert!(!has_dotfile_segment("/a/%2e%2e/b"));
        assert!(!has_dotfile_segment("/simple-pkg"));
    }

    #[test]
    fn relative() {
        let rel = |t| make_relative(t, Some("localhost:4873")).unwrap();
        assert_eq!(rel("/simple-pkg"), "/simple-pkg");
        assert_eq!(rel("/simple-pkg?write=true"), "/simple-pkg?write=true");
        assert_eq!(rel("/a/%2e%2e/b"), "/b");
        assert_eq!(rel("/a/./b/"), "/a/b/");
        assert_eq!(rel("//simple-pkg"), "/");
        assert_eq!(rel("http://other:1/x/y"), "/x/y");
        assert_eq!(rel("/simple%20pkg"), "/simple%20pkg");
        assert_eq!(make_relative("/x", None).unwrap(), "/x");
        let err = make_relative("/x", Some("")).unwrap_err();
        assert_eq!(err.status(), 400);
        assert_eq!(err.message(), "Invalid URL: /x (TypeError: Invalid URL)");
        assert_eq!(make_relative("/x", Some("a b")).unwrap_err().status(), 400);
        assert_eq!(make_relative("/x", Some("h:99999")).unwrap_err().status(), 400);
    }

    #[test]
    fn scope_encoding() {
        assert_eq!(encode_scope("/@s/p"), "/@s%2Fp");
        assert_eq!(encode_scope("/@s/p/1.0.0"), "/@s%2Fp/1.0.0");
        assert_eq!(encode_scope("/%40s/p"), "/@s%2Fp");
        assert_eq!(encode_scope("/%40s%2Fp"), "/@s%2Fp");
        assert_eq!(encode_scope("/@s/"), "/@s/");
        assert_eq!(encode_scope("/@s%41/p"), "/@s%41/p");
        assert_eq!(encode_scope("/@/@s/p"), "/@/@s%2Fp");
        assert_eq!(encode_scope("/-/package/@s/p/dist-tags"), "/-/package/@s%2Fp/dist-tags");
        assert_eq!(encode_scope("/@s/?q"), "/@s%2F?q");
    }

    #[test]
    fn routes() {
        assert_eq!(parse("/-/ping"), Ok(Some(Route::Ping)));
        assert_eq!(parse("/-/PING/"), Ok(Some(Route::Ping)));
        assert_eq!(parse("/simple-pkg"), Ok(manifest("simple-pkg")));
        assert_eq!(parse("/Simple-Pkg"), Ok(manifest("Simple-Pkg")));
        assert_eq!(parse("/simple-pkg/"), Ok(manifest("simple-pkg")));
        assert_eq!(parse("/simple-pkg//"), Ok(None));
        for form in ["/@s/p", "/@s%2Fp", "/@s%2fp", "/%40s%2Fp", "/%40s/p"] {
            assert_eq!(parse(form), Ok(manifest("@s/p")), "{form}");
        }
        assert_eq!(
            parse("/@s/p/latest"),
            Ok(Some(Route::Version { name: "@s/p".into(), version: "latest".into() }))
        );
        assert_eq!(
            parse("/@s/p/-/p-1.0.0.tgz"),
            Ok(Some(Route::Tarball { name: "@s/p".into(), filename: "p-1.0.0.tgz".into() }))
        );
        assert_eq!(
            parse("/-/nope"),
            Ok(Some(Route::Version { name: "-".into(), version: "nope".into() }))
        );
        assert_eq!(
            parse("/-/p%69ng"),
            Ok(Some(Route::Version { name: "-".into(), version: "ping".into() }))
        );
        assert_eq!(parse("/-/v1/nope"), Ok(None));
        assert_eq!(parse("/a/b/c/d"), Ok(None));
        assert_eq!(parse("/a/%2D/b"), Ok(None));
        assert_eq!(parse("/"), Ok(None));
        assert_eq!(parse(""), Ok(None));
    }

    #[test]
    fn route_errors() {
        let err = |p| parse(p).unwrap_err();
        assert_eq!(err("/%E0%A4%A").message(), "Failed to decode param '%E0%A4%A'");
        assert_eq!(err("/%FF").message(), "Failed to decode param '%FF'");
        assert_eq!(err("/simple-pkg%2F..%2Fother-pkg").message(), "invalid package");
        assert_eq!(err("/simple%20pkg").message(), "invalid package");
        assert_eq!(err("/a%2Fb").message(), "invalid package");
        assert_eq!(err("/simple-pkg/%5E1.0.0").message(), "invalid version");
        assert_eq!(err("/simple-pkg/-/%2e%2e").message(), "invalid filename");
        assert_eq!(err("/simple-pkg/-/__proto__").status(), 400);
    }
}

/// Whether Verdaccio routes `method` on `path` (normalised, scope-encoded) to one of its write
/// endpoints (publish, unpublish, dist-tags, user, token, profile, audit). fastopm answers
/// those `405`; any other non-read request gets Express' `404 Cannot <METHOD> <path>` page.
pub(crate) fn is_write_route(method: &str, path: &str) -> bool {
    let Some(path) = path.strip_prefix('/') else { return false };
    let path = path.strip_suffix('/').unwrap_or(path);
    let segments: Vec<String> = path.split('/').map(str::to_ascii_lowercase).collect();
    if segments.iter().any(String::is_empty) {
        return false;
    }
    let s: Vec<&str> = segments.iter().map(String::as_str).collect();
    match method {
        // add_package, publish_package, tagging, tagging_package, add_user
        "PUT" => match s.as_slice() {
            [_] | [_, _] | [_, "-rev", _] | ["-", "package", _, "dist-tags", _] => true,
            ["-", "user", rest @ ..] => (1..=3).contains(&rest.len()),
            _ => false,
        },
        // publish_package, remove_tarball, tagging_package, user_token, delete_token
        "DELETE" => matches!(
            s.as_slice(),
            [_, "-rev", _]
                | [_, "-", _, "-rev", _]
                | ["-", "package", _, "dist-tags", _]
                | ["-", "user", "token", _, ..]
                | ["-", "npm", "v1", "tokens", "token", _]
        ),
        // profile, tokens, audit
        "POST" => matches!(
            s.as_slice(),
            ["-", "npm", "v1", "user"]
                | ["-", "npm", "v1", "tokens"]
                | ["-", "npm", "v1", "security", "audits"]
                | ["-", "npm", "v1", "security", "audits", "quick"]
                | ["-", "npm", "v1", "security", "advisories", "bulk"]
        ),
        _ => false,
    }
}

/// Body of Express' `finalhandler` 404 page: `Cannot <METHOD> <path>`, HTML-escaped.
pub(crate) fn express_not_found_page(method: &str, original_path: &str) -> String {
    let message = format!("Cannot {method} {original_path}");
    let mut escaped = String::with_capacity(message.len());
    for c in message.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n\
         </head>\n<body>\n<pre>{escaped}</pre>\n</body>\n</html>\n"
    )
}

#[cfg(test)]
mod write_route_tests {
    use super::*;

    #[test]
    fn write_routes() {
        assert!(is_write_route("PUT", "/new-pkg"));
        assert!(is_write_route("PUT", "/@scope%2Fnew-pkg"));
        assert!(is_write_route("PUT", "/simple-pkg/-rev/1-0"));
        assert!(is_write_route("PUT", "/simple-pkg/next"));
        assert!(is_write_route("PUT", "/-/user/org.couchdb.user:x"));
        assert!(is_write_route("PUT", "/-/package/simple-pkg/dist-tags/next"));
        assert!(is_write_route("DELETE", "/simple-pkg/-rev/1-0"));
        assert!(is_write_route("DELETE", "/simple-pkg/-/simple-pkg-1.0.0.tgz/-rev/1-0"));
        assert!(is_write_route("DELETE", "/-/user/token/abc"));
        assert!(is_write_route("POST", "/-/npm/v1/security/audits/quick"));
        assert!(is_write_route("POST", "/-/npm/v1/security/advisories/bulk"));
        assert!(!is_write_route("POST", "/simple-pkg"));
        assert!(!is_write_route("PATCH", "/simple-pkg"));
        assert!(!is_write_route("DELETE", "/simple-pkg"));
        assert!(!is_write_route("PUT", "/a/b/c/d"));
        assert!(!is_write_route("PUT", "/"));
    }

    #[test]
    fn not_found_page() {
        assert_eq!(
            express_not_found_page("POST", "/simple-pkg"),
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <title>Error</title>\n</head>\n<body>\n<pre>Cannot POST /simple-pkg</pre>\n\
             </body>\n</html>\n"
        );
        assert!(express_not_found_page("POST", "/a<b").contains("/a&lt;b"));
    }
}
