//! Header names and values (lower-case names, as `http::HeaderName` expects).
//!
//! Verdaccio reference: `HEADERS` / `HEADER_TYPE` in `packages/core/core/src/constants.ts`.

/// `accept`
pub const ACCEPT: &str = "accept";
/// `accept-encoding`
pub const ACCEPT_ENCODING: &str = "accept-encoding";
/// `authorization`
pub const AUTHORIZATION: &str = "authorization";
/// `content-length`
pub const CONTENT_LENGTH: &str = "content-length";
/// `content-type`
pub const CONTENT_TYPE: &str = "content-type";
/// `etag`
pub const ETAG: &str = "etag";
/// `host`
pub const HOST: &str = "host";
/// `if-none-match`
pub const IF_NONE_MATCH: &str = "if-none-match";
/// `user-agent`
pub const USER_AGENT: &str = "user-agent";
/// `via`
pub const VIA: &str = "via";
/// `x-forwarded-for`
pub const X_FORWARDED_FOR: &str = "x-forwarded-for";
/// `x-forwarded-proto` (default value of `VERDACCIO_FORWARDED_PROTO`)
pub const X_FORWARDED_PROTO: &str = "x-forwarded-proto";

/// `Content-Type` of JSON responses (full manifest, version, ping, errors). Express appends
/// the charset to `application/json` when sending a string body.
pub const CONTENT_TYPE_JSON: &str = "application/json; charset=utf-8";
/// `Content-Type` of the abbreviated manifest (`HEADERS.JSON_INSTALL_CHARSET`).
pub const CONTENT_TYPE_INSTALL_V1: &str = "application/vnd.npm.install-v1+json; charset=utf-8";
/// `Content-Type` of tarballs (`HEADERS.OCTET_STREAM`).
pub const CONTENT_TYPE_OCTET_STREAM: &str = "application/octet-stream";
/// `Accept` value that selects the abbreviated manifest (`Storage.ABBREVIATED_HEADER`),
/// compared against the highest-priority media type of the request `Accept` header.
pub const ACCEPT_INSTALL_V1: &str = "application/vnd.npm.install-v1+json";
/// `Accept` sent to uplinks (`contentTypeAccept` in `proxy.ts`; the trailing `;` is
/// Verdaccio's).
pub const UPLINK_ACCEPT: &str = "application/json;";

/// Value appended to `Via` on requests to uplinks: `1.1 <server_id> (Verdaccio)`
/// (`addProxyHeaders` in `packages/proxy/src/proxy.ts`).
pub fn via_value(server_id: &str) -> String {
    format!("1.1 {server_id} ({})", crate::constants::VIA_PRODUCT)
}

#[cfg(test)]
mod tests {
    #[test]
    fn via() {
        assert_eq!(super::via_value("abc123"), "1.1 abc123 (Verdaccio)");
    }
}
