//! Response bodies, byte-compatible with Verdaccio.
//!
//! Pure functions, no I/O. Input is the manifest `fastopm-store` serves (storage form, dist-tags
//! already normalized); output is the exact body bytes, `ETag` and `Content-Type`.
//!
//! Serialization is `JSON.stringify(value, undefined, '  ') + '\n'`: two-space indent, `": "`
//! after keys, empty objects/arrays as `{}`/`[]`, JavaScript string escaping (only `"`, `\\` and
//! control characters below U+0020; `\b \f \n \r \t` short forms, others `\u00xx` lower-case),
//! JavaScript number formatting (`1.0` → `1`, exponent forms as `Number.prototype.toString`).
//! The `ETag` is the quoted lower-case hex MD5 of the body (`final.ts`).
//!
//! Verdaccio reference: `packages/middleware/src/middlewares/final.ts` (serialization, ETag),
//! `packages/store/src/storage.ts` (`getPackage`, `getPackageByVersion`,
//! `convertAbbreviatedManifest`), `storage-utils.ts` (`cleanUpLinksRef` / `WHITELIST`),
//! `packages/core/tarball/src/` (tarball URL rewrite), `packages/core/url/src/index.ts`
//! (public URL).

mod json;
mod public_url;
mod semver;

use bytes::Bytes;
use fastopm_core::headers::{CONTENT_TYPE_INSTALL_V1, CONTENT_TYPE_JSON};
use fastopm_core::{Manifest, RegistryError, extract_tarball_from_url, messages};
use md5::{Digest, Md5};
use serde_json::{Map, Value};

use crate::json::{ObjectWriter, js_entries, write_object, write_string, write_value};
use crate::semver::SemVer;

/// Top-level keys kept in a full manifest response (`WHITELIST`), in their original order.
/// `_attachments: {}` is appended after them.
pub const MANIFEST_WHITELIST: [&str; 9] =
    ["_rev", "name", "versions", "dist-tags", "readme", "time", "_id", "users", "maintainers"];

/// Version fields kept in the abbreviated form, in output order (`convertAbbreviatedManifest`).
/// Absent fields are omitted (JavaScript `undefined`).
pub const ABBREVIATED_VERSION_FIELDS: [&str; 19] = [
    "name",
    "version",
    "deprecated",
    "bin",
    "dist",
    "engines",
    "funding",
    "directories",
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
    "bundleDependencies",
    "cpu",
    "os",
    "peerDependenciesMeta",
    "acceptDependencies",
    "_hasShrinkwrap",
    "hasInstallScript",
];

/// Which manifest form the client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Full document, `Content-Type: application/json; charset=utf-8`.
    Full,
    /// `application/vnd.npm.install-v1+json` ("corgi"),
    /// `Content-Type: application/vnd.npm.install-v1+json; charset=utf-8`.
    Abbreviated,
}

/// A ready response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Body bytes, ending with `\n`.
    pub body: Bytes,
    /// `ETag` header value including the double quotes.
    pub etag: String,
    /// `Content-Type` header value.
    pub content_type: &'static str,
}

/// Request facts needed to build the public base URL.
#[derive(Debug, Clone, Copy, Default)]
pub struct BaseUrlInput<'a> {
    /// `Host` header (for HTTP/2, the `:authority`), including the port if present.
    pub host: Option<&'a str>,
    /// Value of the forwarded-protocol header named by `Config::forwarded_proto_header`.
    pub forwarded_proto: Option<&'a str>,
    /// Protocol of the connection itself (`http`; `https` only behind TLS termination in-process).
    pub scheme: &'a str,
    /// `url_prefix` from config.
    pub url_prefix: Option<&'a str>,
    /// `Config::public_url` (`VERDACCIO_PUBLIC_URL`, already validated).
    pub public_url: Option<&'a str>,
}

/// Public base URL used to rewrite `dist.tarball` (`getPublicUrl` together with the `Host`
/// check of `getLocalRegistryTarballUri`). It ends in `/` except when `public_url` without a
/// path-ending `/` is used with no `url_prefix` (Verdaccio then concatenates as is).
///
/// - `host` absent or empty → `Ok(None)`: tarball URLs are left as stored.
/// - `public_url` set → `new URL(wrapPrefix(url_prefix), public_url).href` (an absolute prefix
///   replaces the path of `public_url`; an empty prefix keeps `public_url` minus its fragment).
///   `host` is not validated in this case, as in Verdaccio.
/// - otherwise `new URL(wrapPrefix(url_prefix), "<proto>://<host>").href`, where `proto` is the
///   part of `forwarded_proto` before the first `,` (when that `,` is not the first character)
///   when non-empty, else `scheme`; any value other than `http`/`https` becomes `http`.
///   `wrapPrefix` turns `p`, `/p`, `p/` into `/p/` and empty into an empty string (so the path
///   is `/`).
/// - `host` fails Verdaccio's `isHost` check, or the URL does not parse →
///   `Err(500 internal server error)` (Verdaccio throws a plain `Error`, which the error
///   middleware hides).
pub fn public_base_url(input: &BaseUrlInput<'_>) -> Result<Option<String>, RegistryError> {
    let Some(host) = input.host.filter(|h| !h.is_empty()) else {
        return Ok(None);
    };
    let prefix = public_url::wrap_prefix(input.url_prefix);
    let invalid = || RegistryError::internal(messages::INTERNAL_SERVER_ERROR);
    if let Some(public) = input.public_url {
        return public_url::resolve(&prefix, public).map(Some).ok_or_else(invalid);
    }
    if !public_url::is_host(host) {
        return Err(invalid());
    }
    let proto = public_url::web_protocol(input.forwarded_proto, input.scheme);
    public_url::resolve(&prefix, &format!("{proto}://{host}")).map(Some).ok_or_else(invalid)
}

/// `<base><name>/-/<filename>` where `filename` is the last path segment of `original_url`
/// ([`fastopm_core::extract_tarball_from_url`]); `name` is not encoded (`@scope/pkg` stays as
/// is). When `original_url` is not an absolute URL it is returned unchanged.
pub fn local_tarball_url(base: &str, name: &str, original_url: &str) -> String {
    match extract_tarball_from_url(original_url) {
        Some(filename) => format!("{base}{name}/-/{filename}"),
        None => original_url.to_owned(),
    }
}

/// `JSON.stringify(value, undefined, '  ') + '\n'` (see crate docs for the exact rules).
pub fn to_pretty_json(value: &Value) -> Bytes {
    let mut out = Vec::new();
    write_value(&mut out, value, 0);
    out.push(b'\n');
    Bytes::from(out)
}

/// `ETag` of a body: `"` + lower-case hex MD5 + `"`.
pub fn etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(body)))
}

fn rendered(mut out: Vec<u8>, variant: Variant) -> Rendered {
    out.push(b'\n');
    let etag = etag(&out);
    Rendered { body: Bytes::from(out), etag, content_type: content_type(variant) }
}

fn content_type(variant: Variant) -> &'static str {
    match variant {
        Variant::Full => CONTENT_TYPE_JSON,
        Variant::Abbreviated => CONTENT_TYPE_INSTALL_V1,
    }
}

/// `GET /:package` body.
///
/// Full: top-level keys filtered to [`MANIFEST_WHITELIST`] in source order, then
/// `"_attachments": {}`; every `versions[v].dist.tarball` rewritten with [`local_tarball_url`]
/// when `base` is `Some` (key positions unchanged).
///
/// Abbreviated: built from the full form: `{name, dist-tags, versions, modified, time}` where
/// each version keeps only [`ABBREVIATED_VERSION_FIELDS`], `modified` is `time.modified`;
/// absent values are omitted. `_rev`, `_id` and `readme` are not included.
///
/// Only string `dist.tarball` values are rewritten (Verdaccio would fail the request with a 500
/// for a truthy non-string or a relative URL; such manifests do not come out of npm registries).
pub fn render_manifest(manifest: &Manifest, variant: Variant, base: Option<&str>) -> Rendered {
    let map = manifest.as_map();
    let rewrite = base.map(|base| Rewrite { base, name: js_to_string(map.get("name")) });
    let mut out = Vec::with_capacity(4096);
    let mut obj = ObjectWriter::new(&mut out, 0);
    match variant {
        Variant::Full => {
            for (key, value) in js_entries(map) {
                if !MANIFEST_WHITELIST.contains(&key) {
                    continue;
                }
                obj.key(key);
                match (key, value, &rewrite) {
                    ("versions", Value::Object(versions), Some(rw)) => {
                        write_object(obj.out, versions, 1, |out, _, version, indent| {
                            write_version(out, version, indent, Some(rw));
                        });
                    }
                    _ => write_value(obj.out, value, 1),
                }
            }
            obj.entry("_attachments", &Value::Object(Map::new()));
        }
        Variant::Abbreviated => {
            if let Some(name) = map.get("name") {
                obj.entry("name", name);
            }
            if let Some(tags) = map.get("dist-tags") {
                obj.entry("dist-tags", tags);
            }
            obj.key("versions");
            match map.get("versions") {
                Some(Value::Object(versions)) => {
                    write_object(obj.out, versions, 1, |out, _, version, indent| {
                        write_abbreviated_version(out, version, indent, rewrite.as_ref());
                    });
                }
                _ => obj.out.extend_from_slice(b"{}"),
            }
            let time = map.get("time");
            if let Some(modified) = time.and_then(|t| t.get("modified")) {
                obj.entry("modified", modified);
            }
            if let Some(time) = time {
                obj.entry("time", time);
            }
        }
    }
    obj.finish();
    rendered(out, variant)
}

/// `dist.tarball` rewrite parameters (`convertDistVersionToLocalTarballsUrl`).
struct Rewrite<'a> {
    base: &'a str,
    /// `convertedManifest.name` as a JavaScript template literal prints it.
    name: String,
}

impl Rewrite<'_> {
    /// The new tarball URL when `dist` is an object with a non-empty string `tarball`.
    fn tarball(&self, dist: &Value) -> Option<String> {
        match dist.get("tarball") {
            Some(Value::String(t)) if !t.is_empty() && dist.is_object() => {
                Some(local_tarball_url(self.base, &self.name, t))
            }
            _ => None,
        }
    }
}

/// Writes a `dist` value, substituting `tarball` when the rewrite applies.
fn write_dist(out: &mut Vec<u8>, dist: &Value, indent: usize, rewrite: Option<&Rewrite<'_>>) {
    match (dist, rewrite.and_then(|rw| rw.tarball(dist))) {
        (Value::Object(map), Some(tarball)) => {
            write_object(out, map, indent, |out, key, value, indent| {
                if key == "tarball" {
                    write_string(out, &tarball);
                } else {
                    write_value(out, value, indent);
                }
            });
        }
        _ => write_value(out, dist, indent),
    }
}

/// Writes a version object with its `dist` rewritten.
fn write_version(out: &mut Vec<u8>, version: &Value, indent: usize, rewrite: Option<&Rewrite<'_>>) {
    match version {
        Value::Object(map) if rewrite.is_some() => {
            write_object(out, map, indent, |out, key, value, indent| {
                if key == "dist" {
                    write_dist(out, value, indent, rewrite);
                } else {
                    write_value(out, value, indent);
                }
            });
        }
        _ => write_value(out, version, indent),
    }
}

fn write_abbreviated_version(
    out: &mut Vec<u8>,
    version: &Value,
    indent: usize,
    rewrite: Option<&Rewrite<'_>>,
) {
    let mut obj = ObjectWriter::new(out, indent);
    if let Value::Object(map) = version {
        for field in ABBREVIATED_VERSION_FIELDS {
            let Some(value) = map.get(field) else { continue };
            obj.key(field);
            if field == "dist" {
                write_dist(obj.out, value, indent + 1, rewrite);
            } else {
                write_value(obj.out, value, indent + 1);
            }
        }
    }
    obj.finish();
}

/// JavaScript `String(value)` for the package name used in tarball URLs.
fn js_to_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => {
            let mut out = Vec::new();
            json::write_number(&mut out, n);
            String::from_utf8(out).unwrap_or_default()
        }
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                v => js_to_string(Some(v)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `getVersion(versions, key)` with `loose` = whether `semver.parse(key, true)` sees a string.
/// `Err(())` is the `TypeError` node-semver throws when the loose scan reaches a key that is not
/// a valid version (`compare(null)`); Verdaccio answers that with a 500.
fn get_version<'a>(
    versions: Option<&'a Value>,
    key: &str,
    loose: bool,
) -> Result<Option<&'a Value>, ()> {
    let Some(Value::Object(versions)) = versions else {
        return Ok(None);
    };
    if let Some(v) = versions.get(key).filter(|v| !v.is_null()) {
        return Ok(Some(v));
    }
    let Some(wanted) = loose.then(|| SemVer::parse_loose(key)).flatten() else {
        return Ok(None);
    };
    for (candidate, value) in js_entries(versions) {
        let parsed = SemVer::parse_loose(candidate).ok_or(())?;
        if wanted.compare(&parsed).is_eq() {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// `getPackageByVersion` lookup: the versions entry, before the tarball rewrite.
fn lookup_version<'a>(manifest: &'a Manifest, query: &str) -> Result<Option<&'a Value>, ()> {
    let map = manifest.as_map();
    let versions = map.get("versions");
    if let Some(v) = get_version(versions, query, true)? {
        return Ok(Some(v));
    }
    let Some(Value::Object(tags)) = map.get("dist-tags") else {
        return Ok(None);
    };
    match tags.get(query) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(tag)) => get_version(versions, tag, true),
        Some(other) => get_version(versions, &js_to_string(Some(other)), false),
    }
}

/// `GET /:package/:version` body: one version object with `dist.tarball` rewritten.
///
/// `query` is resolved with [`find_version`]. Not found →
/// `404 this version doesn't exist: <query>`. The body is always the full version object; only
/// `content_type` follows `variant` (Verdaccio sets the install-v1 type whenever the request
/// asked for it, even on this route).
///
/// Where Verdaccio throws a `TypeError` (the loose scan meets a key that is not a valid version,
/// or the match is `null`), the result is `500 internal server error`. A match that is a string
/// is sent raw and an array pretty-printed, like `final.ts`; other non-object matches give 500.
pub fn render_version(
    manifest: &Manifest,
    query: &str,
    variant: Variant,
    base: Option<&str>,
) -> Result<Rendered, RegistryError> {
    let internal = || RegistryError::internal(messages::INTERNAL_SERVER_ERROR);
    let found = lookup_version(manifest, query).map_err(|()| internal())?;
    let rewrite =
        base.map(|base| Rewrite { base, name: js_to_string(manifest.as_map().get("name")) });
    let mut out = Vec::new();
    match found {
        None => {
            return Err(RegistryError::not_found(format!(
                "{}: {query}",
                messages::VERSION_NOT_EXIST
            )));
        }
        Some(version @ Value::Object(_)) => write_version(&mut out, version, 0, rewrite.as_ref()),
        Some(Value::String(raw)) => {
            let body = Bytes::copy_from_slice(raw.as_bytes());
            return Ok(Rendered { etag: etag(&body), body, content_type: content_type(variant) });
        }
        Some(array @ Value::Array(_)) => write_value(&mut out, array, 0),
        Some(_) => return Err(internal()),
    }
    Ok(rendered(out, variant))
}

/// Version lookup of `getPackageByVersion`: exact key in `versions`; else the first key equal
/// to `query` under node-semver loose parsing (`semver.parse(x, true)` + `compare`, build
/// metadata ignored, keys scanned in JavaScript order); else `dist-tags[query]` looked up the
/// same way. `None` when nothing matches, when the match is not an object, or where Verdaccio
/// would throw (see [`render_version`]).
pub fn find_version<'a>(manifest: &'a Manifest, query: &str) -> Option<&'a Map<String, Value>> {
    lookup_version(manifest, query).ok().flatten().and_then(Value::as_object)
}

/// `GET /-/ping` body: `{}` + `\n`, with its ETag, `Content-Type: application/json; charset=utf-8`.
pub fn render_ping() -> Rendered {
    rendered(b"{}".to_vec(), Variant::Full)
}

/// Error body `{"error": "<message>"}` pretty-printed, using [`RegistryError::client_view`].
/// Errors carry no `ETag`; the status comes from the same `client_view`.
pub fn render_error(error: &RegistryError) -> Bytes {
    let (_, message) = error.client_view();
    let mut out = Vec::new();
    let mut obj = ObjectWriter::new(&mut out, 0);
    obj.key("error");
    write_string(obj.out, message);
    obj.finish();
    out.push(b'\n');
    Bytes::from(out)
}
