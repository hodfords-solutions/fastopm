//! JavaScript value semantics Verdaccio's merge code relies on (truthiness, `===`, `isNil`,
//! `String(x)`, lodash `trim`), over `serde_json` values. `None` stands for `undefined`
//! (a missing key).

use serde_json::Value;

/// `\s` in a JavaScript regular expression (what `String.prototype.trim` and lodash `trim`
/// strip).
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'
            | ' '
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `Boolean(v)`.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// lodash `isNil`.
pub(crate) fn is_nil(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Null))
}

/// `a === b` for values that come from different parses (or were cloned): objects and arrays
/// are never identical.
pub(crate) fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// `String(n)` for a JSON number.
fn number_to_string(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(0.0);
    if f.fract() == 0.0 && f.abs() < 1e21 { format!("{}", f as i128) } else { format!("{f}") }
}

/// lodash `toString` (`null`/`undefined` → `""`, arrays joined with `,`, objects →
/// `[object Object]`).
pub(crate) fn lodash_to_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(n),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            items.iter().map(|i| lodash_to_string(Some(i))).collect::<Vec<_>>().join(",")
        }
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// The property key JavaScript uses for `obj[v]` (`String(v)`).
pub(crate) fn property_key(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        other => lodash_to_string(Some(other)),
    }
}

/// `x || y` over optional values.
pub(crate) fn or<'a>(x: Option<&'a Value>, y: Option<&'a Value>) -> Option<&'a Value> {
    if truthy(x) { x } else { y }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_and_equality() {
        assert!(!truthy(None));
        assert!(!truthy(Some(&json!(""))));
        assert!(!truthy(Some(&json!(0))));
        assert!(truthy(Some(&json!([]))));
        assert!(truthy(Some(&json!("0"))));
        assert!(strict_eq(None, None));
        assert!(strict_eq(Some(&json!(1)), Some(&json!(1.0))));
        assert!(!strict_eq(Some(&json!([1])), Some(&json!([1]))));
        assert!(!strict_eq(None, Some(&Value::Null)));
    }

    #[test]
    fn conversions() {
        assert_eq!(js_trim("\u{FEFF} a \n"), "a");
        assert_eq!(js_trim("\u{0085}a"), "\u{0085}a");
        assert_eq!(lodash_to_string(Some(&json!([1, null, "x"]))), "1,,x");
        assert_eq!(lodash_to_string(Some(&json!({"a": 1}))), "[object Object]");
        assert_eq!(property_key(&json!(2)), "2");
        assert_eq!(property_key(&Value::Null), "null");
    }
}
