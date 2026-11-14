//! Name → path mapping: `resolveSafePath` (`packages/core/core/src/file-utils.ts`), the
//! exact-folder check of `getPackageStorage` (`local-database.ts`) and `sanitize-filename`
//! (applied by `local-fs.ts:_getStorage` to every file name).

use std::path::{Component, Path, PathBuf};

/// Lexical `path.resolve`: `.` dropped, `..` pops (never above the root), no symlink lookup.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
        }
    }
    out
}

/// `path.resolve(base)`: absolute and normalized.
pub(crate) fn absolute(base: &Path) -> Option<PathBuf> {
    if base.is_absolute() {
        Some(normalize(base))
    } else {
        let cwd = std::env::current_dir().ok()?;
        Some(normalize(&cwd.join(base)))
    }
}

/// `decodeURIComponent`: `None` on a malformed escape or invalid UTF-8 (where JS throws).
pub(crate) fn decode_uri_component(input: &str) -> Option<String> {
    if !input.contains('%') {
        return Some(input.to_owned());
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = (hex[0] as char).to_digit(16)?;
            let lo = (hex[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn is_absolute_like(decoded: &str) -> bool {
    let path = Path::new(decoded);
    decoded.starts_with('/')
        || path.is_absolute()
        || path.has_root()
        || matches!(path.components().next(), Some(Component::Prefix(_)))
}

/// `resolveSafePath(base, request)`: `None` when `request` is empty, malformed, absolute, or
/// resolves outside `base`. `base` must already be absolute and normalized.
pub(crate) fn resolve_safe_path(base: &Path, request: &str) -> Option<PathBuf> {
    if request.is_empty() {
        return None;
    }
    let decoded = decode_uri_component(request)?;
    if is_absolute_like(&decoded) {
        return None;
    }
    let resolved = normalize(&base.join(&decoded));
    resolved.starts_with(base).then_some(resolved)
}

/// `path.relative(base, path).split(sep).join('/')` for a `path` inside `base`.
pub(crate) fn relative_slash(base: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(base).ok()?;
    let mut parts = Vec::new();
    for component in rest.components() {
        parts.push(component.as_os_str().to_str()?);
    }
    Some(parts.join("/"))
}

/// `sanitize-filename@1.6` with the empty replacement: drops `/?<>\:*|"`, C0/C1 control
/// characters, names made only of dots, Windows reserved device names (with any extension) and
/// trailing dots/spaces, then truncates to 255 UTF-8 bytes without splitting a character.
pub(crate) fn sanitize_filename(input: &str) -> String {
    let mut s: String = input
        .chars()
        .filter(|&c| {
            !matches!(c, '/' | '?' | '<' | '>' | '\\' | ':' | '*' | '|' | '"')
                && !matches!(c as u32, 0x00..=0x1f | 0x80..=0x9f)
        })
        .collect();
    if !s.is_empty() && s.chars().all(|c| c == '.') {
        s.clear();
    }
    if is_windows_reserved(&s) {
        s.clear();
    }
    let trimmed = s.trim_end_matches(['.', ' ']).len();
    s.truncate(trimmed);
    if s.len() > 255 {
        let mut end = 255;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// `/^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?$/i` (`.` does not match line terminators).
fn is_windows_reserved(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem_len = if ["con", "prn", "aux", "nul"].iter().any(|p| lower.starts_with(p)) {
        3
    } else if (lower.starts_with("com") || lower.starts_with("lpt"))
        && lower.as_bytes().get(3).is_some_and(u8::is_ascii_digit)
    {
        4
    } else {
        return false;
    };
    let rest = &lower[stem_len..];
    rest.is_empty()
        || (rest.starts_with('.') && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_is_lexical() {
        assert_eq!(normalize(Path::new("/a/./b/../c//d")), PathBuf::from("/a/c/d"));
        assert_eq!(normalize(Path::new("/../..")), PathBuf::from("/"));
    }

    #[test]
    fn decode_follows_decode_uri_component() {
        assert_eq!(decode_uri_component("a%2Fb").as_deref(), Some("a/b"));
        assert_eq!(decode_uri_component("%2e%2E").as_deref(), Some(".."));
        assert_eq!(decode_uri_component("%40scope").as_deref(), Some("@scope"));
        assert_eq!(decode_uri_component("plain").as_deref(), Some("plain"));
        assert_eq!(decode_uri_component("%"), None);
        assert_eq!(decode_uri_component("%zz"), None);
        assert_eq!(decode_uri_component("%2"), None);
        assert_eq!(decode_uri_component("%ff"), None, "invalid UTF-8");
        assert_eq!(decode_uri_component("%C3%A9").as_deref(), Some("é"));
    }

    #[test]
    fn resolve_safe_path_cases() {
        let base = Path::new("/srv/storage");
        assert_eq!(resolve_safe_path(base, "pkg"), Some(PathBuf::from("/srv/storage/pkg")));
        assert_eq!(resolve_safe_path(base, "."), Some(PathBuf::from("/srv/storage")));
        assert_eq!(resolve_safe_path(base, "a/../b"), Some(PathBuf::from("/srv/storage/b")));
        assert_eq!(resolve_safe_path(base, ""), None);
        assert_eq!(resolve_safe_path(base, ".."), None);
        assert_eq!(resolve_safe_path(base, "../storage2"), None);
        assert_eq!(resolve_safe_path(base, "%2e%2e"), None);
        assert_eq!(resolve_safe_path(base, "%2e%2e%2fx"), None);
        assert_eq!(resolve_safe_path(base, "/etc"), None);
        assert_eq!(resolve_safe_path(base, "%2Fetc"), None);
        assert_eq!(resolve_safe_path(base, "%"), None);
    }

    #[test]
    fn relative_slash_joins_with_slash() {
        let base = Path::new("/s");
        assert_eq!(relative_slash(base, Path::new("/s/@a/b")).as_deref(), Some("@a/b"));
        assert_eq!(relative_slash(base, Path::new("/s")).as_deref(), Some(""));
        assert_eq!(relative_slash(base, Path::new("/t")), None);
    }

    #[test]
    fn sanitize_matches_sanitize_filename() {
        assert_eq!(sanitize_filename("pkg-1.0.0.tgz"), "pkg-1.0.0.tgz");
        assert_eq!(sanitize_filename("package.json"), "package.json");
        assert_eq!(sanitize_filename("@scope/pkg"), "@scopepkg");
        assert_eq!(sanitize_filename("../x"), "..x");
        assert_eq!(sanitize_filename(".."), "");
        assert_eq!(sanitize_filename("."), "");
        assert_eq!(sanitize_filename("a\\b:c*d?e\"f<g>h|i"), "abcdefghi");
        assert_eq!(sanitize_filename("a\u{0}b\u{1f}c\u{85}d"), "abcd");
        assert_eq!(sanitize_filename("con"), "");
        assert_eq!(sanitize_filename("COM1.tgz"), "");
        assert_eq!(sanitize_filename("con-1.0.0.tgz"), "con-1.0.0.tgz");
        assert_eq!(sanitize_filename("console.tgz"), "console.tgz");
        assert_eq!(sanitize_filename("x.tgz. ."), "x.tgz");
        let long = "a".repeat(300);
        assert_eq!(sanitize_filename(&long).len(), 255);
        let multibyte = format!("{}é", "a".repeat(254));
        assert_eq!(sanitize_filename(&multibyte), "a".repeat(254));
    }
}
