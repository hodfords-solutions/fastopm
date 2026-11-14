//! Router tests: pure pipeline paths and end-to-end runs with the real `Store`, a temporary
//! storage directory and a local mock uplink.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::{Body, to_bytes};
use axum::extract::{Path as AxPath, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use fastopm_config::Config;
use fastopm_storage::FsStorage;
use fastopm_store::{Store, StoreOptions};
use fastopm_uplink::build_uplinks;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;

const NPM_ACCEPT: &str = "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*";

// ---------------------------------------------------------------- pure functions

#[test]
fn loop_detection() {
    assert!(is_loop("1.1 srv (Verdaccio)", "srv"));
    assert!(is_loop("1.1 other, 1.1 srv (Verdaccio)", "srv"));
    assert!(is_loop("1.1 srv", "srv"));
    assert!(is_loop("  1.1   srv(Verdaccio)", "srv"));
    assert!(!is_loop("1.1 other (Verdaccio)", "srv"));
    assert!(!is_loop("1.1 srvx", "srv"));
    assert!(!is_loop("srv", "srv"));
    assert!(!is_loop("", "srv"));
}

#[test]
fn abbreviated_detection() {
    assert!(wants_abbreviated(Some(NPM_ACCEPT)));
    assert!(wants_abbreviated(Some("application/vnd.npm.install-v1+json")));
    assert!(!wants_abbreviated(None));
    assert!(!wants_abbreviated(Some("application/json")));
    assert!(!wants_abbreviated(Some(
        "application/json; q=1.0, application/vnd.npm.install-v1+json; q=0.5"
    )));
}

#[test]
fn error_response_shape() {
    let response = error_response(&RegistryError::not_found(messages::RESOURCE_NOT_FOUND));
    assert_eq!(response.status(), 404);
    let h = response.headers();
    assert_eq!(h["content-type"], CONTENT_TYPE_JSON);
    assert_eq!(h["etag"], "W/\"24-/Exrzi8zHCWw4+NVgc+l+JLt2TI\"");
    assert_eq!(h["content-length"], "36");
    assert!(h.get("www-authenticate").is_none());
    let unauthorized = error_response(&RegistryError::unauthorized("x"));
    assert_eq!(unauthorized.headers()["www-authenticate"], "Bearer");
    let internal = error_response(&RegistryError::new(200, "oops"));
    assert_eq!(internal.status(), 500);
}

// ---------------------------------------------------------------- mock uplink

struct Mock {
    base: String,
    hits: Arc<AtomicUsize>,
    tarball_hits: Arc<AtomicUsize>,
}

fn tarball_bytes() -> Vec<u8> {
    (0..5000u32).map(|i| (i % 251) as u8).collect()
}

fn packument(base: &str, name: &str) -> Value {
    let file = name.rsplit('/').next().unwrap_or(name);
    let version = |v: &str| {
        json!({
            "name": name,
            "version": v,
            "description": "a package",
            "dependencies": {"dep": "^1.0.0"},
            "dist": {
                "shasum": "0000",
                "tarball": format!("{base}/{name}/-/{file}-{v}.tgz"),
            },
        })
    };
    json!({
        "_id": name,
        "name": name,
        "dist-tags": {"latest": "1.1.0"},
        "versions": {"1.0.0": version("1.0.0"), "1.1.0": version("1.1.0")},
        "time": {"modified": "2024-01-02T00:00:00.000Z", "created": "2024-01-01T00:00:00.000Z",
                 "1.0.0": "2024-01-01T00:00:00.000Z", "1.1.0": "2024-01-02T00:00:00.000Z"},
        "readme": "x".repeat(3000),
    })
}

#[derive(Clone)]
struct MockState {
    base: Arc<std::sync::OnceLock<String>>,
    hits: Arc<AtomicUsize>,
    tarball_hits: Arc<AtomicUsize>,
}

async fn mock_manifest(State(s): State<MockState>, AxPath(name): AxPath<String>) -> Response {
    s.hits.fetch_add(1, Ordering::SeqCst);
    let base = s.base.get().cloned().unwrap_or_default();
    let name = name.replace("%2F", "/").replace("%2f", "/");
    if name.contains("missing") {
        return (StatusCode::NOT_FOUND, "{}").into_response();
    }
    let body = serde_json::to_vec(&packument(&base, &name)).unwrap();
    ([("content-type", "application/json")], body).into_response()
}

async fn mock_tarball(s: MockState, file: String) -> Response {
    s.tarball_hits.fetch_add(1, Ordering::SeqCst);
    if file.starts_with("broken") {
        // Announce more bytes than sent: the download fails mid-stream.
        let chunks = futures_util::stream::iter(0..2).then(|i| async move {
            if i == 0 {
                Ok::<_, std::io::Error>(Bytes::from(vec![7u8; 4096]))
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                Err(std::io::Error::other("cut"))
            }
        });
        return ([("content-length", "5000")], Body::from_stream(chunks)).into_response();
    }
    ([("content-type", "application/octet-stream")], tarball_bytes()).into_response()
}

async fn start_mock() -> Mock {
    let state =
        MockState { base: Arc::default(), hits: Arc::default(), tarball_hits: Arc::default() };
    let app = axum::Router::new()
        .route("/{name}", axum::routing::get(mock_manifest))
        .route(
            "/{name}/-/{file}",
            axum::routing::get(
                |State(s): State<MockState>, AxPath((_, file)): AxPath<(String, String)>| {
                    mock_tarball(s, file)
                },
            ),
        )
        .route(
            "/{scope}/{pkg}/-/{file}",
            axum::routing::get(
                |State(s): State<MockState>,
                 AxPath((_, _, file)): AxPath<(String, String, String)>| {
                    mock_tarball(s, file)
                },
            ),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    state.base.set(base.clone()).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Mock { base, hits: state.hits, tarball_hits: state.tarball_hits }
}

// ---------------------------------------------------------------- harness

struct Env {
    app: Router,
    mock: Mock,
    dir: tempfile::TempDir,
}

fn config(dir: &Path, uplink: &str, extra: &str) -> Config {
    let yaml = format!(
        "storage: ./storage\nserver_id: test-srv\nuplinks:\n  upstream:\n    url: {uplink}/\n    \
         maxage: 10m\npackages:\n  'local-*':\n    access: $all\n  '**':\n    access: $all\n    \
         proxy: upstream\n{extra}"
    );
    Config::from_yaml_str(&yaml, &dir.join("config.yaml"), &|_| None).unwrap()
}

async fn env_with(extra: &str) -> Env {
    let mock = start_mock().await;
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(config(dir.path(), &mock.base, extra));
    let storage = FsStorage::new(Arc::clone(&config));
    let uplinks = build_uplinks(&config).unwrap();
    let store =
        Arc::new(Store::new(Arc::clone(&config), storage, uplinks, StoreOptions::default()));
    let app = router(AppState { config, store });
    Env { app, mock, dir }
}

async fn env() -> Env {
    env_with("").await
}

struct Got {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Got {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|v| v.to_str().unwrap())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

async fn send(app: &Router, method: &str, path: &str, headers: &[(&str, &str)]) -> Got {
    let mut request = Request::builder().method(method).uri(path).header("host", "localhost:4873");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Got { status, headers, body }
}

async fn get(app: &Router, path: &str) -> Got {
    send(app, "GET", path, &[]).await
}

fn error_of(got: &Got) -> String {
    got.json()["error"].as_str().unwrap().to_owned()
}

// ---------------------------------------------------------------- pipeline

#[tokio::test]
async fn ping_and_common_headers() {
    let e = env().await;
    let got = get(&e.app, "/-/ping").await;
    assert_eq!(got.status, 200);
    assert_eq!(&got.body[..], b"{}\n");
    assert_eq!(got.header("etag"), Some("\"8a80554c91d9fca8acb82f023de02f11\""));
    assert_eq!(got.header("x-powered-by"), Some("hidden"));
    assert_eq!(got.header("access-control-allow-origin"), Some("*"));
    assert_eq!(got.header("vary"), Some("Accept-Encoding"));
    assert_eq!(got.header("content-type"), Some("application/json; charset=utf-8"));
    assert_eq!(got.header("content-length"), Some("3"));
    assert_eq!(get(&e.app, "/-/ping?x=1").await.status, 200);
    assert_eq!(get(&e.app, "/-/PING/").await.status, 200);
}

#[tokio::test]
async fn ping_conditional() {
    let e = env().await;
    for inm in
        ["\"8a80554c91d9fca8acb82f023de02f11\"", "*", "W/\"8a80554c91d9fca8acb82f023de02f11\""]
    {
        let got = send(&e.app, "GET", "/-/ping", &[("if-none-match", inm)]).await;
        assert_eq!(got.status, 304, "{inm}");
        assert!(got.body.is_empty());
        assert_eq!(got.header("etag"), Some("\"8a80554c91d9fca8acb82f023de02f11\""));
        assert_eq!(got.header("content-type"), None);
        assert_eq!(got.header("vary"), None);
        assert_eq!(got.header("x-powered-by"), Some("hidden"));
    }
    let got = send(&e.app, "GET", "/-/ping", &[("if-none-match", "\"other\"")]).await;
    assert_eq!(got.status, 200);
    let got =
        send(&e.app, "GET", "/-/ping", &[("if-none-match", "*"), ("cache-control", "no-cache")])
            .await;
    assert_eq!(got.status, 200);
}

#[tokio::test]
async fn unknown_routes() {
    let e = env().await;
    for path in ["/a/b/c/d", "/-/v1/nope", "/simple-pkg/1.0.0/extra"] {
        let got = get(&e.app, path).await;
        assert_eq!(got.status, 404, "{path}");
        assert_eq!(error_of(&got), "resource not found");
        assert_eq!(got.header("etag"), Some("W/\"24-/Exrzi8zHCWw4+NVgc+l+JLt2TI\""));
        assert_eq!(got.header("vary"), Some("Accept-Encoding"));
        assert_eq!(got.header("x-powered-by"), Some("hidden"));
    }
    for path in ["/", "//simple-pkg", "/?x=1"] {
        let got = get(&e.app, path).await;
        assert_eq!(got.status, 404, "{path}");
        assert_eq!(error_of(&got), "Web interface is disabled in the config file");
        assert_eq!(got.header("etag"), Some("W/\"3e-XgDlLn0nsmbCcBuSD64gqtppKrs\""));
    }
    // an error is never 304, even with If-None-Match: *
    let got = send(&e.app, "GET", "/a/b/c/d", &[("if-none-match", "*")]).await;
    assert_eq!(got.status, 404);
}

#[tokio::test]
async fn dotfiles_are_ignored() {
    let e = env().await;
    for path in ["/.env", "/a/../b", "/simple-pkg/-/.x.tgz"] {
        let got = get(&e.app, path).await;
        assert_eq!(got.status, 404, "{path}");
        assert_eq!(error_of(&got), "resource not found");
        assert_eq!(got.header("x-powered-by"), Some("Express"));
        assert_eq!(got.header("vary"), None);
        assert_eq!(got.header("access-control-allow-origin"), Some("*"));
    }
    assert_eq!(e.mock.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn preflight_and_methods() {
    let e = env().await;
    let got =
        send(&e.app, "OPTIONS", "/simple-pkg", &[("access-control-request-headers", "x-a")]).await;
    assert_eq!(got.status, 204);
    assert_eq!(got.header("x-powered-by"), Some("Express"));
    assert_eq!(got.header("access-control-allow-origin"), Some("*"));
    assert_eq!(got.header("access-control-allow-methods"), Some("GET,HEAD,PUT,PATCH,POST,DELETE"));
    assert_eq!(got.header("access-control-allow-headers"), Some("x-a"));
    assert_eq!(got.header("vary"), Some("Access-Control-Request-Headers"));
    assert_eq!(got.header("content-length"), Some("0"));
    let got = send(&e.app, "OPTIONS", "/.env", &[]).await;
    assert_eq!(got.status, 204);
    assert_eq!(got.header("access-control-allow-headers"), None);

    for (method, path) in [
        ("PUT", "/simple-pkg"),
        ("PUT", "/@scope%2fnew-pkg"),
        ("DELETE", "/simple-pkg/-rev/1-0"),
        ("POST", "/-/npm/v1/security/audits/quick"),
    ] {
        let got = send(&e.app, method, path, &[]).await;
        assert_eq!(got.status, 405, "{method} {path}");
        assert_eq!(error_of(&got), "unsupported registry call");
    }
    for method in ["POST", "PATCH", "DELETE"] {
        let got = send(&e.app, method, "/simple-pkg", &[]).await;
        assert_eq!(got.status, 404, "{method}");
        assert_eq!(got.header("content-type"), Some("text/html; charset=utf-8"));
        assert_eq!(got.header("content-security-policy"), Some("default-src 'none'"));
        assert_eq!(got.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(got.header("etag"), None);
        assert_eq!(got.header("vary"), Some("Accept-Encoding"));
        assert_eq!(got.header("x-powered-by"), Some("hidden"));
        let page = String::from_utf8(got.body.to_vec()).unwrap();
        assert!(page.contains(&format!("<pre>Cannot {method} /simple-pkg</pre>")), "{page}");
    }
    assert_eq!(e.mock.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn loop_guard_and_bad_requests() {
    let e = env().await;
    for via in ["1.1 test-srv (Verdaccio)", "1.1 other, 1.1 test-srv"] {
        let got = send(&e.app, "GET", "/simple-pkg", &[("via", via)]).await;
        assert_eq!(got.status, 508);
        assert_eq!(error_of(&got), "loop detected");
    }
    let got = send(&e.app, "GET", "/-/ping", &[("via", "1.1 test-srv")]).await;
    assert_eq!(got.status, 508);

    let got = get(&e.app, "/%E0%A4%A").await;
    assert_eq!(got.status, 400);
    assert_eq!(error_of(&got), "Failed to decode param '%E0%A4%A'");
    assert_eq!(got.header("content-length"), Some("51"));
    assert_eq!(error_of(&get(&e.app, "/simple%20pkg").await), "invalid package");
    assert_eq!(error_of(&get(&e.app, "/simple-pkg/%5E1.0.0").await), "invalid version");
    assert_eq!(error_of(&get(&e.app, "/simple-pkg/-/__proto__").await), "invalid filename");

    let mut request = Request::builder().uri("/simple-pkg").header("host", "bad host");
    request = request.header("accept", "*/*");
    let response = e.app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), 400);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"], "Invalid URL: /simple-pkg (TypeError: Invalid URL)");
    assert_eq!(e.mock.hits.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------- end to end

#[tokio::test]
async fn manifest_full_and_abbreviated() {
    let e = env().await;
    let full = get(&e.app, "/simple-pkg").await;
    assert_eq!(full.status, 200);
    assert_eq!(full.header("content-type"), Some("application/json; charset=utf-8"));
    assert_eq!(full.header("etag").map(str::to_owned), Some(fastopm_render::etag(&full.body)));
    assert_eq!(full.header("content-length"), Some(full.body.len().to_string().as_str()));
    assert!(full.body.ends_with(b"}\n"));
    let doc = full.json();
    assert_eq!(
        doc["versions"]["1.0.0"]["dist"]["tarball"],
        "http://localhost:4873/simple-pkg/-/simple-pkg-1.0.0.tgz"
    );
    assert_eq!(doc["_attachments"], json!({}));

    let abbr = send(&e.app, "GET", "/simple-pkg", &[("accept", NPM_ACCEPT)]).await;
    assert_eq!(abbr.status, 200);
    assert_eq!(
        abbr.header("content-type"),
        Some("application/vnd.npm.install-v1+json; charset=utf-8")
    );
    let doc = abbr.json();
    assert!(doc.get("readme").is_none());
    assert_eq!(doc["modified"], "2024-01-02T00:00:00.000Z");

    // within maxage: one uplink request in total, same bytes
    let again = get(&e.app, "/simple-pkg").await;
    assert_eq!(again.body, full.body);
    assert_eq!(e.mock.hits.load(Ordering::SeqCst), 1);

    // If-None-Match with the ETag of the right variant only
    let etag = full.header("etag").unwrap().to_owned();
    assert_eq!(send(&e.app, "GET", "/simple-pkg", &[("if-none-match", &etag)]).await.status, 304);
    let cross =
        send(&e.app, "GET", "/simple-pkg", &[("if-none-match", &etag), ("accept", NPM_ACCEPT)])
            .await;
    assert_eq!(cross.status, 200);

    // the base URL follows Host and the forwarded protocol header
    let other = send(&e.app, "GET", "/simple-pkg", &[("x-forwarded-proto", "https")]).await;
    assert_eq!(
        other.json()["versions"]["1.1.0"]["dist"]["tarball"],
        "https://localhost:4873/simple-pkg/-/simple-pkg-1.1.0.tgz"
    );
}

#[tokio::test]
async fn scoped_forms_share_one_package() {
    let e = env().await;
    let first = get(&e.app, "/@scope/pkg").await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(
        first.json()["versions"]["1.0.0"]["dist"]["tarball"],
        "http://localhost:4873/@scope/pkg/-/pkg-1.0.0.tgz"
    );
    for form in ["/@scope%2Fpkg", "/@scope%2fpkg", "/%40scope%2Fpkg", "/%40scope/pkg"] {
        let got = get(&e.app, form).await;
        assert_eq!(got.status, 200, "{form}");
        assert_eq!(got.body, first.body, "{form}");
    }
    assert_eq!(e.mock.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn compression() {
    let e = env().await;
    let plain = get(&e.app, "/simple-pkg").await;
    assert!(plain.body.len() >= 1024);

    let gz = send(&e.app, "GET", "/simple-pkg", &[("accept-encoding", "gzip")]).await;
    assert_eq!(gz.header("content-encoding"), Some("gzip"));
    assert_eq!(gz.header("content-length"), None);
    assert_eq!(gz.header("vary"), Some("Accept-Encoding"));
    assert_eq!(gz.header("etag"), plain.header("etag"));
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&gz.body[..]).read_to_end(&mut out).unwrap();
    assert_eq!(out, plain.body);

    let br = send(&e.app, "GET", "/simple-pkg", &[("accept-encoding", "gzip, deflate, br")]).await;
    assert_eq!(br.header("content-encoding"), Some("br"));
    let mut out = Vec::new();
    brotli::BrotliDecompress(&mut &br.body[..], &mut out).unwrap();
    assert_eq!(out, plain.body);

    let deflate = send(&e.app, "GET", "/simple-pkg", &[("accept-encoding", "deflate")]).await;
    assert_eq!(deflate.header("content-encoding"), Some("deflate"));

    // below the threshold: Vary but no encoding
    let ping = send(&e.app, "GET", "/-/ping", &[("accept-encoding", "gzip")]).await;
    assert_eq!(ping.header("content-encoding"), None);
    assert_eq!(ping.header("vary"), Some("Accept-Encoding"));

    // HEAD: same headers as an uncompressed GET, no body
    let head = send(&e.app, "HEAD", "/simple-pkg", &[("accept-encoding", "gzip")]).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.header("content-encoding"), None);
    assert_eq!(head.header("content-length"), plain.header("content-length"));
    assert_eq!(head.header("etag"), plain.header("etag"));
}

#[tokio::test]
async fn versions() {
    let e = env().await;
    let got = get(&e.app, "/simple-pkg/1.0.0").await;
    assert_eq!(got.status, 200);
    assert_eq!(got.json()["version"], "1.0.0");
    assert_eq!(
        got.json()["dist"]["tarball"],
        "http://localhost:4873/simple-pkg/-/simple-pkg-1.0.0.tgz"
    );
    let latest = get(&e.app, "/simple-pkg/latest").await;
    assert_eq!(latest.json()["version"], "1.1.0");
    let loose = get(&e.app, "/simple-pkg/v1.0.0").await;
    assert_eq!(loose.json()["version"], "1.0.0");
    let abbr = send(&e.app, "GET", "/simple-pkg/1.0.0", &[("accept", NPM_ACCEPT)]).await;
    assert_eq!(
        abbr.header("content-type"),
        Some("application/vnd.npm.install-v1+json; charset=utf-8")
    );
    assert_eq!(abbr.json()["description"], "a package");
    let missing = get(&e.app, "/simple-pkg/9.9.9").await;
    assert_eq!(missing.status, 404);
    assert_eq!(error_of(&missing), "this version doesn't exist: 9.9.9");
    assert_eq!(missing.header("content-type"), Some("application/json; charset=utf-8"));
    let etag = got.header("etag").unwrap().to_owned();
    assert_eq!(
        send(&e.app, "GET", "/simple-pkg/1.0.0", &[("if-none-match", &etag)]).await.status,
        304
    );
}

#[tokio::test]
async fn upstream_not_found() {
    let e = env().await;
    let got = get(&e.app, "/missing-pkg").await;
    assert_eq!(got.status, 404);
    assert_eq!(error_of(&got), "no such package available");
    let got = get(&e.app, "/missing-pkg/-/missing-pkg-1.0.0.tgz").await;
    assert_eq!(got.status, 404);
}

#[tokio::test]
async fn tarballs() {
    let e = env().await;
    let got = get(&e.app, "/simple-pkg/-/simple-pkg-1.0.0.tgz").await;
    assert_eq!(got.status, 200);
    assert_eq!(got.header("content-type"), Some("application/octet-stream"));
    assert_eq!(got.header("content-length"), Some("5000"));
    assert_eq!(got.header("etag"), None);
    assert_eq!(got.header("vary"), None);
    assert_eq!(got.header("x-powered-by"), Some("hidden"));
    assert_eq!(got.body, tarball_bytes());

    // second request from disk, never compressed, If-None-Match ignored
    let again = send(
        &e.app,
        "GET",
        "/simple-pkg/-/simple-pkg-1.0.0.tgz",
        &[("accept-encoding", "gzip"), ("if-none-match", "*")],
    )
    .await;
    assert_eq!(again.status, 200);
    assert_eq!(again.header("content-encoding"), None);
    assert_eq!(again.body, tarball_bytes());
    assert_eq!(e.mock.tarball_hits.load(Ordering::SeqCst), 1);
    assert!(e.dir.path().join("storage/simple-pkg/simple-pkg-1.0.0.tgz").exists());

    let unknown = get(&e.app, "/simple-pkg/-/nope-1.0.0.tgz").await;
    assert_eq!(unknown.status, 404);
    assert_eq!(error_of(&unknown), "no such file available");
    assert_eq!(unknown.header("vary"), Some("Accept-Encoding"));
}

#[tokio::test]
async fn head_tarball_caches_it() {
    let e = env().await;
    let got = send(&e.app, "HEAD", "/simple-pkg/-/simple-pkg-1.1.0.tgz", &[]).await;
    assert_eq!(got.status, 200);
    assert!(got.body.is_empty());
    assert_eq!(got.header("content-length"), Some("5000"));
    let file = e.dir.path().join("storage/simple-pkg/simple-pkg-1.1.0.tgz");
    for _ in 0..100 {
        if file.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(file.exists());
}

#[tokio::test]
async fn url_prefix_and_public_url() {
    let e = env_with("url_prefix: /npm/\n").await;
    let got = get(&e.app, "/simple-pkg").await;
    assert_eq!(
        got.json()["versions"]["1.0.0"]["dist"]["tarball"],
        "http://localhost:4873/npm/simple-pkg/-/simple-pkg-1.0.0.tgz"
    );
}

// ---------------------------------------------------------------- over TCP

/// Raw HTTP/1.1 exchange over a real socket; returns everything the server sent.
async fn raw_exchange(app: Router, request: &str) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap()
    });
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ =
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    out
}

#[tokio::test]
async fn head_keeps_content_length_on_the_wire() {
    let e = env().await;
    let response = raw_exchange(
        e.app.clone(),
        "HEAD /-/ping HTTP/1.1\r\nHost: localhost:4873\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response).to_lowercase();
    assert!(text.starts_with("http/1.1 200"), "{text}");
    assert!(text.contains("content-length: 3\r\n"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "{text}");
}

#[tokio::test]
async fn compressed_bodies_are_chunked_on_the_wire() {
    let e = env().await;
    let response = raw_exchange(
        e.app.clone(),
        "GET /simple-pkg HTTP/1.1\r\nHost: localhost:4873\r\nAccept-Encoding: gzip\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response).to_lowercase();
    assert!(text.contains("transfer-encoding: chunked"), "{text}");
    assert!(!text.contains("content-length"), "{text}");
}

#[tokio::test]
async fn tarball_failing_mid_stream_aborts_the_connection() {
    let e = env().await;
    // The mock announces 5000 bytes for this tarball and cuts the transfer after 4096.
    assert_eq!(get(&e.app, "/broken").await.status, 200);
    let response = raw_exchange(
        e.app.clone(),
        "GET /broken/-/broken-1.0.0.tgz HTTP/1.1\r\nHost: localhost:4873\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.to_lowercase().contains("content-length: 5000"), "{head}");
    assert!(body.len() < 5000, "connection must be cut, got {} bytes", body.len());
}

#[tokio::test]
async fn preflight_sends_content_length_zero_on_the_wire() {
    let e = env().await;
    let response = raw_exchange(
        e.app.clone(),
        "OPTIONS /simple-pkg HTTP/1.1\r\nHost: localhost:4873\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response).to_lowercase();
    assert!(text.starts_with("http/1.1 204"), "{text}");
    assert!(text.contains("content-length: 0\r\n"), "{text}");
    assert!(!text.contains("transfer-encoding"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "no body after a 204: {text}");
}
