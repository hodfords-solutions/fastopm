//! Request headers with JavaScript object semantics.
//!
//! Verdaccio builds uplink headers in a plain object (`getHeaders`, `addProxyHeaders`,
//! `applyUplinkHeaders`), and got lower-cases the names afterwards with the later key winning
//! (`lowercase-keys`). Keys are case-sensitive while the object is built, so `Accept` set by
//! Verdaccio and `accept` from the config are two entries, and whichever comes later in
//! insertion order is sent. Assigning to an existing key keeps its position. This type
//! reproduces those rules so the headers sent are the same as Verdaccio's.

use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// Ordered `key -> value` pairs, keys compared exactly (case-sensitive).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JsHeaders(Vec<(String, String)>);

impl JsHeaders {
    /// Copy of a config `headers` mapping, in order.
    pub(crate) fn from_map(map: &IndexMap<String, String>) -> Self {
        let mut out = Self::default();
        for (key, value) in map {
            out.set(key, value.clone());
        }
        out
    }

    /// `headers[key]`, `None` when absent.
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// `headers[key]` when present and truthy (a non-empty string).
    pub(crate) fn get_truthy(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }

    /// `headers[key] = value`: replaces in place, or appends a new key.
    pub(crate) fn set(&mut self, key: &str, value: String) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    /// `headers[key] = headers[key] || default`.
    pub(crate) fn set_or(&mut self, key: &str, default: &str) {
        let value = self.get_truthy(key).unwrap_or(default).to_owned();
        self.set(key, value);
    }

    /// The headers got sends: names lower-cased, the later of two keys differing only in case
    /// wins. Fails on a name or value HTTP does not allow (got throws on those too).
    pub(crate) fn to_header_map(&self) -> Result<HeaderMap, String> {
        let mut map = HeaderMap::with_capacity(self.0.len());
        for (key, value) in &self.0 {
            let name = HeaderName::from_bytes(key.to_ascii_lowercase().as_bytes())
                .map_err(|_| format!("invalid header name: {key:?}"))?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| format!("invalid value for header {key:?}"))?;
            map.insert(name, value);
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_keeps_position_and_later_case_variant_wins() {
        let mut h = JsHeaders::default();
        h.set("Accept", "application/json;".into());
        h.set("accept", "application/vnd.npm.install-v1+json".into());
        // Re-assigning `Accept` keeps it before `accept`, so `accept` still wins.
        h.set("Accept", "application/json;".into());
        let map = h.to_header_map().unwrap();
        assert_eq!(map["accept"], "application/vnd.npm.install-v1+json");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn set_or_replaces_only_falsy_values() {
        let mut h = JsHeaders::default();
        h.set("Accept", String::new());
        h.set_or("Accept", "x");
        h.set_or("User-Agent", "ua");
        h.set_or("User-Agent", "other");
        assert_eq!(h.get("Accept"), Some("x"));
        assert_eq!(h.get("User-Agent"), Some("ua"));
        assert_eq!(h.get("user-agent"), None);
    }

    #[test]
    fn invalid_values_are_reported() {
        let mut h = JsHeaders::default();
        h.set("x-bad", "a\nb".into());
        assert!(h.to_header_map().is_err());
    }
}
