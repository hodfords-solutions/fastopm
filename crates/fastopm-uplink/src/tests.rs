//! Behaviour tests against local mock HTTP servers (axum, or raw TCP for broken responses).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use bytes::Bytes;
use fastopm_config::{Config, LogConfig, TokenSource, UplinkAuth, UplinkConfig};
use futures_util::StreamExt;
use indexmap::IndexMap;
use reqwest::header::HeaderMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

// ---------------------------------------------------------------------------------------------
// fixtures

fn config() -> Config {
    Config {
        config_path: "/tmp/fastopm-test/config.yaml".into(),
        storage: "/tmp/fastopm-test/storage".into(),
        listen: Vec::new(),
        url_prefix: None,
        uplinks: IndexMap::new(),
        packages: Vec::new(),
        log: LogConfig::default(),
        http_proxy: None,
        https_proxy: None,
        no_proxy: Vec::new(),
        user_agent: "test-agent/1.0".into(),
        server_id: "srv123".into(),
        public_url: None,
        forwarded_proto_header: "x-forwarded-proto".into(),
        fastopm: Default::default(),
        warnings: Vec::new(),
    }
}

fn uplink_config(url: &str) -> UplinkConfig {
    default_uplink_config("up".into(), url.to_owned())
}

/// Uplink with short backoff so retry tests stay fast.
fn uplink(cfg: UplinkConfig) -> HttpUplink {
    uplink_with(cfg, &config())
}

fn uplink_with(cfg: UplinkConfig, config: &Config) -> HttpUplink {
    let mut uplink = HttpUplink::new(&cfg, config).expect("uplink");
    uplink.backoff_unit = Duration::from_millis(5);
    uplink
}

#[derive(Debug, Clone)]
struct Recorded {
    uri: String,
    headers: HeaderMap,
}

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: Duration,
    chunked: bool,
}

impl Reply {
    fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Reply {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
            chunked: false,
        }
    }
    fn json(body: &str) -> Self {
        Reply::new(200, body).header("content-type", "application/json")
    }
    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
    fn chunked(mut self) -> Self {
        self.chunked = true;
        self
    }
}

type Script = Arc<dyn Fn(usize) -> Reply + Send + Sync>;

struct Mock {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Mock {
    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn last(&self) -> Recorded {
        self.requests().pop().expect("a request")
    }
}

/// HTTP server answering request number `i` (from 0) with `script(i)`, recording every request.
async fn mock(script: impl Fn(usize) -> Reply + Send + Sync + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
    let script: Script = Arc::new(script);
    let recorded = requests.clone();
    let app = axum::Router::new().fallback(move |req: Request| {
        let recorded = recorded.clone();
        let script = script.clone();
        async move {
            let index = {
                let mut list = recorded.lock().unwrap();
                list.push(Recorded { uri: req.uri().to_string(), headers: req.headers().clone() });
                list.len() - 1
            };
            let reply = script(index);
            if !reply.delay.is_zero() {
                tokio::time::sleep(reply.delay).await;
            }
            let mut builder = axum::http::Response::builder().status(reply.status);
            for (name, value) in &reply.headers {
                builder = builder.header(name, value);
            }
            let body = if reply.chunked {
                let chunks: Vec<Result<Bytes, std::io::Error>> =
                    reply.body.chunks(3).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
                Body::from_stream(futures_util::stream::iter(chunks))
            } else {
                Body::from(reply.body)
            };
            builder.body(body).unwrap()
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Mock { url: format!("http://{addr}"), requests }
}

/// TCP server that reads each request head, writes `response` verbatim and closes the
/// connection. Returns the base URL and the number of connections served.
async fn raw_server(response: &'static [u8]) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let served = count.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            served.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = socket.write_all(response).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), count)
}

/// A local URL nothing listens on.
async fn refused_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

fn header<'a>(r: &'a Recorded, name: &str) -> Option<&'a str> {
    r.headers.get(name).map(|v| v.to_str().unwrap())
}

async fn collect(body: TarballBody) -> (Vec<u8>, Option<std::io::Error>) {
    let mut stream = body.stream;
    let mut data = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => data.extend_from_slice(&chunk),
            Err(e) => {
                assert!(stream.next().await.is_none(), "nothing after an error");
                return (data, Some(e));
            }
        }
    }
    (data, None)
}

const PKG: &str = r#"{"name":"pkg","versions":{},"dist-tags":{}}"#;

// ---------------------------------------------------------------------------------------------
// pure functions

#[test]
fn encodes_package_names_like_verdaccio() {
    assert_eq!(encode_package_name("lodash"), "lodash");
    assert_eq!(encode_package_name("@scope/pkg"), "@scope%2Fpkg");
    assert_eq!(encode_package_name("a@b"), "a%40b");
    assert_eq!(encode_package_name("a b/c"), "a%20b%2Fc");
    assert_eq!(encode_package_name("x-_.!~*'()"), "x-_.!~*'()");
    assert_eq!(encode_package_name("ü"), "%C3%BC");
}

#[test]
fn serves_url_on_segment_boundary_with_default_ports() {
    let up = "https://registry.example.com";
    assert!(uplink_serves_url(up, "https://registry.example.com/a/-/a-1.0.0.tgz"));
    assert!(uplink_serves_url(up, "https://REGISTRY.example.com:443/a/-/a-1.tgz"));
    assert!(!uplink_serves_url(up, "http://registry.example.com/a/-/a-1.tgz"));
    assert!(!uplink_serves_url(up, "https://registry.example.com:8443/a/-/a-1.tgz"));
    assert!(!uplink_serves_url(up, "https://cdn.example.com/a/-/a-1.tgz"));
    let sub = "http://host:8080/registry";
    assert!(uplink_serves_url(sub, "http://host:8080/registry/a/-/a-1.tgz"));
    assert!(!uplink_serves_url(sub, "http://host:8080/registry2/a/-/a-1.tgz"));
    assert!(!uplink_serves_url(sub, "http://host:8080/a/-/a-1.tgz"));
    assert!(uplink_serves_url("http://host/registry/", "http://host/registry/x"));
    assert!(!uplink_serves_url("not a url", "http://host/x"));
    assert!(!uplink_serves_url(sub, "/registry/a.tgz"));
}

#[test]
fn manifest_error_mapping() {
    let check = |e: UplinkError, status: u16, message: &str| {
        let err = e.to_manifest_error();
        assert_eq!((err.status(), err.message()), (status, message), "{e:?}");
    };
    check(UplinkError::Offline, 500, "uplink is offline");
    check(UplinkError::NotFound, 404, "package does not exist on uplink");
    check(UplinkError::BadStatus(503), 500, "bad status code: 503");
    check(UplinkError::Timeout, 503, "looks like the server is taking to long to respond");
    check(UplinkError::Transport("x".into()), 500, "internal server error");
    check(UplinkError::InvalidBody("x".into()), 500, "internal server error");
    check(UplinkError::Auth("Auth invalid".into()), 500, "internal server error");
    assert!(UplinkError::Timeout.is_timeout());
    assert!(!UplinkError::BadStatus(503).is_timeout());
}

#[test]
fn tarball_error_mapping() {
    let check = |e: UplinkError, status: u16, message: &str| {
        let err = e.to_tarball_error();
        assert_eq!((err.status(), err.message()), (status, message), "{e:?}");
    };
    check(UplinkError::NotFound, 404, "file doesn't exist on uplink");
    check(UplinkError::Unauthorized, 401, "unauthorized access");
    check(UplinkError::Forbidden("nope".into()), 403, "nope");
    check(UplinkError::BadStatus(502), 500, "bad uplink status code: 502");
    check(UplinkError::Timeout, 500, "internal server error");
    check(UplinkError::Offline, 500, "internal server error");
}

#[test]
fn build_rejects_bad_urls_and_keeps_order() {
    let mut cfg = config();
    for name in ["b", "a", "c"] {
        let mut up = uplink_config("https://registry.example.com");
        up.name = name.into();
        cfg.uplinks.insert(name.into(), up);
    }
    let built = build_uplinks(&cfg).unwrap();
    assert_eq!(built.keys().collect::<Vec<_>>(), ["b", "a", "c"]);
    assert_eq!(built["a"].name(), "a");

    for url in ["not a url", "ftp://host/x", "/relative"] {
        let err = HttpUplink::new(&uplink_config(url), &config()).unwrap_err();
        assert!(matches!(err, UplinkBuildError::InvalidUrl { .. }), "{url}");
    }
    assert!(HttpUplink::for_foreign_url("pkg", "/pkg/-/pkg-1.0.0.tgz", &config()).is_err());
    let foreign =
        HttpUplink::for_foreign_url("pkg", "https://cdn.example.com/pkg-1.tgz", &config()).unwrap();
    assert_eq!(foreign.name(), "verdaccio-pkg");
    assert!(foreign.cache_tarballs());
    assert_eq!(foreign.config().retry, 2);
    assert_eq!(foreign.config().timeout, Duration::from_secs(30));
    assert!(foreign.config().auth.is_none());
}

// ---------------------------------------------------------------------------------------------
// headers

#[tokio::test]
async fn manifest_request_headers() {
    let server = mock(|_| Reply::json(PKG).header("etag", "\"abc\"")).await;
    let up = uplink(uplink_config(&server.url));
    let fetched = up.fetch_manifest("@scope/pkg", None, Some("10.0.0.7")).await.unwrap();
    let ManifestFetch::Fetched { manifest, etag } = fetched else { panic!("expected a manifest") };
    assert_eq!(manifest.name(), Some("pkg"));
    assert_eq!(etag.as_deref(), Some("\"abc\""));

    let req = server.last();
    assert_eq!(req.uri, "/@scope%2Fpkg");
    assert_eq!(header(&req, "accept"), Some("application/json;"));
    assert_eq!(header(&req, "accept-encoding"), Some("gzip"));
    assert_eq!(header(&req, "user-agent"), Some("npm (test-agent/1.0)"));
    assert_eq!(header(&req, "x-forwarded-for"), Some("10.0.0.7"));
    assert_eq!(header(&req, "via"), Some("1.1 srv123 (Verdaccio)"));
    assert_eq!(header(&req, "authorization"), None);
    assert_eq!(header(&req, "if-none-match"), None);

    // No client address: Verdaccio concatenates `undefined`.
    up.fetch_manifest("pkg", None, None).await.unwrap();
    assert_eq!(header(&server.last(), "x-forwarded-for"), Some("undefined"));
}

#[tokio::test]
async fn config_headers_and_auth_on_manifests_and_tarballs() {
    // Same setup as the compat scenario `misc/uplink-headers-and-auth`.
    let server = mock(|i| match i {
        0 | 1 => Reply::json(PKG),
        _ => Reply::new(200, "tarball"),
    })
    .await;
    let mut cfg = uplink_config(&server.url);
    cfg.headers.insert("x-custom".into(), "one".into());
    cfg.headers.insert("accept".into(), "application/vnd.npm.install-v1+json".into());
    cfg.auth = Some(UplinkAuth {
        auth_type: Some("bearer".into()),
        token: TokenSource::Inline("uplink-secret-token".into()),
    });
    let up = uplink(cfg);

    up.fetch_manifest("pkg", None, Some("1.2.3.4")).await.unwrap();
    up.fetch_manifest("pkg", Some("\"e1\""), Some("1.2.3.4")).await.unwrap();
    let body = up.fetch_tarball(&format!("{}/pkg/-/pkg-1.0.0.tgz", server.url)).await.unwrap();
    assert_eq!(collect(body).await.0, b"tarball");

    let reqs = server.requests();
    let (plain, conditional, tarball) = (&reqs[0], &reqs[1], &reqs[2]);
    for r in [plain, conditional, tarball] {
        assert_eq!(header(r, "x-custom"), Some("one"));
        assert_eq!(header(r, "authorization"), Some("Bearer uplink-secret-token"));
        assert_eq!(header(r, "user-agent"), Some("npm (test-agent/1.0)"));
        assert_eq!(header(r, "via"), Some("1.1 srv123 (Verdaccio)"));
    }
    // `accept` from the config is a later key than Verdaccio's `Accept`.
    assert_eq!(header(plain, "accept"), Some("application/vnd.npm.install-v1+json"));
    assert_eq!(header(plain, "x-forwarded-for"), Some("1.2.3.4"));
    // Re-assigning `Accept` for the conditional request keeps its early position.
    assert_eq!(header(conditional, "accept"), Some("application/vnd.npm.install-v1+json"));
    assert_eq!(header(conditional, "if-none-match"), Some("\"e1\""));
    // Tarballs start from the config object: `Accept` is appended after `accept`.
    assert_eq!(header(tarball, "accept"), Some("application/json;"));
    assert_eq!(header(tarball, "x-forwarded-for"), Some("undefined"));
    assert_eq!(tarball.uri, "/pkg/-/pkg-1.0.0.tgz");
}

#[tokio::test]
async fn exact_case_accept_in_config_is_overridden_on_conditional_requests() {
    let server = mock(|_| Reply::new(304, "")).await;
    let mut cfg = uplink_config(&server.url);
    cfg.headers.insert("Accept".into(), "text/plain".into());
    cfg.headers.insert("Via".into(), "1.1 front".into());
    let up = uplink(cfg);
    assert_eq!(
        up.fetch_manifest("pkg", Some("W/\"x\""), Some("::1")).await.unwrap(),
        ManifestFetch::NotModified
    );
    let req = server.last();
    assert_eq!(header(&req, "accept"), Some("application/json;"));
    assert_eq!(header(&req, "if-none-match"), Some("W/\"x\""));
    // `Via` from the config is a later key than `via`: it replaces the computed value.
    assert_eq!(header(&req, "via"), Some("1.1 front"));
}

#[tokio::test]
async fn tarball_headers_extend_config_values() {
    let server = mock(|_| Reply::new(200, "x")).await;
    let mut cfg = uplink_config(&server.url);
    cfg.headers.insert("via".into(), "1.1 front".into());
    cfg.headers.insert("x-forwarded-for".into(), "9.9.9.9".into());
    cfg.headers.insert("User-Agent".into(), "custom".into());
    let up = uplink(cfg);
    up.fetch_tarball(&format!("{}/a.tgz", server.url)).await.unwrap();
    let req = server.last();
    assert_eq!(header(&req, "via"), Some("1.1 front, 1.1 srv123 (Verdaccio)"));
    // `X-Forwarded-For` is a later key than the config's `x-forwarded-for`.
    assert_eq!(header(&req, "x-forwarded-for"), Some("9.9.9.9, undefined"));
    assert_eq!(header(&req, "user-agent"), Some("custom"));
}

// ---------------------------------------------------------------------------------------------
// auth

fn with_auth(url: &str, auth_type: Option<&str>, token: TokenSource) -> UplinkConfig {
    let mut cfg = uplink_config(url);
    cfg.auth = Some(UplinkAuth { auth_type: auth_type.map(str::to_owned), token });
    cfg
}

#[tokio::test]
async fn auth_variants() {
    let server = mock(|_| Reply::json(PKG)).await;
    let url = server.url.clone();
    let sent = |up: HttpUplink| {
        let server = &server;
        async move {
            up.fetch_manifest("pkg", None, Some("::1")).await.unwrap();
            header(&server.last(), "authorization").map(str::to_owned)
        }
    };

    // Default type is Basic; the type is capitalized.
    let up = uplink(with_auth(&url, None, TokenSource::Inline("t0".into())));
    assert_eq!(sent(up).await.as_deref(), Some("Basic t0"));
    let up = uplink(with_auth(&url, Some(""), TokenSource::Inline("t1".into())));
    assert_eq!(sent(up).await.as_deref(), Some("Basic t1"));
    let up = uplink(with_auth(&url, Some("BEARER"), TokenSource::Inline("t2".into())));
    assert_eq!(sent(up).await.as_deref(), Some("Bearer t2"));

    // Token from the environment, read on each request.
    let mut up = uplink(with_auth(&url, Some("bearer"), TokenSource::Env("MY_TOKEN".into())));
    let value = Arc::new(Mutex::new(Some("env-1".to_owned())));
    let source = value.clone();
    up.env = Arc::new(move |key| {
        assert_eq!(key, "MY_TOKEN");
        source.lock().unwrap().clone()
    });
    up.fetch_manifest("pkg", None, None).await.unwrap();
    assert_eq!(header(&server.last(), "authorization"), Some("Bearer env-1"));
    *value.lock().unwrap() = None;
    let before = server.count();
    assert_eq!(
        up.fetch_manifest("pkg", None, None).await,
        Err(UplinkError::Auth("token is required".into()))
    );
    assert_eq!(
        up.fetch_tarball(&format!("{url}/a.tgz")).await.unwrap_err(),
        UplinkError::Auth("token is required".into())
    );
    assert_eq!(server.count(), before, "nothing sent on auth errors");

    let errors = [
        (None, TokenSource::Missing, "token is required"),
        (None, TokenSource::Invalid, "Auth invalid"),
        (Some("Digest"), TokenSource::Inline("t".into()), "Auth type 'digest' not allowed"),
    ];
    for (auth_type, token, message) in errors {
        let up = uplink(with_auth(&url, auth_type, token));
        assert_eq!(
            up.fetch_manifest("pkg", None, None).await,
            Err(UplinkError::Auth(message.into()))
        );
    }
}

#[tokio::test]
async fn config_authorization_header() {
    let server = mock(|_| Reply::json(PKG)).await;
    let mut cfg = with_auth(&server.url, Some("bearer"), TokenSource::Env("UNSET".into()));
    cfg.headers.insert("authorization".into(), "Basic from-config".into());
    let mut up = uplink(cfg);
    up.env = Arc::new(|_| None);
    // Tarballs: the config object already has `authorization`, so `auth` is skipped.
    up.fetch_tarball(&format!("{}/a.tgz", server.url)).await.unwrap();
    assert_eq!(header(&server.last(), "authorization"), Some("Basic from-config"));
    // Manifests compute `auth` before applying config headers, so the missing token fails.
    assert_eq!(
        up.fetch_manifest("pkg", None, None).await,
        Err(UplinkError::Auth("token is required".into()))
    );

    up.env = Arc::new(|_| Some("tok".into()));
    up.fetch_manifest("pkg", None, None).await.unwrap();
    assert_eq!(header(&server.last(), "authorization"), Some("Basic from-config"));
}

// ---------------------------------------------------------------------------------------------
// manifests: statuses, bodies, retries, breaker

#[tokio::test]
async fn not_modified_and_bad_bodies() {
    let server = mock(|i| match i {
        0 => Reply::new(304, ""),
        1 => Reply::json("[1,2]"),
        2 => Reply::json("{not json"),
        _ => Reply::new(200, ""),
    })
    .await;
    let up = uplink(uplink_config(&server.url));
    assert_eq!(up.fetch_manifest("pkg", Some("\"e\""), None).await, Ok(ManifestFetch::NotModified));
    for _ in 0..3 {
        let err = up.fetch_manifest("pkg", None, None).await.unwrap_err();
        assert!(matches!(err, UplinkError::InvalidBody(_)), "{err:?}");
    }
    assert_eq!(server.count(), 4, "neither 304 nor parse errors are retried");
    assert!(up.is_available());
}

#[tokio::test]
async fn large_manifest_is_parsed() {
    let mut big = String::from(r#"{"name":"big","versions":{"#);
    for i in 0..5000 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!(
            r#""1.0.{i}":{{"name":"big","version":"1.0.{i}","description":"{}"}}"#,
            "x".repeat(40)
        ));
    }
    big.push_str("}}");
    assert!(big.len() > BLOCKING_PARSE_THRESHOLD);
    let server = mock(move |_| Reply::json(&big)).await;
    let up = uplink(uplink_config(&server.url));
    let ManifestFetch::Fetched { manifest, .. } =
        up.fetch_manifest("big", None, None).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(manifest.versions().map(|v| v.len()), Some(5000));
}

#[tokio::test]
async fn not_found_is_not_retried() {
    let server = mock(|_| Reply::new(404, r#"{"error":"not found"}"#)).await;
    let up = uplink(uplink_config(&server.url));
    assert_eq!(up.fetch_manifest("nope", None, None).await, Err(UplinkError::NotFound));
    assert_eq!(server.count(), 1);
    assert!(up.is_available());
}

#[tokio::test]
async fn server_errors_are_retried_and_open_the_breaker() {
    let server = mock(|_| Reply::new(500, "boom")).await;
    let up = uplink(uplink_config(&server.url));
    assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::BadStatus(500)));
    assert_eq!(server.count(), 3, "1 attempt + retry 2");
    // Two retries reached max_fails 2.
    assert!(!up.is_available());
    assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::Offline));
    assert_eq!(server.count(), 3, "nothing sent while offline");
    // Tarballs ignore the breaker.
    assert_eq!(
        up.fetch_tarball(&format!("{}/a.tgz", server.url)).await.unwrap_err(),
        UplinkError::BadStatus(500)
    );
    assert_eq!(server.count(), 4, "tarballs are not retried");
}

#[tokio::test]
async fn retry_then_success_and_status_503_is_not_a_timeout() {
    let server = mock(|i| match i {
        0 => Reply::new(503, ""),
        1 => Reply::json(PKG),
        _ => Reply::new(503, ""),
    })
    .await;
    let mut cfg = uplink_config(&server.url);
    cfg.max_fails = 5;
    let up = uplink(cfg);
    assert!(matches!(
        up.fetch_manifest("pkg", None, None).await,
        Ok(ManifestFetch::Fetched { .. })
    ));
    assert_eq!(server.count(), 2);
    let err = up.fetch_manifest("pkg", None, None).await.unwrap_err();
    assert_eq!(err, UplinkError::BadStatus(503));
    assert!(!err.is_timeout());
    assert_eq!(server.count(), 5);
    assert!(up.is_available(), "2 failures < max_fails 5");
}

#[tokio::test]
async fn retry_limit_zero_never_opens_the_breaker() {
    let server = mock(|_| Reply::new(502, "")).await;
    let mut cfg = uplink_config(&server.url);
    cfg.retry = 0;
    let up = uplink(cfg);
    for _ in 0..3 {
        assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::BadStatus(502)));
    }
    assert_eq!(server.count(), 3);
    assert!(up.is_available());
}

#[tokio::test]
async fn retry_after_longer_than_timeout_stops_retries() {
    let server = mock(|i| match i {
        0 => Reply::new(429, "").header("retry-after", "120"),
        1 => Reply::new(413, "").header("retry-after", "0"),
        2 => Reply::json(PKG),
        _ => Reply::new(413, ""),
    })
    .await;
    let up = uplink(uplink_config(&server.url));
    assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::BadStatus(429)));
    assert_eq!(server.count(), 1, "120s > timeout 30s: no retry");
    // 413 with Retry-After is retried.
    assert!(up.fetch_manifest("pkg", None, None).await.is_ok());
    assert_eq!(server.count(), 3);
    // 413 without Retry-After is not.
    assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::BadStatus(413)));
    assert_eq!(server.count(), 4);
}

#[tokio::test]
async fn timeouts_are_retried_then_reported() {
    let server = mock(|_| Reply::json(PKG).delay(Duration::from_millis(400))).await;
    let mut cfg = uplink_config(&server.url);
    cfg.timeout = Duration::from_millis(100);
    cfg.retry = 1;
    cfg.max_fails = 1;
    let up = uplink(cfg);
    let err = up.fetch_manifest("pkg", None, None).await.unwrap_err();
    assert_eq!(err, UplinkError::Timeout);
    assert_eq!(err.to_manifest_error().status(), 503);
    assert_eq!(server.count(), 2);
    assert!(!up.is_available());
}

#[tokio::test]
async fn connection_reset_is_a_timeout() {
    let (url, connections) = raw_server(b"").await;
    let up = uplink(uplink_config(&url));
    assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::Timeout));
    assert!(connections.load(Ordering::SeqCst) >= 3, "reset is retried");
    assert!(!up.is_available());
}

#[tokio::test]
async fn connection_refused_is_retried_then_transport() {
    let url = refused_url().await;
    let up = uplink(uplink_config(&url));
    let err = up.fetch_manifest("pkg", None, None).await.unwrap_err();
    assert!(matches!(err, UplinkError::Transport(_)), "{err:?}");
    assert_eq!(err.to_manifest_error().status(), 500);
    // ECONNREFUSED is retryable, so the breaker opened.
    assert!(!up.is_available());
    let err = up.fetch_tarball(&format!("{url}/a.tgz")).await.unwrap_err();
    assert!(matches!(err, UplinkError::Transport(_)), "{err:?}");
}

#[tokio::test]
async fn breaker_closes_after_fail_timeout_and_resets_on_success() {
    let server = mock(|i| if i < 3 { Reply::new(500, "") } else { Reply::json(PKG) }).await;
    let mut cfg = uplink_config(&server.url);
    cfg.fail_timeout = Duration::from_millis(300);
    let up = uplink(cfg);
    assert!(up.is_available(), "no request yet");
    assert!(up.fetch_manifest("pkg", None, None).await.is_err());
    assert!(!up.is_available());
    assert_eq!(up.fetch_manifest("pkg", None, None).await, Err(UplinkError::Offline));
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(up.is_available(), "fail_timeout elapsed");
    assert!(up.fetch_manifest("pkg", None, None).await.is_ok());
    assert_eq!(up.breaker().failed_requests, 0);
    assert!(up.is_available());
}

#[tokio::test]
async fn tarball_request_extends_the_open_window() {
    let server = mock(|i| if i < 3 { Reply::new(500, "") } else { Reply::new(200, "t") }).await;
    let mut cfg = uplink_config(&server.url);
    cfg.fail_timeout = Duration::from_millis(300);
    let up = uplink(cfg);
    assert!(up.fetch_manifest("pkg", None, None).await.is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;
    up.fetch_tarball(&format!("{}/a.tgz", server.url)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // 400 ms after the failure but only 200 ms after the tarball request.
    assert!(!up.is_available());
}

// ---------------------------------------------------------------------------------------------
// tarballs

#[tokio::test]
async fn tarball_streams_with_content_length() {
    let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();
    let server = mock(move |i| {
        let reply =
            Reply::new(200, payload.clone()).header("content-type", "application/octet-stream");
        if i == 0 { reply } else { reply.chunked() }
    })
    .await;
    let up = uplink(uplink_config(&server.url));
    let url = format!("{}/pkg/-/pkg-1.0.0.tgz", server.url);

    let body = up.fetch_tarball(&url).await.unwrap();
    assert_eq!(body.content_length, Some(expected.len() as u64));
    let (data, err) = collect(body).await;
    assert!(err.is_none());
    assert_eq!(data, expected);

    let body = up.fetch_tarball(&url).await.unwrap();
    assert_eq!(body.content_length, None, "chunked response");
    assert_eq!(collect(body).await.0, expected);
}

#[tokio::test]
async fn tarball_status_mapping() {
    let server = mock(|i| match i {
        0 => Reply::new(404, ""),
        1 => Reply::new(401, ""),
        2 => Reply::new(403, r#"{"detail":"blocked by policy","message":"m"}"#),
        3 => Reply::new(403, r#"{"title":"t","message":"m"}"#),
        4 => Reply::new(403, r#"{"detail":"","message":"m"}"#),
        5 => Reply::new(403, "<html>forbidden</html>"),
        6 => Reply::new(403, r#""just a string""#),
        7 => Reply::new(502, ""),
        _ => Reply::new(304, ""),
    })
    .await;
    let up = uplink(uplink_config(&server.url));
    let url = format!("{}/a.tgz", server.url);
    let forbidden = |m: &str| UplinkError::Forbidden(m.to_owned());
    let expected = [
        UplinkError::NotFound,
        UplinkError::Unauthorized,
        forbidden("blocked by policy"),
        forbidden("t"),
        forbidden("m"),
        forbidden("tarball not available: forbidden by uplink"),
        forbidden("tarball not available: forbidden by uplink"),
        UplinkError::BadStatus(502),
        UplinkError::BadStatus(304),
    ];
    for want in expected {
        assert_eq!(up.fetch_tarball(&url).await.unwrap_err(), want);
    }
    assert_eq!(server.count(), 9, "no retries");
}

#[tokio::test]
async fn truncated_tarball_ends_with_an_error() {
    let (url, _) = raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 100\r\n\r\n0123456789",
    )
    .await;
    let up = uplink(uplink_config(&url));
    let body = up.fetch_tarball(&format!("{url}/a.tgz")).await.unwrap();
    assert_eq!(body.content_length, Some(100));
    let (data, err) = collect(body).await;
    assert_eq!(data, b"0123456789");
    assert!(err.is_some(), "a short body must not end cleanly");
}

#[tokio::test]
async fn length_check_of_the_stream_wrapper() {
    // The wrapper itself, independent of hyper's own framing checks.
    let chunks: Vec<reqwest::Result<Bytes>> = vec![Ok(Bytes::from_static(b"abc"))];
    let body = CheckedBody::new(Box::pin(futures_util::stream::iter(chunks)), Some(5));
    let (data, err) = collect(TarballBody::new(Box::pin(body), Some(5))).await;
    assert_eq!(data, b"abc");
    assert_eq!(err.unwrap().to_string(), "content length mismatch");

    let chunks: Vec<reqwest::Result<Bytes>> = vec![Ok(Bytes::from_static(b"abc"))];
    let body = CheckedBody::new(Box::pin(futures_util::stream::iter(chunks)), Some(3));
    let (_, err) = collect(TarballBody::new(Box::pin(body), Some(3))).await;
    assert!(err.is_none());
}

#[tokio::test]
async fn foreign_uplink_sends_no_credentials() {
    let server = mock(|_| Reply::new(200, "cdn")).await;
    let url = format!("{}/files/pkg-1.0.0.tgz", server.url);
    let up = HttpUplink::for_foreign_url("pkg", &url, &config()).unwrap();
    let body = up.fetch_tarball(&url).await.unwrap();
    assert_eq!(collect(body).await.0, b"cdn");
    let req = server.last();
    assert_eq!(req.uri, "/files/pkg-1.0.0.tgz");
    assert_eq!(header(&req, "authorization"), None);
    assert_eq!(header(&req, "user-agent"), Some("npm (test-agent/1.0)"));
    assert_eq!(header(&req, "via"), Some("1.1 srv123 (Verdaccio)"));
    assert_eq!(header(&req, "accept"), Some("application/json;"));
}

// ---------------------------------------------------------------------------------------------
// proxy

#[tokio::test]
async fn http_proxy_is_used_without_x_forwarded_for() {
    let proxy = mock(|_| Reply::json(PKG)).await;
    let cfg = uplink_config("http://upstream.invalid:4873");
    let up = HttpUplink::build(&cfg, &config(), Some(&proxy.url)).unwrap();
    assert!(up.uses_proxy);
    up.fetch_manifest("@s/p", None, Some("10.0.0.1")).await.unwrap();
    up.fetch_tarball("http://upstream.invalid:4873/p/-/p-1.0.0.tgz").await.unwrap();
    let reqs = proxy.requests();
    assert_eq!(reqs[0].uri, "http://upstream.invalid:4873/@s%2Fp");
    assert_eq!(reqs[1].uri, "http://upstream.invalid:4873/p/-/p-1.0.0.tgz");
    for req in &reqs {
        assert_eq!(header(req, "x-forwarded-for"), None);
        assert_eq!(header(req, "via"), Some("1.1 srv123 (Verdaccio)"));
    }
    // An empty proxy value is no proxy (`if (!this.proxy)`).
    assert!(!HttpUplink::build(&cfg, &config(), Some("")).unwrap().uses_proxy);
    assert!(matches!(
        HttpUplink::build(&cfg, &config(), Some("::not a url::")),
        Err(UplinkBuildError::Client { .. })
    ));
}

#[tokio::test]
async fn proxy_matches_the_uplink_scheme_only() {
    // An `http:` uplink proxies only `http:` requests (hpagent `{ http: agent }`), so an
    // `https:` URL fetched through it bypasses the proxy, like Verdaccio.
    let proxy = mock(|_| Reply::json(PKG)).await;
    let cfg = uplink_config("https://upstream.invalid");
    let up = HttpUplink::build(&cfg, &config(), Some(&proxy.url)).unwrap();
    let direct = mock(|_| Reply::new(200, "t")).await;
    up.fetch_tarball(&format!("{}/a.tgz", direct.url)).await.unwrap();
    assert_eq!(proxy.count(), 0);
    assert_eq!(direct.count(), 1);
}

#[tokio::test]
async fn new_uses_the_effective_proxy() {
    let proxy = mock(|_| Reply::json(PKG)).await;
    let mut cfg = config();
    cfg.http_proxy = Some("http://127.0.0.1:9".into());
    // The uplink's own key wins over the global one.
    let mut up_cfg = uplink_config("http://upstream.invalid:4873");
    up_cfg.http_proxy = Some(proxy.url.clone());
    let up = uplink_with(up_cfg, &cfg);
    assert!(up.uses_proxy);
    up.fetch_manifest("pkg", None, None).await.unwrap();
    assert_eq!(proxy.last().uri, "http://upstream.invalid:4873/pkg");

    // The foreign uplink uses the global settings, checked against the tarball host.
    let mut cfg = config();
    cfg.http_proxy = Some(proxy.url.clone());
    let foreign = HttpUplink::for_foreign_url("pkg", "http://cdn.invalid/pkg-1.tgz", &cfg).unwrap();
    assert!(foreign.uses_proxy);
    cfg.no_proxy = vec!["cdn.invalid".into()];
    let foreign = HttpUplink::for_foreign_url("pkg", "http://cdn.invalid/pkg-1.tgz", &cfg).unwrap();
    assert!(!foreign.uses_proxy);
}

#[tokio::test]
async fn no_proxy_and_scheme_select_a_direct_connection() {
    let proxy = mock(|_| Reply::json(PKG)).await;
    let direct = mock(|_| Reply::json(PKG)).await;

    // Excluded by `no_proxy`.
    let mut cfg = config();
    cfg.http_proxy = Some(proxy.url.clone());
    cfg.no_proxy = vec!["127.0.0.1".into()];
    let up = uplink_with(uplink_config(&direct.url), &cfg);
    assert!(!up.uses_proxy);
    up.fetch_manifest("pkg", None, Some("10.0.0.1")).await.unwrap();
    assert_eq!(header(&direct.last(), "x-forwarded-for"), Some("10.0.0.1"));

    // Only `https_proxy` set: an `http:` uplink connects directly.
    let mut cfg = config();
    cfg.https_proxy = Some(proxy.url.clone());
    let up = uplink_with(uplink_config(&direct.url), &cfg);
    assert!(!up.uses_proxy);
    up.fetch_manifest("pkg", None, None).await.unwrap();

    assert_eq!(proxy.count(), 0);
    assert_eq!(direct.count(), 2);
}
