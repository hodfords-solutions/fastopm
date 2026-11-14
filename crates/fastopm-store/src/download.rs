//! One uplink download shared by every client asking for the same missing tarball.
//!
//! A spawned task reads the uplink body, writes it to storage (when the uplink caches tarballs)
//! and appends each chunk to a shared buffer; every client stream replays the buffer from the
//! start and then follows it live, so late joiners get the whole file. Clients only hold
//! [`Subscription`]s: dropping one never stops the download for the others. When the last one
//! goes away before every announced byte arrived, the task stops like Verdaccio's aborted
//! pipeline: no commit. A download whose `Content-Length` bytes are all in is always finished.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use fastopm_core::memory::PassiveHold;
use fastopm_core::{RegistryError, TarballBody};
use tokio::sync::Notify;

/// Status of the uplink response.
#[derive(Debug, Clone)]
pub(crate) enum Head {
    /// Not known yet.
    Pending,
    /// 2xx; the body follows. `content_length` from the uplink.
    Ready(Option<u64>),
    /// The download failed before any byte (status error, transport error...).
    Failed(RegistryError),
}

#[derive(Debug)]
struct State {
    head: Head,
    chunks: Vec<Bytes>,
    /// `Some(Ok)` after a clean end, `Some(Err)` after a failed or aborted transfer.
    end: Option<Result<(), (io::ErrorKind, String)>>,
}

/// Shared state of one download.
#[derive(Debug)]
pub(crate) struct Download {
    state: Mutex<State>,
    notify: Notify,
    subscribers: AtomicUsize,
    /// Keeps the buffered chunks counted in the work budget until the last client is done.
    hold: Mutex<Option<PassiveHold>>,
}

impl Download {
    /// New download, no subscriber yet.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State { head: Head::Pending, chunks: Vec::new(), end: None }),
            notify: Notify::new(),
            subscribers: AtomicUsize::new(0),
            hold: Mutex::new(None),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a client. Must be called while the download is reachable from the in-flight map
    /// (under its lock), so it never races with [`Download::abandoned`].
    pub(crate) fn subscribe(self: &Arc<Self>) -> Subscription {
        self.subscribers.fetch_add(1, Ordering::SeqCst);
        Subscription { download: Arc::clone(self) }
    }

    /// Whether every client went away. Checked under the in-flight map lock.
    pub(crate) fn abandoned(&self) -> bool {
        self.subscribers.load(Ordering::SeqCst) == 0
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.lock());
        self.notify.notify_waiters();
    }

    pub(crate) fn set_head(&self, head: Head) {
        self.update(|s| s.head = head);
    }

    pub(crate) fn push(&self, chunk: Bytes) {
        self.update(|s| s.chunks.push(chunk));
    }

    /// Keep `hold` until this download is dropped (every client gone).
    pub(crate) fn keep(&self, hold: PassiveHold) {
        *self.hold.lock().unwrap_or_else(|e| e.into_inner()) = Some(hold);
    }

    pub(crate) fn finish(&self, end: Result<(), &io::Error>) {
        let end = end.map_err(|e| (e.kind(), e.to_string()));
        self.update(|s| s.end = Some(end));
    }
}

/// A client's interest in a download.
#[derive(Debug)]
pub(crate) struct Subscription {
    download: Arc<Download>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.download.subscribers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Subscription {
    /// Wait for the uplink status, then hand out the body as a stream that owns this
    /// subscription.
    pub(crate) async fn into_body(self) -> Result<TarballBody, RegistryError> {
        let download = Arc::clone(&self.download);
        let content_length = loop {
            let notified = download.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match download.lock().head.clone() {
                Head::Ready(len) => break len,
                Head::Failed(e) => return Err(e),
                Head::Pending => {}
            }
            notified.await;
        };

        let stream =
            futures_util::stream::unfold((self, 0usize, false), |(sub, pos, done)| async move {
                if done {
                    return None;
                }
                let download = Arc::clone(&sub.download);
                loop {
                    let notified = download.notify.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    {
                        let state = download.lock();
                        if let Some(chunk) = state.chunks.get(pos) {
                            return Some((Ok(chunk.clone()), (sub, pos + 1, false)));
                        }
                        match &state.end {
                            Some(Ok(())) => return None,
                            Some(Err((kind, message))) => {
                                let err = io::Error::new(*kind, message.clone());
                                return Some((Err(err), (sub, pos, true)));
                            }
                            None => {}
                        }
                    }
                    notified.await;
                }
            });
        Ok(TarballBody::new(Box::pin(stream), content_length))
    }
}
