//! Registry errors: an HTTP status plus a client-visible message.
//!
//! Every error that reaches a client becomes `{"error": "<message>"}` with this status
//! (Verdaccio: `packages/middleware/src/middlewares/error.ts`). Errors with a status outside
//! `400..600` are internal: the server logs them and answers
//! `500 {"error": "internal server error"}`.
//!
//! Verdaccio reference: `packages/core/core/src/error-utils.ts`.

use std::borrow::Cow;
use std::fmt;

/// HTTP status codes used by the read routes (`HTTP_STATUS` in `constants.ts`).
pub mod status {
    /// 200
    pub const OK: u16 = 200;
    /// 304
    pub const NOT_MODIFIED: u16 = 304;
    /// 400
    pub const BAD_REQUEST: u16 = 400;
    /// 401
    pub const UNAUTHORIZED: u16 = 401;
    /// 403
    pub const FORBIDDEN: u16 = 403;
    /// 404
    pub const NOT_FOUND: u16 = 404;
    /// 405 (fastopm only: write routes; Verdaccio has them)
    pub const METHOD_NOT_ALLOWED: u16 = 405;
    /// 500
    pub const INTERNAL_ERROR: u16 = 500;
    /// 502
    pub const BAD_GATEWAY: u16 = 502;
    /// 503
    pub const SERVICE_UNAVAILABLE: u16 = 503;
    /// 504
    pub const GATEWAY_TIMEOUT: u16 = 504;
    /// 508
    pub const LOOP_DETECTED: u16 = 508;
}

/// Standard messages, copied byte for byte from Verdaccio (`API_ERROR` and literals in the
/// read path). Only the ones reachable from the four read routes are listed.
pub mod messages {
    /// `API_ERROR.NO_PACKAGE`: default 404 message.
    pub const NO_PACKAGE: &str = "no such package available";
    /// `API_ERROR.NOT_PACKAGE_UPLINK`: uplink answered 404; also used as
    /// `"package does not exist on uplink: <name>"` when there is neither cache nor uplink.
    pub const NOT_PACKAGE_UPLINK: &str = "package does not exist on uplink";
    /// `API_ERROR.UPLINK_OFFLINE`: the circuit breaker is open.
    pub const UPLINK_OFFLINE: &str = "uplink is offline";
    /// `API_ERROR.NOT_MODIFIED_NO_DATA`: uplink answered 304.
    pub const NOT_MODIFIED_NO_DATA: &str = "no data";
    /// `API_ERROR.CONTENT_MISMATCH`: tarball length differs from `content-length`.
    pub const CONTENT_MISMATCH: &str = "content length mismatch";
    /// `API_ERROR.NOT_FILE_UPLINK`: uplink answered 404 for a tarball.
    pub const NOT_FILE_UPLINK: &str = "file doesn't exist on uplink";
    /// `API_ERROR.VERSION_NOT_EXIST`: used as `"this version doesn't exist: <query>"`.
    pub const VERSION_NOT_EXIST: &str = "this version doesn't exist";
    /// `API_ERROR.NO_SUCH_FILE`: tarball not found locally nor via any uplink.
    pub const NO_SUCH_FILE: &str = "no such file available";
    /// `API_ERROR.UNKNOWN_ERROR`: default 500 message.
    pub const UNKNOWN_ERROR: &str = "unknown error";
    /// `API_ERROR.INTERNAL_SERVER_ERROR`: body sent for errors without an HTTP status.
    pub const INTERNAL_SERVER_ERROR: &str = "internal server error";
    /// `API_ERROR.BAD_STATUS_CODE`: used as `"bad status code: <code>"` for manifests.
    pub const BAD_STATUS_CODE: &str = "bad status code";
    /// `API_ERROR.SERVER_TIME_OUT`: uplink timed out or the connection was reset (503).
    pub const SERVER_TIME_OUT: &str = "looks like the server is taking to long to respond";
    /// `API_ERROR.RESOURCE_UNAVAILABLE`: default 503 message.
    pub const RESOURCE_UNAVAILABLE: &str = "resource unavailable";
    /// `API_ERROR.UNAUTHORIZED_ACCESS`: uplink answered 401 for a tarball.
    pub const UNAUTHORIZED_ACCESS: &str = "unauthorized access";
    /// `API_ERROR.BAD_DATA`: default 422 message.
    pub const BAD_DATA: &str = "bad data";
    /// `API_ERROR.UNSUPORTED_REGISTRY_CALL`: fastopm uses it for the 405 on write routes.
    pub const UNSUPPORTED_REGISTRY_CALL: &str = "unsupported registry call";
    /// `API_ERROR.NO_CREDENTIALS_PROVIDED`: default 401 message.
    pub const NO_CREDENTIALS_PROVIDED: &str = "no credentials provided";
    /// `API_ERROR.CAN_NOT_USE_THIS_FILENAME`: default 403 message.
    pub const CAN_NOT_USE_THIS_FILENAME: &str = "can't use this filename";
    /// Uplink answered 403 for a tarball and its body had no `detail`/`title`/`message`.
    pub const TARBALL_FORBIDDEN: &str = "tarball not available: forbidden by uplink";
    /// Used as `"bad uplink status code: <code>"` for tarballs.
    pub const BAD_UPLINK_STATUS_CODE: &str = "bad uplink status code";
    /// The catch-all GET route (`server.ts`).
    pub const RESOURCE_NOT_FOUND: &str = "resource not found";
    /// `antiLoop` middleware (508).
    pub const LOOP_DETECTED: &str = "loop detected";
    /// `ERROR_CODE.token_required`: uplink `auth` configured but no token found.
    pub const TOKEN_REQUIRED: &str = "token is required";
}

/// An error with an HTTP status and a client-visible message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError {
    status: u16,
    message: Cow<'static, str>,
}

impl RegistryError {
    /// Error with any status (`getCode`).
    pub fn new(status: u16, message: impl Into<Cow<'static, str>>) -> Self {
        Self { status, message: message.into() }
    }

    /// 400 (`getBadRequest`).
    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(status::BAD_REQUEST, message)
    }

    /// 401 (`getUnauthorized`).
    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(status::UNAUTHORIZED, message)
    }

    /// 403 (`getForbidden`).
    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(status::FORBIDDEN, message)
    }

    /// 404 (`getNotFound`). Use [`messages::NO_PACKAGE`] for Verdaccio's default.
    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(status::NOT_FOUND, message)
    }

    /// 405 for write routes (fastopm only).
    pub fn method_not_allowed() -> Self {
        Self::new(status::METHOD_NOT_ALLOWED, messages::UNSUPPORTED_REGISTRY_CALL)
    }

    /// 500 (`getInternalError`). Use [`messages::UNKNOWN_ERROR`] for Verdaccio's default.
    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(status::INTERNAL_ERROR, message)
    }

    /// 503 (`getServiceUnavailable`).
    pub fn service_unavailable(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(status::SERVICE_UNAVAILABLE, message)
    }

    /// 508 `loop detected`.
    pub fn loop_detected() -> Self {
        Self::new(status::LOOP_DETECTED, messages::LOOP_DETECTED)
    }

    /// `400 invalid <param>`, as sent by Verdaccio's `app.param` validators
    /// (`invalid package`, `invalid filename`, `invalid version`).
    pub fn invalid_param(param: &str) -> Self {
        Self::bad_request(format!("invalid {param}"))
    }

    /// HTTP status.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Message as stored.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether the status is a client-visible error status (`400..600`). When `false` the
    /// server must log the error and answer `500` with [`messages::INTERNAL_SERVER_ERROR`].
    pub fn is_http_error(&self) -> bool {
        (400..600).contains(&self.status)
    }

    /// The `(status, message)` pair the client receives (applies the rule of
    /// [`RegistryError::is_http_error`]; an empty message becomes [`messages::UNKNOWN_ERROR`]).
    pub fn client_view(&self) -> (u16, &str) {
        if !self.is_http_error() {
            return (status::INTERNAL_ERROR, messages::INTERNAL_SERVER_ERROR);
        }
        if self.message.is_empty() {
            (self.status, messages::UNKNOWN_ERROR)
        } else {
            (self.status, &self.message)
        }
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_status() {
        assert_eq!(RegistryError::not_found(messages::NO_PACKAGE).status(), 404);
        assert_eq!(RegistryError::bad_request("x").status(), 400);
        assert_eq!(RegistryError::internal(messages::UNKNOWN_ERROR).status(), 500);
        assert_eq!(RegistryError::service_unavailable("x").status(), 503);
        assert_eq!(RegistryError::forbidden("x").status(), 403);
        assert_eq!(RegistryError::unauthorized("x").status(), 401);
        assert_eq!(RegistryError::loop_detected().status(), 508);
        assert_eq!(RegistryError::loop_detected().message(), "loop detected");
        assert_eq!(RegistryError::method_not_allowed().status(), 405);
    }

    #[test]
    fn invalid_param_message() {
        let e = RegistryError::invalid_param("package");
        assert_eq!((e.status(), e.message()), (400, "invalid package"));
    }

    #[test]
    fn client_view_hides_non_http_errors() {
        assert_eq!(
            RegistryError::new(304, "no data").client_view(),
            (500, "internal server error")
        );
        assert_eq!(RegistryError::new(600, "x").client_view(), (500, "internal server error"));
        assert_eq!(RegistryError::not_found("").client_view(), (404, "unknown error"));
        assert_eq!(RegistryError::not_found("nope").client_view(), (404, "nope"));
    }

    #[test]
    fn display() {
        assert_eq!(RegistryError::not_found("nope").to_string(), "404 nope");
    }
}
