//! Package and tarball name validation.
//!
//! Verdaccio reference: `packages/core/core/src/validation-utils.ts` (`validatePackage`,
//! `validateName`, `validatePackageName`) and `extractTarballFromUrl` in
//! `packages/core/core/src/tarball-utils.ts`.
//!
//! The server validates route parameters with these functions *after* percent-decoding, as
//! Express `app.param` does: `:package` with [`validate_package`] (`400 invalid package`),
//! `:filename` and `:version` with [`validate_name`] (`400 invalid filename` /
//! `400 invalid version`).

/// Characters `encodeURIComponent` leaves untouched.
fn is_url_friendly(value: &str) -> bool {
    value.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')')
    })
}

/// JavaScript `String.prototype.trim` whitespace: Unicode `White_Space` plus U+FEFF.
fn is_js_trim_char(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

const EXCLUDED_PACKAGE_NAMES: [&str; 2] = ["node_modules", "favicon.ico"];
const EXCLUDED_NAMES: [&str; 3] = ["node_modules", "__proto__", "favicon.ico"];

/// Whether a name is scoped (starts with `@`).
pub fn is_scoped(name: &str) -> bool {
    name.starts_with('@')
}

/// `validatePackageName`: rules for an existing package name (a leading hyphen stays valid).
fn validate_package_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') || name.starts_with('_') {
        return false;
    }
    if name.trim_matches(is_js_trim_char) != name {
        return false;
    }
    if EXCLUDED_PACKAGE_NAMES.contains(&name.to_lowercase().as_str()) {
        return false;
    }
    if is_url_friendly(name) {
        return true;
    }
    // /^(?:@([^/]+?)[/])?([^/]+?)$/ — only the scoped form can still be valid here.
    let Some(rest) = name.strip_prefix('@') else {
        return false;
    };
    let Some((user, pkg)) = rest.split_once('/') else {
        return false;
    };
    !user.is_empty()
        && !pkg.is_empty()
        && !pkg.contains('/')
        && is_url_friendly(user)
        && !pkg.starts_with('.')
        && is_url_friendly(pkg)
}

/// `isValidPackagePath`: one segment, or `@scope/name` with both parts valid names.
fn is_valid_package_path(name: &str) -> bool {
    let segments: Vec<&str> = name.split('/').collect();
    match segments.as_slice() {
        [single] => validate_name(single),
        [scope, pkg] => scope.starts_with('@') && validate_name(&scope[1..]) && validate_name(pkg),
        _ => false,
    }
}

/// Validate a package name (`validatePackage`): `name` or `@scope/name`, decoded.
///
/// Returns `false` for empty names, names with 3+ segments, names starting with `.` or `_`,
/// surrounding whitespace, `node_modules`, `favicon.ico`, `__proto__`, and characters outside
/// `[-a-zA-Z0-9_.!~*'()@]` (checked after lower-casing, like Verdaccio).
pub fn validate_package(name: &str) -> bool {
    is_valid_package_path(name) && validate_package_name(name)
}

/// Validate a simple name: a tarball filename, a version or a dist-tag (`validateName`).
///
/// A scoped value containing `/` is validated as a package. Otherwise the lower-cased value must
/// match `^[-a-zA-Z0-9_.!~*'()@]+$`, must not start with `.` and must not be `node_modules`,
/// `__proto__` or `favicon.ico`.
pub fn validate_name(name: &str) -> bool {
    if is_scoped(name) && name.contains('/') {
        return validate_package(name);
    }
    let lower = name.to_lowercase();
    !lower.is_empty()
        && lower.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')' | '@')
        })
        && !lower.starts_with('.')
        && !EXCLUDED_NAMES.contains(&lower.as_str())
}

/// Last path segment of a tarball URL (`extractTarballFromUrl`):
/// `https://registry.npmjs.org/@s/p/-/p-1.0.0.tgz?x=1` → `p-1.0.0.tgz`.
///
/// The query string and fragment are ignored. The segment is returned as it appears in the
/// parsed URL path (still percent-encoded). Returns `None` when `url` is not an absolute URL
/// (Verdaccio's `new URL(url)` throws there).
pub fn extract_tarball_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let path = parsed.path();
    Some(path.rsplit('/').next().unwrap_or(path).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_packages() {
        for name in [
            "react",
            "lodash.merge",
            "-leading-hyphen",
            "UPPER",
            "@types/node",
            "@babel/core",
            "a",
            "pkg!~*'()",
            "@scope/name.with.dots",
            "@scope/_name",
        ] {
            assert!(validate_package(name), "{name} should be valid");
        }
    }

    #[test]
    fn invalid_packages() {
        for name in [
            "",
            ".bin",
            "_private",
            " space",
            "space ",
            "\u{FEFF}bom",
            "node_modules",
            "NODE_MODULES",
            "favicon.ico",
            "__proto__",
            "a/b",
            "@scope",
            "@/name",
            "@scope/",
            "@scope/.name",
            "@scope/name/extra",
            "a/b/c",
            "with space",
            "with%20pct",
            "ünïcode",
            "@sc ope/name",
            "with@at",
        ] {
            assert!(!validate_package(name), "{name:?} should be invalid");
        }
    }

    #[test]
    fn scoped_name_with_underscore_scope_is_valid() {
        // isValidPackagePath accepts "@_scope" because validateName("_scope") is true;
        // validatePackageName only checks that the whole name does not start with "_".
        assert!(validate_package("@_scope/name"));
    }

    #[test]
    fn valid_names() {
        for name in ["pkg-1.0.0.tgz", "1.0.0", "1.0.0-beta.1", "latest", "next", "a@1.0.0.tgz"] {
            assert!(validate_name(name), "{name} should be valid");
        }
        // scoped values with a slash are validated as packages
        assert!(validate_name("@scope/pkg"));
        assert!(!validate_name("@scope/.pkg"));
        // Kelvin sign lower-cases to ASCII 'k', as in JavaScript
        assert!(validate_name("\u{212A}"));
    }

    #[test]
    fn invalid_names() {
        for name in [
            "",
            ".hidden",
            "..",
            "node_modules",
            "__proto__",
            "FAVICON.ICO",
            "a/b",
            "a b",
            "1.0.0+build",
            "x%2Fy",
            "a\\b",
        ] {
            assert!(!validate_name(name), "{name:?} should be invalid");
        }
    }

    #[test]
    fn tarball_from_url() {
        assert_eq!(
            extract_tarball_from_url("https://registry.npmjs.org/react/-/react-18.2.0.tgz")
                .as_deref(),
            Some("react-18.2.0.tgz")
        );
        assert_eq!(
            extract_tarball_from_url("https://r.example/@s/p/-/p-1.0.0.tgz?token=x#frag")
                .as_deref(),
            Some("p-1.0.0.tgz")
        );
        assert_eq!(extract_tarball_from_url("http://h/").as_deref(), Some(""));
        assert_eq!(extract_tarball_from_url("not a url"), None);
        assert_eq!(extract_tarball_from_url("/relative/p-1.tgz"), None);
    }
}
