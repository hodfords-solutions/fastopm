//! Expected values come from Verdaccio itself: `utils-vectors.json` and `sync-vectors.json` are
//! the output of `utils-vectors.gen.cjs` / `sync-vectors.gen.cjs` run against the compat
//! harness install (`node <script> compat`). The inputs below mirror those scripts.

use serde_json::{Value, json};

use super::*;

fn manifest(v: Value) -> Manifest {
    Manifest::from_map(v.as_object().expect("object").clone())
}

fn utils() -> Value {
    serde_json::from_str(include_str!("utils-vectors.json")).unwrap()
}

#[test]
fn normalize_dist_tags_matches_verdaccio() {
    let inputs = [
        json!({"versions": {"1.0.0": {}, "2.0.0-rc.1": {}, "1.5.0": {}, "bad": {}}, "dist-tags": {}}),
        json!({"versions": {"1.0.0": {}}, "dist-tags": {"latest": "nope", "beta": ["1.0.0", "2.0.0", "x"], "empty": [], "bad": ["x"], "num": 5}}),
        json!({"versions": {"1.0.0": {}, "3.0.0": {}}, "dist-tags": {"latest": "", "next": "v2.0.0"}}),
    ];
    let expected = utils()["normalizeDistTags"].clone();
    for (i, input) in inputs.into_iter().enumerate() {
        let mut m = manifest(input);
        normalize_dist_tags(&mut m);
        let got = serde_json::to_string(m.get(DIST_TAGS).unwrap()).unwrap();
        assert_eq!(got, serde_json::to_string(&expected[i]).unwrap(), "case {i}");
    }
    // not idempotent: an invalid `latest` is dropped first, then recomputed
    let mut m = manifest(json!({"versions": {"1.0.0": {}}, "dist-tags": {"latest": "nope"}}));
    assert!(normalize_dist_tags(&mut m));
    assert_eq!(m.dist_tags().unwrap().len(), 0);
    assert!(normalize_dist_tags(&mut m));
    assert_eq!(m.dist_tags().unwrap()["latest"], json!("1.0.0"));
    assert!(!normalize_dist_tags(&mut m));
}

#[test]
fn get_latest_readme_matches_verdaccio() {
    let inputs = [
        json!({"readme": "  top \n", "versions": {}, "dist-tags": {}}),
        json!({"versions": {"1.0.0": {"readme": " from latest "}}, "dist-tags": {"latest": "1.0.0"}}),
        json!({"readme": "", "versions": {"1.0.0": {"readme": ""}, "2.0.0-b": {"readme": "beta readme\t"}}, "dist-tags": {"latest": "1.0.0", "beta": "2.0.0-b"}}),
        json!({"versions": {}, "dist-tags": {}}),
    ];
    let expected = utils()["getLatestReadme"].clone();
    for (i, input) in inputs.into_iter().enumerate() {
        assert_eq!(json!(get_latest_readme(&manifest(input))), expected[i], "case {i}");
    }
}

#[test]
fn merge_versions_matches_verdaccio() {
    let expected = utils();
    let mut cache = manifest(
        json!({"name": "a", "versions": {"1.0.0": {"version": "1.0.0"}}, "time": {}, "dist-tags": {"latest": "1.0.0", "beta": "1.1.0-b"}, "readme": "old", "_uplinks": {}}),
    );
    let remote = manifest(
        json!({"name": "a", "readme": "new", "versions": {"1.0.0": {"version": "1.0.0", "deprecated": "dep"}, "2.0.0": {"version": "2.0.0"}}, "time": {"2.0.0": "t2"}, "dist-tags": {"latest": "2.0.0", "beta": "1.0.0", "next": "3.0.0"}}),
    );
    merge_versions(&mut cache, &remote).unwrap();
    assert_eq!(
        serde_json::to_string(&cache).unwrap(),
        serde_json::to_string(&expected["mergeVersions"]).unwrap()
    );

    let mut keep = manifest(
        json!({"versions": {}, "time": {}, "dist-tags": {"latest": "3.0.0"}, "readme": "keep"}),
    );
    merge_versions(
        &mut keep,
        &manifest(json!({"versions": {}, "dist-tags": {"latest": "2.0.0"}, "readme": "remote"})),
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&keep).unwrap(), expected["mergeKeepsHigherLocal"]);

    assert_eq!(expected["mergeThrows"], json!(true));
    let mut c = manifest(json!({"versions": {}, "time": {}, "dist-tags": {"latest": "1.0.0"}}));
    let bad = manifest(json!({"versions": {}, "dist-tags": {"latest": "not-semver"}}));
    assert_eq!(merge_versions(&mut c, &bad), Err(Malformed));
}

#[test]
fn scope_maintainers_matches_verdaccio() {
    let expected = utils();
    let list = [
        json!("bob"),
        json!({"email": "e", "name": "alice"}),
        json!({"email": "x"}),
        json!("$uplink:other:carol"),
    ];
    assert_eq!(json!(scope_upstream_maintainers(&list, "npmjs")), expected["scope"]);
    assert_eq!(json!(scope_upstream_maintainers(&[json!("bob")], "")), expected["scopeEmpty"]);
}

#[test]
fn lookup_dist_file_matches_verdaccio() {
    let expected = utils()["lookup"].clone();
    let m = manifest(
        json!({"name": "@s/pkg", "_distfiles": {"pkg-1.0.0.tgz": {"url": "https://r/@s/pkg/-/pkg-1.0.0.tgz", "sha": "a", "registry": "npmjs"}},
        "versions": {"2.0.0": {"dist": {"tarball": "https://cdn/x/pkg-2.0.0.tgz?sig=1", "shasum": "b"}}, "3.0.0": {"dist": {"tarball": "https://cdn/odd-name"}}}}),
    );
    for (i, f) in
        ["pkg-1.0.0.tgz", "pkg-2.0.0.tgz", "odd-name", "pkg-9.9.9.tgz", "name"].iter().enumerate()
    {
        let got = lookup_dist_file(&m, f);
        match &expected[i] {
            Value::Null => assert_eq!(got, None, "{f}"),
            e => {
                let got = got.unwrap_or_else(|| panic!("{f} not found"));
                assert_eq!(json!(got.url), e["url"], "{f}");
                assert_eq!(got.registry.map(Value::String), e.get("registry").cloned(), "{f}");
            }
        }
    }
    assert!(has_distfile_record(&m, "pkg-1.0.0.tgz"));
    assert!(!has_distfile_record(&m, "pkg-2.0.0.tgz"));
}

fn sync_remote() -> Manifest {
    manifest(json!({
        "name": "foo", "dist-tags": {"latest": "1.1.0", "next": "2.0.0-rc.1"},
        "maintainers": [{"name": "bob", "email": "b@x"}, "carol"],
        "readme": "Remote readme\n",
        "time": {"created": "c", "modified": "m2", "1.0.0": "t1", "1.1.0": "t11", "2.0.0-rc.1": "t2"},
        "versions": {
            "1.0.0": {"name": "foo", "version": "1.0.0", "readme": "v readme", "dist": {"tarball": "http://registry.example.com/foo/-/foo-1.0.0.tgz", "shasum": "s1"}},
            "1.1.0": {"name": "foo", "version": "1.1.0", "contributors": "someone", "dist": {"tarball": "http://registry.example.com/foo/-/foo-1.1.0.tgz", "shasum": "s11"}},
            "2.0.0-rc.1": {"name": "foo", "version": "2.0.0-rc.1", "contributors": [{"name": "x"}], "dist": {"tarball": "https://cdn.example.net/foo-2.0.0-rc.1.tgz"}}
        }
    }))
}

fn sync_disk() -> Manifest {
    manifest(json!({
        "name": "foo", "versions": {"1.0.0": {"name": "foo", "version": "1.0.0", "readme": "", "contributors": [], "dist": {"tarball": "https://registry.example.com/foo/-/foo-1.0.0.tgz", "shasum": "s1"}}},
        "time": {"created": "c", "modified": "m1", "1.0.0": "t1"}, "users": {}, "dist-tags": {"latest": "1.0.0"}, "maintainers": [],
        "_uplinks": {"npmjs": {"etag": "\"old\"", "fetched": 1_600_000_000_000u64}},
        "_distfiles": {"foo-1.0.0.tgz": {"url": "https://registry.example.com/foo/-/foo-1.0.0.tgz", "sha": "s1", "registry": "npmjs"}},
        "_attachments": {}, "_rev": "3-0123456789abcdef", "readme": "Old readme"
    }))
}

/// mergeCacheRemoteMetadata + updateVersionsNext, as `Store::sync` chains them.
fn run_sync(disk: Option<Manifest>, etag: Option<&str>) -> (Manifest, bool) {
    let normalized = |m: Manifest| {
        let mut m = m;
        normalize_package(&mut m);
        m
    };
    let base = disk.clone().map(normalized).unwrap_or_else(|| Manifest::template("foo"));
    let merged = merge_cache_remote(
        base,
        sync_remote(),
        "foo",
        "npmjs",
        etag.map(str::to_owned),
        1_700_000_000_000,
    )
    .unwrap();
    let cache = normalized(disk.unwrap_or_else(|| Manifest::template("foo")));
    let uplinks = uplinks_of(&merged);
    let src = SyncSource {
        manifest: &merged,
        uplinks: &uplinks,
        uplink_name: "npmjs",
        uplink_url: Some("https://registry.example.com/"),
    };
    // the borrowed form gives the same result
    let borrowed = update_versions_next(cache.clone(), &src).unwrap();
    let owned = update_versions_next_owned(
        cache,
        merged.clone(),
        &uplinks,
        "npmjs",
        Some("https://registry.example.com/"),
    )
    .unwrap();
    assert_eq!(owned.1, borrowed.1);
    assert_eq!(owned.0.to_compact_vec(), borrowed.0.to_compact_vec());
    owned
}

#[test]
fn sync_of_new_package_matches_verdaccio() {
    let expected: Value = serde_json::from_str(include_str!("sync-vectors.json")).unwrap();
    let (m, changed) = run_sync(None, Some("\"etag-1\""));
    assert!(changed);
    assert_eq!(String::from_utf8(m.to_compact_vec()).unwrap(), expected["fresh"]["written"]);
    let mut served = m.clone();
    normalize_dist_tags(&mut served);
    assert_eq!(String::from_utf8(served.to_compact_vec()).unwrap(), expected["fresh"]["served"]);
}

#[test]
fn sync_of_cached_package_matches_verdaccio() {
    let expected: Value = serde_json::from_str(include_str!("sync-vectors.json")).unwrap();
    let (m, changed) = run_sync(Some(sync_disk()), None);
    assert!(changed);
    assert_eq!(String::from_utf8(m.to_compact_vec()).unwrap(), expected["existing"]["written"]);
}

#[test]
fn second_identical_sync_changes_nothing_but_fetched() {
    // After writing the merge result, syncing the same remote again only moves `fetched`.
    let (first, _) = run_sync(Some(sync_disk()), None);
    let mut cache = first.clone();
    normalize_package(&mut cache);
    let mut local = first.clone();
    normalize_package(&mut local);
    let uplinks = uplinks_of(&local);
    let src = SyncSource {
        manifest: &local,
        uplinks: &uplinks,
        uplink_name: "npmjs",
        uplink_url: Some("https://registry.example.com/"),
    };
    let (_, changed) = update_versions_next(cache, &src).unwrap();
    assert!(!changed, "a maxage hit on an already merged manifest does not write");
    assert!(!would_change_self(&local));

    let mut remote_uplinks = uplinks.clone();
    remote_uplinks.insert("npmjs".into(), json!({"fetched": 1_700_000_000_001u64}));
    let src = SyncSource { uplinks: &remote_uplinks, ..src };
    let (m, changed) = update_versions_next(local.clone(), &src).unwrap();
    assert!(changed);
    assert_eq!(m.uplinks().unwrap()["npmjs"], json!({"fetched": 1_700_000_000_001u64}));
}

#[test]
fn would_change_self_detects_untrimmed_readme_and_odd_values() {
    let mut m = Manifest::template("foo");
    normalize_package(&mut m);
    m.insert("readme", json!(""));
    assert!(!would_change_self(&m));
    m.insert("readme", json!("text\n"));
    assert!(would_change_self(&m));
    m.insert("readme", json!("text"));
    assert!(!would_change_self(&m));
    m.dist_tags_mut().insert("weird".into(), json!(["1.0.0"]));
    assert!(would_change_self(&m), "arrays are never === to themselves after a re-read");
}

#[test]
fn merge_rejects_local_copy_of_another_package() {
    let local = Manifest::template("other");
    let err = merge_cache_remote(local, sync_remote(), "foo", "npmjs", None, 1);
    assert_eq!(err.err(), Some(Malformed));
}

#[test]
fn update_versions_next_rejects_relative_tarball() {
    let remote = manifest(
        json!({"name": "foo", "versions": {"1.0.0": {"dist": {"tarball": "/foo/-/foo-1.0.0.tgz"}}}, "time": {}}),
    );
    let mut cache = Manifest::template("foo");
    normalize_package(&mut cache);
    let uplinks = Map::new();
    let src =
        SyncSource { manifest: &remote, uplinks: &uplinks, uplink_name: "npmjs", uplink_url: None };
    assert!(update_versions_next(cache, &src).is_err());
}

#[test]
fn contributors_and_protocol_rules() {
    assert_eq!(normalize_contributors(None), json!([]));
    assert_eq!(normalize_contributors(Some(&json!("x"))), json!(["x"]));
    assert_eq!(normalize_contributors(Some(&json!(""))), json!([{"name": ""}]));
    assert_eq!(normalize_contributors(Some(&json!(false))), json!(false));
    assert_eq!(normalize_contributors(Some(&json!([1]))), json!([1]));

    let mut hash = Map::new();
    hash.insert("url".into(), json!("http://r.example:8080/a/-/a-1.tgz"));
    update_uplink_to_remote_protocol(&mut hash, "u", "https://r.example/");
    assert!(hash.get("registry").is_none(), "port differs");
    update_uplink_to_remote_protocol(&mut hash, "u", "https://r.example:8080/");
    assert_eq!(json!(hash), json!({"url": "https://r.example:8080/a/-/a-1.tgz", "registry": "u"}));
}
