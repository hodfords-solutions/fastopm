//! Orchestration: where a manifest or tarball comes from.
//!
//! [`Store`] is generic over [`PackageStorage`] and [`Uplink`], so its tests run against
//! in-memory fakes; production uses `Store<FsStorage, HttpUplink>` ([`DefaultStore`]).
//!
//! Manifests: RAM cache → disk → uplinks (respecting `maxage`) → merge → write back.
//! Tarballs: disk → uplink, streaming to the client while writing a temporary file.
//! Concurrent requests for the same manifest refresh, or the same missing tarball, share one
//! uplink request.
//!
//! With `fastopm.refresh_on_miss` (off by default, not a Verdaccio behavior), a version or
//! tarball missing from a manifest served from cache within `maxage` triggers one *forced*
//! sync that ignores `maxage`; see [`Store::get_manifest_for`] and [`Store::get_tarball`].
//!
//! Disk writes must stay compatible with Verdaccio sharing the same storage: same number of
//! writes, same merge result, `_rev` bumped on every write (`Manifest::bump_revision`).
//!
//! The store needs a Tokio runtime: refreshes and downloads run in spawned tasks so that a
//! client going away does not cancel work other clients wait for.
//!
//! Memory: the RAM cache counts parsed manifests at their real heap size and keeps only the
//! small [`cache::Meta`] of packages whose tree is too large (their bodies come from the
//! server's render cache; the tree is read again when needed, see [`PackageHandle`]). Uplink
//! syncs, disk reads and tarball downloads take their transient memory from the
//! [`MemoryBudget`] of [`StoreOptions::memory`] and wait when it is spent. Merges move the
//! uplink data instead of copying it.
//!
//! Verdaccio reference: `packages/store/src/storage.ts` (`getPackage`, `syncUplinksMetadata`,
//! `mergeCacheRemoteMetadata`, `updateVersionsNext`, `getTarball`, `getTarballFromUpstream`,
//! `fetchTarballFromDistFile`, `getUpLinkForDistFile`) and `packages/store/src/lib/`
//! (`storage-utils.ts`, `versions-utils.ts`).

mod cache;
mod download;
mod js;
mod merge;
mod semver;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fastopm_config::Config;
use fastopm_core::constants::DIST_TAGS;
use fastopm_core::manifest::UPLINKS_KEY;
use fastopm_core::memory::{Reservation, file_load_bytes};
use fastopm_core::{Manifest, RegistryError, TarballBody, UplinkMeta, messages};
use fastopm_storage::{FileIdentity, FsStorage, PackageStorage, TarballWrite};
use fastopm_uplink::{HttpUplink, ManifestFetch, Uplink};
use futures_util::StreamExt;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::cache::{Loaded, ManifestCache, Meta, Resident};
use crate::download::{Download, Head, Subscription};
use crate::merge::{
    DistSource, SyncSource, has_distfile_record, lookup_dist_file, merge_cache_remote,
    normalize_package, normalized_dist_tags, update_versions_next, update_versions_next_owned,
    uplink_etag, uplink_etag_for_304, uplink_fetched, uplinks_of, would_change_self,
};

/// The work memory budget type ([`StoreOptions::memory`]).
pub use fastopm_core::memory::MemoryBudget;

/// The production store.
pub type DefaultStore = Store<FsStorage, HttpUplink>;

/// Tuning knobs.
#[derive(Debug, Clone)]
pub struct StoreOptions {
    /// Upper bound, in bytes of RAM (parsed trees counted at their heap size, see
    /// [`fastopm_core::memory::value_heap_bytes`]), of the in-memory manifest cache (LRU by
    /// size). `0` disables the cache.
    pub manifest_cache_bytes: u64,
    /// Budget for the transient memory of syncs, disk reads and tarball downloads (shared with
    /// the server's renders). Default: unlimited.
    pub memory: Arc<MemoryBudget>,
}

impl Default for StoreOptions {
    /// 256 MiB cache, unlimited work memory.
    fn default() -> Self {
        Self { manifest_cache_bytes: 256 * 1024 * 1024, memory: MemoryBudget::unlimited() }
    }
}

/// A manifest ready to render.
#[derive(Debug, Clone)]
pub struct PackageSnapshot {
    /// Storage form of the manifest (bookkeeping keys included) with `normalizeDistTags`
    /// applied. Shared and immutable; `fastopm-render` filters and rewrites it.
    pub manifest: Arc<Manifest>,
    /// Changes whenever the content of `manifest` changes (and only then, within one process).
    /// Lets the server cache rendered bodies keyed by `(name, generation, variant, base URL)`.
    ///
    /// With the manifest cache disabled (`manifest_cache_bytes: 0`) every read gets a new
    /// generation.
    pub generation: u64,
    /// Size of the `package.json` this came from (a size hint for renders).
    file_len: u64,
    /// Whether the tree was kept in the manifest cache (else it is only alive while used).
    cached: bool,
    /// Keeps a tree that is not in the cache counted in the memory budget while it is used.
    _keep: Option<Arc<Loaded>>,
}

impl PackageSnapshot {
    /// Size in bytes of the `package.json` the manifest was read from or written to: the
    /// compact JSON form, a bound for the size of rendered bodies (pretty JSON adds at most
    /// ~40%).
    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    /// Whether the manifest is held by the manifest cache (else it lives only as long as its
    /// snapshots).
    pub fn is_cached(&self) -> bool {
        self.cached
    }
}

/// What `GET /:package` gets from the store: the generation of the manifest to serve, and the
/// manifest itself when it is in hand. A manifest too large for the RAM cache is only read again
/// ([`PackageHandle::load_within`]) when its body is not in the server's render cache.
pub struct PackageHandle {
    generation: u64,
    file_len: u64,
    snapshot: Option<PackageSnapshot>,
    name: String,
    load_bytes: usize,
    loader: Arc<dyn LoadTree>,
}

impl std::fmt::Debug for PackageHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageHandle")
            .field("name", &self.name)
            .field("generation", &self.generation)
            .field("loaded", &self.snapshot.is_some())
            .finish()
    }
}

impl PackageHandle {
    /// [`PackageSnapshot::generation`] of the manifest to serve.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The manifest, when it is in hand.
    pub fn snapshot(&self) -> Option<&PackageSnapshot> {
        self.snapshot.as_ref()
    }

    /// Drop the manifest in hand unless the manifest cache holds it, so that waiting (for
    /// memory) does not keep a large tree alive; [`PackageHandle::load_within`] reads it again.
    pub fn release_uncached(&mut self) {
        if self.snapshot.as_ref().is_some_and(|s| !s.cached) {
            self.snapshot = None;
        }
    }

    /// Bytes [`PackageHandle::load_within`] needs while reading (0 when the manifest is in
    /// hand): the file and its tree.
    pub fn load_bytes(&self) -> usize {
        if self.snapshot.is_some() { 0 } else { self.load_bytes }
    }

    /// Size of the `package.json` ([`PackageSnapshot::file_len`]).
    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    /// The manifest, read from disk when it is not in hand, counted in `work` (grown by
    /// [`PackageHandle::load_bytes`] when needed; it then keeps counting the tree). When the
    /// file changed since the handle was made, the snapshot is the new content with its own
    /// generation.
    pub async fn load_within(
        &self,
        work: &mut Reservation,
    ) -> Result<PackageSnapshot, RegistryError> {
        match &self.snapshot {
            Some(snapshot) => Ok(snapshot.clone()),
            None => Arc::clone(&self.loader).load(self.name.clone(), work).await,
        }
    }

    /// [`PackageHandle::load_within`] with a reservation of its own, released before returning
    /// (for callers that only look at the manifest briefly).
    pub async fn load(&self, memory: &Arc<MemoryBudget>) -> Result<PackageSnapshot, RegistryError> {
        match &self.snapshot {
            Some(snapshot) => Ok(snapshot.clone()),
            None => {
                let mut work = memory.reserve(self.load_bytes).await;
                self.load_within(&mut work).await
            }
        }
    }
}

/// Reads a manifest for a [`PackageHandle`] (the store, behind a trait object so the handle is
/// not generic).
trait LoadTree: Send + Sync {
    fn load<'a>(
        self: Arc<Self>,
        name: String,
        work: &'a mut Reservation,
    ) -> BoxFuture<'a, LoadResult>;
}

type LoadResult = Result<PackageSnapshot, RegistryError>;

impl<S: PackageStorage, U: Uplink> LoadTree for Inner<S, U> {
    fn load<'a>(
        self: Arc<Self>,
        name: String,
        work: &'a mut Reservation,
    ) -> BoxFuture<'a, LoadResult> {
        async move {
            let loaded = self.local_tree_within(&name, work).await?;
            loaded
                .map(|l| l.snapshot())
                .ok_or_else(|| RegistryError::not_found(messages::NO_PACKAGE))
        }
        .boxed()
    }
}

/// Coordinates storage and uplinks. Share as `Arc<Store<..>>`; all methods take `&self`.
pub struct Store<S, U> {
    inner: Arc<Inner<S, U>>,
}

type RefreshResult = Result<Option<PackageSnapshot>, RegistryError>;
type SharedRefresh = Shared<BoxFuture<'static, RefreshResult>>;
type ReadResult = Result<Option<Arc<Loaded>>, RegistryError>;
type SharedRead = Shared<BoxFuture<'static, ReadResult>>;
type DownloadKey = (String, String);
/// Package name and whether the sync is forced (ignores `maxage`, `refresh_on_miss`).
type RefreshKey = (String, bool);

struct Inner<S, U> {
    config: Arc<Config>,
    storage: S,
    uplinks: IndexMap<String, U>,
    options: StoreOptions,
    cache: Mutex<ManifestCache>,
    generation: AtomicU64,
    next_id: AtomicU64,
    /// In-flight uplink syncs by package name and kind (normal / forced), with an id so a task
    /// only removes its own entry.
    refreshes: Mutex<HashMap<RefreshKey, (u64, SharedRefresh)>>,
    /// When the last forced sync of each package started (`refresh_on_miss_min_interval`).
    forced: Mutex<ForcedSyncs>,
    /// In-flight tarball downloads by (package, filename).
    downloads: Mutex<HashMap<DownloadKey, Arc<Download>>>,
    /// Transient memory of syncs, disk reads and downloads.
    memory: Arc<MemoryBudget>,
    /// Uplink requests in flight at once: the sum of the uplinks' `agent_options.maxSockets`
    /// (Verdaccio: 40 per uplink), `None` when one of them is unlimited. A slot is taken before
    /// any work memory, so a task never waits for one while holding memory.
    uplink_slots: Option<Arc<tokio::sync::Semaphore>>,
    /// In-flight reads of `package.json` by package name, with an id so a read only removes its
    /// own entry: concurrent requests for a manifest not in RAM share one read.
    reads: Mutex<HashMap<(String, bool), (u64, SharedRead)>>,
}

/// Work memory taken before an uplink request (headers, small bodies).
const SYNC_BASE_BYTES: usize = 64 * 1024;
/// Work memory of a tarball download before its first chunk.
const DOWNLOAD_BASE_BYTES: usize = 64 * 1024;

/// Where a tarball download goes.
enum Source<U> {
    /// A configured uplink, by name.
    Configured(String),
    /// A throw-away client for an off-uplink URL ([`Uplink::for_foreign_url`]).
    Foreign(U),
}

/// What [`Inner::write_back`] merges into the disk copy.
enum WriteSource {
    /// The local manifest (a 304 or a `maxage` hit): borrowed, versions copied.
    Local(Arc<Loaded>),
    /// The merge result: consumed, versions moved.
    Merged(Manifest),
}

/// The uplink side of a sync, once the uplink loop is over.
enum Synced {
    /// No uplink answered (or none is configured for the package).
    Nothing,
    /// The local copy, with `_uplinks` possibly refreshed by 304 answers or a `maxage` hit.
    Local,
    /// Local copy (or template) merged with a 200 answer.
    Merged(Manifest),
}

/// Start times of forced syncs by package, to keep them `min_interval` apart.
#[derive(Default)]
struct ForcedSyncs {
    started: HashMap<String, Instant>,
    /// Size at which expired entries are dropped (amortized pruning).
    prune_at: usize,
}

impl ForcedSyncs {
    const PRUNE_FLOOR: usize = 1024;

    /// Record a forced sync of `name` starting now, unless the previous one started less than
    /// `min_interval` ago.
    fn try_start(&mut self, name: &str, min_interval: Duration) -> bool {
        let now = Instant::now();
        if let Some(last) = self.started.get(name)
            && now.duration_since(*last) < min_interval
        {
            return false;
        }
        if min_interval.is_zero() {
            return true;
        }
        self.started.insert(name.to_owned(), now);
        if self.started.len() >= self.prune_at.max(Self::PRUNE_FLOOR) {
            self.started.retain(|_, t| now.duration_since(*t) < min_interval);
            self.prune_at = self.started.len() * 2;
        }
        true
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn internal_error() -> RegistryError {
    RegistryError::internal(messages::INTERNAL_SERVER_ERROR)
}

fn no_such_file() -> RegistryError {
    RegistryError::not_found(messages::NO_SUCH_FILE)
}

fn no_package_on_uplink(name: &str) -> RegistryError {
    RegistryError::not_found(format!("{}: {name}", messages::NOT_PACKAGE_UPLINK))
}

impl<S: PackageStorage, U: Uplink> Store<S, U> {
    /// New store. `uplinks` is keyed by uplink name (as in [`Config::uplinks`]); the `proxy` list
    /// of the matching `packages` rule selects and orders them per package. A name in `proxy`
    /// with no entry in `uplinks` counts as an uplink that failed (Verdaccio looks it up and
    /// throws), so it never answers but never stops the package from being served from cache.
    pub fn new(
        config: Arc<Config>,
        storage: S,
        uplinks: IndexMap<String, U>,
        options: StoreOptions,
    ) -> Self {
        let cache = Mutex::new(ManifestCache::new(options.manifest_cache_bytes));
        let memory = Arc::clone(&options.memory);
        let sockets: Option<u64> =
            config.uplinks.values().map(|u| u.max_sockets.map(u64::from)).sum();
        let uplink_slots = sockets
            .filter(|n| *n > 0)
            .map(|n| {
                usize::try_from(n).unwrap_or(usize::MAX).min(tokio::sync::Semaphore::MAX_PERMITS)
            })
            .map(|n| Arc::new(tokio::sync::Semaphore::new(n)));
        Self {
            inner: Arc::new(Inner {
                config,
                storage,
                uplinks,
                options,
                cache,
                generation: AtomicU64::new(0),
                next_id: AtomicU64::new(0),
                refreshes: Mutex::new(HashMap::new()),
                forced: Mutex::new(ForcedSyncs::default()),
                downloads: Mutex::new(HashMap::new()),
                memory,
                uplink_slots,
                reads: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The work memory budget ([`StoreOptions::memory`]).
    pub fn memory(&self) -> &Arc<MemoryBudget> {
        &self.inner.memory
    }

    /// Bytes the manifest cache counts now (see [`StoreOptions::manifest_cache_bytes`]).
    pub fn manifest_cache_used(&self) -> u64 {
        lock(&self.inner.cache).used()
    }

    /// The config this store was built with.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// The storage backend.
    pub fn storage(&self) -> &S {
        &self.inner.storage
    }

    /// Manifest for `GET /:package` and `GET /:package/:version` (`getPackage`).
    ///
    /// `name` is a valid package name. `remote_address` is forwarded to uplinks as
    /// `X-Forwarded-For`.
    ///
    /// Algorithm (Verdaccio `syncUplinksMetadata` + `updateVersionsNext`):
    ///
    /// 1. Local copy: the RAM cache entry if its [`fastopm_storage::FileIdentity`] still matches
    ///    the file, else [`PackageStorage::read_manifest`]. A corrupt file fails with
    ///    `500 internal server error`.
    /// 2. Uplinks = the `proxy` list of the first matching `packages` rule. None: serve the local
    ///    copy without writing, or fail with `404 package does not exist on uplink: <name>`.
    /// 3. For each uplink in order, starting from the local copy (or `Manifest::template(name)`):
    ///    - `_uplinks[u].fetched` younger than `maxage` → use the local copy as is, stop;
    ///    - else fetch with `If-None-Match: _uplinks[u].etag`;
    ///      - 200 → merge (`_uplinks[u] = {etag, fetched: now}`, `time` = local ∪ remote with
    ///        remote winning, `mergeVersions`, maintainers scoped as `$uplink:<u>:<name>` when
    ///        the local list is empty) and stop;
    ///      - 304 with a local copy whose `_uplinks[u]` has a string etag → refresh
    ///        `_uplinks[u].fetched`, continue with the next uplink;
    ///      - error → remember it, continue.
    /// 4. If some uplink succeeded (or answered 304): `updateVersionsNext` under
    ///    [`PackageStorage::update_manifest`] — add new versions (readme cleared, `_distfiles`
    ///    entry added with `registry` when the tarball host is the uplink host), update readme,
    ///    dist-tags, `_uplinks`, `time`; write (with `_rev` bumped) only if something changed.
    ///    The result is the manifest served.
    /// 5. Else, if there is a local copy, serve it unchanged (uplink down → cached data).
    /// 6. Else fail: `503 looks like the server is taking to long to respond` if any uplink error
    ///    was [`fastopm_uplink::UplinkError::Timeout`], otherwise `404 no such package available`.
    ///
    /// Served manifests have `normalizeDistTags` applied (missing `latest` = highest valid
    /// version; array tags collapsed; invalid tags dropped). Concurrent calls for the same name
    /// share step 3–4.
    ///
    /// When step 3 stops at a `maxage` hit and step 4 would find nothing to change, the cached
    /// copy is served without re-reading the file (same result, no write either way).
    pub async fn get_manifest(
        &self,
        name: &str,
        remote_address: Option<&str>,
    ) -> Result<PackageSnapshot, RegistryError> {
        let handle = self.open_manifest(name, remote_address).await?;
        handle.load(&self.inner.memory).await
    }

    /// [`Store::get_manifest`] without reading a manifest that is not in RAM: the handle tells
    /// the generation to serve, so a body in the render cache needs no parsed manifest. Same
    /// algorithm and results otherwise.
    pub async fn open_manifest(
        &self,
        name: &str,
        remote_address: Option<&str>,
    ) -> Result<PackageHandle, RegistryError> {
        Inner::manifest(&self.inner, name, remote_address).await.map(|(handle, _)| handle)
    }

    /// Manifest for `GET /:package/:version`: [`Store::get_manifest`], plus `refresh_on_miss`.
    ///
    /// `contains` tells whether a manifest has what the client asked for (the version or
    /// dist-tag). With `fastopm.refresh_on_miss` off this is exactly [`Store::get_manifest`]
    /// (`contains` is not called). With it on, when the manifest was served from the local copy
    /// because the first configured uplink of the package is within `maxage` (no uplink was
    /// asked) and `contains` is `false`, the package gets a forced sync: the same uplink loop
    /// and write-back as a normal sync, but ignoring `maxage` (`If-None-Match` is still sent).
    /// The synced manifest is returned whether or not it now contains the version.
    ///
    /// Forced syncs of one package start at most once per `refresh_on_miss_min_interval`;
    /// concurrent callers share the one in flight. When the sync cannot start (interval) or
    /// fails, the manifest from cache is returned: the client gets what it would get with the
    /// option off. A failing uplink goes through the usual retry and circuit-breaker rules.
    pub async fn get_manifest_for(
        &self,
        name: &str,
        remote_address: Option<&str>,
        contains: impl Fn(&Manifest) -> bool,
    ) -> Result<PackageSnapshot, RegistryError> {
        let handle = self.open_manifest_for(name, remote_address, contains).await?;
        handle.load(&self.inner.memory).await
    }

    /// [`Store::get_manifest_for`] as a [`PackageHandle`] (see [`Store::open_manifest`]). The
    /// manifest is read only when `contains` has to be asked.
    pub async fn open_manifest_for(
        &self,
        name: &str,
        remote_address: Option<&str>,
        contains: impl Fn(&Manifest) -> bool,
    ) -> Result<PackageHandle, RegistryError> {
        let inner = &self.inner;
        let (handle, maxage_hit) = Inner::manifest(inner, name, remote_address).await?;
        if !maxage_hit || !inner.config.fastopm.refresh_on_miss {
            return Ok(handle);
        }
        let snapshot = handle.load(&inner.memory).await?;
        if contains(&snapshot.manifest) {
            return Ok(inner.handle_of(name, snapshot));
        }
        let Some(refresh) = Inner::forced_refresh(inner, name, remote_address) else {
            return Ok(inner.handle_of(name, snapshot));
        };
        let chosen = match refresh.await {
            Ok(Some(fresh)) => fresh,
            Ok(None) => snapshot,
            Err(e) => {
                tracing::debug!(package = name, error = %e, "refresh_on_miss: forced sync failed");
                snapshot
            }
        };
        Ok(inner.handle_of(name, chosen))
    }

    /// Tarball for `GET /:package/-/:filename` (`getTarball`).
    ///
    /// `name` is a valid package name, `filename` a valid name.
    ///
    /// 1. Local file → stream it.
    /// 2. Else find the source URL with `lookupDistFile` (`_distfiles[filename]`, else a version
    ///    whose `dist.tarball` path ends in `/<filename>`) in the local manifest; when absent,
    ///    refresh the manifest from uplinks as in [`Store::get_manifest`] (errors propagate) and
    ///    look again. Still absent → `404 no such file available`.
    ///
    ///    With `fastopm.refresh_on_miss`, when the local manifest is within `maxage` of its first
    ///    configured uplink, that refresh is a forced sync instead (ignores `maxage`; shared and
    ///    rate-limited as in [`Store::get_manifest_for`]). If it cannot start or fails, the
    ///    normal refresh runs, so the client gets what it would get with the option off.
    /// 3. Pick the uplink: the one named in `registry` if it serves the URL; else, among the
    ///    uplinks listed in the package's `proxy`, the last one in `uplinks` declaration order
    ///    that serves it ([`Uplink::serves_url`]). Still none: if the URL came from
    ///    `_distfiles[filename]` of the cached manifest (an uplink-synced package whose tarballs
    ///    live on another host, e.g. a CDN), download it with a throw-away
    ///    [`Uplink::for_foreign_url`] client (no credentials); otherwise (the URL only comes from
    ///    a version's `dist.tarball`, i.e. client-controlled) refuse with
    ///    `404 no such file available`.
    /// 4. Download ([`Uplink::fetch_tarball`]; status errors via
    ///    [`fastopm_uplink::UplinkError::to_tarball_error`]). If the uplink caches tarballs,
    ///    write the bytes to [`PackageStorage::create_tarball`] while streaming them to the
    ///    caller; commit when the stream ends cleanly, abort on any error or when the client
    ///    goes away before the end (the download may continue for other waiters).
    ///
    /// Concurrent requests for the same missing tarball share one download: every caller gets
    /// the whole file, and the download stops early only when all of them went away. A storage
    /// error while writing the cache copy only drops the cache copy; clients still get the bytes.
    pub async fn get_tarball(
        &self,
        name: &str,
        filename: &str,
    ) -> Result<TarballBody, RegistryError> {
        let inner = &self.inner;
        if let Some(body) =
            inner.storage.open_tarball(name, filename).await.map_err(|e| e.to_registry_error())?
        {
            return Ok(body);
        }
        let key: DownloadKey = (name.to_owned(), filename.to_owned());
        if let Some(sub) = inner.join_download(&key) {
            return sub.into_body().await;
        }

        // getTarballFromUpstream: a local read error counts as "no local copy". The lookup
        // reads the manifest within a reservation, released before waiting for the download.
        let mut work = Some(inner.memory.reserve(DOWNLOAD_BASE_BYTES).await);
        let local = match work.as_mut() {
            Some(w) => inner.local_tree_within(name, w).await.ok().flatten(),
            None => None,
        };
        let found = local
            .as_ref()
            .and_then(|l| lookup_dist_file(&l.local, filename).map(|d| (d, Arc::clone(&l.local))));
        let (dist, origin) = match found {
            Some(found) => found,
            None => {
                let forced = local
                    .as_ref()
                    .filter(|l| {
                        inner.config.fastopm.refresh_on_miss && inner.maxage_hit(&l.meta, name)
                    })
                    .and_then(|_| Inner::forced_refresh(inner, name, None));
                drop(local);
                // never wait for a sync while holding a reservation
                drop(work.take());
                let refreshed = match forced {
                    Some(refresh) => match refresh.await {
                        Ok(snapshot) => snapshot,
                        Err(e) => {
                            tracing::debug!(package = name, error = %e, "refresh_on_miss: forced sync failed");
                            Inner::refresh(inner, name, None, false).await?
                        }
                    },
                    None => Inner::refresh(inner, name, None, false).await?,
                };
                let snapshot = refreshed.ok_or_else(no_such_file)?;
                let dist =
                    lookup_dist_file(&snapshot.manifest, filename).ok_or_else(no_such_file)?;
                (dist, snapshot.manifest)
            }
        };
        let source = inner.select_uplink(name, filename, &dist, &origin)?;
        drop(origin);
        drop(work);
        Inner::start_download(inner, key, dist.url, source).into_body().await
    }
}

impl<S: PackageStorage, U: Uplink> Inner<S, U> {
    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// `getProxiesForPackage`.
    fn proxy_list(&self, name: &str) -> Vec<String> {
        self.config.package_rule(name).map(|r| r.proxy.clone()).unwrap_or_default()
    }

    fn cache_enabled(&self) -> bool {
        self.options.manifest_cache_bytes > 0
    }

    /// A slot for uplink requests (see [`Inner::uplink_slots`]); taken before any memory.
    async fn uplink_slot(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let slots = self.uplink_slots.as_ref()?;
        Arc::clone(slots).acquire_owned().await.ok()
    }

    /// Free entries removed from the cache (outside its lock), see [`cache::Evicted::release`].
    fn release(&self, evicted: impl IntoIterator<Item = cache::Evicted>) {
        for e in evicted {
            e.release(&self.memory);
        }
    }

    /// Step 1: what RAM holds about the local manifest, reading the file when RAM has nothing
    /// for its current identity. The tree may be absent ([`cache::Resident::tree`]).
    async fn local(self: &Arc<Self>, name: &str) -> Result<Option<Resident>, RegistryError> {
        let identity =
            self.storage.manifest_identity(name).await.map_err(|e| e.to_registry_error())?;
        let Some(identity) = identity else {
            let evicted = lock(&self.cache).remove(name);
            self.release(evicted);
            return Ok(None);
        };
        let hit = lock(&self.cache).get(name).filter(|r| r.meta.identity == identity);
        if hit.is_some() {
            return Ok(hit);
        }
        let loaded = self.read_tree(name, false).await?;
        Ok(loaded.map(|l| Resident { meta: Arc::clone(&l.meta), tree: Some(l) }))
    }

    /// The local manifest (`getPackageLocalMetadata`), read from disk when its tree is not in
    /// the cache. The read is counted in `work` (grown first, possibly waiting), which then keeps
    /// counting the tree unless the cache took it.
    async fn local_tree_within(
        self: &Arc<Self>,
        name: &str,
        work: &mut Reservation,
    ) -> Result<Option<Arc<Loaded>>, RegistryError> {
        let identity =
            self.storage.manifest_identity(name).await.map_err(|e| e.to_registry_error())?;
        let Some(identity) = identity else {
            let evicted = lock(&self.cache).remove(name);
            self.release(evicted);
            return Ok(None);
        };
        let hit = lock(&self.cache).get(name).filter(|r| r.meta.identity == identity);
        let need = match hit {
            Some(Resident { tree: Some(tree), .. }) => return Ok(Some(tree)),
            // the file and its tree, whose size is known from an earlier read
            Some(Resident { meta, tree: None }) => {
                usize::try_from(meta.tree_bytes + identity.len).unwrap_or(usize::MAX)
            }
            None => file_load_bytes(identity.len),
        };
        let before = work.held();
        work.grow(need).await;
        let loaded = self.read_tree(name, true).await?;
        let kept = loaded.as_ref().filter(|l| !l.cached).map_or(0, |l| l.meta.tree_bytes);
        work.shrink_to(before + usize::try_from(kept).unwrap_or(usize::MAX));
        Ok(loaded)
    }

    /// Read and parse `package.json`, shared by concurrent callers; a corrupt file fails with
    /// `500 internal server error`. `covered`: the caller counts the read in its own
    /// reservation; otherwise the read reserves its memory itself (and keeps counting a tree the
    /// cache does not take until its last user is done). The two kinds are never shared: a
    /// caller holding a reservation must not wait for another one.
    async fn read_tree(self: &Arc<Self>, name: &str, covered: bool) -> ReadResult {
        let key = (name.to_owned(), covered);
        let shared = {
            let mut reads = lock(&self.reads);
            match reads.get(&key) {
                Some((_, running)) => running.clone(),
                None => {
                    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    let inner = Arc::clone(self);
                    let task_key = key.clone();
                    let shared = async move {
                        let result = inner.read_tree_now(&task_key.0, task_key.1).await;
                        let mut reads = lock(&inner.reads);
                        if reads.get(&task_key).is_some_and(|(i, _)| *i == id) {
                            reads.remove(&task_key);
                        }
                        result
                    }
                    .boxed()
                    .shared();
                    reads.insert(key, (id, shared.clone()));
                    shared
                }
            }
        };
        shared.await
    }

    async fn read_tree_now(&self, name: &str, covered: bool) -> ReadResult {
        let identity =
            self.storage.manifest_identity(name).await.map_err(|e| e.to_registry_error())?;
        let Some(identity) = identity else {
            let evicted = lock(&self.cache).remove(name);
            self.release(evicted);
            return Ok(None);
        };
        let work = match covered {
            true => None,
            false => Some(self.memory.reserve(file_load_bytes(identity.len)).await),
        };
        match self.storage.read_manifest(name).await.map_err(|e| e.to_registry_error())? {
            Some(stored) => {
                Ok(Some(self.remember(name, stored.manifest, stored.identity, false, work)))
            }
            None => {
                let evicted = lock(&self.cache).remove(name);
                self.release(evicted);
                Ok(None)
            }
        }
    }

    /// Turn a manifest as stored on disk into a cache entry and keep it. The generation is
    /// reused when the file is the one already known (same identity) or the served content
    /// equals the previous entry's (unless `changed` says it cannot). `work` is the reservation
    /// that covered building the tree: it keeps counting the tree while it is not in the cache
    /// (a [`fastopm_core::memory::PassiveHold`] released with the last snapshot), and is
    /// released otherwise.
    fn remember(
        &self,
        name: &str,
        disk: Manifest,
        identity: FileIdentity,
        changed: bool,
        work: Option<Reservation>,
    ) -> Arc<Loaded> {
        let mut local = disk;
        normalize_package(&mut local);
        let local = Arc::new(local);
        let served = match normalized_dist_tags(&local) {
            None => Arc::clone(&local),
            Some(tags) => {
                let mut m = (*local).clone();
                m.insert(DIST_TAGS, Value::Object(tags));
                Arc::new(m)
            }
        };
        let mut tree_bytes = local.heap_bytes() as u64;
        if !Arc::ptr_eq(&local, &served) {
            tree_bytes += served.heap_bytes() as u64;
        }
        let previous = if changed { None } else { lock(&self.cache).peek(name) };
        let generation = match previous {
            Some(p) if p.meta.identity == identity => p.meta.generation,
            Some(Resident { meta, tree: Some(p) })
                if Arc::ptr_eq(&p.served, &served) || *p.served == *served =>
            {
                meta.generation
            }
            _ => self.next_generation(),
        };
        let meta = Arc::new(Meta {
            identity,
            generation,
            uplinks: uplinks_of(&local),
            changes_self: would_change_self(&local),
            tree_bytes,
        });
        let cached = self.cache_enabled() && lock(&self.cache).keeps_tree(tree_bytes);
        let hold = match work {
            Some(mut work) if !cached => {
                work.shrink_to(usize::try_from(tree_bytes).unwrap_or(usize::MAX));
                Some(work.into_passive())
            }
            _ => None,
        };
        let loaded = Arc::new(Loaded::new(meta, local, served, hold, cached));
        if self.cache_enabled() {
            let (_, evicted) = lock(&self.cache).insert(name, &loaded);
            self.release(evicted);
        }
        loaded
    }

    /// A handle for what RAM holds.
    fn handle(self: &Arc<Self>, name: &str, resident: &Resident) -> PackageHandle {
        let load_bytes = file_load_bytes(resident.meta.identity.len).min(
            usize::try_from(resident.meta.tree_bytes + resident.meta.identity.len)
                .unwrap_or(usize::MAX),
        );
        PackageHandle {
            generation: resident.meta.generation,
            file_len: resident.meta.identity.len,
            snapshot: resident.tree.as_ref().map(Loaded::snapshot),
            name: name.to_owned(),
            load_bytes,
            loader: Arc::clone(self) as Arc<dyn LoadTree>,
        }
    }

    /// A handle for a manifest in hand.
    fn handle_of(self: &Arc<Self>, name: &str, snapshot: PackageSnapshot) -> PackageHandle {
        PackageHandle {
            generation: snapshot.generation,
            file_len: snapshot.file_len,
            snapshot: Some(snapshot),
            name: name.to_owned(),
            load_bytes: 0,
            loader: Arc::clone(self) as Arc<dyn LoadTree>,
        }
    }

    /// Steps 1–5 of [`Store::get_manifest`]. The flag tells whether the result is the local copy
    /// served because of a `maxage` hit ([`Inner::maxage_hit`]: no uplink was asked).
    async fn manifest(
        self: &Arc<Self>,
        name: &str,
        remote_address: Option<&str>,
    ) -> Result<(PackageHandle, bool), RegistryError> {
        let local = self.local(name).await?;
        let proxy = self.proxy_list(name);
        let fresh = local.as_ref().and_then(|l| self.first_uplink_fresh(&l.meta.uplinks, &proxy));
        let maxage_hit = fresh == Some(true);
        match &local {
            // no configured uplink, or a `maxage` hit with nothing for the sync to change
            Some(l) if fresh.is_none() || (maxage_hit && !l.meta.changes_self) => {
                return Ok((self.handle(name, l), maxage_hit));
            }
            None if proxy.is_empty() => return Err(no_package_on_uplink(name)),
            _ => {}
        }
        drop(local);
        let snapshot = Inner::refresh(self, name, remote_address, false)
            .await?
            .ok_or_else(|| no_package_on_uplink(name))?;
        Ok((self.handle_of(name, snapshot), maxage_hit))
    }

    /// `_uplinks[u].fetched` of the first configured uplink `u` of `proxy` is younger than its
    /// `maxage`; `None` when `proxy` names no configured uplink. A normal sync stops at that
    /// uplink without asking it, so with `Some(true)` the local copy is served as is.
    fn first_uplink_fresh(&self, uplinks: &Map<String, Value>, proxy: &[String]) -> Option<bool> {
        let (name, uplink) = proxy.iter().find_map(|n| self.uplinks.get(n).map(|u| (n, u)))?;
        Some(
            uplink_fetched(uplinks, name)
                .is_some_and(|f| (now_ms() as f64) - f < uplink.maxage().as_millis() as f64),
        )
    }

    /// Whether a normal sync of `name` would serve the local copy without asking any uplink.
    fn maxage_hit(&self, local: &Meta, name: &str) -> bool {
        self.first_uplink_fresh(&local.uplinks, &self.proxy_list(name)) == Some(true)
    }

    /// Steps 3–4, shared by concurrent callers for the same package. Runs in a spawned task so
    /// it completes even if every caller goes away. `forced` syncs ignore `maxage` and never
    /// share a normal sync (which may stop at a `maxage` hit), nor the other way round.
    fn refresh(
        self: &Arc<Self>,
        name: &str,
        remote_address: Option<&str>,
        forced: bool,
    ) -> SharedRefresh {
        let mut refreshes = lock(&self.refreshes);
        let key: RefreshKey = (name.to_owned(), forced);
        if let Some((_, running)) = refreshes.get(&key) {
            return running.clone();
        }
        self.spawn_refresh(&mut refreshes, key, remote_address)
    }

    /// A forced sync of `name` (`refresh_on_miss`): joins the one in flight, else starts one
    /// unless the previous one started less than `refresh_on_miss_min_interval` ago (`None`).
    fn forced_refresh(
        self: &Arc<Self>,
        name: &str,
        remote_address: Option<&str>,
    ) -> Option<SharedRefresh> {
        let mut refreshes = lock(&self.refreshes);
        let key: RefreshKey = (name.to_owned(), true);
        if let Some((_, running)) = refreshes.get(&key) {
            return Some(running.clone());
        }
        let min_interval = self.config.fastopm.refresh_on_miss_min_interval;
        if !lock(&self.forced).try_start(name, min_interval) {
            tracing::debug!(package = name, "refresh_on_miss: forced sync skipped (min interval)");
            return None;
        }
        tracing::debug!(package = name, "refresh_on_miss: forced sync");
        Some(self.spawn_refresh(&mut refreshes, key, remote_address))
    }

    fn spawn_refresh(
        self: &Arc<Self>,
        refreshes: &mut HashMap<RefreshKey, (u64, SharedRefresh)>,
        key: RefreshKey,
        remote_address: Option<&str>,
    ) -> SharedRefresh {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::clone(self);
        let task_key = key.clone();
        let address = remote_address.map(str::to_owned);
        let handle = tokio::spawn(async move {
            let slot = inner.uplink_slot().await;
            let result = inner.sync(&task_key.0, address.as_deref(), task_key.1).await;
            drop(slot);
            let mut refreshes = lock(&inner.refreshes);
            if refreshes.get(&task_key).is_some_and(|(i, _)| *i == id) {
                refreshes.remove(&task_key);
            }
            result
        });
        let shared =
            async move { handle.await.unwrap_or_else(|_| Err(internal_error())) }.boxed().shared();
        refreshes.insert(key, (id, shared.clone()));
        shared
    }

    /// `syncUplinksMetadata` followed by `updateVersionsNext`. `Ok(None)`: no uplink is
    /// configured for the package and there is no local copy. `forced`: skip the `maxage` check
    /// (every uplink is asked, in order, as if its cache had expired).
    ///
    /// Memory: one work reservation for the whole sync, grown while the uplink body streams in
    /// and is parsed, by the copy of the local tree the answer is merged into, then by the disk
    /// copy and its serialized form in [`Inner::write_back`]; it waits when the budget is spent.
    async fn sync(
        self: &Arc<Self>,
        name: &str,
        remote_address: Option<&str>,
        forced: bool,
    ) -> RefreshResult {
        let mut work = self.memory.reserve(SYNC_BASE_BYTES).await;
        let local = self.local_tree_within(name, &mut work).await?;
        let proxy = self.proxy_list(name);
        if proxy.is_empty() {
            return Ok(local.map(|l| l.snapshot()));
        }

        // `_uplinks` of `localManifest`, updated in place by 304 answers.
        let mut local_uplinks = local.as_ref().map(|l| l.meta.uplinks.clone());
        let mut local_modified = false;
        let mut synced = Synced::Nothing;
        let mut synced_uplink: Option<String> = None;
        // Errors that can become the client error (timeouts, a 304 without local data), in order.
        let mut errors: Vec<RegistryError> = Vec::new();
        let no_uplinks = Map::new();

        for uplink_name in &proxy {
            let Some(uplink) = self.uplinks.get(uplink_name) else {
                tracing::debug!(package = name, uplink = %uplink_name, "uplink not configured");
                continue;
            };
            let temp_uplinks = local_uplinks.as_ref().unwrap_or(&no_uplinks);
            if let Some(fetched) = uplink_fetched(temp_uplinks, uplink_name).filter(|_| !forced) {
                if (now_ms() as f64) - fetched < uplink.maxage().as_millis() as f64 {
                    synced = Synced::Local;
                    synced_uplink = Some(uplink_name.clone());
                    break;
                }
            }
            let etag = uplink_etag(temp_uplinks, uplink_name).map(str::to_owned);
            let fetched = uplink
                .fetch_manifest_within(name, etag.as_deref(), remote_address, &mut work)
                .await;
            match fetched {
                Ok(ManifestFetch::Fetched { manifest: remote, etag }) => {
                    let base = match &local {
                        Some(l) => {
                            work.grow(usize::try_from(l.meta.tree_bytes).unwrap_or(usize::MAX))
                                .await;
                            let mut m = (*l.local).clone();
                            if local_modified {
                                let ups = local_uplinks.clone().unwrap_or_default();
                                m.insert(UPLINKS_KEY, Value::Object(ups));
                            }
                            m
                        }
                        None => Manifest::template(name),
                    };
                    match merge_cache_remote(base, remote, name, uplink_name, etag, now_ms()) {
                        Ok(merged) => {
                            synced = Synced::Merged(merged);
                            synced_uplink = Some(uplink_name.clone());
                            break;
                        }
                        Err(_) => {
                            work.shrink_to(SYNC_BASE_BYTES);
                            tracing::warn!(package = name, uplink = %uplink_name, "manifest merge failed");
                        }
                    }
                }
                Ok(ManifestFetch::NotModified) => {
                    let etag = local_uplinks
                        .as_ref()
                        .and_then(|u| uplink_etag_for_304(u, uplink_name))
                        .map(str::to_owned);
                    match (local.is_some(), etag, local_uplinks.as_mut()) {
                        (true, Some(etag), Some(ups)) => {
                            let meta = UplinkMeta { etag: Some(etag), fetched: Some(now_ms()) };
                            let value = serde_json::to_value(&meta).unwrap_or_default();
                            ups.insert(uplink_name.clone(), value);
                            local_modified = true;
                            synced = Synced::Local;
                            synced_uplink = Some(uplink_name.clone());
                        }
                        _ => errors.push(RegistryError::not_found(messages::NOT_MODIFIED_NO_DATA)),
                    }
                }
                Err(e) => {
                    work.shrink_to(SYNC_BASE_BYTES);
                    tracing::debug!(package = name, uplink = %uplink_name, error = %e, "uplink error");
                    if e.is_timeout() {
                        errors.push(RegistryError::service_unavailable(messages::SERVER_TIME_OUT));
                    }
                }
            }
        }

        let disk_bytes = local.as_ref().map_or(0, |l| l.meta.tree_bytes);
        let (source, source_uplinks) = match synced {
            Synced::Nothing => {
                return match local {
                    Some(l) => Ok(Some(l.snapshot())),
                    None => Err(errors
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| RegistryError::not_found(messages::NO_PACKAGE))),
                };
            }
            Synced::Local => {
                let Some(l) = local else {
                    return Err(RegistryError::not_found(messages::NO_PACKAGE));
                };
                if !local_modified && !would_change_self(&l.local) {
                    return Ok(Some(l.snapshot()));
                }
                (WriteSource::Local(l), local_uplinks.unwrap_or_default())
            }
            Synced::Merged(m) => {
                // the local tree is not needed any more (write_back reads the disk copy)
                drop(local);
                let ups = uplinks_of(&m);
                (WriteSource::Merged(m), ups)
            }
        };
        let uplink_name = synced_uplink.unwrap_or_default();
        self.write_back(name, source, source_uplinks, uplink_name, disk_bytes, work).await.map(Some)
    }

    /// `updateVersionsNext` under the storage lock; returns what is served. `disk_bytes`: tree
    /// size of the disk copy that gets read again (0 for a new package).
    async fn write_back(
        &self,
        name: &str,
        source: WriteSource,
        source_uplinks: Map<String, Value>,
        uplink_name: String,
        disk_bytes: u64,
        mut work: Reservation,
    ) -> Result<PackageSnapshot, RegistryError> {
        // count the source tree (the uplink body is gone), the disk copy read again, and the file
        // written (its buffer is sized from an estimate of its length)
        let written_len = match &source {
            WriteSource::Merged(m) => {
                work.shrink_to(SYNC_BASE_BYTES + m.heap_bytes());
                m.compact_len_estimate()
            }
            WriteSource::Local(l) => usize::try_from(l.meta.identity.len).unwrap_or(usize::MAX),
        };
        let disk_bytes = usize::try_from(disk_bytes).unwrap_or(usize::MAX);
        work.grow(disk_bytes.saturating_add(written_len)).await;

        let uplink_url = self.config.uplinks.get(&uplink_name).map(|u| u.url.clone());
        let written = Arc::new(AtomicBool::new(false));
        let written_flag = Arc::clone(&written);
        let template_name = name.to_owned();
        let stored = self
            .storage
            .update_manifest(name, move |disk| {
                // readCreatePackage
                let mut cache = disk.unwrap_or_else(|| Manifest::template(&template_name));
                normalize_package(&mut cache);
                let updated = match source {
                    WriteSource::Merged(m) => update_versions_next_owned(
                        cache,
                        m,
                        &source_uplinks,
                        &uplink_name,
                        uplink_url.as_deref(),
                    ),
                    WriteSource::Local(l) => {
                        let src = SyncSource {
                            manifest: &l.local,
                            uplinks: &source_uplinks,
                            uplink_name: &uplink_name,
                            uplink_url: uplink_url.as_deref(),
                        };
                        update_versions_next(cache, &src)
                    }
                };
                let (mut cache, changed) = updated.map_err(|_| internal_error())?;
                if !changed {
                    return Ok(None);
                }
                cache.bump_revision();
                written_flag.store(true, Ordering::SeqCst);
                Ok(Some(cache))
            })
            .await
            .map_err(|e| e.to_registry_error())?
            .ok_or_else(|| RegistryError::not_found(messages::NO_PACKAGE))?;

        if written.load(Ordering::SeqCst) {
            // Verdaccio serves `normalizeDistTags(written)`: that is `local` of the new entry.
            let loaded = self.remember(name, stored.manifest, stored.identity, true, Some(work));
            return Ok(if Arc::ptr_eq(&loaded.local, &loaded.served) {
                loaded.snapshot()
            } else {
                PackageSnapshot {
                    manifest: Arc::clone(&loaded.local),
                    generation: self.next_generation(),
                    file_len: loaded.meta.identity.len,
                    cached: loaded.cached,
                    _keep: Some(Arc::clone(&loaded)),
                }
            });
        }
        let current = lock(&self.cache).peek(name).filter(|l| l.meta.identity == stored.identity);
        Ok(match current.and_then(|c| c.tree) {
            Some(tree) => tree.snapshot(),
            None => {
                self.remember(name, stored.manifest, stored.identity, false, Some(work)).snapshot()
            }
        })
    }

    /// `getUpLinkForDistFile`.
    fn select_uplink(
        &self,
        name: &str,
        filename: &str,
        dist: &DistSource,
        origin: &Manifest,
    ) -> Result<Source<U>, RegistryError> {
        // 1. the recorded uplink, only when it serves the URL
        if let Some(registry) = dist.registry.as_deref().filter(|r| !r.is_empty()) {
            if self.uplinks.get(registry).is_some_and(|u| u.serves_url(&dist.url)) {
                return Ok(Source::Configured(registry.to_owned()));
            }
        }
        // 2. the last uplink (declaration order) of the package's proxy list serving the URL
        let proxy = self.proxy_list(name);
        let chosen = self
            .uplinks
            .iter()
            .filter(|(n, u)| proxy.iter().any(|p| p == *n) && u.serves_url(&dist.url))
            .map(|(n, _)| n.clone())
            .last();
        if let Some(chosen) = chosen {
            return Ok(Source::Configured(chosen));
        }
        // 3. a client-controlled URL is never fetched
        if !has_distfile_record(origin, filename) {
            tracing::warn!(package = name, url = %dist.url, "refused off-uplink tarball fetch");
            return Err(no_such_file());
        }
        // 4. uplink-synced package with tarballs on another host
        tracing::info!(package = name, url = %dist.url, "off-uplink tarball via throw-away client");
        U::for_foreign_url(name, &dist.url, &self.config)
            .map(Source::Foreign)
            .map_err(|_| internal_error())
    }

    fn join_download(&self, key: &DownloadKey) -> Option<Subscription> {
        lock(&self.downloads).get(key).map(|d| d.subscribe())
    }

    fn forget_download(&self, key: &DownloadKey, download: &Arc<Download>) {
        let mut downloads = lock(&self.downloads);
        if downloads.get(key).is_some_and(|d| Arc::ptr_eq(d, download)) {
            downloads.remove(key);
        }
    }

    /// Remove the download when nobody listens any more. Under the map lock, so no client can
    /// join it afterwards.
    fn abandon_if_unwatched(&self, key: &DownloadKey, download: &Arc<Download>) -> bool {
        let mut downloads = lock(&self.downloads);
        if !download.abandoned() {
            return false;
        }
        if downloads.get(key).is_some_and(|d| Arc::ptr_eq(d, download)) {
            downloads.remove(key);
        }
        true
    }

    /// Join the download of `key`, or start it from `url` through `source`.
    fn start_download(
        self: &Arc<Self>,
        key: DownloadKey,
        url: String,
        source: Source<U>,
    ) -> Subscription {
        let mut downloads = lock(&self.downloads);
        if let Some(running) = downloads.get(&key) {
            return running.subscribe();
        }
        let download = Download::new();
        let subscription = download.subscribe();
        downloads.insert(key.clone(), Arc::clone(&download));
        drop(downloads);

        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let _slot = inner.uplink_slot().await;
            match &source {
                Source::Configured(uplink_name) => match inner.uplinks.get(uplink_name) {
                    Some(uplink) => inner.download(&key, &url, uplink, &download).await,
                    None => {
                        inner.forget_download(&key, &download);
                        download.set_head(Head::Failed(internal_error()));
                    }
                },
                Source::Foreign(uplink) => inner.download(&key, &url, uplink, &download).await,
            }
        });
        subscription
    }

    /// `fetchTarballFromDistFile`: stream `url` to the subscribers, writing it to storage when
    /// the uplink caches tarballs.
    async fn download<V: Uplink>(
        &self,
        key: &DownloadKey,
        url: &str,
        uplink: &V,
        download: &Arc<Download>,
    ) {
        let (name, filename) = (&key.0, &key.1);
        // The chunks stay in memory until every client has read them: counted while they
        // arrive (a full budget pauses the download), then kept counted by the download.
        let mut work = self.memory.reserve(DOWNLOAD_BASE_BYTES).await;
        let body = match uplink.fetch_tarball(url).await {
            Ok(body) => body,
            Err(e) => {
                tracing::debug!(package = %name, url, error = %e, "tarball download failed");
                self.forget_download(key, download);
                download.set_head(Head::Failed(e.to_tarball_error()));
                return;
            }
        };
        let mut writer = if uplink.cache_tarballs() {
            match self.storage.create_tarball(name, filename).await {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(package = %name, filename = %filename, error = %e, "cannot cache tarball");
                    None
                }
            }
        } else {
            None
        };
        download.set_head(Head::Ready(body.content_length));

        let mut stream = body.stream;
        let mut received: u64 = 0;
        loop {
            // Once every announced byte is in, the clients may already be gone (hyper drops a
            // body after `Content-Length` bytes) while the uplink stream has not reported its
            // end yet (e.g. a trailing empty HTTP/2 DATA frame): finish and cache it.
            let complete = body.content_length.is_some_and(|len| received >= len);
            if !complete && self.abandon_if_unwatched(key, download) {
                tracing::debug!(package = %name, filename = %filename, "tarball download abandoned by every client");
                if let Some(w) = writer.take() {
                    w.abort().await;
                }
                download.finish(Err(&std::io::Error::other("download abandoned")));
                return;
            }
            match stream.next().await {
                Some(Ok(chunk)) => {
                    received += chunk.len() as u64;
                    work.grow(chunk.len()).await;
                    if let Some(w) = writer.as_mut() {
                        if let Err(e) = w.write(chunk.clone()).await {
                            tracing::warn!(package = %name, filename = %filename, error = %e, "cannot cache tarball");
                            if let Some(w) = writer.take() {
                                w.abort().await;
                            }
                        }
                    }
                    download.push(chunk);
                }
                Some(Err(e)) => {
                    tracing::warn!(package = %name, filename = %filename, error = %e, "tarball stream failed");
                    self.forget_download(key, download);
                    if let Some(w) = writer.take() {
                        w.abort().await;
                    }
                    download.finish(Err(&e));
                    return;
                }
                None => break,
            }
        }
        if let Some(w) = writer.take() {
            if let Err(e) = w.commit().await {
                tracing::warn!(package = %name, filename = %filename, error = %e, "cannot cache tarball");
            }
        }
        self.forget_download(key, download);
        download.keep(work.into_passive());
        download.finish(Ok(()));
    }
}
