//! `JSON.stringify(value)` and `JSON.stringify(value, undefined, '  ')` over `serde_json::Value`.
//!
//! The storage file (`package.json`, compact) and the wire bodies (pretty, two spaces) are written
//! the way Verdaccio's `JSON.stringify` writes them, so the bytes match. Three JavaScript
//! behaviours that `serde_json`'s own serializers do not have:
//!
//! - numbers are printed with `Number.prototype.toString` (every number is a double: `1.0` → `1`,
//!   integers beyond 2^53 are rounded, `1e21` and `1e-7` use exponent form, `-0` → `0`);
//! - strings use the ECMAScript `QuoteJSONString` escapes (only `"`, `\`, and code points below
//!   U+0020; U+2028/U+2029 and DEL are written as is);
//! - object keys that are array indices (`"0"` … `"4294967294"`, canonical form) come first in
//!   ascending numeric order, then the other keys in insertion order (`OrdinaryOwnPropertyKeys`).

use serde_json::{Map, Number, Value};

/// Writes `value` at nesting level `indent` (two spaces per level). The first line is not
/// indented: the caller is positioned after `"key": ` or at the start of an array element.
pub fn write_value(out: &mut Vec<u8>, value: &Value, indent: usize) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.extend_from_slice(b"[]");
                return;
            }
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                newline(out, indent + 1);
                write_value(out, item, indent + 1);
            }
            newline(out, indent);
            out.push(b']');
        }
        Value::Object(map) => write_object(out, map, indent, |out, _, v, indent| {
            write_value(out, v, indent);
        }),
    }
}

/// Writes an object whose entries are visited in JavaScript key order; `write_entry` writes the
/// value of each entry (it receives the key, so callers can substitute values).
pub fn write_object<'a>(
    out: &mut Vec<u8>,
    map: &'a Map<String, Value>,
    indent: usize,
    mut write_entry: impl FnMut(&mut Vec<u8>, &'a str, &'a Value, usize),
) {
    let mut writer = ObjectWriter::new(out, indent);
    for (key, value) in js_entries(map) {
        writer.key(key);
        write_entry(writer.out, key, value, indent + 1);
    }
    writer.finish();
}

/// Incremental writer for a pretty-printed object built field by field.
pub struct ObjectWriter<'o> {
    /// The output buffer.
    pub out: &'o mut Vec<u8>,
    indent: usize,
    count: usize,
}

impl<'o> ObjectWriter<'o> {
    /// A writer for an object at nesting level `indent`.
    pub fn new(out: &'o mut Vec<u8>, indent: usize) -> Self {
        Self { out, indent, count: 0 }
    }

    /// Starts an entry: writes the separator, indentation, quoted key and `": "`. The caller then
    /// writes the value at level `indent + 1`.
    pub fn key(&mut self, key: &str) {
        self.out.push(if self.count == 0 { b'{' } else { b',' });
        self.count += 1;
        newline(self.out, self.indent + 1);
        write_string(self.out, key);
        self.out.extend_from_slice(b": ");
    }

    /// Writes a whole entry with a plain value.
    pub fn entry(&mut self, key: &str, value: &Value) {
        self.key(key);
        write_value(self.out, value, self.indent + 1);
    }

    /// Closes the object (`{}` when no entry was written).
    pub fn finish(self) {
        if self.count == 0 {
            self.out.extend_from_slice(b"{}");
        } else {
            newline(self.out, self.indent);
            self.out.push(b'}');
        }
    }
}

fn newline(out: &mut Vec<u8>, indent: usize) {
    out.push(b'\n');
    out.resize(out.len() + indent * 2, b' ');
}

/// Entries of `map` in JavaScript own-property order.
pub fn js_entries(map: &Map<String, Value>) -> impl Iterator<Item = (&str, &Value)> {
    let indices: Option<Vec<(u32, &str, &Value)>> = if map.keys().any(|k| array_index(k).is_some())
    {
        let mut v: Vec<_> =
            map.iter().filter_map(|(k, v)| array_index(k).map(|i| (i, k.as_str(), v))).collect();
        v.sort_unstable_by_key(|(i, _, _)| *i);
        Some(v)
    } else {
        None
    };
    let has_indices = indices.is_some();
    indices.into_iter().flatten().map(|(_, k, v)| (k, v)).chain(
        map.iter()
            .filter(move |(k, _)| !has_indices || array_index(k).is_none())
            .map(|(k, v)| (k.as_str(), v)),
    )
}

/// `Some(i)` when `key` is a canonical array index (`0` ≤ i ≤ 2^32 − 2).
pub fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    (n <= u64::from(u32::MAX - 1)).then_some(n as u32)
}

/// ECMAScript `QuoteJSONString`.
pub fn write_string(out: &mut Vec<u8>, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let escape: &[u8] = match b {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\x08' => b"\\b",
            b'\x0c' => b"\\f",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0..=0x1f => b"",
            _ => continue,
        };
        out.extend_from_slice(&bytes[start..i]);
        if escape.is_empty() {
            out.extend_from_slice(b"\\u00");
            out.push(HEX[usize::from(b >> 4)]);
            out.push(HEX[usize::from(b & 0xf)]);
        } else {
            out.extend_from_slice(escape);
        }
        start = i + 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

/// A JSON number as JavaScript prints it: every number is a double.
pub fn write_number(out: &mut Vec<u8>, n: &Number) {
    const EXACT: u64 = 1 << 53;
    if let Some(u) = n.as_u64() {
        if u <= EXACT {
            out.extend_from_slice(u.to_string().as_bytes());
        } else {
            write_f64(out, u as f64);
        }
    } else if let Some(i) = n.as_i64() {
        if i.unsigned_abs() <= EXACT {
            out.extend_from_slice(i.to_string().as_bytes());
        } else {
            write_f64(out, i as f64);
        }
    } else if let Some(f) = n.as_f64() {
        write_f64(out, f);
    } else {
        out.extend_from_slice(b"null");
    }
}

/// `Number.prototype.toString()` for a double (`Number::toString(x, 10)` in ECMA-262); JSON
/// writes non-finite numbers as `null`.
pub fn write_f64(out: &mut Vec<u8>, f: f64) {
    out.extend_from_slice(js_number_string(f).as_bytes());
}

/// `Number.prototype.toString()`; `NaN`/`±Infinity` give `null` (their JSON form).
pub fn js_number_string(f: f64) -> String {
    if !f.is_finite() {
        return "null".to_owned();
    }
    if f == 0.0 {
        return "0".to_owned();
    }
    let mut s = String::new();
    if f < 0.0 {
        s.push('-');
    }
    // Rust's `{:e}` is the shortest round-tripping digit string, as ECMA-262 requires.
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').expect("LowerExp always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exp: i32 = exp.parse().expect("LowerExp exponent is an integer");
    let k = digits.len() as i32;
    let n = exp + 1;
    if k <= n && n <= 21 {
        s.push_str(&digits);
        s.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        s.push_str(&digits[..n as usize]);
        s.push('.');
        s.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        s.push_str("0.");
        s.extend(std::iter::repeat_n('0', (-n) as usize));
        s.push_str(&digits);
    } else {
        s.push_str(&digits[..1]);
        if k > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if n - 1 < 0 { '-' } else { '+' });
        s.push_str(&(n - 1).abs().to_string());
    }
    s
}

/// `JSON.stringify(value)`: compact, JavaScript key order, numbers and escapes as JavaScript
/// writes them.
pub fn write_compact(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_compact(out, item);
            }
            out.push(b']');
        }
        Value::Object(map) => write_compact_object(out, map),
    }
}

/// `JSON.stringify(object)` for a map (see [`write_compact`]).
pub fn write_compact_object(out: &mut Vec<u8>, map: &Map<String, Value>) {
    out.push(b'{');
    for (i, (key, value)) in js_entries(map).enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write_string(out, key);
        out.push(b':');
        write_compact(out, value);
    }
    out.push(b'}');
}

/// Rough number of bytes a number takes when written (to size buffers, not to write).
const NUMBER_LEN_ESTIMATE: usize = 13;

/// Estimated length of [`write_compact`]'s output: exact except that string escapes are not
/// counted and numbers count [`NUMBER_LEN_ESTIMATE`] bytes. For sizing buffers.
pub fn compact_len_estimate(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(_) => NUMBER_LEN_ESTIMATE,
        Value::String(s) => s.len() + 2,
        Value::Array(items) => {
            2 + items.len().saturating_sub(1)
                + items.iter().map(compact_len_estimate).sum::<usize>()
        }
        Value::Object(map) => compact_object_len_estimate(map),
    }
}

/// [`compact_len_estimate`] of an object.
pub fn compact_object_len_estimate(map: &Map<String, Value>) -> usize {
    2 + map.len().saturating_sub(1)
        + map.iter().map(|(k, v)| k.len() + 3 + compact_len_estimate(v)).sum::<usize>()
}

/// Estimated length of [`write_value`]'s output at `indent` (same approximations as
/// [`compact_len_estimate`]).
pub fn pretty_len_estimate(value: &Value, indent: usize) -> usize {
    let inner = 2 * (indent + 1);
    match value {
        Value::Array(items) if !items.is_empty() => {
            // "[" + per item "\n" + spaces + value + "," + "\n" + spaces + "]"
            2 + 2 * indent
                + items.len()
                + items
                    .iter()
                    .map(|v| 1 + inner + pretty_len_estimate(v, indent + 1))
                    .sum::<usize>()
        }
        Value::Object(map) if !map.is_empty() => {
            // per entry: "\n" + spaces + "\"key\": " + value + ","
            2 + 2 * indent
                + map.len()
                + map
                    .iter()
                    .map(|(k, v)| 1 + inner + k.len() + 4 + pretty_len_estimate(v, indent + 1))
                    .sum::<usize>()
        }
        other => compact_len_estimate(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn length_estimates_are_exact_without_escapes_or_numbers() {
        let v = json!({"a": [true, null, "xy", [], {}], "b": {"c": "d", "e": false}, "": ""});
        let mut compact = Vec::new();
        write_compact(&mut compact, &v);
        assert_eq!(compact_len_estimate(&v), compact.len());
        let mut pretty = Vec::new();
        write_value(&mut pretty, &v, 0);
        assert_eq!(pretty_len_estimate(&v, 0), pretty.len());
    }

    fn compact(v: &Value) -> String {
        let mut out = Vec::new();
        write_compact(&mut out, v);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn compact_matches_json_stringify() {
        // JSON.stringify(JSON.parse(input)) in node 24
        let v: Value = serde_json::from_str(
            r#"{"b":1.0,"10":[],"a":{"z":1,"3":2,"1":3},"2":[1,{"x":null}],"01":true,"4294967295":0,"n":1e21,"s":"\u2028\u0001"}"#,
        )
        .unwrap();
        assert_eq!(
            compact(&v),
            "{\"2\":[1,{\"x\":null}],\"10\":[],\"b\":1,\"a\":{\"1\":3,\"3\":2,\"z\":1},\"01\":true,\
             \"4294967295\":0,\"n\":1e+21,\"s\":\"\u{2028}\\u0001\"}"
        );
        assert_eq!(compact(&json!({})), "{}");
        assert_eq!(compact(&json!([])), "[]");
        assert_eq!(
            compact(&json!({"a": [true, false, null, "x"]})),
            "{\"a\":[true,false,null,\"x\"]}"
        );
    }
}
