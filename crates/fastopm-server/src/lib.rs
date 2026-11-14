//! HTTP layer: four read routes, Verdaccio-compatible responses.
//!
//! | Request | Response |
//! |---|---|
//! | `GET /-/ping` | `200 {}` |
//! | `GET /:package` | manifest; abbreviated when the top-priority `Accept` type is `application/vnd.npm.install-v1+json` |
//! | `GET /:package/:version` | one version (exact version or dist-tag) |
//! | `GET /:package/-/:filename` | tarball, `application/octet-stream`, `Content-Length` when known |
//! | `GET /` | `404 {"error": "Web interface is disabled in the config file"}` |
//! | other `GET`/`HEAD` | `404 {"error": "resource not found"}` |
//! | `OPTIONS` (any path) | `204`, CORS preflight as `cors()` answers it |
//! | Verdaccio write routes (publish, unpublish, dist-tags, user, token, profile, audit) | `405 {"error": "unsupported registry call"}` |
//! | any other method/path | `404` Express HTML page `Cannot <METHOD> <path>` |
//!
//! `HEAD` behaves as `GET` without a body (a `HEAD` on a tarball still downloads and caches it,
//! as Verdaccio does). Pipeline per request, in order:
//!
//! 1. `OPTIONS` → CORS preflight (`cors()` runs first in Verdaccio);
//! 2. dot-segment guard: a raw path segment starting with `.` (longer than 1 char) →
//!    `404 resource not found` (`dotfiles: ignore`; Verdaccio 7.1.0 never answers these);
//! 3. loop guard: a `Via` entry whose received-by is our `server_id` → `508 loop detected`
//!    ([`is_loop`]);
//! 4. `makeURLrelative` (`new URL(url, "http://<Host>/")`: dot segments removed, an invalid
//!    `Host` → `400 Invalid URL: <url> (TypeError: Invalid URL)`);
//! 5. methods other than `GET`/`HEAD`: `405` on a path Verdaccio routes to a write endpoint
//!    for that method, else Express' `404` HTML page; for `GET`/`HEAD`, route ([`parse_route`])
//!    and parameter validation (`400 invalid package` / `invalid version` / `invalid filename`,
//!    `400 Failed to decode param '<raw>'`);
//! 6. handler: [`fastopm_store::Store`] for data, `fastopm-render` for bodies;
//! 7. JSON responses carry the `ETag` of `final.ts` (MD5, success only) or, for errors,
//!    Express' weak `ETag`; `GET`/`HEAD` 2xx are answered `304` with no body when the request
//!    is fresh (Express `req.fresh`). JSON responses get `Vary: Accept-Encoding` and are
//!    compressed (br, gzip or deflate, `compression@1.8` negotiation) from 1024 bytes on,
//!    without `Content-Length`; tarballs are never compressed.
//!
//! Errors: a [`RegistryError`] becomes `{"error": "<message>"}` via
//! [`RegistryError::client_view`] (`fastopm_render::render_error`). When a tarball stream fails
//! after headers were sent, the connection is aborted.
//!
//! Every response carries `Access-Control-Allow-Origin: *` and `X-Powered-By: <user_agent>`
//! (`X-Powered-By: Express` for the preflight and the dot-segment 404, which Verdaccio answers
//! before its `user-agent` middleware). `Authorization` from clients is ignored. Rendered bodies
//! are cached by `(name, generation, form, base URL)`.
//!
//! Verdaccio reference: `packages/server/express/src/server.ts`, `packages/api/src/package.ts`,
//! `ping.ts`, `packages/middleware/src/middlewares/` (`encode-pkg.ts`, `antiLoop.ts`,
//! `dotfiles.ts`, `final.ts`, `error.ts`, `validation.ts`, `make-url-relative.ts`, `log.ts`) and
//! `getByQualityPriorityValue` in `packages/core/core/src/string-utils.ts`.

mod cache;
mod compress;
mod etag;
mod js;
mod negotiate;
mod route;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use fastopm_config::Config;
use fastopm_core::headers::{
    ACCEPT, ACCEPT_ENCODING, ACCEPT_INSTALL_V1, CONTENT_TYPE_JSON, CONTENT_TYPE_OCTET_STREAM, HOST,
    IF_NONE_MATCH, VIA, X_FORWARDED_FOR,
};
use fastopm_core::{RegistryError, TarballBody, messages};
use fastopm_render::{BaseUrlInput, Variant};
use fastopm_storage::{FsStorage, PackageStorage};
use fastopm_store::Store;
use fastopm_uplink::{HttpUplink, Uplink};
use futures_util::StreamExt;

use crate::cache::{CachedBody, Form, RenderCache};
use crate::negotiate::Coding;

/// `API_ERROR.WEB_DISABLED`, the answer to `GET /` when the web UI is off (fastopm has none).
const WEB_DISABLED: &str = "Web interface is disabled in the config file";
/// Express `compression` default threshold.
const COMPRESSION_THRESHOLD: usize = 1024;
/// Byte budget of the rendered-body cache.
const RENDER_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Bodies at least this large are compressed on the blocking pool.
const BLOCKING_COMPRESSION_BYTES: usize = 256 * 1024;
/// `tracing` target of the access log (Verdaccio's `http` level).
pub const ACCESS_LOG_TARGET: &str = "fastopm_server::access";

/// Shared state of the router.
pub struct AppState<S = FsStorage, U = HttpUplink> {
    /// Loaded config (`server_id`, `url_prefix`, `public_url`, `forwarded_proto_header`).
    pub config: Arc<Config>,
    /// Data source.
    pub store: Arc<Store<S, U>>,
}

impl<S, U> Clone for AppState<S, U> {
    fn clone(&self) -> Self {
        Self { config: Arc::clone(&self.config), store: Arc::clone(&self.store) }
    }
}

/// Build the router. The binary serves it with `axum::serve` on the configured listener; serve
/// it with `into_make_service_with_connect_info::<SocketAddr>()` so the client address reaches
/// uplinks as `X-Forwarded-For` and the access log.
pub fn router<S: PackageStorage, U: Uplink>(state: AppState<S, U>) -> Router {
    let server = Arc::new(Server { state, cache: RenderCache::new(RENDER_CACHE_BYTES) });
    Router::new().fallback(move |request: Request| {
        let server = Arc::clone(&server);
        async move { server.handle(request).await }
    })
}

/// A recognised read route, with parameters percent-decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/-/ping`
    Ping,
    /// `/:package`
    Manifest {
        /// Package name, e.g. `@scope/pkg`.
        name: String,
    },
    /// `/:package/:version`
    Version {
        /// Package name.
        name: String,
        /// Version or dist-tag as requested.
        version: String,
    },
    /// `/:package/-/:filename`
    Tarball {
        /// Package name.
        name: String,
        /// Tarball file name.
        filename: String,
    },
}

/// Match a raw request path (still percent-encoded, without query string).
///
/// Scoped names are accepted as `/@s/p`, `/@s%2Fp` and `/%40s/p` (`encodeScopePackage`:
/// replace the first `/%40` with `/@`, then encode the `/` after the first `/@<scope>` unless it
/// ends the path). After that the path is split on `/` (one trailing `/` ignored, as Express'
/// non-strict routing), each segment is percent-decoded, and:
///
/// - `["-", "ping"]` → [`Route::Ping`] (literal parts match case-insensitively, before decoding);
/// - `[pkg]` → [`Route::Manifest`]; `[pkg, v]` → [`Route::Version`];
///   `[pkg, "-", file]` → [`Route::Tarball`];
/// - anything else → `Ok(None)` (the caller answers `404 resource not found`).
///
/// Errors: a segment that does not percent-decode to UTF-8 →
/// `400 Failed to decode param '<raw>'`; a package failing [`fastopm_core::validate_package`] →
/// `400 invalid package`; a version or filename failing [`fastopm_core::validate_name`] →
/// `400 invalid version` / `400 invalid filename`.
pub fn parse_route(raw_path: &str) -> Result<Option<Route>, RegistryError> {
    route::parse(raw_path)
}

/// Whether a `Via` header shows this request already passed through `server_id` (`antiLoop`):
/// split on `,`, trim each entry, split on whitespace; the second token, cut at the first `(`
/// and trimmed, equal to `server_id` means a loop.
pub fn is_loop(via: &str, server_id: &str) -> bool {
    via.split(',').any(|entry| {
        let mut parts = js::trim(entry).split(js::is_js_space).filter(|p| !p.is_empty());
        let _protocol = parts.next();
        parts
            .next()
            .is_some_and(|by| js::trim(by.split('(').next().unwrap_or_default()) == server_id)
    })
}

/// Whether the client asked for the abbreviated manifest: the highest-`q` media type of
/// `accept` (`getByQualityPriorityValue`: a single value is taken up to its first `;`; for a
/// list, entries without `q` count as `q=0` and the sort is stable) equals
/// `application/vnd.npm.install-v1+json`.
pub fn wants_abbreviated(accept: Option<&str>) -> bool {
    negotiate::quality_priority_value(accept) == ACCEPT_INSTALL_V1
}

/// Error response: status and body from [`RegistryError::client_view`],
/// `Content-Type: application/json; charset=utf-8`, Express' weak `ETag` (`W/"<len>-<sha1>"`,
/// which Verdaccio's `res.send` adds to errors) and `WWW-Authenticate: Bearer` on a 401
/// (`final.ts`). Internal errors are logged. The router adds the common headers on top.
pub fn error_response(error: &RegistryError) -> Response {
    let json = error_json(error);
    let mut response = Response::new(Body::from(json.body.rendered.body.clone()));
    *response.status_mut() = json.status;
    let headers = response.headers_mut();
    headers.extend(json.extra);
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE_JSON));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(json.body.rendered.body.len()));
    if let Ok(value) = HeaderValue::from_str(&json.body.rendered.etag) {
        headers.insert(header::ETAG, value);
    }
    response
}

/// Router state plus the rendered-body cache.
struct Server<S, U> {
    state: AppState<S, U>,
    cache: RenderCache,
}

/// Request facts the handlers need.
struct Ctx {
    method: Method,
    head: bool,
    headers: HeaderMap,
    /// `Host`, or the HTTP/2 `:authority`.
    host: Option<String>,
    remote: Option<String>,
}

impl Ctx {
    /// A header as Node.js shows it: Latin-1, repeated headers joined with `, `.
    fn header(&self, name: &str) -> Option<String> {
        let mut values = self.headers.get_all(name).iter().peekable();
        values.peek()?;
        Some(values.map(|v| js::latin1(v.as_bytes())).collect::<Vec<_>>().join(", "))
    }
}

/// A JSON body before the response-wide rules (`final.ts`, `res.send`, `compression`) apply.
struct Json {
    status: StatusCode,
    content_type: &'static str,
    /// `final.ts` ETag (2xx); `None` means Express' weak ETag of the body.
    etag: Option<String>,
    body: Arc<CachedBody>,
    extra: HeaderMap,
    /// Error message for the access log.
    error: Option<String>,
}

enum Reply {
    Json(Json),
    Tarball(TarballBody),
    /// Answered before Verdaccio's `user-agent` and `compression` middlewares ran.
    Early(Response),
    /// A complete response that only needs the common headers.
    Plain(Response),
}

/// Outcome summary for the access log.
struct LogInfo {
    status: u16,
    error: Option<String>,
    bytes: Option<u64>,
}

impl<S: PackageStorage, U: Uplink> Server<S, U> {
    async fn handle(&self, request: Request) -> Response {
        let started = Instant::now();
        let (parts, _body) = request.into_parts();
        let remote = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip().to_string());
        let host = parts
            .headers
            .get(HOST)
            .map(|h| js::latin1(h.as_bytes()))
            .or_else(|| parts.uri.authority().map(|a| a.as_str().to_owned()));
        let target = parts.uri.to_string();
        let ctx = Ctx {
            head: parts.method == Method::HEAD,
            method: parts.method,
            headers: parts.headers,
            host,
            remote,
        };
        let raw_path = parts.uri.path().to_owned();
        let reply = self.dispatch(&ctx, &target, &raw_path).await;
        let (response, info) = self.finish(&ctx, reply).await;
        self.log(&ctx, &target, &raw_path, &info, started);
        response
    }

    async fn dispatch(&self, ctx: &Ctx, target: &str, raw_path: &str) -> Reply {
        if ctx.method == Method::OPTIONS {
            return Reply::Early(preflight(ctx));
        }
        if route::has_dotfile_segment(raw_path) {
            let error = RegistryError::not_found(messages::RESOURCE_NOT_FOUND);
            let mut response = error_response(&error);
            response.headers_mut().insert(
                HeaderName::from_static("x-powered-by"),
                HeaderValue::from_static("Express"),
            );
            return Reply::Early(response);
        }
        match self.route(ctx, target, raw_path).await {
            Ok(reply) => reply,
            Err(error) => error_reply(&error),
        }
    }

    async fn route(&self, ctx: &Ctx, target: &str, raw_path: &str) -> Result<Reply, RegistryError> {
        if let Some(via) = ctx.header(VIA)
            && is_loop(&via, &self.state.config.server_id)
        {
            return Err(RegistryError::loop_detected());
        }
        let relative = route::encode_scope(&route::make_relative(target, ctx.host.as_deref())?);
        let path = relative.split('?').next().unwrap_or_default();
        if ctx.method != Method::GET && !ctx.head {
            if route::is_write_route(ctx.method.as_str(), path) {
                return Err(RegistryError::method_not_allowed());
            }
            return Ok(Reply::Plain(express_not_found(ctx, raw_path)));
        }
        match parse_route(path)? {
            Some(Route::Ping) => Ok(json_ok(fastopm_render::render_ping())),
            Some(Route::Manifest { name }) => self.manifest(ctx, &name, None).await,
            Some(Route::Version { name, version }) => {
                self.manifest(ctx, &name, Some(version)).await
            }
            Some(Route::Tarball { name, filename }) => {
                let body = self.state.store.get_tarball(&name, &filename).await?;
                Ok(Reply::Tarball(body))
            }
            None if path == "/" => Err(RegistryError::not_found(WEB_DISABLED)),
            None => Err(RegistryError::not_found(messages::RESOURCE_NOT_FOUND)),
        }
    }

    async fn manifest(
        &self,
        ctx: &Ctx,
        name: &str,
        version: Option<String>,
    ) -> Result<Reply, RegistryError> {
        let variant = if wants_abbreviated(ctx.header(ACCEPT).as_deref()) {
            Variant::Abbreviated
        } else {
            Variant::Full
        };
        let snapshot = self.state.store.get_manifest(name, ctx.remote.as_deref()).await?;
        let config = &self.state.config;
        let forwarded_proto = ctx.header(&config.forwarded_proto_header);
        let base = fastopm_render::public_base_url(&BaseUrlInput {
            host: ctx.host.as_deref(),
            forwarded_proto: forwarded_proto.as_deref(),
            scheme: "http",
            url_prefix: config.url_prefix.as_deref(),
            public_url: config.public_url.as_deref(),
        })?;
        let form = match version {
            None => Form::Manifest(variant),
            Some(version) => Form::Version(version, variant),
        };
        let body = match self.cache.get(name, snapshot.generation, &form, base.as_deref()) {
            Some(body) => body,
            None => {
                let manifest = &snapshot.manifest;
                let rendered = match &form {
                    Form::Manifest(variant) => {
                        fastopm_render::render_manifest(manifest, *variant, base.as_deref())
                    }
                    Form::Version(query, variant) => {
                        fastopm_render::render_version(manifest, query, *variant, base.as_deref())?
                    }
                };
                let body = Arc::new(CachedBody::new(rendered));
                self.cache.insert(
                    name,
                    snapshot.generation,
                    form,
                    base.as_deref(),
                    Arc::clone(&body),
                );
                body
            }
        };
        Ok(Reply::Json(Json {
            status: StatusCode::OK,
            content_type: body.rendered.content_type,
            etag: Some(body.rendered.etag.clone()),
            body,
            extra: HeaderMap::new(),
            error: None,
        }))
    }

    /// Apply the response-wide rules and add the common headers.
    async fn finish(&self, ctx: &Ctx, reply: Reply) -> (Response, LogInfo) {
        let json = match reply {
            Reply::Early(mut response) => {
                response
                    .headers_mut()
                    .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
                let info = log_info(&response, None);
                return (strip_head(ctx, response), info);
            }
            Reply::Json(json) => json,
            Reply::Plain(response) => return self.common_headers(response, None),
            Reply::Tarball(body) => match tarball_response(ctx, body).await {
                Ok(response) => return self.common_headers(response, None),
                Err(error) => error_json(&error),
            },
        };
        let error = json.error.clone();
        let response = self.finish_json(ctx, json).await;
        self.common_headers(response, error)
    }

    /// `X-Powered-By` (`user-agent` middleware) and `Access-Control-Allow-Origin` (`cors()`).
    fn common_headers(&self, mut response: Response, error: Option<String>) -> (Response, LogInfo) {
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&self.state.config.user_agent) {
            headers.insert(HeaderName::from_static("x-powered-by"), value);
        }
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        let info = log_info(&response, error);
        (response, info)
    }

    async fn finish_json(&self, ctx: &Ctx, json: Json) -> Response {
        let Json { status, content_type, etag, body, extra, error: _ } = json;
        let raw = body.rendered.body.clone();
        let etag = etag.unwrap_or_else(|| etag::weak_etag(&raw));
        let mut response = Response::new(Body::empty());
        *response.status_mut() = status;
        let headers = response.headers_mut();
        headers.extend(extra);
        if let Ok(value) = HeaderValue::from_str(&etag) {
            headers.insert(header::ETAG, value);
        }
        let fresh = status.is_success()
            && negotiate::is_fresh(
                ctx.header(IF_NONE_MATCH).as_deref(),
                ctx.header(header::IF_MODIFIED_SINCE.as_str()).as_deref(),
                ctx.header(header::CACHE_CONTROL.as_str()).as_deref(),
                &etag,
            );
        if fresh {
            // `res.send` drops the entity headers and `compression` sees no Content-Type.
            *response.status_mut() = StatusCode::NOT_MODIFIED;
            return response;
        }
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        let coding = if raw.len() < COMPRESSION_THRESHOLD || ctx.head {
            Coding::Identity
        } else {
            negotiate::negotiate_coding(ctx.header(ACCEPT_ENCODING).as_deref())
        };
        if coding == Coding::Identity {
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(raw.len()));
            if !ctx.head {
                *response.body_mut() = Body::from(raw);
            }
            return response;
        }
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(coding.header_value()));
        let encoded = if raw.len() >= BLOCKING_COMPRESSION_BYTES {
            let body = Arc::clone(&body);
            tokio::task::spawn_blocking(move || body.encoded(coding))
                .await
                .unwrap_or_else(|_| compress::encode(coding, &raw))
        } else {
            body.encoded(coding)
        };
        // Express streams compressed bodies: chunked, no Content-Length.
        let chunk = futures_util::stream::once(async move { Ok::<_, Infallible>(encoded) });
        *response.body_mut() = Body::from_stream(chunk);
        response
    }

    fn log(&self, ctx: &Ctx, target: &str, raw_path: &str, info: &LogInfo, started: Instant) {
        // `hidePingLogs`: successful pings are not logged.
        if raw_path == "/-/ping" && info.status < 400 {
            return;
        }
        let remote = match (ctx.header(X_FORWARDED_FOR), &ctx.remote) {
            (Some(forwarded), Some(remote)) => format!("{forwarded} via {remote}"),
            (Some(forwarded), None) => format!("{forwarded} via undefined"),
            (None, Some(remote)) => remote.clone(),
            (None, None) => "undefined".to_owned(),
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        match &info.error {
            Some(error) => tracing::info!(
                target: ACCESS_LOG_TARGET,
                status = info.status,
                remote = %remote,
                method = %ctx.method,
                url = %target,
                error = %error,
                elapsed_ms,
                "{} {} {} - error: {}", info.status, ctx.method, target, error
            ),
            None => tracing::info!(
                target: ACCESS_LOG_TARGET,
                status = info.status,
                remote = %remote,
                method = %ctx.method,
                url = %target,
                bytes_out = info.bytes,
                elapsed_ms,
                "{} {} {}", info.status, ctx.method, target
            ),
        }
    }
}

fn error_reply(error: &RegistryError) -> Reply {
    Reply::Json(error_json(error))
}

/// The JSON reply of an error (`error.ts` + `final.ts`).
fn error_json(error: &RegistryError) -> Json {
    if !error.is_http_error() {
        tracing::error!(status = error.status(), message = error.message(), "unexpected error");
    }
    let (status, message) = error.client_view();
    let body = fastopm_render::render_error(error);
    let mut extra = HeaderMap::new();
    if status == 401 {
        extra.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    let rendered = fastopm_render::Rendered {
        etag: etag::weak_etag(&body),
        body,
        content_type: CONTENT_TYPE_JSON,
    };
    Json {
        status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        content_type: CONTENT_TYPE_JSON,
        etag: None,
        body: Arc::new(CachedBody::new(rendered)),
        extra,
        error: Some(message.to_owned()),
    }
}

/// Express' `finalhandler` 404 for a method/path no Verdaccio route handles (`compression` has
/// run, hence `Vary`; the page is below its threshold).
fn express_not_found(ctx: &Ctx, raw_path: &str) -> Response {
    let page = route::express_not_found_page(ctx.method.as_str(), raw_path);
    let mut response = Response::new(Body::from(page.clone()));
    *response.status_mut() = StatusCode::NOT_FOUND;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(page.len()));
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    response
}

fn json_ok(rendered: fastopm_render::Rendered) -> Reply {
    Reply::Json(Json {
        status: StatusCode::OK,
        content_type: rendered.content_type,
        etag: Some(rendered.etag.clone()),
        body: Arc::new(CachedBody::new(rendered)),
        extra: HeaderMap::new(),
        error: None,
    })
}

/// `cors()` preflight: `204`, reflected `Access-Control-Request-Headers`, `Content-Length: 0`
/// (Express sets it explicitly).
///
/// Hyper drops a `Content-Length` header on a 204 whose body is known to be empty
/// (`Body::empty()`), so the body is an empty *stream*: its length is unknown to hyper, which
/// then writes the header as given and still sends no body bytes (204 never has a body).
fn preflight(ctx: &Ctx) -> Response {
    let empty = futures_util::stream::empty::<Result<bytes::Bytes, Infallible>>();
    let mut response = Response::new(Body::from_stream(empty));
    *response.status_mut() = StatusCode::NO_CONTENT;
    let headers = response.headers_mut();
    headers.insert(HeaderName::from_static("x-powered-by"), HeaderValue::from_static("Express"));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,HEAD,PUT,PATCH,POST,DELETE"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Access-Control-Request-Headers"));
    if let Some(requested) = ctx.header(header::ACCESS_CONTROL_REQUEST_HEADERS.as_str())
        && !requested.is_empty()
        && let Ok(value) = HeaderValue::from_str(&requested)
    {
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
}

/// `HEAD`: keep the headers, drop the body.
fn strip_head(ctx: &Ctx, mut response: Response) -> Response {
    if ctx.head {
        *response.body_mut() = Body::empty();
    }
    response
}

fn log_info(response: &Response, error: Option<String>) -> LogInfo {
    let bytes = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    LogInfo { status: response.status().as_u16(), error, bytes }
}

/// The registry error carried by a failed tarball chunk, else `500 internal server error`.
fn stream_error(error: &std::io::Error) -> RegistryError {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<RegistryError>())
        .cloned()
        .unwrap_or_else(|| RegistryError::internal(messages::INTERNAL_SERVER_ERROR))
}

/// `200 application/octet-stream`. The first chunk is awaited before answering, so a stream that
/// fails at once still becomes a JSON error (Verdaccio has not sent headers at that point);
/// later failures abort the connection. `HEAD` drains the stream in the background so the
/// tarball is still cached, as Verdaccio's pipe into a `HEAD` response does.
async fn tarball_response(ctx: &Ctx, body: TarballBody) -> Result<Response, RegistryError> {
    let TarballBody { content_length, mut stream } = body;
    let first = match stream.next().await {
        Some(Err(error)) => return Err(stream_error(&error)),
        other => other,
    };
    let mut response = Response::new(Body::empty());
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE_OCTET_STREAM));
    if let Some(length) = content_length {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    }
    if ctx.head {
        tokio::spawn(async move { while let Some(Ok(_)) = stream.next().await {} });
        return Ok(response);
    }
    let rest = futures_util::stream::iter(first).chain(stream).inspect(|item| {
        if let Err(error) = item {
            tracing::warn!(%error, "tarball stream failed after headers were sent");
        }
    });
    *response.body_mut() = Body::from_stream(rest);
    Ok(response)
}

#[cfg(test)]
mod tests;
