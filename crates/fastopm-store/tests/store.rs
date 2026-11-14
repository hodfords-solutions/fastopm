//! `Store` against in-memory storage and uplinks.

mod fakes;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fastopm_core::{Manifest, messages};
use fastopm_store::StoreOptions;
use fastopm_uplink::UplinkError;
use futures_util::StreamExt;
use futures_util::future::join_all;
use serde_json::{Value, json};

use fakes::*;

const NPM: &str = "https://registry.example.com";
const MIRROR: &str = "https://mirror.example.com";
const TEN_MINUTES_MS: u64 = 600_000;

fn version(v: &str, base: &str) -> Value {
    json!({"name": "foo", "version": v, "dist": {"tarball": format!("{base}/foo/-/foo-{v}.tgz"), "shasum": format!("sha-{v}")}})
}

/// What an uplink answers for `foo`.
fn remote_foo(versions: &[&str]) -> Manifest {
    let mut vs = serde_json::Map::new();
    let mut time = serde_json::Map::new();
    for v in versions {
        vs.insert((*v).into(), version(v, NPM));
        time.insert((*v).into(), json!(format!("t-{v}")));
    }
    manifest(json!({
        "name": "foo",
        "dist-tags": {"latest": versions.last().unwrap()},
        "maintainers": [{"name": "bob"}],
        "readme": "readme",
        "time": time,
        "versions": vs,
    }))
}

/// `foo@1.0.0` as Verdaccio would have cached it from `npmjs`, last fetched at `fetched`.
fn cached_foo(fetched: u64) -> Manifest {
    manifest(json!({
        "name": "foo",
        "versions": {"1.0.0": {"name": "foo", "version": "1.0.0", "readme": "", "contributors": [], "dist": {"tarball": format!("{NPM}/foo/-/foo-1.0.0.tgz"), "shasum": "sha-1.0.0"}}},
        "time": {"1.0.0": "t-1.0.0"},
        "users": {},
        "dist-tags": {"latest": "1.0.0"},
        "maintainers": ["$uplink:npmjs:bob"],
        "_uplinks": {"npmjs": {"etag": "\"e1\"", "fetched": fetched}},
        "_distfiles": {"foo-1.0.0.tgz": {"url": format!("{NPM}/foo/-/foo-1.0.0.tgz"), "sha": "sha-1.0.0", "registry": "npmjs"}},
        "_attachments": {},
        "_rev": "1-0000000000000001",
        "_id": "foo",
        "readme": "readme",
    }))
}

fn npmjs() -> (FakeUplink, Arc<UplinkState>) {
    let up = FakeUplink::new("npmjs", NPM);
    let state = Arc::clone(&up.state);
    (up, state)
}

fn stale() -> u64 {
    now_ms() - TEN_MINUTES_MS
}

// ------------------------------------------------------------------ manifests: maxage, 304

#[tokio::test]
async fn fresh_maxage_serves_cache_without_uplink_or_write() {
    let (up, h) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));

    let a = s.get_manifest("foo", None).await.unwrap();
    let b = s.get_manifest("foo", None).await.unwrap();
    assert_eq!(h.manifest_calls(), 0);
    assert_eq!(s.storage().writes(), 0);
    assert_eq!(a.generation, b.generation);
    assert!(Arc::ptr_eq(&a.manifest, &b.manifest), "second call is a RAM cache hit");
    assert_eq!(s.storage().state.manifest_reads.load(Ordering::SeqCst), 1);
    assert_eq!(a.manifest.name(), Some("foo"));
}

#[tokio::test]
async fn stale_maxage_refetches_with_etag_and_writes_once() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0", "1.1.0"]), Some("\"e2\"".into())));
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(stale()));

    let snap = s.get_manifest("foo", Some("10.0.0.1")).await.unwrap();
    assert_eq!(h.etags(), vec![Some("\"e1\"".to_owned())]);
    assert_eq!(s.storage().writes(), 1);
    assert_eq!(snap.manifest.dist_tags().unwrap()["latest"], json!("1.1.0"));
    assert!(snap.manifest.version("1.1.0").is_some());

    let disk = s.storage().get("foo").unwrap();
    assert!(disk.rev().unwrap().starts_with("2-"), "{:?}", disk.rev());
    assert_eq!(disk.uplink_meta("npmjs").unwrap().etag.as_deref(), Some("\"e2\""));
    let dist = disk.distfile("foo-1.1.0.tgz").unwrap();
    assert_eq!(dist.registry.as_deref(), Some("npmjs"));
    assert_eq!(disk.version("1.1.0").unwrap()["readme"], json!(""));
    assert_eq!(*snap.manifest, disk, "served = written (dist-tags already normal)");

    // now fresh: no further uplink request, no write, same generation
    let again = s.get_manifest("foo", None).await.unwrap();
    assert_eq!(h.manifest_calls(), 1);
    assert_eq!(s.storage().writes(), 1);
    assert_eq!(again.generation, snap.generation);
}

#[tokio::test]
async fn not_modified_refreshes_fetched_then_tries_next_uplink() {
    let a = FakeUplink::new("npmjs", NPM);
    let b = FakeUplink::new("mirror", MIRROR);
    let (ha, hb) = (Arc::clone(&a.state), Arc::clone(&b.state));
    ha.answer("foo", ManifestAnswer::NotModified);
    hb.answer("foo", ManifestAnswer::Err(UplinkError::Transport("refused".into())));
    let s = store(vec![a, b], &["npmjs", "mirror"]);
    let old = stale();
    s.storage().put("foo", &cached_foo(old));

    let snap = s.get_manifest("foo", None).await.unwrap();
    assert_eq!((ha.manifest_calls(), hb.manifest_calls()), (1, 1));
    assert_eq!(s.storage().writes(), 1);
    let disk = s.storage().get("foo").unwrap();
    let meta = disk.uplink_meta("npmjs").unwrap();
    assert_eq!(meta.etag.as_deref(), Some("\"e1\""));
    assert!(meta.fetched.unwrap() > old);
    assert_eq!(disk.versions().unwrap().len(), 1);
    assert_eq!(snap.manifest.uplink_meta("npmjs"), Some(meta));
}

#[tokio::test]
async fn not_modified_then_ok_merges_from_second_uplink() {
    let a = FakeUplink::new("npmjs", NPM);
    let b = FakeUplink::new("mirror", MIRROR);
    let (ha, hb) = (Arc::clone(&a.state), Arc::clone(&b.state));
    ha.answer("foo", ManifestAnswer::NotModified);
    hb.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0", "2.0.0"]), None));
    let s = store(vec![a, b], &["npmjs", "mirror"]);
    s.storage().put("foo", &cached_foo(stale()));

    let snap = s.get_manifest("foo", None).await.unwrap();
    assert!(snap.manifest.version("2.0.0").is_some());
    let disk = s.storage().get("foo").unwrap();
    assert!(disk.uplink_meta("npmjs").unwrap().fetched.unwrap() > stale());
    assert_eq!(disk.uplink_meta("mirror").unwrap().etag, None);
    // the NPM tarball host is not the mirror's: no `registry` recorded
    assert_eq!(disk.distfile("foo-2.0.0.tgz").unwrap().registry, None);
    assert_eq!(s.storage().writes(), 1);
}

#[tokio::test]
async fn first_successful_uplink_wins() {
    let a = FakeUplink::new("npmjs", NPM);
    let b = FakeUplink::new("mirror", MIRROR);
    let (ha, hb) = (Arc::clone(&a.state), Arc::clone(&b.state));
    ha.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0"]), None));
    hb.answer("foo", ManifestAnswer::Ok(remote_foo(&["9.0.0"]), None));
    let s = store(vec![a, b], &["npmjs", "mirror"]);
    let snap = s.get_manifest("foo", None).await.unwrap();
    assert_eq!((ha.manifest_calls(), hb.manifest_calls()), (1, 0));
    assert!(snap.manifest.version("9.0.0").is_none());
}

#[tokio::test]
async fn not_modified_without_local_copy_bubbles_up() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::NotModified);
    let s = store(vec![up], &["npmjs"]);
    let err = s.get_manifest("foo", None).await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, messages::NOT_MODIFIED_NO_DATA));
}

// ------------------------------------------------------------------ manifests: merge result

#[tokio::test]
async fn new_package_is_merged_written_and_served() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0", "1.1.0"]), Some("W/\"x\"".into())));
    let s = store(vec![up], &["npmjs"]);

    let snap = s.get_manifest("foo", None).await.unwrap();
    assert_eq!(s.storage().writes(), 1);
    let disk = s.storage().get("foo").unwrap();
    let keys: Vec<&str> = disk.as_map().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "name",
            "versions",
            "time",
            "users",
            "dist-tags",
            "maintainers",
            "_uplinks",
            "_distfiles",
            "_attachments",
            "_rev",
            "_id",
            "readme"
        ]
    );
    assert!(disk.rev().unwrap().starts_with("1-"));
    assert_eq!(disk.get("maintainers"), Some(&json!([{"name": "$uplink:npmjs:bob"}])));
    assert_eq!(disk.get("readme"), Some(&json!("readme")));
    assert_eq!(disk.version("1.0.0").unwrap()["contributors"], json!([]));
    assert_eq!(disk.time().unwrap()["1.1.0"], json!("t-1.1.0"));
    assert_eq!(*snap.manifest, disk);
}

#[tokio::test]
async fn served_copy_has_dist_tags_normalized() {
    let (up, h) = npmjs();
    let mut remote = remote_foo(&["1.0.0", "2.0.0"]);
    remote.insert("dist-tags", json!({"beta": "not-a-version", "next": "2.0.0"}));
    h.answer("foo", ManifestAnswer::Ok(remote, None));
    let s = store(vec![up], &["npmjs"]);

    let snap = s.get_manifest("foo", None).await.unwrap();
    // written as merged (invalid tag kept), served normalized (latest computed, invalid dropped)
    let disk = s.storage().get("foo").unwrap();
    assert_eq!(disk.dist_tags().unwrap()["beta"], json!("not-a-version"));
    assert_eq!(
        snap.manifest.dist_tags().unwrap(),
        json!({"next": "2.0.0", "latest": "2.0.0"}).as_object().unwrap()
    );
}

// ------------------------------------------------------------------ manifests: errors

#[tokio::test]
async fn uplink_down_with_cache_serves_cache_without_write() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Err(UplinkError::Offline));
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(stale()));
    let snap = s.get_manifest("foo", None).await.unwrap();
    assert_eq!(snap.manifest.version("1.0.0").unwrap()["version"], json!("1.0.0"));
    assert_eq!(s.storage().writes(), 0);
}

#[tokio::test]
async fn uplink_down_without_cache_is_404() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Err(UplinkError::BadStatus(503)));
    let s = store(vec![up], &["npmjs"]);
    let err = s.get_manifest("foo", None).await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, messages::NO_PACKAGE));
}

#[tokio::test]
async fn timeout_without_cache_is_503() {
    let a = FakeUplink::new("npmjs", NPM);
    let b = FakeUplink::new("mirror", MIRROR);
    a.state.answer("foo", ManifestAnswer::Err(UplinkError::NotFound));
    b.state.answer("foo", ManifestAnswer::Err(UplinkError::Timeout));
    let s = store(vec![a, b], &["npmjs", "mirror"]);
    let err = s.get_manifest("foo", None).await.unwrap_err();
    assert_eq!((err.status(), err.message()), (503, messages::SERVER_TIME_OUT));
}

#[tokio::test]
async fn timeout_with_cache_serves_cache() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Err(UplinkError::Timeout));
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(stale()));
    assert!(s.get_manifest("foo", None).await.is_ok());
}

#[tokio::test]
async fn local_only_rule_never_asks_uplinks() {
    let (up, h) = npmjs();
    let s = store_with(
        vec![up],
        vec![rule("local-*", &[]), rule("**", &["npmjs"])],
        StoreOptions::default(),
    );
    let mut local = Manifest::template("local-x");
    local.versions_mut().insert("1.0.0".into(), json!({"version": "1.0.0"}));
    s.storage().put("local-x", &local);

    let snap = s.get_manifest("local-x", None).await.unwrap();
    assert_eq!(snap.manifest.dist_tags().unwrap()["latest"], json!("1.0.0"));
    assert_eq!(snap.manifest.get("_id"), Some(&json!("local-x")));
    let err = s.get_manifest("local-y", None).await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, "package does not exist on uplink: local-y"));
    assert_eq!(h.manifest_calls(), 0);
    assert_eq!(s.storage().writes(), 0);
}

#[tokio::test]
async fn unconfigured_uplink_in_proxy_behaves_like_a_failing_uplink() {
    let s = store(vec![], &["ghost"]);
    s.storage().put("foo", &cached_foo(stale()));
    assert!(s.get_manifest("foo", None).await.is_ok());
    let err = s.get_manifest("bar", None).await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, messages::NO_PACKAGE));
}

#[tokio::test]
async fn corrupt_manifest_is_500() {
    let (up, _) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    s.storage().put_raw("foo", b"{not json".to_vec());
    let err = s.get_manifest("foo", None).await.unwrap_err();
    assert_eq!((err.status(), err.message()), (500, messages::INTERNAL_SERVER_ERROR));
}

// ------------------------------------------------------------------ RAM cache

#[tokio::test]
async fn ram_cache_follows_file_identity() {
    let (up, _) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    let original = cached_foo(now_ms());
    s.storage().put("foo", &original);
    let first = s.get_manifest("foo", None).await.unwrap();

    // rewritten with the same content: re-read, same generation
    s.storage().put("foo", &original);
    let same = s.get_manifest("foo", None).await.unwrap();
    assert_eq!(same.generation, first.generation);
    assert_eq!(s.storage().state.manifest_reads.load(Ordering::SeqCst), 2);

    // another process added a version: new content, new generation
    let mut changed = original.clone();
    changed.versions_mut().insert("1.0.1".into(), json!({"version": "1.0.1"}));
    s.storage().put("foo", &changed);
    let next = s.get_manifest("foo", None).await.unwrap();
    assert_ne!(next.generation, first.generation);
    assert!(next.manifest.version("1.0.1").is_some());
}

#[tokio::test]
async fn disabled_cache_reads_every_time() {
    let (up, _) = npmjs();
    let s = store_with(
        vec![up],
        vec![rule("**", &["npmjs"])],
        StoreOptions { manifest_cache_bytes: 0 },
    );
    s.storage().put("foo", &cached_foo(now_ms()));
    s.get_manifest("foo", None).await.unwrap();
    s.get_manifest("foo", None).await.unwrap();
    assert_eq!(s.storage().state.manifest_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn untrimmed_readme_is_rewritten_once_then_stable() {
    let (up, h) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    let mut m = cached_foo(now_ms());
    m.insert("readme", json!("readme\n"));
    s.storage().put("foo", &m);

    s.get_manifest("foo", None).await.unwrap();
    assert_eq!(s.storage().writes(), 1, "Verdaccio rewrites the trimmed readme");
    s.get_manifest("foo", None).await.unwrap();
    assert_eq!(s.storage().writes(), 1);
    assert_eq!(h.manifest_calls(), 0);
    assert_eq!(s.storage().get("foo").unwrap().get("readme"), Some(&json!("readme")));
}

// ------------------------------------------------------------------ coalescing

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_manifest_requests_share_one_uplink_call() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0"]), None));
    h.delay(Duration::from_millis(100));
    let s = Arc::new(store(vec![up], &["npmjs"]));

    let results = join_all((0..10).map(|_| {
        let s = Arc::clone(&s);
        tokio::spawn(async move { s.get_manifest("foo", None).await })
    }))
    .await;
    let gens: Vec<u64> = results.into_iter().map(|r| r.unwrap().unwrap().generation).collect();
    assert_eq!(h.manifest_calls(), 1);
    assert_eq!(s.storage().writes(), 1);
    assert!(gens.iter().all(|g| *g == gens[0]));
}

#[tokio::test]
async fn concurrent_failures_share_one_uplink_call() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Err(UplinkError::Timeout));
    h.delay(Duration::from_millis(50));
    let s = store(vec![up], &["npmjs"]);
    let results = join_all((0..5).map(|_| s.get_manifest("foo", None))).await;
    assert!(results.iter().all(|r| r.as_ref().unwrap_err().status() == 503));
    assert_eq!(h.manifest_calls(), 1);
}

#[tokio::test]
async fn dropped_caller_does_not_cancel_the_refresh() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0"]), None));
    h.delay(Duration::from_millis(50));
    let s = store(vec![up], &["npmjs"]);
    let _ = tokio::time::timeout(Duration::from_millis(10), s.get_manifest("foo", None)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.storage().writes(), 1, "the sync finished without any caller");
    s.get_manifest("foo", None).await.unwrap();
    assert_eq!(h.manifest_calls(), 1);
}

// ------------------------------------------------------------------ tarballs

const TGZ_1: &str = "https://registry.example.com/foo/-/foo-1.0.0.tgz";

#[tokio::test]
async fn local_tarball_is_served_from_storage() {
    let (up, h) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    s.storage().put_tarball("foo", "foo-1.0.0.tgz", b"local");
    let body = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap();
    assert_eq!(body.content_length, Some(5));
    assert_eq!(collect(body).await.unwrap(), b"local");
    assert_eq!(h.tarball_calls(), 0);
}

#[tokio::test]
async fn tarball_is_streamed_and_cached() {
    let (up, h) = npmjs();
    h.tarball(TGZ_1, TarballAnswer::ok(vec![b"ab", b"cd"]));
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));

    let body = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap();
    assert_eq!(body.content_length, Some(4));
    assert_eq!(collect(body).await.unwrap(), b"abcd");
    assert_eq!(s.storage().tarball("foo", "foo-1.0.0.tgz").as_deref(), Some(&b"abcd"[..]));
    assert_eq!(s.storage().state.commits.load(Ordering::SeqCst), 1);

    collect(s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap()).await.unwrap();
    assert_eq!(h.tarball_calls(), 1, "second request is served from storage");
}

#[tokio::test]
async fn tarball_is_not_written_when_uplink_cache_is_off() {
    let (mut up, h) = npmjs();
    up.cache = false;
    h.tarball(TGZ_1, TarballAnswer::ok(vec![b"abcd"]));
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));
    assert_eq!(
        collect(s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap()).await.unwrap(),
        b"abcd"
    );
    assert_eq!(s.storage().state.creates.load(Ordering::SeqCst), 0);
    assert!(s.storage().tarball("foo", "foo-1.0.0.tgz").is_none());
}

#[tokio::test]
async fn length_mismatch_fails_the_stream_and_aborts_the_write() {
    let (up, h) = npmjs();
    let mut answer = TarballAnswer::ok(vec![b"ab", b"cd"]);
    answer.content_length = Some(10);
    h.tarball(TGZ_1, answer);
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));

    let err = collect(s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap()).await.unwrap_err();
    assert!(err.to_string().contains("content length mismatch"), "{err}");
    assert_eq!(s.storage().state.aborts.load(Ordering::SeqCst), 1);
    assert_eq!(s.storage().state.commits.load(Ordering::SeqCst), 0);
    assert!(s.storage().tarball("foo", "foo-1.0.0.tgz").is_none());

    // the failed download is not reused
    let _ = collect(s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap()).await;
    assert_eq!(h.tarball_calls(), 2);
}

#[tokio::test]
async fn uplink_status_errors_map_to_tarball_errors() {
    let (up, h) = npmjs();
    let mut answer = TarballAnswer::ok(vec![]);
    answer.status = Some(UplinkError::Forbidden("blocked".into()));
    h.tarball(TGZ_1, answer);
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));
    let err = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap_err();
    assert_eq!((err.status(), err.message()), (403, "blocked"));

    let mut m = cached_foo(now_ms());
    m.set_distfile(
        "foo-2.0.0.tgz",
        &fastopm_core::DistFile {
            url: format!("{NPM}/foo/-/foo-2.0.0.tgz"),
            sha: None,
            registry: None,
        },
    );
    s.storage().put("foo", &m);
    let err = s.get_tarball("foo", "foo-2.0.0.tgz").await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, messages::NOT_FILE_UPLINK));
}

#[tokio::test]
async fn missing_dist_file_refreshes_the_manifest_first() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Ok(remote_foo(&["1.0.0", "1.1.0"]), None));
    h.tarball("https://registry.example.com/foo/-/foo-1.1.0.tgz", TarballAnswer::ok(vec![b"new"]));
    let s = store(vec![up], &["npmjs"]);

    let body = s.get_tarball("foo", "foo-1.1.0.tgz").await.unwrap();
    assert_eq!(collect(body).await.unwrap(), b"new");
    assert_eq!(h.manifest_calls(), 1);
    assert_eq!(s.storage().writes(), 1);

    let err = s.get_tarball("foo", "foo-9.9.9.tgz").await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, messages::NO_SUCH_FILE));
}

#[tokio::test]
async fn missing_dist_file_propagates_refresh_errors() {
    let (up, h) = npmjs();
    h.answer("foo", ManifestAnswer::Err(UplinkError::Timeout));
    let s = store(vec![up], &["npmjs"]);
    let err = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap_err();
    assert_eq!(err.status(), 503);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tarball_requests_share_one_download() {
    let (up, h) = npmjs();
    let mut answer = TarballAnswer::ok(vec![b"aa", b"bb", b"cc"]);
    answer.chunk_delay = Duration::from_millis(20);
    h.tarball(TGZ_1, answer);
    let s = Arc::new(store(vec![up], &["npmjs"]));
    s.storage().put("foo", &cached_foo(now_ms()));

    let results = join_all((0..6).map(|i| {
        let s = Arc::clone(&s);
        tokio::spawn(async move {
            // stagger the joins so some arrive mid-download
            tokio::time::sleep(Duration::from_millis(5 * i)).await;
            let body = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap();
            collect(body).await.unwrap()
        })
    }))
    .await;
    for r in results {
        assert_eq!(r.unwrap(), b"aabbcc");
    }
    assert_eq!(h.tarball_calls(), 1);
    assert_eq!(s.storage().state.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_client_leaving_does_not_cancel_the_download_for_others() {
    let (up, h) = npmjs();
    let mut answer = TarballAnswer::ok(vec![b"aa", b"bb", b"cc"]);
    answer.chunk_delay = Duration::from_millis(20);
    h.tarball(TGZ_1, answer);
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));

    let (a, b) =
        tokio::join!(s.get_tarball("foo", "foo-1.0.0.tgz"), s.get_tarball("foo", "foo-1.0.0.tgz"));
    let mut a = a.unwrap();
    assert_eq!(&a.stream.next().await.unwrap().unwrap()[..], b"aa");
    drop(a);
    assert_eq!(collect(b.unwrap()).await.unwrap(), b"aabbcc");
    assert_eq!(s.storage().state.commits.load(Ordering::SeqCst), 1);
    assert_eq!(h.tarball_calls(), 1);
}

#[tokio::test]
async fn download_stops_when_every_client_left() {
    let (up, h) = npmjs();
    let mut answer = TarballAnswer::ok(vec![b"aa", b"bb", b"cc"]);
    answer.chunk_delay = Duration::from_millis(20);
    h.tarball(TGZ_1, answer);
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));

    let mut body = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap();
    body.stream.next().await.unwrap().unwrap();
    drop(body);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(s.storage().state.aborts.load(Ordering::SeqCst), 1);
    assert_eq!(s.storage().state.commits.load(Ordering::SeqCst), 0);
    assert!(s.storage().tarball("foo", "foo-1.0.0.tgz").is_none());
}

/// Regression (e2e with npm against registry.npmjs.org): the uplink body ends with an empty
/// chunk that arrives after the data (an HTTP/2 DATA frame carrying only END_STREAM). The client
/// has all `Content-Length` bytes by then and its body is dropped (hyper stops polling once the
/// announced length is sent), so the download looked abandoned and the tarball was never cached:
/// 40 of ~300 tarballs were fetched again by the next install.
#[tokio::test]
async fn complete_download_is_cached_when_the_client_leaves_before_the_stream_end() {
    let (up, h) = npmjs();
    let mut answer = TarballAnswer::ok(vec![b"aa", b"bb", b""]);
    answer.chunk_delay = Duration::from_millis(20);
    h.tarball(TGZ_1, answer);
    let s = store(vec![up], &["npmjs"]);
    s.storage().put("foo", &cached_foo(now_ms()));

    let mut body = s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap();
    assert_eq!(body.content_length, Some(4));
    let mut got = Vec::new();
    while got.len() < 4 {
        got.extend_from_slice(&body.stream.next().await.unwrap().unwrap());
    }
    assert_eq!(got, b"aabb");
    drop(body);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(s.storage().state.aborts.load(Ordering::SeqCst), 0);
    assert_eq!(s.storage().state.commits.load(Ordering::SeqCst), 1);
    assert_eq!(s.storage().tarball("foo", "foo-1.0.0.tgz").as_deref(), Some(&b"aabb"[..]));

    collect(s.get_tarball("foo", "foo-1.0.0.tgz").await.unwrap()).await.unwrap();
    assert_eq!(h.tarball_calls(), 1, "second request is served from storage");
}

// ------------------------------------------------------------------ uplink selection

fn with_distfile(file: &str, url: &str, registry: Option<&str>) -> Manifest {
    let mut m = cached_foo(now_ms());
    m.set_distfile(
        file,
        &fastopm_core::DistFile {
            url: url.into(),
            sha: None,
            registry: registry.map(str::to_owned),
        },
    );
    m
}

#[tokio::test]
async fn off_uplink_url_from_distfiles_uses_a_throwaway_client() {
    let (up, h) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    let url = "https://cdn.example.net/store-test/foo-2.0.0.tgz";
    foreign_tarballs().lock().unwrap().insert(url.into(), (TarballAnswer::ok(vec![b"cdn"]), 0));
    s.storage().put("foo", &with_distfile("foo-2.0.0.tgz", url, None));

    let body = s.get_tarball("foo", "foo-2.0.0.tgz").await.unwrap();
    assert_eq!(collect(body).await.unwrap(), b"cdn");
    assert_eq!(foreign_tarballs().lock().unwrap()[url].1, 1);
    assert_eq!(h.tarball_calls(), 0, "the configured uplink (and its credentials) is not used");
    assert_eq!(s.storage().tarball("foo", "foo-2.0.0.tgz").as_deref(), Some(&b"cdn"[..]));
}

#[tokio::test]
async fn off_uplink_url_only_in_versions_is_refused() {
    let (up, h) = npmjs();
    let s = store(vec![up], &["npmjs"]);
    let url = "https://evil.example/store-test/foo-3.0.0.tgz";
    foreign_tarballs().lock().unwrap().insert(url.into(), (TarballAnswer::ok(vec![b"x"]), 0));
    let mut m = cached_foo(now_ms());
    m.versions_mut().insert("3.0.0".into(), json!({"version": "3.0.0", "dist": {"tarball": url}}));
    s.storage().put("foo", &m);

    let err = s.get_tarball("foo", "foo-3.0.0.tgz").await.unwrap_err();
    assert_eq!((err.status(), err.message()), (404, messages::NO_SUCH_FILE));
    assert_eq!(foreign_tarballs().lock().unwrap()[url].1, 0);
    assert_eq!(h.tarball_calls(), 0);
}

#[tokio::test]
async fn uplink_choice_prefers_recorded_registry_then_last_serving_proxy() {
    // two uplinks on the same host: without a usable `registry`, the last one wins
    let a = FakeUplink::new("a", MIRROR);
    let b = FakeUplink::new("b", MIRROR);
    let (ha, hb) = (Arc::clone(&a.state), Arc::clone(&b.state));
    let url = "https://mirror.example.com/foo/-/foo-5.0.0.tgz";
    ha.tarball(url, TarballAnswer::ok(vec![b"from-a"]));
    hb.tarball(url, TarballAnswer::ok(vec![b"from-b"]));
    let s = store(vec![a, b], &["a", "b"]);

    s.storage().put("foo", &with_distfile("foo-5.0.0.tgz", url, None));
    assert_eq!(
        collect(s.get_tarball("foo", "foo-5.0.0.tgz").await.unwrap()).await.unwrap(),
        b"from-b"
    );

    s.storage()
        .put("foo", &with_distfile("foo-6.0.0.tgz", &url.replace("5.0.0", "6.0.0"), Some("a")));
    ha.tarball(&url.replace("5.0.0", "6.0.0"), TarballAnswer::ok(vec![b"from-a"]));
    assert_eq!(
        collect(s.get_tarball("foo", "foo-6.0.0.tgz").await.unwrap()).await.unwrap(),
        b"from-a"
    );

    // recorded registry that does not serve the URL is ignored
    let other = "https://registry.example.com/foo/-/foo-7.0.0.tgz";
    s.storage().put("foo", &with_distfile("foo-7.0.0.tgz", other, Some("a")));
    let foreign = foreign_tarballs();
    foreign.lock().unwrap().insert(other.into(), (TarballAnswer::ok(vec![b"cdn"]), 0));
    assert_eq!(
        collect(s.get_tarball("foo", "foo-7.0.0.tgz").await.unwrap()).await.unwrap(),
        b"cdn"
    );
    assert_eq!((ha.tarball_calls(), hb.tarball_calls()), (1, 1));
}
