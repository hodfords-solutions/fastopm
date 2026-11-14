//! The package manifest (packument).
//!
//! One type for both forms of the document:
//!
//! - in storage: `<storage>/<name>/package.json`, compact JSON (`JSON.stringify(manifest)`), with
//!   the bookkeeping keys `_uplinks`, `_distfiles`, `_attachments`, `_rev`, `_id`;
//! - on the wire: what the server renders from it (see `fastopm-render`).
//!
//! The wrapper keeps every key in its original order (`serde_json` with `preserve_order`);
//! serializing visits keys in JavaScript order (`crate::json`), so the bytes are Verdaccio's. Accessors never reorder keys:
//! `*_mut` accessors replace a non-object value in place, or append the key when it is missing,
//! like JavaScript property assignment. To delete a key without reordering the rest use
//! `Map::shift_remove`, never `swap_remove`.
//!
//! Verdaccio reference: `packages/store/src/lib/storage-utils.ts` (`generatePackageTemplate`,
//! `normalizePackage`, `generateRevision`, `updateUpLinkMetadata`) and the `Manifest` type in
//! `packages/core/types`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::constants::{DEFAULT_REVISION, DIST_TAGS};

/// Key of the per-uplink bookkeeping object.
pub const UPLINKS_KEY: &str = "_uplinks";
/// Key of the tarball-filename → source-URL object.
pub const DISTFILES_KEY: &str = "_distfiles";
/// Key of the attachments object (always `{}` for cached packages).
pub const ATTACHMENTS_KEY: &str = "_attachments";
/// Key of the revision string.
pub const REV_KEY: &str = "_rev";
/// Key of the CouchDB id (the package name).
pub const ID_KEY: &str = "_id";
/// Key of the versions object.
pub const VERSIONS_KEY: &str = "versions";
/// Key of the time object.
pub const TIME_KEY: &str = "time";

/// A package manifest: an order-preserving JSON object.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Manifest(Map<String, Value>);

/// One entry of `_uplinks`: `{"etag": "...", "fetched": <ms since epoch>}`.
///
/// `etag` is omitted when the uplink sent none (JavaScript `undefined`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UplinkMeta {
    /// `ETag` of the last 200 response from this uplink.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub etag: Option<String>,
    /// Milliseconds since the Unix epoch when this uplink last confirmed the manifest
    /// (200 or 304). `None` when missing or not a number.
    #[serde(default, deserialize_with = "lenient_u64")]
    pub fetched: Option<u64>,
}

/// One entry of `_distfiles`: where a tarball can be downloaded from.
///
/// Serialized as `{"url": ..., "sha": ..., "registry": ...}` in that order; `sha` and
/// `registry` are omitted when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistFile {
    /// Absolute tarball URL on the uplink (`versions[v].dist.tarball` as received).
    pub url: String,
    /// `versions[v].dist.shasum` as received.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sha: Option<String>,
    /// Name of the uplink the URL belongs to, set only when the URL host equals the uplink host
    /// (`updateUplinkToRemoteProtocol`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub registry: Option<String>,
}

fn lenient_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Number(n) => {
            n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        }
        _ => None,
    })
}

/// Next revision after `rev` (`generateRevision`): `<n + 1>-<16 random hex chars>`, where `n`
/// is the number before the first `-` (0 when it is not a number).
pub fn next_revision(rev: &str) -> String {
    let head = rev.split('-').next().unwrap_or("").trim();
    let n = if head.is_empty() {
        0.0
    } else {
        head.parse::<f64>().ok().filter(|n| n.is_finite()).unwrap_or(0.0)
    };
    let next = n + 1.0;
    let number = if next.fract() == 0.0 && next.abs() < 1e21 {
        format!("{}", next as i128)
    } else {
        format!("{next}")
    };
    format!("{number}-{}", crate::util::random_hex(8))
}

fn ensure_object<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let slot = map.entry(key.to_owned()).or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    match slot {
        Value::Object(obj) => obj,
        _ => unreachable!("slot was just set to an object"),
    }
}

impl Manifest {
    /// Wrap a JSON object.
    pub fn from_map(map: Map<String, Value>) -> Self {
        Self(map)
    }

    /// Parse a manifest. Fails unless the input is a JSON object.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Storage form: compact JSON, the same bytes as Verdaccio's `JSON.stringify(manifest)`
    /// (JavaScript key order: integer-like keys first; JavaScript number forms).
    pub fn to_compact_vec(&self) -> Vec<u8> {
        // sized up front: a multi-megabyte buffer grown by doubling would briefly need 3×
        let mut out = Vec::with_capacity(self.compact_len_estimate());
        crate::json::write_compact_object(&mut out, &self.0);
        out
    }

    /// About the length of [`Manifest::to_compact_vec`] (escapes included as a 3% margin).
    pub fn compact_len_estimate(&self) -> usize {
        let n = crate::json::compact_object_len_estimate(&self.0);
        n + n / 32 + 64
    }

    /// Heap bytes of the parsed tree ([`crate::memory::value_heap_bytes`]).
    pub fn heap_bytes(&self) -> usize {
        crate::memory::map_heap_bytes(&self.0)
    }

    /// The underlying object.
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    /// The underlying object, mutable.
    pub fn as_map_mut(&mut self) -> &mut Map<String, Value> {
        &mut self.0
    }

    /// Unwrap into the underlying object.
    pub fn into_map(self) -> Map<String, Value> {
        self.0
    }

    /// New empty package (`generatePackageTemplate`): keys `name`, `versions`, `time`, `users`,
    /// `dist-tags`, `maintainers`, `_uplinks`, `_distfiles`, `_attachments`, `_rev` (empty string)
    /// in that order.
    pub fn template(name: &str) -> Self {
        let mut m = Map::new();
        m.insert("name".into(), Value::String(name.to_owned()));
        m.insert(VERSIONS_KEY.into(), Value::Object(Map::new()));
        m.insert(TIME_KEY.into(), Value::Object(Map::new()));
        m.insert("users".into(), Value::Object(Map::new()));
        m.insert(DIST_TAGS.into(), Value::Object(Map::new()));
        m.insert("maintainers".into(), Value::Array(Vec::new()));
        m.insert(UPLINKS_KEY.into(), Value::Object(Map::new()));
        m.insert(DISTFILES_KEY.into(), Value::Object(Map::new()));
        m.insert(ATTACHMENTS_KEY.into(), Value::Object(Map::new()));
        m.insert(REV_KEY.into(), Value::String(String::new()));
        Self(m)
    }

    /// Structural part of `normalizePackage`: make `versions`, `dist-tags`, `_distfiles`,
    /// `_attachments`, `_uplinks`, `time` objects (in that order when appended), set `_rev` to
    /// [`DEFAULT_REVISION`] when it is not a string, and `_id` to `name` when it is not a string
    /// (left absent when there is no string `name`).
    ///
    /// Dist-tag normalization (`normalizeDistTags`, which needs semver) is not done here; it
    /// belongs to `fastopm-store`.
    pub fn normalize(&mut self) {
        for key in [VERSIONS_KEY, DIST_TAGS, DISTFILES_KEY, ATTACHMENTS_KEY, UPLINKS_KEY, TIME_KEY]
        {
            ensure_object(&mut self.0, key);
        }
        if !self.0.get(REV_KEY).is_some_and(Value::is_string) {
            self.0.insert(REV_KEY.into(), Value::String(DEFAULT_REVISION.into()));
        }
        if !self.0.get(ID_KEY).is_some_and(Value::is_string) {
            if let Some(name) = self.name().map(str::to_owned) {
                self.0.insert(ID_KEY.into(), Value::String(name));
            }
        }
    }

    /// `name`, when it is a string.
    pub fn name(&self) -> Option<&str> {
        self.0.get("name").and_then(Value::as_str)
    }

    /// Any top-level key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// Set a top-level key (keeps its position when it already exists, appends otherwise).
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        self.0.insert(key.into(), value)
    }

    fn object(&self, key: &str) -> Option<&Map<String, Value>> {
        self.0.get(key).and_then(Value::as_object)
    }

    /// `versions`, when it is an object.
    pub fn versions(&self) -> Option<&Map<String, Value>> {
        self.object(VERSIONS_KEY)
    }

    /// `versions`, created (or replaced by `{}` when not an object) if needed.
    pub fn versions_mut(&mut self) -> &mut Map<String, Value> {
        ensure_object(&mut self.0, VERSIONS_KEY)
    }

    /// `versions[version]`, when both are objects. Exact key lookup, no semver matching.
    pub fn version(&self, version: &str) -> Option<&Map<String, Value>> {
        self.versions()?.get(version)?.as_object()
    }

    /// `dist-tags`, when it is an object.
    pub fn dist_tags(&self) -> Option<&Map<String, Value>> {
        self.object(DIST_TAGS)
    }

    /// `dist-tags`, created if needed.
    pub fn dist_tags_mut(&mut self) -> &mut Map<String, Value> {
        ensure_object(&mut self.0, DIST_TAGS)
    }

    /// `time`, when it is an object.
    pub fn time(&self) -> Option<&Map<String, Value>> {
        self.object(TIME_KEY)
    }

    /// `time`, created if needed.
    pub fn time_mut(&mut self) -> &mut Map<String, Value> {
        ensure_object(&mut self.0, TIME_KEY)
    }

    /// `_uplinks`, when it is an object.
    pub fn uplinks(&self) -> Option<&Map<String, Value>> {
        self.object(UPLINKS_KEY)
    }

    /// `_uplinks`, created if needed.
    pub fn uplinks_mut(&mut self) -> &mut Map<String, Value> {
        ensure_object(&mut self.0, UPLINKS_KEY)
    }

    /// `_distfiles`, when it is an object.
    pub fn distfiles(&self) -> Option<&Map<String, Value>> {
        self.object(DISTFILES_KEY)
    }

    /// `_distfiles`, created if needed.
    pub fn distfiles_mut(&mut self) -> &mut Map<String, Value> {
        ensure_object(&mut self.0, DISTFILES_KEY)
    }

    /// `_uplinks[uplink]` as a typed record, when it is an object.
    pub fn uplink_meta(&self, uplink: &str) -> Option<UplinkMeta> {
        let value = self.uplinks()?.get(uplink)?;
        if !value.is_object() {
            return None;
        }
        serde_json::from_value(value.clone()).ok()
    }

    /// Set `_uplinks[uplink]` (`updateUpLinkMetadata`).
    pub fn set_uplink_meta(&mut self, uplink: &str, meta: &UplinkMeta) {
        let value = serde_json::to_value(meta).expect("UplinkMeta always serializes");
        self.uplinks_mut().insert(uplink.to_owned(), value);
    }

    /// `_distfiles[filename]` as a typed record, when it is an object with a string `url`.
    pub fn distfile(&self, filename: &str) -> Option<DistFile> {
        let value = self.distfiles()?.get(filename)?;
        serde_json::from_value(value.clone()).ok()
    }

    /// Set `_distfiles[filename]`.
    pub fn set_distfile(&mut self, filename: &str, distfile: &DistFile) {
        let value = serde_json::to_value(distfile).expect("DistFile always serializes");
        self.distfiles_mut().insert(filename.to_owned(), value);
    }

    /// `_rev`, when it is a string.
    pub fn rev(&self) -> Option<&str> {
        self.0.get(REV_KEY).and_then(Value::as_str)
    }

    /// Set `_rev`.
    pub fn set_rev(&mut self, rev: impl Into<String>) {
        self.0.insert(REV_KEY.into(), Value::String(rev.into()));
    }

    /// `setDefaultRevision`: `_rev` becomes [`next_revision`] of the current one
    /// ([`DEFAULT_REVISION`] when absent). Verdaccio calls this before every write of
    /// `package.json`.
    pub fn bump_revision(&mut self) {
        let current = self.rev().unwrap_or(DEFAULT_REVISION).to_owned();
        self.set_rev(next_revision(&current));
    }
}

impl From<Map<String, Value>> for Manifest {
    fn from(map: Map<String, Value>) -> Self {
        Self(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(m: &Manifest) -> Vec<&str> {
        m.as_map().keys().map(String::as_str).collect()
    }

    #[test]
    fn template_matches_verdaccio() {
        let m = Manifest::template("foo");
        assert_eq!(
            String::from_utf8(m.to_compact_vec()).unwrap(),
            r#"{"name":"foo","versions":{},"time":{},"users":{},"dist-tags":{},"maintainers":[],"_uplinks":{},"_distfiles":{},"_attachments":{},"_rev":""}"#
        );
    }

    #[test]
    fn from_slice_requires_object() {
        assert!(Manifest::from_slice(b"[]").is_err());
        assert!(Manifest::from_slice(b"\"x\"").is_err());
        assert!(Manifest::from_slice(b"{\"name\":\"x\"}").is_ok());
    }

    #[test]
    fn preserves_key_order_round_trip() {
        let src =
            r#"{"z":1,"name":"a","_rev":"3-abc","versions":{"2.0.0":{},"1.0.0":{}},"a":[1,2]}"#;
        let m = Manifest::from_slice(src.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(m.to_compact_vec()).unwrap(), src);
    }

    #[test]
    fn normalize_appends_missing_keys_and_replaces_in_place() {
        let mut m = Manifest::from_map(
            json!({"name": "a", "time": "bad", "versions": {"1.0.0": {}}})
                .as_object()
                .unwrap()
                .clone(),
        );
        m.normalize();
        assert_eq!(
            keys(&m),
            [
                "name",
                "time",
                "versions",
                "dist-tags",
                "_distfiles",
                "_attachments",
                "_uplinks",
                "_rev",
                "_id"
            ]
        );
        assert_eq!(m.get("time"), Some(&json!({})));
        assert_eq!(m.rev(), Some(DEFAULT_REVISION));
        assert_eq!(m.get("_id"), Some(&json!("a")));
    }

    #[test]
    fn normalize_without_name_leaves_id_absent() {
        let mut m = Manifest::default();
        m.normalize();
        assert!(m.get("_id").is_none());
    }

    #[test]
    fn accessors() {
        let mut m = Manifest::template("a");
        m.versions_mut().insert("1.0.0".into(), json!({"name": "a", "version": "1.0.0"}));
        m.dist_tags_mut().insert("latest".into(), json!("1.0.0"));
        m.time_mut().insert("modified".into(), json!("2020-01-01T00:00:00.000Z"));
        assert_eq!(m.name(), Some("a"));
        assert_eq!(m.version("1.0.0").unwrap()["version"], json!("1.0.0"));
        assert!(m.version("2.0.0").is_none());
        assert_eq!(m.dist_tags().unwrap()["latest"], json!("1.0.0"));
        assert_eq!(m.time().unwrap().len(), 1);
    }

    #[test]
    fn uplink_meta_round_trip() {
        let mut m = Manifest::template("a");
        assert!(m.uplink_meta("npmjs").is_none());
        m.set_uplink_meta("npmjs", &UplinkMeta { etag: Some("\"e\"".into()), fetched: Some(42) });
        assert_eq!(m.uplinks().unwrap()["npmjs"], json!({"etag": "\"e\"", "fetched": 42}));
        assert_eq!(
            m.uplink_meta("npmjs"),
            Some(UplinkMeta { etag: Some("\"e\"".into()), fetched: Some(42) })
        );
        m.set_uplink_meta("other", &UplinkMeta { etag: None, fetched: Some(1) });
        assert_eq!(m.uplinks().unwrap()["other"], json!({"fetched": 1}));
        m.uplinks_mut().insert("bad".into(), json!({"fetched": "x"}));
        assert_eq!(m.uplink_meta("bad"), Some(UplinkMeta { etag: None, fetched: None }));
        m.uplinks_mut().insert("str".into(), json!("x"));
        assert_eq!(m.uplink_meta("str"), None);
    }

    #[test]
    fn distfile_round_trip_and_key_order() {
        let mut m = Manifest::template("a");
        let d = DistFile {
            url: "https://r/a/-/a-1.0.0.tgz".into(),
            sha: Some("abc".into()),
            registry: Some("npmjs".into()),
        };
        m.set_distfile("a-1.0.0.tgz", &d);
        assert_eq!(
            serde_json::to_string(&m.distfiles().unwrap()["a-1.0.0.tgz"]).unwrap(),
            r#"{"url":"https://r/a/-/a-1.0.0.tgz","sha":"abc","registry":"npmjs"}"#
        );
        assert_eq!(m.distfile("a-1.0.0.tgz"), Some(d));
        assert_eq!(m.distfile("missing.tgz"), None);
        m.distfiles_mut().insert("nourl.tgz".into(), json!({"sha": "x"}));
        assert_eq!(m.distfile("nourl.tgz"), None);
    }

    #[test]
    fn revisions() {
        let r = next_revision("3-abcdef");
        assert!(r.starts_with("4-"), "{r}");
        assert_eq!(r.len(), 2 + 16);
        assert!(next_revision("").starts_with("1-"));
        assert!(next_revision("garbage").starts_with("1-"));
        assert!(next_revision(DEFAULT_REVISION).starts_with("1-"));
        assert!(next_revision("1.5-x").starts_with("2.5-"));

        let mut m = Manifest::template("a");
        m.bump_revision();
        assert!(m.rev().unwrap().starts_with("1-"));
        m.bump_revision();
        assert!(m.rev().unwrap().starts_with("2-"));
        let mut m = Manifest::default();
        m.bump_revision();
        assert!(m.rev().unwrap().starts_with("1-"));
    }
}
