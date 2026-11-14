//! Byte streams that carry tarballs between crates.

use std::fmt;
use std::io;
use std::pin::Pin;

use bytes::Bytes;
use futures_core::Stream;

/// A boxed stream of byte chunks.
///
/// An `Err` item means the transfer failed mid-way (disk read error, uplink connection reset,
/// length mismatch). After an `Err` the stream must not be polled again. A consumer that already
/// sent response headers must abort the connection instead of ending the body cleanly, so the
/// client sees a truncated download rather than a corrupt tarball.
pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send + 'static>>;

/// A tarball ready to be sent to a client.
///
/// Produced by `fastopm-storage` (local file) and `fastopm-uplink` (remote download), passed
/// through `fastopm-store`, consumed by `fastopm-server`.
pub struct TarballBody {
    /// Exact number of bytes `stream` yields, when known. The server sends it as
    /// `Content-Length` (Verdaccio emits a `content-length` event from the file size or the
    /// uplink response header, see `packages/api/src/package.ts`).
    pub content_length: Option<u64>,
    /// The bytes.
    pub stream: ByteStream,
}

impl TarballBody {
    /// Wrap a stream with an optional known length.
    pub fn new(stream: ByteStream, content_length: Option<u64>) -> Self {
        Self { content_length, stream }
    }
}

impl fmt::Debug for TarballBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TarballBody")
            .field("content_length", &self.content_length)
            .finish_non_exhaustive()
    }
}
