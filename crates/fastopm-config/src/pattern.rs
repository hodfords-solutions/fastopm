//! `packages:` pattern matching with `minimatch(name, pattern, { nocase: true })` semantics.
//!
//! Verdaccio reference: `getMatchedPackagesSpec` in `packages/core/core/src/auth-utils.ts`
//! (minimatch 9/10, default options plus `nocase`). This is a port of minimatch 11.0.3, the
//! version `verdaccio@7.1.0` ships: comment and negation handling, brace expansion
//! (`brace.rs`), splitting on `/`, the level-1 `**` / `..` optimizations, `**` walking, and the
//! per-portion matcher (`segment.rs`). `tests/fixtures/minimatch-vectors.json` holds the
//! expected results, generated from the real package.
//!
//! Known approximation: POSIX classes such as `[[:alpha:]]` use Rust's `char` properties
//! instead of exact Unicode general categories outside ASCII.

mod brace;
mod segment;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use segment::Matcher;

/// Whether `name` matches `pattern`, case-insensitively.
///
/// Must agree with JavaScript `minimatch(name, pattern, { nocase: true })` for the patterns
/// found in Verdaccio configs: `**`, `*`, `@*/*`, `@scope/*`, `prefix-*`, `?`, character classes
/// `[abc]` / `[!a]`, brace expansion `{a,b}` and extglobs as far as minimatch supports them.
/// Notable minimatch rules: `*` and `?` never match `/`; `*` does not match a leading `.` unless
/// the pattern segment starts with `.` (`dot: false`); `**` alone as a segment matches any number
/// of segments; a pattern starting with `#` is a comment and matches nothing; a leading `!`
/// negates. Test vectors should be generated from the real `minimatch` package.
pub fn pattern_matches(pattern: &str, name: &str) -> bool {
    if pattern.starts_with('#') {
        return false;
    }
    if pattern.is_empty() {
        return name.is_empty();
    }
    compiled(pattern).matches(name)
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Rc<Compiled>>> = RefCell::new(HashMap::new());
}

/// Compiled patterns are kept per thread: the same few `packages` keys are matched on every
/// request.
fn compiled(pattern: &str) -> Rc<Compiled> {
    const CACHE_MAX: usize = 256;
    CACHE.with(|cache| {
        if let Some(c) = cache.borrow().get(pattern) {
            return Rc::clone(c);
        }
        let c = Rc::new(Compiled::new(pattern));
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_MAX {
            cache.clear();
        }
        cache.insert(pattern.to_owned(), Rc::clone(&c));
        c
    })
}

/// One portion of a pattern row.
#[derive(Debug)]
enum Portion {
    /// `**`
    Globstar,
    /// A portion with no magic, compared exactly (it has no cased chars).
    Literal(String),
    /// Anything else.
    Glob(Matcher),
}

impl Portion {
    fn parse(p: &str) -> Portion {
        if p == "**" {
            return Portion::Globstar;
        }
        if p.is_empty() {
            return Portion::Literal(String::new());
        }
        // with nocase, a literal with cased chars still needs a Matcher
        match segment::literal(p) {
            Some(lit) if p.to_uppercase() == p.to_lowercase() => Portion::Literal(lit),
            _ => Portion::Glob(Matcher::new(p)),
        }
    }

    /// `portionMatch`: an empty path portion only matches an empty pattern portion.
    fn matches(&self, f: &str) -> bool {
        match self {
            Portion::Literal(lit) => lit == f,
            Portion::Glob(m) => !f.is_empty() && m.test(f),
            Portion::Globstar => false,
        }
    }
}

/// `p.split(/\/+/)`
fn slash_split(p: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = p;
    loop {
        match rest.find('/') {
            None => {
                parts.push(rest);
                return parts;
            }
            Some(i) => {
                parts.push(&rest[..i]);
                rest = rest[i..].trim_start_matches('/');
            }
        }
    }
}

/// Drop adjacent `**` and resolve `..` against the portion before it (`levelOneOptimize`).
fn level_one_optimize(parts: Vec<&str>) -> Vec<&str> {
    let mut set: Vec<&str> = Vec::new();
    for part in parts {
        let prev = set.last().copied();
        if part == "**" && prev == Some("**") {
            continue;
        }
        if part == ".."
            && let Some(prev) = prev
            && !prev.is_empty()
            && prev != ".."
            && prev != "."
            && prev != "**"
        {
            set.pop();
            continue;
        }
        set.push(part);
    }
    if set.is_empty() { vec![""] } else { set }
}

#[derive(Debug)]
struct Compiled {
    negate: bool,
    rows: Vec<Vec<Portion>>,
}

impl Compiled {
    fn new(pattern: &str) -> Compiled {
        let bangs = pattern.chars().take_while(|&c| c == '!').count();
        let negate = bangs % 2 == 1;
        let rest = &pattern[bangs..];
        let rows = brace::brace_expand(rest)
            .iter()
            .map(|s| level_one_optimize(slash_split(s)).into_iter().map(Portion::parse).collect())
            .collect();
        Compiled { negate, rows }
    }

    fn matches(&self, name: &str) -> bool {
        let file = slash_split(name);
        let hit = self.rows.iter().any(|row| {
            if row.iter().any(|p| matches!(p, Portion::Globstar)) {
                match_globstar(&file, row)
            } else {
                match_one(&file, row, 0, file.len())
            }
        });
        hit != self.negate
    }
}

/// Match `pattern` against `file[file_index..file_end]` (`#matchOne`, not partial).
fn match_one(file: &[&str], pattern: &[Portion], file_index: usize, file_end: usize) -> bool {
    let fl = file_end.min(file.len());
    let mut fi = file_index;
    let mut pi = 0;
    while fi < fl && pi < pattern.len() {
        if !pattern[pi].matches(file[fi]) {
            return false;
        }
        fi += 1;
        pi += 1;
    }
    if fi >= fl && pi == pattern.len() {
        true
    } else if fi >= fl {
        false
    } else {
        // out of pattern: only a trailing `/` (a final empty portion) may be left
        fi + 1 == fl && file[fi].is_empty()
    }
}

/// A path portion that `**` must not walk over.
fn bad_dot(f: &str) -> bool {
    f.starts_with('.')
}

/// `#matchGlobstar`: head, tail, then the sections between `**`s in order.
fn match_globstar(file: &[&str], pattern: &[Portion]) -> bool {
    let is_gs = |p: &Portion| matches!(p, Portion::Globstar);
    let first = pattern.iter().position(is_gs).unwrap_or(0);
    let last = pattern.iter().rposition(is_gs).unwrap_or(0);
    let head = &pattern[..first];
    let body = if last > first { &pattern[first + 1..last] } else { &[][..] };
    let tail = &pattern[last + 1..];

    let mut file_index = 0;
    if !head.is_empty() {
        if !match_one(file, head, 0, head.len()) {
            return false;
        }
        file_index = head.len();
    }

    let mut tail_match = 0;
    if !tail.is_empty() {
        if tail.len() + file_index > file.len() {
            return false;
        }
        let tail_start = file.len() - tail.len();
        if match_one(file, tail, tail_start, file.len()) {
            tail_match = tail.len();
        } else {
            // a/**/* matches a/b/: try again without the trailing ''
            if file.last() != Some(&"") || file_index + tail.len() == file.len() {
                return false;
            }
            if !match_one(file, tail, tail_start - 1, file.len()) {
                return false;
            }
            tail_match = tail.len() + 1;
        }
    }

    let file_length = file.len() - tail_match;
    if body.is_empty() {
        let mut saw_some = tail_match > 0;
        for f in &file[file_index.min(file_length)..file_length] {
            saw_some = true;
            if bad_dot(f) {
                return false;
            }
        }
        return saw_some;
    }

    // the body is one or more sections separated by `**`; each section must be found in order
    let sections: Vec<&[Portion]> = body.split(is_gs).collect();
    let needs: Vec<usize> =
        (0..sections.len()).map(|i| sections[i..].iter().map(|s| s.len()).sum()).collect();
    let mut tried = std::collections::HashSet::new();
    let mut walk =
        Walk { file, sections: &sections, needs: &needs, file_length, tried: &mut tried };
    walk.section(file_index, 0, 0, tail_match > 0) == Some(true)
}

/// `#matchGlobStarBodySections`. `Some(false)`: no match from here, try further on;
/// `None`: no match and walking further cannot help.
struct Walk<'a, 'f> {
    file: &'a [&'f str],
    sections: &'a [&'a [Portion]],
    needs: &'a [usize],
    file_length: usize,
    tried: &'a mut std::collections::HashSet<(usize, usize)>,
}

impl Walk<'_, '_> {
    const MAX_DEPTH: usize = 200;

    fn section(
        &mut self,
        mut file_index: usize,
        index: usize,
        depth: usize,
        saw_tail: bool,
    ) -> Option<bool> {
        let Some(body) = self.sections.get(index) else {
            // no section left: the rest before the tail must be walkable
            let mut saw = saw_tail;
            for f in self.file.get(file_index..self.file_length).unwrap_or(&[]) {
                saw = true;
                if bad_dot(f) {
                    return Some(false);
                }
            }
            return Some(saw);
        };
        let after = self.file_length.checked_sub(self.needs[index])?;
        while file_index <= after {
            if !self.tried.insert((index, file_index)) {
                return Some(false);
            }
            if match_one(self.file, body, file_index, file_index + body.len())
                && depth < Self::MAX_DEPTH
            {
                let sub = self.section(file_index + body.len(), index + 1, depth + 1, saw_tail);
                if sub != Some(false) {
                    return sub;
                }
            }
            if file_index == after {
                break;
            }
            if bad_dot(self.file[file_index]) {
                return Some(false);
            }
            file_index += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_config_patterns() {
        assert!(pattern_matches("**", "lodash"));
        assert!(pattern_matches("**", "@babel/core"));
        assert!(pattern_matches("@*/*", "@babel/core"));
        assert!(!pattern_matches("@*/*", "lodash"));
        assert!(pattern_matches("@Scope/*", "@scope/Foo"));
        assert!(pattern_matches("prefix-*", "PREFIX-x"));
        assert!(!pattern_matches("*", "@scope/foo"));
        assert!(!pattern_matches("*", ".hidden"));
    }

    #[test]
    fn comments_negation_empty() {
        assert!(!pattern_matches("#foo", "#foo"));
        assert!(pattern_matches("\\#foo", "#foo"));
        assert!(pattern_matches("!foo", "bar"));
        assert!(!pattern_matches("!foo", "foo"));
        assert!(pattern_matches("!!foo", "foo"));
        assert!(pattern_matches("", ""));
        assert!(!pattern_matches("", "foo"));
    }

    #[test]
    fn globstar_rows() {
        assert!(pattern_matches("a/**/b", "a/b"));
        assert!(pattern_matches("a/**/b", "a/x/y/b"));
        assert!(!pattern_matches("a/**/b", "a/.x/b"));
        assert!(pattern_matches("@*/**", "@scope/a/b"));
        assert!(pattern_matches("@*/*", "@scope/foo/"));
    }

    #[test]
    fn split_and_optimize() {
        assert_eq!(slash_split("a//b/"), ["a", "b", ""]);
        assert_eq!(slash_split("/a"), ["", "a"]);
        assert_eq!(level_one_optimize(vec!["a", "..", "b"]), ["b"]);
        assert_eq!(level_one_optimize(vec!["**", "**", "a"]), ["**", "a"]);
        assert_eq!(level_one_optimize(vec!["a", ".."]), [""]);
    }
}
