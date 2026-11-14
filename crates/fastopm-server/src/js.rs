//! Small JavaScript semantics the request pipeline depends on.

/// JavaScript `WhiteSpace` + `LineTerminator` (what `String.prototype.trim` strips and what the
/// regex class `\s` matches): Unicode `White_Space` without U+0085, plus U+FEFF.
pub(crate) fn is_js_space(c: char) -> bool {
    c != '\u{85}' && (c.is_whitespace() || c == '\u{FEFF}')
}

/// `String.prototype.trim`.
pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// A header value as Node.js exposes it: bytes decoded as Latin-1, so it never fails.
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// `ToNumber(string)` (`StringToNumber`): trimmed; empty → 0; `0x`/`0o`/`0b` integers;
/// `[+-]Infinity`; decimal literals; anything else is `NaN`.
pub(crate) fn string_to_number(s: &str) -> f64 {
    let s = trim(s);
    if s.is_empty() {
        return 0.0;
    }
    let radix = match s.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &s[2..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |acc, c| {
            acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
        });
    }
    let (sign, unsigned) = match s.as_bytes()[0] {
        b'+' => (1.0, &s[1..]),
        b'-' => (-1.0, &s[1..]),
        _ => (1.0, s),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(unsigned) {
        return f64::NAN;
    }
    unsigned.parse::<f64>().map_or(f64::NAN, |v| sign * v)
}

/// `StrUnsignedDecimalLiteral`: `digits[.digits]`, `digits.`, `.digits`, optional exponent.
fn is_decimal_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    let int_digits = count_digits(&bytes[i..]);
    i += int_digits;
    let mut frac_digits = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = count_digits(&bytes[i..]);
        i += frac_digits;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp = count_digits(&bytes[i..]);
        if exp == 0 {
            return false;
        }
        i += exp;
    }
    i == bytes.len()
}

fn count_digits(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|b| b.is_ascii_digit()).count()
}

/// `parseFloat`: the longest decimal prefix (after leading whitespace), `NaN` when none.
pub(crate) fn parse_float(s: &str) -> f64 {
    let s = s.trim_start_matches(is_js_space);
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'+') => (1.0, &s[1..]),
        Some(b'-') => (-1.0, &s[1..]),
        _ => (1.0, s),
    };
    if rest.starts_with("Infinity") {
        return sign * f64::INFINITY;
    }
    let bytes = rest.as_bytes();
    let int_digits = count_digits(bytes);
    let mut end = int_digits;
    let mut frac_digits = 0;
    if bytes.get(end) == Some(&b'.') {
        frac_digits = count_digits(&bytes[end + 1..]);
        if int_digits > 0 || frac_digits > 0 {
            end += 1 + frac_digits;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return f64::NAN;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut j = end + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp = count_digits(&bytes[j..]);
        if exp > 0 {
            end = j + exp;
        }
    }
    rest[..end].parse::<f64>().map_or(f64::NAN, |v| sign * v)
}

/// `Array.prototype.sort` as V8 runs it for arrays shorter than 64 elements (one natural run,
/// then binary insertion sort), including its behaviour with inconsistent comparators such as
/// one returning `NaN`. Longer arrays use the same procedure, which equals V8's TimSort for
/// consistent comparators.
pub(crate) fn v8_sort<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> f64) {
    let n = items.len();
    if n < 2 {
        return;
    }
    // CountAndMakeRun(0, n)
    let mut run = 2;
    let descending = compare(&items[1], &items[0]) < 0.0;
    while run < n {
        let order = compare(&items[run], &items[run - 1]);
        let stop = if descending { order >= 0.0 } else { order < 0.0 };
        if stop {
            break;
        }
        run += 1;
    }
    if descending {
        items[..run].reverse();
    }
    // BinaryInsertionSort(0, run, n)
    for start in run..n {
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if compare(&items[start], &items[mid]) < 0.0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        items[left..=start].rotate_right(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_number() {
        assert_eq!(string_to_number(""), 0.0);
        assert_eq!(string_to_number("  "), 0.0);
        assert_eq!(string_to_number("1.0"), 1.0);
        assert_eq!(string_to_number(" 0.8 "), 0.8);
        assert_eq!(string_to_number("0x10"), 16.0);
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(string_to_number("1e3"), 1000.0);
        assert!(string_to_number("inf").is_nan());
        assert!(string_to_number("1.0x").is_nan());
        assert!(string_to_number("utf8").is_nan());
        assert!(string_to_number("-0x1").is_nan());
        assert!(string_to_number(".").is_nan());
    }

    #[test]
    fn parse_float_prefix() {
        assert_eq!(parse_float("0.5"), 0.5);
        assert_eq!(parse_float(" 1abc"), 1.0);
        assert_eq!(parse_float("1.e5x"), 1.0e5);
        assert_eq!(parse_float("1e"), 1.0);
        assert_eq!(parse_float(".5."), 0.5);
        assert!(parse_float("").is_nan());
        assert!(parse_float("abc").is_nan());
        assert!(parse_float(".").is_nan());
    }

    #[test]
    fn sort_is_stable_and_handles_nan() {
        let mut v = vec![(1, 0.0), (2, 1.0), (3, 0.0), (4, 1.0)];
        v8_sort(&mut v, |a, b| b.1 - a.1);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [2, 4, 1, 3]);
        // NaN never counts as "less", so these stay in place.
        let mut v = vec![(1, f64::NAN), (2, 1.0)];
        v8_sort(&mut v, |a, b| b.1 - a.1);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [1, 2]);
        let mut v = vec![(1, 0.0), (2, 0.5), (3, 1.0), (4, f64::NAN), (5, 2.0)];
        v8_sort(&mut v, |a, b| b.1 - a.1);
        // A descending run never stops on NaN: the whole array is one run, reversed.
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [5, 4, 3, 2, 1]);
    }

    #[test]
    fn whitespace() {
        assert_eq!(trim("\u{FEFF} a \u{A0}"), "a");
        assert_eq!(trim("\u{85}a"), "\u{85}a");
        assert_eq!(latin1(&[0x61, 0xE9]), "aé");
    }
}
