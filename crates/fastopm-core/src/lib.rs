//! Shared types for every fastopm crate.
//!
//! This crate has no I/O. It defines the types that cross crate boundaries:
//!
//! - [`RegistryError`]: an HTTP status plus a message, rendered as `{"error": "..."}`.
//!   The standard Verdaccio messages live in [`messages`].
//! - [`Manifest`]: a package document (`package.json` in storage, packument on the wire),
//!   a thin wrapper over an order-preserving JSON object.
//! - [`TarballBody`]: a tarball as a byte stream with an optional length.
//! - Package and tarball name validation ([`name`]), nginx-style intervals ([`interval`]),
//!   header names and values ([`headers`]) and other constants ([`constants`]).
//!
//! Verdaccio reference: `packages/core/core/src/` (commit `e2ab94b10`).

pub mod body;
pub mod constants;
pub mod error;
pub mod headers;
pub mod interval;
pub mod json;
pub mod manifest;
pub mod name;
pub mod util;

pub use body::{ByteStream, TarballBody};
pub use error::{RegistryError, messages, status};
pub use interval::{Interval, IntervalError, parse_interval};
pub use manifest::{DistFile, Manifest, UplinkMeta};
pub use name::{extract_tarball_from_url, is_scoped, validate_name, validate_package};
