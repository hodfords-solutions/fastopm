//! The YAML document as Verdaccio sees it: js-yaml output (core schema plus merge keys) held in
//! JavaScript values, with the JavaScript conversions the config code relies on.

use serde_yaml_ng::Value;

/// A JavaScript value produced by js-yaml.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Js {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Js>),
    Obj(Obj),
}

/// A plain object: string keys in insertion (document) order.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Obj(pub(crate) Vec<(String, Js)>);

impl Obj {
    pub(crate) fn get(&self, key: &str) -> Option<&Js> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Js)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Entries in the order `for (const k in obj)` visits them: integer-like keys (array
    /// indices, `0` or `[1-9][0-9]*` below 2^32 - 1) first in ascending order, then the other
    /// keys in insertion order.
    pub(crate) fn js_ordered(&self) -> Vec<(&str, &Js)> {
        let mut ints: Vec<(u32, &str, &Js)> = Vec::new();
        let mut rest = Vec::new();
        for (k, v) in self.iter() {
            match array_index(k) {
                Some(i) => ints.push((i, k, v)),
                None => rest.push((k, v)),
            }
        }
        ints.sort_by_key(|(i, _, _)| *i);
        ints.into_iter().map(|(_, k, v)| (k, v)).chain(rest).collect()
    }
}

fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) || !k.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    k.parse::<u32>().ok().filter(|&i| i != u32::MAX)
}

impl Js {
    /// JavaScript truthiness.
    pub(crate) fn truthy(&self) -> bool {
        match self {
            Js::Null => false,
            Js::Bool(b) => *b,
            Js::Num(n) => *n != 0.0 && !n.is_nan(),
            Js::Str(s) => !s.is_empty(),
            Js::Arr(_) | Js::Obj(_) => true,
        }
    }

    /// `String(value)`.
    pub(crate) fn to_js_string(&self) -> String {
        match self {
            Js::Null => "null".to_owned(),
            Js::Bool(b) => b.to_string(),
            Js::Num(n) => number_to_string(*n),
            Js::Str(s) => s.clone(),
            Js::Arr(items) => items
                .iter()
                .map(|i| if matches!(i, Js::Null) { String::new() } else { i.to_js_string() })
                .collect::<Vec<_>>()
                .join(","),
            Js::Obj(_) => "[object Object]".to_owned(),
        }
    }

    /// `Number(value)`.
    pub(crate) fn to_number(&self) -> f64 {
        match self {
            Js::Null => 0.0,
            Js::Bool(b) => f64::from(u8::from(*b)),
            Js::Num(n) => *n,
            Js::Str(s) => string_to_number(s),
            Js::Arr(items) => match items.as_slice() {
                [] => 0.0,
                [one] => string_to_number(&one.to_js_string()),
                _ => f64::NAN,
            },
            Js::Obj(_) => f64::NAN,
        }
    }

    /// `JSON.stringify(value)`, for error messages.
    pub(crate) fn to_json(&self) -> String {
        match self {
            Js::Null => "null".to_owned(),
            Js::Num(n) if !n.is_finite() => "null".to_owned(),
            Js::Bool(_) | Js::Num(_) => self.to_js_string(),
            Js::Str(s) => json_string(s),
            Js::Arr(items) => {
                format!("[{}]", items.iter().map(Js::to_json).collect::<Vec<_>>().join(","))
            }
            Js::Obj(o) => format!(
                "{{{}}}",
                o.js_ordered()
                    .iter()
                    .map(|(k, v)| format!("{}:{}", json_string(k), v.to_json()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `String(number)`, exact for integers below 1e21.
pub(crate) fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if n == 0.0 {
        "0".to_owned()
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// JavaScript white space and line terminators (`\s`).
pub(crate) fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}'
}

/// `Number(string)` for the forms config values use: decimal, `0x`/`0o`/`0b`, `Infinity`.
fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(is_js_space);
    if t.is_empty() {
        return 0.0;
    }
    let radix = |p: &str, r: u32| u64::from_str_radix(p, r).map_or(f64::NAN, |v| v as f64);
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return radix(h, 16);
    }
    if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return radix(o, 8);
    }
    if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return radix(b, 2);
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let ok = t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    if ok { t.parse().unwrap_or(f64::NAN) } else { f64::NAN }
}

/// Parse a YAML document like Verdaccio's `loadYamlConfig`: an empty or comment-only document
/// is `null`, merge keys are applied, keys become strings.
pub(crate) fn parse(yaml: &str) -> Result<Js, String> {
    let yaml = yaml.strip_prefix('\u{FEFF}').unwrap_or(yaml);
    let value: Value = serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())?;
    convert(&value)
}

fn convert(v: &Value) -> Result<Js, String> {
    Ok(match v {
        Value::Null => Js::Null,
        Value::Bool(b) => Js::Bool(*b),
        Value::Number(n) => Js::Num(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => Js::Str(s.clone()),
        Value::Sequence(items) => Js::Arr(items.iter().map(convert).collect::<Result<_, _>>()?),
        Value::Tagged(t) => convert(&t.value)?,
        Value::Mapping(m) => {
            // js-yaml: merged keys land where `<<` is, explicit keys win over merged ones
            // (keeping the merged position), earlier merge sources win over later ones.
            let mut out: Vec<(String, Js)> = Vec::new();
            let mut overridable: Vec<String> = Vec::new();
            for (k, val) in m {
                if k.as_str() == Some("<<") {
                    let sources: Vec<&Value> = match val {
                        Value::Mapping(_) => vec![val],
                        Value::Sequence(items) => items.iter().collect(),
                        _ => return Err(MERGE_ERROR.to_owned()),
                    };
                    for src in sources {
                        let Js::Obj(src) = convert(src)? else {
                            return Err(MERGE_ERROR.to_owned());
                        };
                        for (sk, sv) in src.0 {
                            if !out.iter().any(|(k, _)| *k == sk) {
                                overridable.push(sk.clone());
                                out.push((sk, sv));
                            }
                        }
                    }
                    continue;
                }
                let key = convert(k)?.to_js_string();
                let val = convert(val)?;
                match out.iter().position(|(k, _)| *k == key) {
                    Some(pos) => {
                        let Some(o) = overridable.iter().position(|k| *k == key) else {
                            return Err(format!("duplicated mapping key: {key}"));
                        };
                        overridable.swap_remove(o);
                        out[pos].1 = val;
                    }
                    None => out.push((key, val)),
                }
            }
            Js::Obj(Obj(out))
        }
    })
}

const MERGE_ERROR: &str = "cannot merge mappings; the provided source object is unacceptable";

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(yaml: &str) -> Obj {
        match parse(yaml).unwrap() {
            Js::Obj(o) => o,
            other => panic!("not an object: {other:?}"),
        }
    }

    fn keys(o: &Obj) -> Vec<&str> {
        o.iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn empty_documents_are_null() {
        assert_eq!(parse("").unwrap(), Js::Null);
        assert_eq!(parse("# only a comment\n").unwrap(), Js::Null);
        assert_eq!(parse("~").unwrap(), Js::Null);
    }

    #[test]
    fn merge_keys_follow_js_yaml() {
        let o = obj("base: &b\n  x: 1\n  y: 2\nm:\n  a: 0\n  <<: *b\n  x: 9\n  z: 3\n");
        let Some(Js::Obj(m)) = o.get("m") else { panic!() };
        assert_eq!(keys(m), ["a", "x", "y", "z"]);
        assert_eq!(m.get("x"), Some(&Js::Num(9.0)));
        let o = obj("a: &a {k: 1}\nb: &b {k: 2, j: 2}\nc:\n  <<: [*a, *b]\n");
        let Some(Js::Obj(c)) = o.get("c") else { panic!() };
        assert_eq!(c.get("k"), Some(&Js::Num(1.0)));
        assert_eq!(c.get("j"), Some(&Js::Num(2.0)));
        assert!(parse("a:\n  <<: 1\n").is_err());
    }

    #[test]
    fn keys_are_strings_and_unique() {
        let o = obj("1: a\ntrue: b\n~: c\n");
        assert_eq!(keys(&o), ["1", "true", "null"]);
        assert!(parse("1: a\n'1': b\n").is_err());
    }

    #[test]
    fn js_key_order() {
        let o = obj("b: 1\n'10': 2\na: 3\n'2': 4\n'01': 5\n'4294967295': 6\n");
        let order: Vec<&str> = o.js_ordered().iter().map(|(k, _)| *k).collect();
        assert_eq!(order, ["2", "10", "b", "a", "01", "4294967295"]);
    }

    #[test]
    fn conversions() {
        assert_eq!(Js::Num(1.0).to_js_string(), "1");
        assert_eq!(Js::Num(1.5).to_js_string(), "1.5");
        assert_eq!(
            Js::Arr(vec![Js::Num(1.0), Js::Null, Js::Str("x".into())]).to_js_string(),
            "1,,x"
        );
        assert_eq!(Js::Str(" 0x10 ".into()).to_number(), 16.0);
        assert_eq!(Js::Str("".into()).to_number(), 0.0);
        assert!(Js::Str("2m".into()).to_number().is_nan());
        assert!(!Js::Str(String::new()).truthy());
        assert!(Js::Arr(Vec::new()).truthy());
    }
}
