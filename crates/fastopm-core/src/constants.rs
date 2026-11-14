//! Constants shared by several crates.
//!
//! Verdaccio reference: `packages/core/core/src/constants.ts`,
//! `packages/store/src/lib/storage-utils.ts` (`STORAGE`).

/// Manifest key for dist-tags.
pub const DIST_TAGS: &str = "dist-tags";
/// The default dist-tag.
pub const LATEST: &str = "latest";
/// Name of the manifest file inside a package folder.
pub const PACKAGE_FILE_NAME: &str = "package.json";
/// `_rev` written when a manifest has none (`STORAGE.DEFAULT_REVISION`).
pub const DEFAULT_REVISION: &str = "0-0000000000000000";
/// Default listen port (`DEFAULT_PORT`).
pub const DEFAULT_PORT: u16 = 4873;
/// Default listen host (`DEFAULT_DOMAIN`).
pub const DEFAULT_DOMAIN: &str = "localhost";
/// Default listen protocol (`DEFAULT_PROTOCOL`).
pub const DEFAULT_PROTOCOL: &str = "http";
/// Product token placed in the `Via` header, kept as `Verdaccio` so loop detection works
/// across a mixed Verdaccio / fastopm chain.
pub const VIA_PRODUCT: &str = "Verdaccio";

/// Environment variable names read by fastopm (same names as Verdaccio).
pub mod env {
    /// Overrides the public base URL used to rewrite `dist.tarball`.
    pub const PUBLIC_URL: &str = "VERDACCIO_PUBLIC_URL";
    /// Name of the header that carries the original protocol (default `X-Forwarded-Proto`).
    pub const FORWARDED_PROTO: &str = "VERDACCIO_FORWARDED_PROTO";
    /// Overrides `storage` from the config file.
    pub const STORAGE_PATH: &str = "VERDACCIO_STORAGE_PATH";
    /// Default uplink token when `auth.token_env` is `true` or absent.
    pub const NPM_TOKEN: &str = "NPM_TOKEN";
}
