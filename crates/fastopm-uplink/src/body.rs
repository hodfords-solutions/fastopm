//! Tarball body stream with Verdaccio's length check.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use fastopm_core::messages;
use futures_core::Stream;

/// The decoded response body as reqwest yields it.
pub(crate) type ReqwestStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// Passes chunks through and, when the response announced a `Content-Length`, ends with an
/// error item if the byte count differs (`content length mismatch` in
/// `fetchTarballFromDistFile`). An error from the transfer itself is passed on as is. Nothing is
/// yielded after an error.
pub(crate) struct CheckedBody {
    inner: ReqwestStream,
    expected: Option<u64>,
    received: u64,
    done: bool,
}

impl CheckedBody {
    pub(crate) fn new(inner: ReqwestStream, expected: Option<u64>) -> Self {
        Self { inner, expected, received: 0, done: false }
    }
}

impl Stream for CheckedBody {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(chunk))) => {
                self.received += chunk.len() as u64;
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                self.done = true;
                Poll::Ready(Some(Err(io::Error::other(err))))
            }
            Poll::Ready(None) => {
                self.done = true;
                match self.expected {
                    Some(expected) if expected != self.received => {
                        tracing::debug!(
                            expected,
                            received = self.received,
                            "tarball length mismatch"
                        );
                        Poll::Ready(Some(Err(io::Error::other(messages::CONTENT_MISMATCH))))
                    }
                    _ => Poll::Ready(None),
                }
            }
        }
    }
}
