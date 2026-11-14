//! Brace expansion as minimatch 11 does it (`braceExpand`, `brace-expansion` 5 and
//! `balanced-match` 4): `a{b,c}` → `ab ac`, `{1..3}`, `{a..e..2}`, `{01..03}`, nested sets,
//! `${...}` left alone, `\{ \} \, \. \\` escaped, empty results of a top-level set dropped.

/// Most expansions one pattern produces (`EXPANSION_MAX`).
const MAX: usize = 100_000;
const MAX_DEPTH: usize = 1_000;
const MAX_REWRITES: usize = 1_000;

// Sentinels for escaped chars. They contain no `{ } , .` and real patterns contain no NUL.
const ESC_SLASH: &str = "\0S\0";
const ESC_OPEN: &str = "\0O\0";
const ESC_CLOSE: &str = "\0C\0";
const ESC_COMMA: &str = "\0M\0";
const ESC_PERIOD: &str = "\0P\0";

/// The expansions of `pattern`, deduplicated in order. A pattern without a `{…}` pair (a `{`
/// followed by a `}` with no `{` or line break in between) is returned as is.
pub(super) fn brace_expand(pattern: &str) -> Vec<String> {
    if !has_brace_pair(pattern) {
        return vec![pattern.to_owned()];
    }
    let mut out: Vec<String> = Vec::new();
    for e in expand(pattern) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// `/\{(?:(?!\{).)*\}/`
fn has_brace_pair(s: &str) -> bool {
    let mut open = false;
    for c in s.chars() {
        match c {
            '{' => open = true,
            '}' if open => return true,
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => open = false,
            _ => {}
        }
    }
    false
}

fn expand(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let s = match s.strip_prefix("{}") {
        Some(rest) => format!("\\{{\\}}{rest}"),
        None => s.to_owned(),
    };
    expand_(&escape_braces(&s), 0, true).iter().map(|e| unescape_braces(e)).collect()
}

fn escape_braces(s: &str) -> String {
    s.replace("\\\\", ESC_SLASH)
        .replace("\\{", ESC_OPEN)
        .replace("\\}", ESC_CLOSE)
        .replace("\\,", ESC_COMMA)
        .replace("\\.", ESC_PERIOD)
}

fn unescape_braces(s: &str) -> String {
    s.replace(ESC_SLASH, "\\")
        .replace(ESC_OPEN, "{")
        .replace(ESC_CLOSE, "}")
        .replace(ESC_COMMA, ",")
        .replace(ESC_PERIOD, ".")
}

fn index_of(s: &str, c: u8, from: usize) -> Option<usize> {
    s.as_bytes().get(from..)?.iter().position(|&b| b == c).map(|p| p + from)
}

/// JavaScript `indexOf`: -1 when absent.
fn js_index_of(s: &str, c: u8, from: isize) -> isize {
    index_of(s, c, usize::try_from(from.max(0)).unwrap_or(0)).map_or(-1, |i| i as isize)
}

/// `balanced-match` `range('{', '}', s)`: byte offsets of the first balanced pair.
fn range(s: &str) -> Option<(usize, usize)> {
    let mut ai = js_index_of(s, b'{', 0);
    let mut bi = js_index_of(s, b'}', ai + 1);
    if ai < 0 || bi <= 0 {
        return None;
    }
    let mut i = ai;
    let mut begs: Vec<isize> = Vec::new();
    let mut left = s.len() as isize;
    let mut right = None;
    let mut result = None;
    while i >= 0 && result.is_none() {
        if i == ai {
            begs.push(i);
            ai = js_index_of(s, b'{', i + 1);
        } else if begs.len() == 1 {
            result = begs.pop().map(|r| (r, bi));
        } else {
            if let Some(beg) = begs.pop()
                && beg < left
            {
                left = beg;
                right = Some(bi);
            }
            bi = js_index_of(s, b'}', i + 1);
        }
        i = if ai < bi && ai >= 0 { ai } else { bi };
    }
    if !begs.is_empty()
        && let Some(r) = right
    {
        result = Some((left, r));
    }
    result.map(|(a, b)| (a as usize, b as usize))
}

struct Balanced {
    end: usize,
    pre: String,
    body: String,
    post: String,
    unmatched: bool,
}

fn balanced(s: &str, masked: &str) -> Option<Balanced> {
    let (start, end) = range(masked)?;
    let unmatched = index_of(masked, b'{', 0).is_some_and(|first| first < start);
    Some(Balanced {
        end,
        pre: s[..start].to_owned(),
        body: s[start + 1..end].to_owned(),
        post: s[end + 1..].to_owned(),
        unmatched,
    })
}

/// Replace each `{` without a matching `}` by NUL, so it is skipped by the next `range`.
fn mask_unmatched_opens(masked: &str) -> String {
    let mut open = Vec::new();
    for (i, b) in masked.bytes().enumerate() {
        if b == b'{' {
            open.push(i);
        } else if b == b'}' {
            open.pop();
        }
    }
    let mut bytes = masked.as_bytes().to_vec();
    for i in open {
        bytes[i] = 0;
    }
    String::from_utf8(bytes).expect("replacing an ASCII byte keeps UTF-8 valid")
}

/// Whether a `,` that starts a set member comes before a `}` later in `s` (`commaSetFollows`).
fn comma_set_follows(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let mut close = false;
    for i in (0..chars.len()).rev() {
        match chars[i] {
            '}' => close = true,
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => close = false,
            ',' if close && chars.get(i + 1) != Some(&',') => return true,
            _ => {}
        }
    }
    false
}

/// Split a set body on its top-level commas (`parseCommaParts`).
fn parse_comma_parts(body: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut carry = String::new();
    let mut s = body.to_owned();
    loop {
        let Some(m) = balanced(&s, &s) else {
            let mut tail: Vec<String> = s.split(',').map(str::to_owned).collect();
            tail[0] = format!("{carry}{}", tail[0]);
            parts.extend(tail);
            return parts;
        };
        let mut p: Vec<String> = m.pre.split(',').map(str::to_owned).collect();
        p[0] = format!("{carry}{}", p[0]);
        let last = p.len() - 1;
        p[last] = format!("{}{{{}}}", p[last], m.body);
        if m.post.is_empty() {
            parts.extend(p);
            return parts;
        }
        carry = p.pop().unwrap_or_default();
        parts.extend(p);
        s = m.post;
    }
}

fn combine(acc: &[String], pre: &str, values: &[String], drop_empties: bool) -> Vec<String> {
    let mut out = Vec::new();
    for a in acc {
        let prefix = format!("{a}{pre}");
        for v in values {
            if drop_empties && prefix.is_empty() && v.is_empty() {
                continue;
            }
            if out.len() >= MAX {
                return out;
            }
            out.push(format!("{prefix}{v}"));
        }
    }
    out
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_int(s: &str) -> bool {
    is_digits(s.strip_prefix('-').unwrap_or(s))
}

fn is_alpha(s: &str) -> bool {
    s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic()
}

/// `^-?\d+\.\.-?\d+(?:\.\.-?\d+)?$` / `^[a-zA-Z]\.\.[a-zA-Z](?:\.\.-?\d+)?$`
fn sequence_kind(body: &str) -> Option<bool> {
    let parts: Vec<&str> = body.split("..").collect();
    if !(parts.len() == 2 || parts.len() == 3) || (parts.len() == 3 && !is_int(parts[2])) {
        return None;
    }
    if is_int(parts[0]) && is_int(parts[1]) {
        Some(false)
    } else if is_alpha(parts[0]) && is_alpha(parts[1]) {
        Some(true)
    } else {
        None
    }
}

/// `numeric`: an integer, or the char code of the first char.
fn numeric(s: &str) -> Option<i64> {
    if is_int(s) { s.parse().ok() } else { s.chars().next().map(|c| i64::from(u32::from(c))) }
}

fn expand_sequence(body: &str, alpha: bool) -> Option<Vec<String>> {
    let n: Vec<&str> = body.split("..").collect();
    let x = numeric(n[0])?;
    let y = numeric(n[1])?;
    let width = n[0].len().max(n[1].len());
    let mut incr = match n.get(2) {
        Some(step) => numeric(step)?.checked_abs()?.max(1),
        None => 1,
    };
    let reverse = y < x;
    if reverse {
        incr = -incr;
    }
    let padded = |s: &str| {
        let s = s.strip_prefix('-').unwrap_or(s);
        s.len() >= 2 && s.as_bytes()[0] == b'0'
    };
    let pad = n.iter().any(|s| padded(s));
    let mut out = Vec::new();
    let mut i = x;
    while (if reverse { i >= y } else { i <= y }) && out.len() < MAX {
        let c = if alpha {
            match u32::try_from(i).ok().and_then(char::from_u32) {
                Some('\\') | None => String::new(),
                Some(c) => c.to_string(),
            }
        } else {
            let mut c = i.to_string();
            if pad {
                let need = width.saturating_sub(c.len());
                if need > 0 {
                    let zeros = "0".repeat(need);
                    c = if i < 0 { format!("-{zeros}{}", &c[1..]) } else { format!("{zeros}{c}") };
                }
            }
            c
        };
        out.push(c);
        i = i.checked_add(incr)?;
    }
    Some(out)
}

fn expand_(input: &str, depth: usize, mut is_top: bool) -> Vec<String> {
    if depth > MAX_DEPTH {
        return vec![input.to_owned()];
    }
    let mut s = input.to_owned();
    let mut masked = s.clone();
    let mut acc = vec![String::new()];
    let mut rewrites = 0;
    let mut drop_empties = false;
    let mut first_group = true;
    loop {
        let Some(m) = balanced(&s, &masked) else {
            return combine(&acc, &s, &[String::new()], drop_empties);
        };
        if m.unmatched {
            masked = mask_unmatched_opens(&masked);
        }
        let literal = |acc: &[String], drop_empties: bool| {
            let whole = format!("{}{{{}}}{}", m.pre, m.body, m.post);
            combine(acc, &whole, &[String::new()], drop_empties)
        };
        if m.pre.ends_with('$') {
            let kept = format!("{}{{{}}}", m.pre, m.body);
            acc = combine(&acc, &kept, &[String::new()], drop_empties && m.post.is_empty());
            first_group = false;
            if m.post.is_empty() {
                break;
            }
            masked = masked[m.end + 1..].to_owned();
            s = m.post;
            continue;
        }
        let seq = sequence_kind(&m.body);
        let is_options = m.body.contains(',');
        if seq.is_none() && !is_options {
            if rewrites < MAX_REWRITES && comma_set_follows(&m.post) {
                rewrites += 1;
                s = format!("{}{{{}{ESC_CLOSE}{}", m.pre, m.body, m.post);
                masked = format!("{}{ESC_CLOSE}{}", &masked[..m.end], &masked[m.end + 1..]);
                is_top = true;
                continue;
            }
            return literal(&acc, drop_empties);
        }
        if first_group {
            drop_empties = is_top && seq.is_none();
            first_group = false;
        }
        let values = if let Some(alpha) = seq {
            match expand_sequence(&m.body, alpha) {
                Some(v) => v,
                None => return literal(&acc, drop_empties),
            }
        } else {
            let mut n = parse_comma_parts(&m.body);
            if n.len() == 1 {
                n = expand_(&n[0], depth + 1, false)
                    .into_iter()
                    .map(|e| format!("{{{e}}}"))
                    .collect();
                if n.len() == 1 {
                    let joined = format!("{}{}", m.pre, n[0]);
                    acc =
                        combine(&acc, &joined, &[String::new()], drop_empties && m.post.is_empty());
                    if m.post.is_empty() {
                        break;
                    }
                    masked = masked[m.end + 1..].to_owned();
                    s = m.post;
                    continue;
                }
            }
            let drops_empties = drop_empties
                && m.post.is_empty()
                && m.pre.is_empty()
                && acc.iter().all(String::is_empty);
            let mut values = Vec::new();
            'outer: for part in &n {
                for v in expand_(part, depth + 1, false) {
                    if drops_empties && v.is_empty() {
                        continue;
                    }
                    if values.len() >= MAX {
                        break 'outer;
                    }
                    values.push(v);
                }
            }
            values
        };
        acc = combine(&acc, &m.pre, &values, drop_empties && m.post.is_empty());
        if m.post.is_empty() {
            break;
        }
        masked = masked[m.end + 1..].to_owned();
        s = m.post;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::brace_expand;

    fn e(p: &str) -> Vec<String> {
        brace_expand(p)
    }

    #[test]
    fn sets_and_sequences() {
        assert_eq!(e("a{b,c}d"), ["abd", "acd"]);
        assert_eq!(e("{a,{b,c}}"), ["a", "b", "c"]);
        assert_eq!(e("{1..3}"), ["1", "2", "3"]);
        assert_eq!(e("{01..03}"), ["01", "02", "03"]);
        assert_eq!(e("{a..e..2}"), ["a", "c", "e"]);
        assert_eq!(e("{3..1}"), ["3", "2", "1"]);
        assert_eq!(e("{a,b}{c,d}"), ["ac", "ad", "bc", "bd"]);
        assert_eq!(e("foo{,-bar}"), ["foo", "foo-bar"]);
    }

    #[test]
    fn literal_braces() {
        assert_eq!(e("{}"), ["{}"]);
        assert_eq!(e("{a}"), ["{a}"]);
        assert_eq!(e("a{b"), ["a{b"]);
        assert_eq!(e("${a,b}"), ["${a,b}"]);
        assert_eq!(e("\\{a,b}"), ["{a,b}"]);
        assert_eq!(e("{,}"), Vec::<String>::new());
    }
}
