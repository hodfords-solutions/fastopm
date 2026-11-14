//! Ports of Verdaccio's manifest functions (`packages/store/src/lib/storage-utils.ts`,
//! `versions-utils.ts`, `validation-utils.ts:normalizeMetadata` and the merge steps of
//! `storage.ts`). They work on plain JSON so key order and value shapes match what Verdaccio
//! writes.
//!
//! Where Verdaccio would throw a `TypeError` on malformed input (for example `semver.lte` on an
//! invalid dist-tag), the function returns [`Malformed`]; the caller turns it into the same
//! outcome Verdaccio reaches (an uplink error, or `500 internal server error`).

use fastopm_core::constants::{DIST_TAGS, LATEST};
use fastopm_core::manifest::{TIME_KEY, UPLINKS_KEY, VERSIONS_KEY};
use fastopm_core::{Manifest, UplinkMeta, extract_tarball_from_url};
use serde_json::{Map, Value};

use crate::js::{is_nil, js_trim, lodash_to_string, or, property_key, strict_eq, truthy};
use crate::semver;

/// Verdaccio would throw here (malformed manifest data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Malformed;

const MAINTAINERS: &str = "maintainers";
const README: &str = "readme";
/// `UPSTREAM_MAINTAINER_PREFIX`.
const UPSTREAM_MAINTAINER_PREFIX: &str = "$uplink:";
/// Dist-tags tried for a readme when `latest` has none (`getLatestReadme`).
const README_TAG_PRIORITY: [&str; 6] = ["next", "beta", "alpha", "test", "dev", "canary"];

/// `normalizeDistTags` as a pure function: the dist-tags it would leave, or `None` when it
/// would change nothing (or `dist-tags` is not an object).
///
/// - no truthy `latest` → `latest` = highest version key that parses (loose);
/// - array tags: empty → removed, else the highest valid entry (left as is when none is valid);
/// - string tags that do not parse (loose) → removed; other values untouched.
pub(crate) fn normalized_dist_tags(m: &Manifest) -> Option<Map<String, Value>> {
    let tags = m.dist_tags()?;
    let mut out = tags.clone();
    if !truthy(out.get(LATEST)) {
        let keys = m.versions().map(|v| v.keys().map(String::as_str).collect::<Vec<_>>());
        let sorted = semver::sort_versions_and_filter_invalid(keys.unwrap_or_default());
        if let Some(highest) = sorted.last() {
            out.insert(LATEST.to_owned(), Value::String((*highest).to_owned()));
        }
    }
    let keys: Vec<String> = out.keys().cloned().collect();
    for key in keys {
        match out.get(&key) {
            Some(Value::Array(items)) if items.is_empty() => {
                out.shift_remove(&key);
            }
            Some(Value::Array(items)) => {
                let sorted = semver::sort_versions_and_filter_invalid(
                    items.iter().filter_map(Value::as_str),
                );
                if let Some(highest) = sorted.last().map(|s| (*s).to_owned()) {
                    out.insert(key, Value::String(highest));
                }
            }
            Some(Value::String(s)) if semver::parse(s, true).is_none() => {
                out.shift_remove(&key);
            }
            _ => {}
        }
    }
    (out != *tags).then_some(out)
}

/// `normalizeDistTags` in place. Returns whether it changed anything.
pub(crate) fn normalize_dist_tags(m: &mut Manifest) -> bool {
    match normalized_dist_tags(m) {
        Some(tags) => {
            m.insert(DIST_TAGS, Value::Object(tags));
            true
        }
        None => false,
    }
}

/// `normalizePackage`: structural keys, `_rev`, `_id`, then `normalizeDistTags`.
pub(crate) fn normalize_package(m: &mut Manifest) {
    m.normalize();
    normalize_dist_tags(m);
}

/// `getLatestReadme`.
pub(crate) fn get_latest_readme(m: &Manifest) -> String {
    let versions = m.get(VERSIONS_KEY).and_then(Value::as_object);
    let tags = m.get(DIST_TAGS).and_then(Value::as_object);
    let tag_readme = |tag: &str| -> Option<&Value> {
        let target = tags?.get(tag).filter(|v| truthy(Some(v)))?;
        versions?.get(&property_key(target))?.as_object()?.get(README)
    };
    let to_trimmed = |v: Option<&Value>| -> Option<String> {
        truthy(v).then(|| js_trim(&lodash_to_string(v)).to_owned())
    };

    let mut readme = to_trimmed(or(m.get(README), tag_readme(LATEST))).unwrap_or_default();
    for tag in README_TAG_PRIORITY {
        if !readme.is_empty() {
            break;
        }
        readme = to_trimmed(tag_readme(tag)).unwrap_or_default();
    }
    readme
}

/// `normalizeContributors`.
fn normalize_contributors(v: Option<&Value>) -> Value {
    match v {
        None | Some(Value::Null) => Value::Array(Vec::new()),
        Some(x) if truthy(Some(x)) && !x.is_array() => Value::Array(vec![x.clone()]),
        Some(Value::String(s)) => {
            let mut person = Map::new();
            person.insert("name".into(), Value::String(s.clone()));
            Value::Array(vec![Value::Object(person)])
        }
        Some(x) => x.clone(),
    }
}

fn scope_upstream_maintainer_name(origin: &str, name: &str) -> String {
    if name.starts_with(UPSTREAM_MAINTAINER_PREFIX) {
        name.to_owned()
    } else {
        format!("{UPSTREAM_MAINTAINER_PREFIX}{origin}:{name}")
    }
}

/// `scopeUpstreamMaintainers`: `name` → `$uplink:<uplink>:<name>` (unless already scoped).
pub(crate) fn scope_upstream_maintainers(list: &[Value], uplink: &str) -> Vec<Value> {
    let origin = if uplink.is_empty() { "remote" } else { uplink };
    list.iter()
        .map(|m| match m {
            Value::String(s) => Value::String(scope_upstream_maintainer_name(origin, s)),
            other => {
                let name = other.get("name").and_then(Value::as_str).unwrap_or("");
                let scoped = scope_upstream_maintainer_name(origin, name);
                // `{ ...maintainer, name }`
                let mut obj = match other {
                    Value::Object(o) => o.clone(),
                    Value::Array(items) => {
                        items.iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect()
                    }
                    _ => Map::new(),
                };
                obj.insert("name".into(), Value::String(scoped));
                Value::Object(obj)
            }
        })
        .collect()
}

/// `(value ?? []).length === 0`.
fn maintainers_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Object(o)) => o.get("length").and_then(Value::as_f64).is_some_and(|n| n == 0.0),
        Some(_) => false,
    }
}

/// Non-empty `maintainers` array of an uplink manifest, if any.
fn remote_maintainers(m: &Manifest) -> Option<&Vec<Value>> {
    m.get(MAINTAINERS).and_then(Value::as_array).filter(|a| !a.is_empty())
}

/// What `mergeCacheRemoteMetadata` does once the uplink answered 200, on a copy of the local
/// manifest (or a template): `normalizeMetadata` (whose name assertion fails when the local
/// file is for another name), `updateUpLinkMetadata`, `mergeUplinkTimeIntoLocalNext`,
/// `mergeVersions` and the maintainers scoping.
pub(crate) fn merge_cache_remote(
    mut cache: Manifest,
    remote: &Manifest,
    name: &str,
    uplink: &str,
    etag: Option<String>,
    now_ms: u64,
) -> Result<Manifest, Malformed> {
    // normalizeMetadata
    if cache.name() != Some(name) {
        return Err(Malformed);
    }
    let map = cache.as_map_mut();
    if !map.get(DIST_TAGS).is_some_and(Value::is_object) {
        map.insert(DIST_TAGS.into(), Value::Object(Map::new()));
    }
    if !map.get(MAINTAINERS).is_some_and(Value::is_array) {
        map.insert(MAINTAINERS.into(), Value::Array(Vec::new()));
    }
    if !map.get(VERSIONS_KEY).is_some_and(Value::is_object) {
        map.insert(VERSIONS_KEY.into(), Value::Object(Map::new()));
    }
    if !map.get(TIME_KEY).is_some_and(Value::is_object) {
        map.insert(TIME_KEY.into(), Value::Object(Map::new()));
    }

    // updateUpLinkMetadata
    cache.set_uplink_meta(uplink, &UplinkMeta { etag, fetched: Some(now_ms) });

    // mergeUplinkTimeIntoLocalNext: remote wins on conflicts
    if let Some(Value::Object(remote_time)) = remote.get(TIME_KEY) {
        let time = cache.time_mut();
        for (k, v) in remote_time {
            time.insert(k.clone(), v.clone());
        }
    }

    merge_versions(&mut cache, remote)?;

    if maintainers_empty(cache.get(MAINTAINERS)) {
        if let Some(list) = remote_maintainers(remote) {
            let scoped = scope_upstream_maintainers(list, uplink);
            cache.insert(MAINTAINERS, Value::Array(scoped));
        }
    }
    Ok(cache)
}

/// `mergeVersions`: add versions missing locally (refreshing `deprecated` on known ones), add
/// missing `time` keys, move dist-tags forward (`semver.lte`) and take the remote readme when
/// `latest` moves.
pub(crate) fn merge_versions(cache: &mut Manifest, remote: &Manifest) -> Result<(), Malformed> {
    if let Some(Value::Object(remote_versions)) = remote.get(VERSIONS_KEY) {
        let versions = cache.versions_mut();
        for (id, remote_version) in remote_versions {
            match versions.get_mut(id) {
                None => {
                    versions.insert(id.clone(), remote_version.clone());
                }
                Some(local_version) => {
                    // `versions[i].deprecated` throws on null
                    if remote_version.is_null() {
                        return Err(Malformed);
                    }
                    let deprecated = remote_version.get("deprecated");
                    if truthy(deprecated) {
                        match local_version {
                            Value::Object(o) => {
                                o.insert(
                                    "deprecated".into(),
                                    deprecated.cloned().unwrap_or_default(),
                                );
                            }
                            // a property on an array is not serialized
                            Value::Array(_) => {}
                            // assigning a property of null or a primitive throws (strict mode)
                            _ => return Err(Malformed),
                        }
                    }
                }
            }
        }
    }

    if let Some(Value::Object(remote_time)) = remote.get(TIME_KEY) {
        let time = cache.time_mut();
        for (k, v) in remote_time {
            if !time.contains_key(k) {
                time.insert(k.clone(), v.clone());
            }
        }
    }

    if let Some(Value::Object(remote_tags)) = remote.get(DIST_TAGS) {
        for (tag, remote_value) in remote_tags {
            let local_value = cache.dist_tags().and_then(|t| t.get(tag));
            if strict_eq(local_value, Some(remote_value)) {
                continue;
            }
            let assign = if !truthy(local_value) {
                true
            } else {
                // `semver.lte` throws on anything but two valid (strict) version strings
                let (Some(Value::String(a)), Value::String(b)) = (local_value, remote_value) else {
                    return Err(Malformed);
                };
                semver::lte(a, b).ok_or(Malformed)?
            };
            if assign {
                cache.dist_tags_mut().insert(tag.clone(), remote_value.clone());
                if tag == LATEST {
                    // `{ ..._cacheManifest, readme: remoteManifest.readme }`
                    match remote.get(README) {
                        Some(r) => {
                            cache.insert(README, r.clone());
                        }
                        None => {
                            cache.as_map_mut().shift_remove(README);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// `updateUplinkToRemoteProtocol`: when the tarball host (with port) is the uplink host, use the
/// uplink protocol and record the uplink in `registry`.
fn update_uplink_to_remote_protocol(hash: &mut Map<String, Value>, key: &str, uplink_url: &str) {
    let Some(tarball) = hash.get("url").and_then(Value::as_str) else {
        return;
    };
    let (Ok(mut tarball), Ok(uplink)) = (url::Url::parse(tarball), url::Url::parse(uplink_url))
    else {
        return;
    };
    if tarball.host_str() == uplink.host_str() && tarball.port() == uplink.port() {
        // WHATWG ignores a protocol change between special and non-special schemes
        let _ = tarball.set_scheme(uplink.scheme());
        hash.insert("registry".into(), Value::String(key.to_owned()));
        hash.insert("url".into(), Value::String(tarball.as_str().to_owned()));
    }
}

/// The uplink side of `updateVersionsNext`: the merged manifest, the `_uplinks` it carries and
/// the uplink it came from.
pub(crate) struct SyncSource<'a> {
    /// The merged (or local) manifest.
    pub manifest: &'a Manifest,
    /// Its `_uplinks` (kept apart so a 304 refresh does not need a copy of the whole manifest).
    pub uplinks: &'a Map<String, Value>,
    /// `syncedUplinkName`.
    pub uplink_name: &'a str,
    /// `config.uplinks[uplink_name].url`, when that uplink is configured.
    pub uplink_url: Option<&'a str>,
}

/// `updateVersionsNext` on the normalized disk copy (`readCreatePackage`). Returns the updated
/// manifest and whether it changed (then the caller bumps `_rev` and writes it).
pub(crate) fn update_versions_next(
    mut cache: Manifest,
    remote: &SyncSource<'_>,
) -> Result<(Manifest, bool), Malformed> {
    let src = remote.manifest;
    let mut change = false;

    let readme = Value::String(get_latest_readme(src));
    if !strict_eq(src.get(README), Some(&readme)) {
        change = true;
    }
    cache.insert(README, readme);

    if let Some(Value::Object(remote_versions)) = src.get(VERSIONS_KEY) {
        for (id, remote_version) in remote_versions {
            if !is_nil(cache.versions().and_then(|v| v.get(id))) {
                continue;
            }
            let mut version = remote_version.clone();
            match &mut version {
                Value::Object(o) => {
                    // cleanUpReadme (no `publish.keep_readmes` in fastopm)
                    o.insert(README.into(), Value::String(String::new()));
                    let contributors = normalize_contributors(o.get("contributors"));
                    o.insert("contributors".into(), contributors);
                }
                Value::Array(_) => {}
                // `version.contributors` on null, or a property write on a primitive
                _ => return Err(Malformed),
            }
            change = true;

            let dist = version.as_object().and_then(|o| o.get("dist")).and_then(Value::as_object);
            let tarball = dist.and_then(|d| d.get("tarball"));
            if truthy(tarball) {
                let url = tarball.and_then(Value::as_str).ok_or(Malformed)?;
                let filename = extract_tarball_from_url(url).ok_or(Malformed)?;
                if is_nil(cache.distfiles().and_then(|d| d.get(&filename))) {
                    let mut hash = Map::new();
                    hash.insert("url".into(), Value::String(url.to_owned()));
                    if let Some(sha) = dist.and_then(|d| d.get("shasum")) {
                        hash.insert("sha".into(), sha.clone());
                    }
                    if let Some(uplink_url) = remote.uplink_url {
                        update_uplink_to_remote_protocol(&mut hash, remote.uplink_name, uplink_url);
                    }
                    cache.distfiles_mut().insert(filename, Value::Object(hash));
                }
            }
            cache.versions_mut().insert(id.clone(), version);
        }
    }

    if maintainers_empty(cache.get(MAINTAINERS)) {
        if let Some(list) = remote_maintainers(src) {
            let scoped = scope_upstream_maintainers(list, remote.uplink_name);
            cache.insert(MAINTAINERS, Value::Array(scoped));
            change = true;
        }
    }

    if let Some(Value::Object(remote_tags)) = src.get(DIST_TAGS) {
        for (tag, value) in remote_tags {
            let local = cache.dist_tags().and_then(|t| t.get(tag));
            if !truthy(local) || !strict_eq(local, Some(value)) {
                change = true;
                cache.dist_tags_mut().insert(tag.clone(), value.clone());
            }
        }
    }

    for (up, value) in remote.uplinks {
        let local = cache.uplinks().and_then(|u| u.get(up));
        let need_change = match local {
            Some(Value::Object(local)) => {
                if value.is_null() {
                    return Err(Malformed);
                }
                !strict_eq(value.get("etag"), local.get("etag"))
                    || !strict_eq(value.get("fetched"), local.get("fetched"))
            }
            _ => true,
        };
        if need_change {
            change = true;
            cache.uplinks_mut().insert(up.clone(), value.clone());
        }
    }

    if let Some(time) = src.get(TIME_KEY) {
        if cache.get(TIME_KEY) != Some(time) {
            cache.insert(TIME_KEY, time.clone());
            change = true;
        }
    }

    Ok((cache, change))
}

/// Whether [`update_versions_next`] would report a change (or fail) when the uplink side is the
/// local manifest itself (a fresh `maxage` hit) and the disk still holds that manifest. Lets the
/// hot path skip re-reading `package.json` without changing what gets written.
pub(crate) fn would_change_self(m: &Manifest) -> bool {
    let readme = Value::String(get_latest_readme(m));
    if !strict_eq(m.get(README), Some(&readme)) {
        return true;
    }
    if m.versions().is_some_and(|v| v.values().any(Value::is_null)) {
        return true;
    }
    if m.dist_tags()
        .is_some_and(|t| t.values().any(|v| !truthy(Some(v)) || !strict_eq(Some(v), Some(v))))
    {
        return true;
    }
    m.uplinks().is_some_and(|u| {
        u.values().any(|r| {
            !r.is_object()
                || !strict_eq(r.get("etag"), r.get("etag"))
                || !strict_eq(r.get("fetched"), r.get("fetched"))
        })
    })
}

/// Where a tarball can be downloaded from (`lookupDistFile` result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DistSource {
    /// Absolute tarball URL.
    pub url: String,
    /// `_distfiles[f].registry`, when a string.
    pub registry: Option<String>,
}

/// `tarballMatchesFilename`.
fn tarball_matches_filename(tarball: &str, filename: &str) -> bool {
    let path = tarball.split(['?', '#']).next().unwrap_or("");
    path.strip_suffix(filename).is_some_and(|head| head.ends_with('/'))
}

/// `distFileFromVersion`.
fn dist_from_version(version: Option<&Value>, filename: &str) -> Option<DistSource> {
    let tarball = version?.get("dist")?.get("tarball")?.as_str()?;
    tarball_matches_filename(tarball, filename)
        .then(|| DistSource { url: tarball.to_owned(), registry: None })
}

/// `lookupDistFile`: `_distfiles[filename]`, else the `dist.tarball` of the version the file
/// name suggests (`<basename>-<version>.tgz`), else of the first version whose tarball path ends
/// in `/<filename>`. A `_distfiles` record without a string `url` is unusable (Verdaccio then
/// refreshes the manifest, and fails the download).
pub(crate) fn lookup_dist_file(m: &Manifest, filename: &str) -> Option<DistSource> {
    if let Some(record) = m.distfiles().and_then(|d| d.get(filename)).filter(|r| !r.is_null()) {
        let url = record.get("url")?.as_str()?;
        let registry = record.get("registry").and_then(Value::as_str).map(str::to_owned);
        return Some(DistSource { url: url.to_owned(), registry });
    }

    let versions = m.versions()?;
    if let Some(basename) = m.name().map(|n| n.rsplit('/').next().unwrap_or(n)) {
        let prefix_len = basename.len() + 1;
        if !basename.is_empty()
            && filename.starts_with(basename)
            && filename[basename.len()..].starts_with('-')
            && filename.ends_with(".tgz")
            && filename.len() >= prefix_len + 4
        {
            let id = &filename[prefix_len..filename.len() - 4];
            if let Some(found) = dist_from_version(versions.get(id), filename) {
                return Some(found);
            }
        }
    }
    versions.values().find_map(|v| dist_from_version(Some(v), filename))
}

/// `Boolean(manifest._distfiles?.[filename])`: whether the URL was recorded from an uplink.
pub(crate) fn has_distfile_record(m: &Manifest, filename: &str) -> bool {
    truthy(m.distfiles().and_then(|d| d.get(filename)))
}

/// The `fetched` timestamp of `_uplinks[uplink]` when it is an object (`upLinkMeta.fetched`),
/// coerced to a number like `Date.now() - fetched`.
pub(crate) fn uplink_fetched(uplinks: &Map<String, Value>, uplink: &str) -> Option<f64> {
    let meta = uplinks.get(uplink)?.as_object()?;
    let fetched = meta.get("fetched");
    if !truthy(fetched) {
        return None;
    }
    match fetched? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = js_trim(s);
            if t.is_empty() { Some(0.0) } else { t.parse().ok() }
        }
        Value::Bool(true) => Some(1.0),
        _ => None,
    }
}

/// The etag sent as `If-None-Match` (`upLinkMeta?.etag`), when it is a non-empty string.
pub(crate) fn uplink_etag<'a>(uplinks: &'a Map<String, Value>, uplink: &str) -> Option<&'a str> {
    uplinks.get(uplink)?.get("etag")?.as_str().filter(|s| !s.is_empty())
}

/// The 304 condition of `syncUplinksMetadata`: the local `_uplinks[uplink]` is an object with a
/// string `etag`. Returns that etag.
pub(crate) fn uplink_etag_for_304<'a>(
    uplinks: &'a Map<String, Value>,
    uplink: &str,
) -> Option<&'a str> {
    uplinks.get(uplink)?.as_object()?.get("etag")?.as_str()
}

/// Keys of the manifest `_uplinks`, or an empty map.
pub(crate) fn uplinks_of(m: &Manifest) -> Map<String, Value> {
    m.get(UPLINKS_KEY).and_then(Value::as_object).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests;
