//! Client for one uplink registry.
//!
//! [`Uplink`] is the seam `fastopm-store` depends on; [`HttpUplink`] is the real implementation
//! (reqwest + rustls). Store tests use an in-memory implementation of the same trait.
//!
//! Request headers are built the way Verdaccio builds them (`getHeaders`, `addProxyHeaders`,
//! `applyUplinkHeaders` in `packages/proxy/src/proxy.ts`), in a JavaScript object whose keys
//! are case-sensitive; got then lower-cases the names and, when two keys differ only in case,
//! the one inserted later wins. Manifest requests start from an empty object:
//!
//! 1. `Accept: application/json;`, `Accept-Encoding: gzip`, `User-Agent: npm (<user_agent>)`;
//! 2. `authorization: <Type> <token>` from `auth` (`type` capitalized like lodash
//!    `capitalize`; only `basic` / `bearer`, case-insensitive, are allowed);
//! 3. `X-Forwarded-For: <client ip>` only when no HTTP proxy is in use, and
//!    `via: 1.1 <server_id> (Verdaccio)` always;
//! 4. `uplinks.<name>.headers`, assigned key by key, so they override the above;
//! 5. for conditional requests, `If-None-Match: <etag>` and `Accept: application/json;` again.
//!    Re-assigning `Accept` keeps its original position, so a config key spelled `accept`
//!    (another key for JavaScript) still wins over it.
//!
//! Tarball requests start from a copy of `uplinks.<name>.headers` instead (Verdaccio passes
//! the config object as the base): steps 1 to 3 only fill keys that are absent or empty under
//! the exact spellings above, `auth` is skipped when the config has an `authorization` key,
//! step 4 does nothing new, there is no step 5, and `X-Forwarded-For` is the literal
//! `undefined` (Verdaccio does not pass the client address there).
//!
//! Circuit breaker (`_ifRequestFailure`, `breakerHooks`): before each retry the uplink records
//! the retry number as its failure count; once the count reaches `max_fails` and the last
//! attempt started less than `fail_timeout` ago, manifest requests fail immediately with
//! [`UplinkError::Offline`]. A 2xx manifest response resets the count. Tarball downloads do not
//! check the breaker and are not retried (got streams only retry with a `retry` listener),
//! but they do count as the last request time.
//!
//! Verdaccio reference: `packages/proxy/src/proxy.ts`, `uplink-util.ts`,
//! `uplinkServesUrl` in `packages/store/src/lib/storage-utils.ts`.

mod body;
mod js_headers;
mod retry;
#[cfg(test)]
mod tests;

use std::collections::hash_map::RandomState;
use std::fmt;
use std::future::Future;
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use fastopm_config::{Config, TokenSource, UplinkConfig};
use fastopm_core::memory::{Reservation, predict_heap_bytes};
use fastopm_core::{Manifest, RegistryError, TarballBody, headers as hdr, messages, status};
use indexmap::IndexMap;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{CONTENT_LENGTH, ETAG, HeaderMap, RETRY_AFTER};
use serde_json::Value;
use url::Url;

use crate::body::CheckedBody;
use crate::js_headers::JsHeaders;
use crate::retry::{Failure, NetCode};

/// Result of a manifest request.
#[derive(Debug, Clone, PartialEq)]
pub enum ManifestFetch {
    /// 304: the manifest identified by the `etag` sent is still current.
    NotModified,
    /// 200 with a JSON object body.
    Fetched {
        /// The uplink document, keys in received order.
        manifest: Manifest,
        /// `ETag` response header, if any (stored in `_uplinks[<name>].etag`).
        etag: Option<String>,
    },
}

/// Why an uplink request failed.
///
/// The variants keep what `fastopm-store` needs to pick the client error the way Verdaccio does
/// (`syncUplinksMetadata` and `fetchTarballFromDistFile` in `packages/store/src/storage.ts`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UplinkError {
    /// The circuit breaker is open; no request was sent.
    #[error("uplink is offline")]
    Offline,
    /// The uplink answered 404.
    #[error("not found on uplink")]
    NotFound,
    /// The uplink answered 401 (tarballs).
    #[error("unauthorized by uplink")]
    Unauthorized,
    /// The uplink answered 403 (tarballs). The message is `detail`, `title` or `message` from the
    /// JSON body, else `tarball not available: forbidden by uplink`.
    #[error("forbidden by uplink: {0}")]
    Forbidden(String),
    /// Any other non-2xx status after retries (manifests: also 401 and 403). A 503 from the
    /// uplink lands here too, not in [`UplinkError::Timeout`].
    #[error("bad status code: {0}")]
    BadStatus(u16),
    /// The request timed out or the connection was reset (`ETIMEDOUT`, `ESOCKETTIMEDOUT`,
    /// `ECONNRESET`) after retries.
    #[error("uplink timed out")]
    Timeout,
    /// Any other network failure: connection refused, DNS, TLS, proxy...
    #[error("uplink request failed: {0}")]
    Transport(String),
    /// A 200 whose body is not a JSON object.
    #[error("invalid uplink response: {0}")]
    InvalidBody(String),
    /// `auth` cannot be applied; checked on every request like Verdaccio (`setAuthNext`).
    /// Messages: `Auth invalid` (`auth` is not an object), `token is required` (no token or
    /// the env variable is unset), `Auth type '<type>' not allowed`.
    #[error("{0}")]
    Auth(String),
}

impl UplinkError {
    /// Client error for a failed manifest request when no cached copy can be served
    /// (`getRemoteMetadata` error mapping):
    ///
    /// - `Offline` → `500 uplink is offline`
    /// - `NotFound` → `404 package does not exist on uplink`
    /// - `BadStatus(c)` → `500 bad status code: <c>`
    /// - `Timeout` → `503 looks like the server is taking to long to respond`
    /// - `Transport`, `InvalidBody`, `Auth` → `500 internal server error`
    /// - `Unauthorized` / `Forbidden` do not occur for manifests (they are `BadStatus`).
    ///
    /// Note that `fastopm-store` usually replaces these with `404 no such package available`
    /// (see its docs); only `Timeout` survives as 503.
    pub fn to_manifest_error(&self) -> RegistryError {
        match self {
            UplinkError::Offline => RegistryError::internal(messages::UPLINK_OFFLINE),
            UplinkError::NotFound => RegistryError::not_found(messages::NOT_PACKAGE_UPLINK),
            UplinkError::BadStatus(code) => {
                RegistryError::internal(format!("{}: {code}", messages::BAD_STATUS_CODE))
            }
            UplinkError::Timeout => RegistryError::service_unavailable(messages::SERVER_TIME_OUT),
            UplinkError::Unauthorized => {
                RegistryError::internal(format!("{}: {}", messages::BAD_STATUS_CODE, 401))
            }
            UplinkError::Forbidden(_) => {
                RegistryError::internal(format!("{}: {}", messages::BAD_STATUS_CODE, 403))
            }
            UplinkError::Transport(_) | UplinkError::InvalidBody(_) | UplinkError::Auth(_) => {
                RegistryError::internal(messages::INTERNAL_SERVER_ERROR)
            }
        }
    }

    /// Client error for a failed tarball download (`fetchTarballFromDistFile`):
    ///
    /// - `NotFound` → `404 file doesn't exist on uplink`
    /// - `Unauthorized` → `401 unauthorized access`
    /// - `Forbidden(m)` → `403 <m>`
    /// - `BadStatus(c)` → `500 bad uplink status code: <c>`
    /// - anything else → `500 internal server error`
    pub fn to_tarball_error(&self) -> RegistryError {
        match self {
            UplinkError::NotFound => RegistryError::not_found(messages::NOT_FILE_UPLINK),
            UplinkError::Unauthorized => RegistryError::unauthorized(messages::UNAUTHORIZED_ACCESS),
            UplinkError::Forbidden(message) => RegistryError::forbidden(message.clone()),
            UplinkError::BadStatus(code) => {
                RegistryError::internal(format!("{}: {code}", messages::BAD_UPLINK_STATUS_CODE))
            }
            _ => RegistryError::internal(messages::INTERNAL_SERVER_ERROR),
        }
    }

    /// Whether the store must answer 503 instead of 404 when nothing else is available.
    pub fn is_timeout(&self) -> bool {
        matches!(self, UplinkError::Timeout)
    }
}

/// The uplink operations `fastopm-store` needs.
pub trait Uplink: Send + Sync + 'static {
    /// Uplink name (key under `uplinks`; recorded in `_uplinks` and `_distfiles[*].registry`).
    fn name(&self) -> &str;

    /// `maxage`: a cached manifest fetched from this uplink less than this long ago is used
    /// without a request.
    fn maxage(&self) -> Duration;

    /// `cache`: whether tarballs downloaded through this uplink are written to storage.
    fn cache_tarballs(&self) -> bool;

    /// Whether `tarball_url` belongs to this uplink: same scheme, same host and port (default
    /// ports normalized) and the uplink URL path is a prefix of the tarball path on a segment
    /// boundary. Credentials are only ever sent to such URLs. See [`uplink_serves_url`].
    fn serves_url(&self, tarball_url: &str) -> bool;

    /// `false` while the circuit breaker is open.
    fn is_available(&self) -> bool;

    /// `GET <url>/<encoded name>`, where the name is encoded with [`encode_package_name`].
    ///
    /// With `etag`, sends `If-None-Match` and maps 304 to [`ManifestFetch::NotModified`].
    /// `remote_address` is the client IP for `X-Forwarded-For` (with request coalescing, the
    /// address of the first caller; `None` sends `undefined` like Verdaccio with no address).
    /// Retries up to `retry` times on network errors and retryable statuses; each retry feeds
    /// the circuit breaker. Fails with [`UplinkError::Offline`] without sending anything while
    /// the breaker is open.
    fn fetch_manifest(
        &self,
        name: &str,
        etag: Option<&str>,
        remote_address: Option<&str>,
    ) -> impl Future<Output = Result<ManifestFetch, UplinkError>> + Send;

    /// [`Uplink::fetch_manifest`], counting the memory it takes in `work`: the body as it
    /// streams in (waiting in [`Reservation::grow_to`] when the budget is spent, which pauses
    /// the download), then the parsed tree. On success `work` holds at least the tree's
    /// [`Manifest::heap_bytes`]. The default ignores `work`.
    fn fetch_manifest_within(
        &self,
        name: &str,
        etag: Option<&str>,
        remote_address: Option<&str>,
        work: &mut Reservation,
    ) -> impl Future<Output = Result<ManifestFetch, UplinkError>> + Send {
        let _ = work;
        self.fetch_manifest(name, etag, remote_address)
    }

    /// `GET <tarball_url>` with this uplink's headers and credentials. The caller has checked
    /// [`Uplink::serves_url`].
    ///
    /// Resolves once the response status is known: non-2xx statuses become errors (see
    /// [`UplinkError::to_tarball_error`]); 2xx returns a [`TarballBody`] whose `content_length`
    /// comes from the response header. The body is the decoded payload. When `content_length`
    /// is known and the stream ends with a different number of bytes, the stream's last item is
    /// an `io::Error` (Verdaccio: `content length mismatch`), so the store does not commit a
    /// truncated file. Not retried, like Verdaccio's got stream.
    fn fetch_tarball(
        &self,
        tarball_url: &str,
    ) -> impl Future<Output = Result<TarballBody, UplinkError>> + Send;

    /// Throw-away uplink for a tarball URL no configured uplink serves (Verdaccio's
    /// "autogenerated proxy" in `getUpLinkForDistFile`, step 4).
    ///
    /// Equivalent to `new ProxyStorage('verdaccio-<package>', { url: tarball_url, cache: true },
    /// config)`: name `verdaccio-<package>`, every uplink option at its default (`cache: true`,
    /// `timeout 30s`, `retry 2`...), no `auth`, no extra `headers`; `user_agent`, `server_id`
    /// (`Via`) and the global proxy settings still apply, with `no_proxy` checked against the
    /// tarball host. The store creates one per download and drops it afterwards, so it never
    /// shares breaker state. Errors when `tarball_url` is not an absolute URL (Verdaccio throws a
    /// plain error there, which the client sees as `500 internal server error`).
    fn for_foreign_url(
        package: &str,
        tarball_url: &str,
        config: &Config,
    ) -> Result<Self, UplinkBuildError>
    where
        Self: Sized;
}

/// Error building an [`HttpUplink`] from config.
#[derive(Debug, thiserror::Error)]
pub enum UplinkBuildError {
    /// `url` is not an absolute `http`/`https` URL.
    #[error("invalid url for uplink {uplink}: {url}")]
    InvalidUrl {
        /// Uplink name.
        uplink: String,
        /// The configured URL.
        url: String,
    },
    /// The proxy URL is invalid or the HTTP client cannot be created.
    #[error("cannot create HTTP client for uplink {uplink}: {message}")]
    Client {
        /// Uplink name.
        uplink: String,
        /// Details.
        message: String,
    },
}

/// Reads an environment variable for `auth.token_env` (the process environment in production).
type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Bodies larger than this are parsed on the blocking pool.
const BLOCKING_PARSE_THRESHOLD: usize = 256 * 1024;
/// Largest `Content-Length` a manifest body buffer is allocated for up front.
const MAX_PRESIZED_BODY: usize = 512 * 1024 * 1024;

/// At most this much of a 403 body is read to find its message.
const FORBIDDEN_BODY_LIMIT: usize = 1024 * 1024;

/// Verdaccio's `failed_requests` / `last_request_time`.
#[derive(Debug, Default)]
struct Breaker {
    failed_requests: u32,
    last_request_time: Option<Instant>,
}

/// [`Uplink`] over HTTP(S).
pub struct HttpUplink {
    config: UplinkConfig,
    client: reqwest::Client,
    /// `User-Agent` value: `npm (<user_agent>)`.
    user_agent: String,
    /// `Via` entry: `1.1 <server_id> (Verdaccio)`.
    via: String,
    /// Whether requests go through an HTTP proxy (then no `X-Forwarded-For`).
    uses_proxy: bool,
    breaker: Mutex<Breaker>,
    env: EnvFn,
    /// One second; tests shrink it to keep retry backoff short.
    backoff_unit: Duration,
}

impl fmt::Debug for HttpUplink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpUplink")
            .field("config", &self.config)
            .field("uses_proxy", &self.uses_proxy)
            .field("breaker", &*self.breaker())
            .finish_non_exhaustive()
    }
}

impl HttpUplink {
    /// Client for `uplink`. Uses `config` for `user_agent`, `server_id` and the proxy settings
    /// ([`Config::effective_proxy`]). Connections are pooled and kept alive.
    pub fn new(uplink: &UplinkConfig, config: &Config) -> Result<Self, UplinkBuildError> {
        HttpUplink::build(uplink, config, config.effective_proxy(uplink))
    }

    /// [`HttpUplink::new`] with the proxy already chosen.
    fn build(
        uplink: &UplinkConfig,
        config: &Config,
        proxy: Option<&str>,
    ) -> Result<Self, UplinkBuildError> {
        let invalid_url = || UplinkBuildError::InvalidUrl {
            uplink: uplink.name.clone(),
            url: uplink.url.clone(),
        };
        let parsed = Url::parse(&uplink.url).map_err(|_| invalid_url())?;
        let is_https = match parsed.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(invalid_url()),
        };
        let client_error =
            |message: String| UplinkBuildError::Client { uplink: uplink.name.clone(), message };

        let proxy = proxy.filter(|p| !p.is_empty());
        let mut builder = reqwest::Client::builder()
            .gzip(true)
            .timeout(uplink.timeout)
            // Node.js agent Verdaccio uses: keepAlive, `agent_options.maxFreeSockets` (10).
            // `maxSockets` is enforced by the store (`fastopm-store`), before it takes memory.
            .pool_max_idle_per_host(uplink.max_free_sockets)
            .tcp_nodelay(true);
        builder = match proxy {
            // hpagent: the agent matching the uplink protocol goes through the proxy.
            Some(proxy_url) => {
                let proxy = if is_https {
                    reqwest::Proxy::https(proxy_url)
                } else {
                    reqwest::Proxy::http(proxy_url)
                }
                .map_err(|e| client_error(format!("invalid proxy {proxy_url}: {e}")))?;
                tracing::debug!(uplink = %uplink.name, proxy = proxy_url, "using proxy");
                builder.proxy(proxy)
            }
            None => builder.no_proxy(),
        };
        let client = builder.build().map_err(|e| client_error(e.to_string()))?;

        Ok(HttpUplink {
            config: uplink.clone(),
            client,
            user_agent: format!("npm ({})", config.user_agent),
            via: hdr::via_value(&config.server_id),
            uses_proxy: proxy.is_some(),
            breaker: Mutex::new(Breaker::default()),
            env: Arc::new(fastopm_config::process_env),
            backoff_unit: Duration::from_secs(1),
        })
    }

    /// The configuration this client was built from.
    pub fn config(&self) -> &UplinkConfig {
        &self.config
    }

    fn breaker(&self) -> MutexGuard<'_, Breaker> {
        self.breaker.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// got's `request` event: every attempt, manifest or tarball, sets `last_request_time`.
    fn mark_request_start(&self) {
        self.breaker().last_request_time = Some(Instant::now());
    }

    /// `afterResponse` hook on a 2xx manifest response.
    fn record_success(&self) {
        let mut breaker = self.breaker();
        if breaker.failed_requests >= self.config.max_fails {
            breaker.failed_requests = 0;
            tracing::warn!(uplink = %self.config.name, url = %self.config.url, "host is now online");
        }
    }

    /// `beforeRetry` hook: the failure count becomes the retry number.
    fn record_retry(&self, retry_count: u32, uri: &str) {
        let mut breaker = self.breaker();
        breaker.failed_requests = retry_count;
        tracing::info!(uplink = %self.config.name, url = uri, retry = retry_count, "retrying uplink request");
        if breaker.failed_requests >= self.config.max_fails {
            tracing::warn!(uplink = %self.config.name, url = %self.config.url, "host is now offline");
        }
    }

    /// `setAuthNext`: adds `authorization` from `auth` unless the object already has one.
    fn apply_auth(&self, headers: &mut JsHeaders) -> Result<(), UplinkError> {
        let Some(auth) = &self.config.auth else {
            return Ok(());
        };
        if headers.get(hdr::AUTHORIZATION).is_some() {
            return Ok(());
        }
        let token = match &auth.token {
            TokenSource::Invalid => return Err(auth_error("Auth invalid")),
            TokenSource::Missing => return Err(auth_error(messages::TOKEN_REQUIRED)),
            TokenSource::Inline(token) => token.clone(),
            TokenSource::Env(var) => {
                (self.env)(var).ok_or_else(|| auth_error(messages::TOKEN_REQUIRED))?
            }
        };
        // `tokenConf.type || TOKEN_BASIC`
        let auth_type = auth.auth_type.as_deref().filter(|t| !t.is_empty()).unwrap_or("Basic");
        let lower = auth_type.to_lowercase();
        let capitalized = match lower.as_str() {
            "basic" => "Basic",
            "bearer" => "Bearer",
            _ => return Err(auth_error(&format!("Auth type '{lower}' not allowed"))),
        };
        headers.set(hdr::AUTHORIZATION, format!("{capitalized} {token}"));
        Ok(())
    }

    /// `getHeaders(base)` followed by `addProxyHeaders(.., remote_address)`.
    fn computed_headers(
        &self,
        mut headers: JsHeaders,
        remote_address: Option<&str>,
    ) -> Result<JsHeaders, UplinkError> {
        headers.set_or("Accept", hdr::UPLINK_ACCEPT);
        headers.set_or("Accept-Encoding", "gzip");
        headers.set_or("User-Agent", &self.user_agent);
        self.apply_auth(&mut headers)?;
        if !self.uses_proxy {
            let remote = remote_address.unwrap_or("undefined");
            let value = match headers.get_truthy("x-forwarded-for") {
                Some(previous) => format!("{previous}, {remote}"),
                None => remote.to_owned(),
            };
            headers.set("X-Forwarded-For", value);
        }
        let via = match headers.get_truthy("via") {
            Some(previous) => format!("{previous}, {}", self.via),
            None => self.via.clone(),
        };
        headers.set("via", via);
        Ok(headers)
    }

    fn manifest_headers(
        &self,
        etag: Option<&str>,
        remote_address: Option<&str>,
    ) -> Result<HeaderMap, UplinkError> {
        let mut headers = self.computed_headers(JsHeaders::default(), remote_address)?;
        for (key, value) in &self.config.headers {
            headers.set(key, value.clone());
        }
        if let Some(etag) = etag {
            headers.set("If-None-Match", etag.to_owned());
            headers.set("Accept", hdr::UPLINK_ACCEPT.to_owned());
        }
        headers.to_header_map().map_err(UplinkError::Transport)
    }

    fn tarball_headers(&self) -> Result<HeaderMap, UplinkError> {
        // Verdaccio passes `this.config.headers` itself as the base object, so the config keys
        // come first and `applyUplinkHeaders` afterwards changes nothing. (It also mutates the
        // config object, which leaks these headers into later requests; not reproduced.)
        let headers = self.computed_headers(JsHeaders::from_map(&self.config.headers), None)?;
        headers.to_header_map().map_err(UplinkError::Transport)
    }

    /// Delay before retry number `attempt` (got's default `calculateDelay`).
    fn retry_delay(&self, attempt: u32, failure: Failure) -> Option<Duration> {
        let noise = (RandomState::new().hash_one(Instant::now()) % 10_000) as f64 / 10_000.0;
        retry::retry_delay(
            attempt,
            self.config.retry,
            failure,
            self.config.timeout,
            self.backoff_unit,
            noise,
        )
    }

    /// [`Uplink::fetch_manifest`] with retries; `work` as in [`Uplink::fetch_manifest_within`].
    async fn fetch_manifest_counted(
        &self,
        name: &str,
        etag: Option<&str>,
        remote_address: Option<&str>,
        mut work: Option<&mut Reservation>,
    ) -> Result<ManifestFetch, UplinkError> {
        if !self.is_available() {
            return Err(UplinkError::Offline);
        }
        let headers = self.manifest_headers(etag, remote_address)?;
        let uri = format!("{}/{}", self.config.url, encode_package_name(name));
        let base = work.as_ref().map(|w| w.held());
        let mut attempt = 1;
        loop {
            match self.manifest_attempt(&uri, &headers, work.as_deref_mut()).await {
                Ok(outcome) => return outcome,
                Err((failure, detail)) => {
                    // a partial body is gone
                    if let (Some(w), Some(base)) = (work.as_deref_mut(), base) {
                        w.shrink_to(base);
                    }
                    match self.retry_delay(attempt, failure) {
                        Some(delay) => {
                            tokio::time::sleep(delay).await;
                            self.record_retry(attempt, &uri);
                            attempt += 1;
                        }
                        None => return Err(final_manifest_error(failure, detail)),
                    }
                }
            }
        }
    }

    /// One attempt of a manifest request. With `work`, the body is read chunk by chunk and its
    /// buffer, then the tree, are counted in it (see [`Uplink::fetch_manifest_within`]).
    async fn manifest_attempt(
        &self,
        uri: &str,
        headers: &HeaderMap,
        work: Option<&mut Reservation>,
    ) -> Result<Result<ManifestFetch, UplinkError>, (Failure, String)> {
        self.mark_request_start();
        let response = self
            .client
            .get(uri)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| net_failure(&e, uri))?;
        let code = response.status().as_u16();
        tracing::info!(uplink = %self.config.name, url = uri, status = code, "uplink response");
        if code == status::NOT_MODIFIED {
            return Ok(Ok(ManifestFetch::NotModified));
        }
        if !(200..300).contains(&code) {
            let retry_after_ms = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| retry::parse_retry_after(v, SystemTime::now()));
            let failure = Failure::Status { code, retry_after_ms };
            return Err((failure, format!("status {code}")));
        }
        self.record_success();
        let etag = response.headers().get(ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let Some(work) = work else {
            let body = response.bytes().await.map_err(|e| net_failure(&e, uri))?;
            return Ok(parse_manifest(body)
                .await
                .map(|manifest| ManifestFetch::Fetched { manifest, etag }));
        };
        let base = work.held();
        let mut response = response;
        // sized up front when the length is known (a buffer grown by doubling briefly needs 3×)
        let announced = response.content_length().and_then(|n| usize::try_from(n).ok());
        let mut buf = match announced.filter(|n| *n <= MAX_PRESIZED_BODY) {
            Some(len) => {
                work.grow_to(base + len).await;
                bytes::BytesMut::with_capacity(len)
            }
            None => bytes::BytesMut::new(),
        };
        while let Some(chunk) = response.chunk().await.map_err(|e| net_failure(&e, uri))? {
            if buf.len() + chunk.len() > buf.capacity() {
                // the buffer at least doubles; while it moves, old and new copies coexist
                let next = (buf.capacity() * 2).max(buf.len() + chunk.len()).max(64 * 1024);
                work.grow_to(base + next + buf.capacity()).await;
                buf.reserve(next - buf.len());
            }
            buf.extend_from_slice(&chunk);
        }
        let body = buf.freeze();
        // the body and its tree coexist while parsing
        work.grow_to(base + body.len() + predict_heap_bytes(&body)).await;
        let parsed = parse_manifest(body).await;
        match &parsed {
            Ok(manifest) => work.shrink_to(base + manifest.heap_bytes()),
            Err(_) => work.shrink_to(base),
        }
        Ok(parsed.map(|manifest| ManifestFetch::Fetched { manifest, etag }))
    }
}

/// `new Error(message)` from `_throwErrorAuth`.
fn auth_error(message: &str) -> UplinkError {
    tracing::error!("{message}");
    UplinkError::Auth(message.to_owned())
}

fn net_failure(err: &reqwest::Error, uri: &str) -> (Failure, String) {
    let code = NetCode::of(err);
    tracing::debug!(url = uri, ?code, error = %err, "uplink request failed");
    (Failure::Net(code), error_chain(err))
}

/// An error and its sources, `: `-separated (reqwest's own message is terse).
fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        let part = e.to_string();
        if !text.contains(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = e.source();
    }
    text
}

/// `JSON.parse` of a 2xx body, on the [`fastopm_core::heavy`] pool when large.
async fn parse_manifest(body: Bytes) -> Result<Manifest, UplinkError> {
    let large = body.len() > BLOCKING_PARSE_THRESHOLD;
    let parse =
        move || Manifest::from_slice(&body).map_err(|e| UplinkError::InvalidBody(e.to_string()));
    if large { fastopm_core::heavy::run(parse).await } else { parse() }
}

/// Final error of a manifest request once retries are exhausted (`getRemoteMetadata`'s catch).
fn final_manifest_error(failure: Failure, detail: String) -> UplinkError {
    match failure {
        Failure::Status { code: 404, .. } => UplinkError::NotFound,
        Failure::Status { code, .. } => UplinkError::BadStatus(code),
        Failure::Net(code) if code.is_timeout() => UplinkError::Timeout,
        Failure::Net(_) => UplinkError::Transport(detail),
    }
}

impl Uplink for HttpUplink {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn maxage(&self) -> Duration {
        self.config.maxage
    }

    fn cache_tarballs(&self) -> bool {
        self.config.cache
    }

    fn serves_url(&self, tarball_url: &str) -> bool {
        uplink_serves_url(&self.config.url, tarball_url)
    }

    fn is_available(&self) -> bool {
        let breaker = self.breaker();
        let recent =
            breaker.last_request_time.is_some_and(|t| t.elapsed() < self.config.fail_timeout);
        !(breaker.failed_requests >= self.config.max_fails && recent)
    }

    async fn fetch_manifest(
        &self,
        name: &str,
        etag: Option<&str>,
        remote_address: Option<&str>,
    ) -> Result<ManifestFetch, UplinkError> {
        self.fetch_manifest_counted(name, etag, remote_address, None).await
    }

    async fn fetch_manifest_within(
        &self,
        name: &str,
        etag: Option<&str>,
        remote_address: Option<&str>,
        work: &mut Reservation,
    ) -> Result<ManifestFetch, UplinkError> {
        self.fetch_manifest_counted(name, etag, remote_address, Some(work)).await
    }

    async fn fetch_tarball(&self, tarball_url: &str) -> Result<TarballBody, UplinkError> {
        let headers = self.tarball_headers()?;
        tracing::info!(uplink = %self.config.name, url = tarball_url, "fetching tarball");
        self.mark_request_start();
        let response = self.client.get(tarball_url).headers(headers).send().await.map_err(|e| {
            let code = NetCode::of(&e);
            if code.is_timeout() {
                UplinkError::Timeout
            } else {
                UplinkError::Transport(error_chain(&e))
            }
        })?;
        let code = response.status().as_u16();
        tracing::info!(uplink = %self.config.name, url = tarball_url, status = code, "uplink response (streaming)");
        match code {
            404 => return Err(UplinkError::NotFound),
            401 => return Err(UplinkError::Unauthorized),
            403 => return Err(UplinkError::Forbidden(forbidden_message(response).await)),
            200..300 => {}
            _ => return Err(UplinkError::BadStatus(code)),
        }
        // reqwest drops `Content-Length` when it decodes a compressed body, so this is the
        // length of what the stream yields whenever it is present.
        let content_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let stream = CheckedBody::new(Box::pin(response.bytes_stream()), content_length);
        Ok(TarballBody::new(Box::pin(stream), content_length))
    }

    fn for_foreign_url(
        package: &str,
        tarball_url: &str,
        config: &Config,
    ) -> Result<Self, UplinkBuildError> {
        let name = format!("verdaccio-{package}");
        let url = tarball_url.strip_suffix('/').unwrap_or(tarball_url);
        HttpUplink::new(&default_uplink_config(name, url.to_owned()), config)
    }
}

/// An uplink entry with every option at Verdaccio's default.
fn default_uplink_config(name: String, url: String) -> UplinkConfig {
    UplinkConfig {
        name,
        url,
        timeout: Duration::from_secs(30),
        maxage: Duration::from_secs(120),
        max_fails: 2,
        fail_timeout: Duration::from_secs(300),
        retry: 2,
        cache: true,
        headers: IndexMap::new(),
        auth: None,
        http_proxy: None,
        https_proxy: None,
        no_proxy: None,
        max_sockets: Some(40),
        max_free_sockets: 10,
    }
}

/// `body?.detail || body?.title || body?.message || 'tarball not available: forbidden by uplink'`.
async fn forbidden_message(mut response: reqwest::Response) -> String {
    let mut raw = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        raw.extend_from_slice(&chunk);
        if raw.len() > FORBIDDEN_BODY_LIMIT {
            break;
        }
    }
    let message = serde_json::from_slice::<Value>(&raw).ok().and_then(|body| {
        ["detail", "title", "message"].iter().find_map(|key| body.get(key).and_then(js_truthy_text))
    });
    message.unwrap_or_else(|| messages::TARBALL_FORBIDDEN.to_owned())
}

/// The text of a truthy JSON value, as the error message would show it.
fn js_truthy_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) if n.as_f64() != Some(0.0) => Some(n.to_string()),
        Value::Bool(true) => Some("true".to_owned()),
        Value::Array(_) | Value::Object(_) => Some(value.to_string()),
        _ => None,
    }
}

/// One [`HttpUplink`] per entry of [`Config::uplinks`], in declaration order.
pub fn build_uplinks(config: &Config) -> Result<IndexMap<String, HttpUplink>, UplinkBuildError> {
    config
        .uplinks
        .iter()
        .map(|(name, uplink)| Ok((name.clone(), HttpUplink::new(uplink, config)?)))
        .collect()
}

/// `uplinkServesUrl`: whether `tarball_url` is under `uplink_url` (same scheme, host, port
/// after default-port normalization, and path prefix on a `/` boundary). `false` when either
/// URL does not parse.
pub fn uplink_serves_url(uplink_url: &str, tarball_url: &str) -> bool {
    let (Ok(uplink), Ok(tarball)) = (Url::parse(uplink_url), Url::parse(tarball_url)) else {
        return false;
    };
    let base = uplink.path();
    let base = if base.ends_with('/') { base.to_owned() } else { format!("{base}/") };
    tarball.scheme() == uplink.scheme()
        && tarball.host_str() == uplink.host_str()
        && tarball.port() == uplink.port()
        && tarball.path().starts_with(&base)
}

/// Characters `encodeURIComponent` leaves alone: `A-Z a-z 0-9 - _ . ! ~ * ' ( )`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// Package name as sent in the uplink URL (`encode` in `proxy.ts`): `encodeURIComponent(name)`
/// with a leading `%40` turned back into `@`, so `@scope/pkg` → `@scope%2Fpkg`.
pub fn encode_package_name(name: &str) -> String {
    let encoded = utf8_percent_encode(name, URI_COMPONENT).to_string();
    match encoded.strip_prefix("%40") {
        Some(rest) => format!("@{rest}"),
        None => encoded,
    }
}
