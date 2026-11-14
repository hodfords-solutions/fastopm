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
//! Disk writes must stay compatible with Verdaccio sharing the same storage: same number of
//! writes, same merge result, `_rev` bumped on every write (`Manifest::bump_revision`).
//!
//! The store needs a Tokio runtime: refreshes and downloads run in spawned tasks so that a
//! client going away does not cancel work other clients wait for.
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
use std::time::{SystemTime, UNIX_EPOCH};

use fastopm_config::Config;
use fastopm_core::constants::DIST_TAGS;
use fastopm_core::manifest::UPLINKS_KEY;
use fastopm_core::{Manifest, RegistryError, TarballBody, UplinkMeta, messages};
use fastopm_storage::{FileIdentity, FsStorage, PackageStorage, TarballWrite};
use fastopm_uplink::{HttpUplink, ManifestFetch, Uplink};
use futures_util::StreamExt;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::cache::{Loaded, ManifestCache};
use crate::download::{Download, Head, Subscription};
use crate::merge::{
    DistSource, SyncSource, has_distfile_record, lookup_dist_file, merge_cache_remote,
    normalize_package, normalized_dist_tags, update_versions_next, uplink_etag,
    uplink_etag_for_304, uplink_fetched, uplinks_of, would_change_self,
};

/// The production store.
pub type DefaultStore = Store<FsStorage, HttpUplink>;

/// Tuning knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOptions {
    /// Upper bound, in bytes of manifest JSON, of the in-memory manifest cache (LRU by size,
    /// not by entry count). `0` disables the cache.
    pub manifest_cache_bytes: u64,
}

impl Default for StoreOptions {
    /// 256 MiB.
    fn default() -> Self {
        Self { manifest_cache_bytes: 256 * 1024 * 1024 }
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
}

/// Coordinates storage and uplinks. Share as `Arc<Store<..>>`; all methods take `&self`.
pub struct Store<S, U> {
    inner: Arc<Inner<S, U>>,
}

type RefreshResult = Result<Option<PackageSnapshot>, RegistryError>;
type SharedRefresh = Shared<BoxFuture<'static, RefreshResult>>;
type DownloadKey = (String, String);

struct Inner<S, U> {
    config: Arc<Config>,
    storage: S,
    uplinks: IndexMap<String, U>,
    options: StoreOptions,
    cache: Mutex<ManifestCache>,
    generation: AtomicU64,
    next_id: AtomicU64,
    /// In-flight uplink syncs by package name (with an id so a task only removes its own entry).
    refreshes: Mutex<HashMap<String, (u64, SharedRefresh)>>,
    /// In-flight tarball downloads by (package, filename).
    downloads: Mutex<HashMap<DownloadKey, Arc<Download>>>,
}

/// Where a tarball download goes.
enum Source<U> {
    /// A configured uplink, by name.
    Configured(String),
    /// A throw-away client for an off-uplink URL ([`Uplink::for_foreign_url`]).
    Foreign(U),
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
                downloads: Mutex::new(HashMap::new()),
            }),
        }
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
        let inner = &self.inner;
        let local = inner.load_local(name).await?;
        let proxy = inner.proxy_list(name);
        match &local {
            Some(l) if proxy.is_empty() || inner.served_without_network(l, &proxy) => {
                return Ok(l.snapshot());
            }
            None if proxy.is_empty() => return Err(no_package_on_uplink(name)),
            _ => {}
        }
        Inner::refresh(inner, name, remote_address).await?.ok_or_else(|| no_package_on_uplink(name))
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

        // getTarballFromUpstream: a local read error counts as "no local copy"
        let local = inner.load_local(name).await.ok().flatten();
        let found = local
            .as_ref()
            .and_then(|l| lookup_dist_file(&l.local, filename).map(|d| (d, Arc::clone(&l.local))));
        let (dist, origin) = match found {
            Some(found) => found,
            None => {
                let snapshot = Inner::refresh(inner, name, None).await?.ok_or_else(no_such_file)?;
                let dist =
                    lookup_dist_file(&snapshot.manifest, filename).ok_or_else(no_such_file)?;
                (dist, snapshot.manifest)
            }
        };
        let source = inner.select_uplink(name, filename, &dist, &origin)?;
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

    /// Step 1: the local manifest, from the RAM cache when the file did not change.
    async fn load_local(&self, name: &str) -> Result<Option<Arc<Loaded>>, RegistryError> {
        let identity =
            self.storage.manifest_identity(name).await.map_err(|e| e.to_registry_error())?;
        let Some(identity) = identity else {
            lock(&self.cache).remove(name);
            return Ok(None);
        };
        if let Some(hit) = lock(&self.cache).get(name).filter(|l| l.identity == identity) {
            return Ok(Some(hit));
        }
        match self.storage.read_manifest(name).await.map_err(|e| e.to_registry_error())? {
            Some(stored) => Ok(Some(self.remember(name, stored.manifest, stored.identity, false))),
            None => {
                lock(&self.cache).remove(name);
                Ok(None)
            }
        }
    }

    /// Turn a manifest as stored on disk into a cache entry and keep it. The generation is
    /// reused when the served content equals the previous entry's (unless `changed` says it
    /// cannot).
    fn remember(
        &self,
        name: &str,
        disk: Manifest,
        identity: FileIdentity,
        changed: bool,
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
        let previous = if changed { None } else { lock(&self.cache).peek(name) };
        let generation = match previous {
            Some(p) if Arc::ptr_eq(&p.served, &served) || *p.served == *served => p.generation,
            _ => self.next_generation(),
        };
        let loaded = Arc::new(Loaded { identity, local, served, generation });
        if self.cache_enabled() {
            lock(&self.cache).insert(name, Arc::clone(&loaded), identity.len);
        }
        loaded
    }

    /// Whether the local copy can be served as is without asking any uplink: the first
    /// configured uplink of `proxy` is within `maxage` and the sync would not change the file
    /// (or `proxy` names no configured uplink at all).
    fn served_without_network(&self, local: &Loaded, proxy: &[String]) -> bool {
        let uplinks = local.local.uplinks();
        for name in proxy {
            let Some(uplink) = self.uplinks.get(name) else { continue };
            let fresh = uplinks
                .and_then(|u| uplink_fetched(u, name))
                .is_some_and(|f| (now_ms() as f64) - f < uplink.maxage().as_millis() as f64);
            return fresh && !would_change_self(&local.local);
        }
        true
    }

    /// Steps 3–4, shared by concurrent callers for the same package. Runs in a spawned task so
    /// it completes even if every caller goes away.
    fn refresh(self: &Arc<Self>, name: &str, remote_address: Option<&str>) -> SharedRefresh {
        let mut refreshes = lock(&self.refreshes);
        if let Some((_, running)) = refreshes.get(name) {
            return running.clone();
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::clone(self);
        let task_name = name.to_owned();
        let address = remote_address.map(str::to_owned);
        let handle = tokio::spawn(async move {
            let result = inner.sync(&task_name, address.as_deref()).await;
            let mut refreshes = lock(&inner.refreshes);
            if refreshes.get(&task_name).is_some_and(|(i, _)| *i == id) {
                refreshes.remove(&task_name);
            }
            result
        });
        let shared =
            async move { handle.await.unwrap_or_else(|_| Err(internal_error())) }.boxed().shared();
        refreshes.insert(name.to_owned(), (id, shared.clone()));
        shared
    }

    /// `syncUplinksMetadata` followed by `updateVersionsNext`. `Ok(None)`: no uplink is
    /// configured for the package and there is no local copy.
    async fn sync(&self, name: &str, remote_address: Option<&str>) -> RefreshResult {
        let local = self.load_local(name).await?;
        let proxy = self.proxy_list(name);
        if proxy.is_empty() {
            return Ok(local.map(|l| l.snapshot()));
        }

        // `_uplinks` of `localManifest`, updated in place by 304 answers.
        let mut local_uplinks = local.as_ref().map(|l| uplinks_of(&l.local));
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
            if let Some(fetched) = uplink_fetched(temp_uplinks, uplink_name) {
                if (now_ms() as f64) - fetched < uplink.maxage().as_millis() as f64 {
                    synced = Synced::Local;
                    synced_uplink = Some(uplink_name.clone());
                    break;
                }
            }
            let etag = uplink_etag(temp_uplinks, uplink_name).map(str::to_owned);
            match uplink.fetch_manifest(name, etag.as_deref(), remote_address).await {
                Ok(ManifestFetch::Fetched { manifest: remote, etag }) => {
                    let base = match &local {
                        Some(l) => {
                            let mut m = (*l.local).clone();
                            if local_modified {
                                let ups = local_uplinks.clone().unwrap_or_default();
                                m.insert(UPLINKS_KEY, Value::Object(ups));
                            }
                            m
                        }
                        None => Manifest::template(name),
                    };
                    match merge_cache_remote(base, &remote, name, uplink_name, etag, now_ms()) {
                        Ok(merged) => {
                            synced = Synced::Merged(merged);
                            synced_uplink = Some(uplink_name.clone());
                            break;
                        }
                        Err(_) => {
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
                    tracing::debug!(package = name, uplink = %uplink_name, error = %e, "uplink error");
                    if e.is_timeout() {
                        errors.push(RegistryError::service_unavailable(messages::SERVER_TIME_OUT));
                    }
                }
            }
        }

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
                let Some(l) = &local else {
                    return Err(RegistryError::not_found(messages::NO_PACKAGE));
                };
                if !local_modified && !would_change_self(&l.local) {
                    return Ok(Some(l.snapshot()));
                }
                (Arc::clone(&l.local), local_uplinks.unwrap_or_default())
            }
            Synced::Merged(m) => {
                let ups = uplinks_of(&m);
                (Arc::new(m), ups)
            }
        };
        let uplink_name = synced_uplink.unwrap_or_default();
        self.write_back(name, source, source_uplinks, uplink_name).await.map(Some)
    }

    /// `updateVersionsNext` under the storage lock; returns what is served.
    async fn write_back(
        &self,
        name: &str,
        source: Arc<Manifest>,
        source_uplinks: Map<String, Value>,
        uplink_name: String,
    ) -> Result<PackageSnapshot, RegistryError> {
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
                let src = SyncSource {
                    manifest: &source,
                    uplinks: &source_uplinks,
                    uplink_name: &uplink_name,
                    uplink_url: uplink_url.as_deref(),
                };
                let (mut cache, changed) =
                    update_versions_next(cache, &src).map_err(|_| internal_error())?;
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
            let loaded = self.remember(name, stored.manifest, stored.identity, true);
            return Ok(if Arc::ptr_eq(&loaded.local, &loaded.served) {
                loaded.snapshot()
            } else {
                PackageSnapshot {
                    manifest: Arc::clone(&loaded.local),
                    generation: self.next_generation(),
                }
            });
        }
        let current = lock(&self.cache).peek(name).filter(|l| l.identity == stored.identity);
        Ok(match current {
            Some(l) => l.snapshot(),
            None => self.remember(name, stored.manifest, stored.identity, false).snapshot(),
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
        download.finish(Ok(()));
    }
}
