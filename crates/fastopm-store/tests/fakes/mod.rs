//! In-memory implementations of [`PackageStorage`] and [`Uplink`] for store tests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fastopm_config::{Config, LogConfig, PackageRule, UplinkConfig};
use fastopm_core::{Manifest, RegistryError, TarballBody};
use fastopm_storage::{FileIdentity, PackageStorage, StorageError, StoredManifest, TarballWrite};
use fastopm_store::{Store, StoreOptions};
use fastopm_uplink::{ManifestFetch, Uplink, UplinkBuildError, UplinkError};
use futures_util::StreamExt;
use indexmap::IndexMap;
use serde_json::Value;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

pub fn manifest(v: Value) -> Manifest {
    Manifest::from_map(v.as_object().expect("object").clone())
}

// ---------------------------------------------------------------- storage

#[derive(Default)]
pub struct StorageState {
    pub manifests: Mutex<HashMap<String, (Vec<u8>, FileIdentity)>>,
    pub tarballs: Mutex<HashMap<(String, String), Vec<u8>>>,
    pub manifest_writes: AtomicUsize,
    pub manifest_reads: AtomicUsize,
    pub creates: AtomicUsize,
    pub commits: AtomicUsize,
    pub aborts: AtomicUsize,
    clock: AtomicU64,
    lock: tokio::sync::Mutex<()>,
}

#[derive(Clone, Default)]
pub struct FakeStorage {
    pub state: Arc<StorageState>,
}

impl FakeStorage {
    fn identity(&self, len: usize) -> FileIdentity {
        let tick = self.state.clock.fetch_add(1, Ordering::SeqCst) + 1;
        FileIdentity { modified: UNIX_EPOCH + Duration::from_nanos(tick), len: len as u64 }
    }

    /// Write `package.json` as another process would (not counted as a store write).
    pub fn put_raw(&self, name: &str, bytes: Vec<u8>) {
        let id = self.identity(bytes.len());
        self.state.manifests.lock().unwrap().insert(name.to_owned(), (bytes, id));
    }

    pub fn put(&self, name: &str, m: &Manifest) {
        self.put_raw(name, m.to_compact_vec());
    }

    pub fn get(&self, name: &str) -> Option<Manifest> {
        let guard = self.state.manifests.lock().unwrap();
        guard.get(name).map(|(b, _)| Manifest::from_slice(b).unwrap())
    }

    pub fn put_tarball(&self, name: &str, file: &str, bytes: &[u8]) {
        self.state.tarballs.lock().unwrap().insert((name.into(), file.into()), bytes.to_vec());
    }

    pub fn tarball(&self, name: &str, file: &str) -> Option<Vec<u8>> {
        self.state.tarballs.lock().unwrap().get(&(name.into(), file.into())).cloned()
    }

    pub fn writes(&self) -> usize {
        self.state.manifest_writes.load(Ordering::SeqCst)
    }

    fn parse(name: &str, bytes: &[u8]) -> Result<Manifest, StorageError> {
        Manifest::from_slice(bytes)
            .map_err(|source| StorageError::Corrupt { name: name.into(), source })
    }
}

pub struct FakeWriter {
    state: Arc<StorageState>,
    key: (String, String),
    buf: Vec<u8>,
}

impl TarballWrite for FakeWriter {
    async fn write(&mut self, chunk: Bytes) -> io::Result<()> {
        self.buf.extend_from_slice(&chunk);
        Ok(())
    }

    async fn commit(self) -> io::Result<()> {
        self.state.tarballs.lock().unwrap().insert(self.key, self.buf);
        self.state.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn abort(self) {
        self.state.aborts.fetch_add(1, Ordering::SeqCst);
    }
}

impl PackageStorage for FakeStorage {
    type Writer = FakeWriter;

    async fn read_manifest(&self, name: &str) -> Result<Option<StoredManifest>, StorageError> {
        self.state.manifest_reads.fetch_add(1, Ordering::SeqCst);
        let entry = self.state.manifests.lock().unwrap().get(name).cloned();
        match entry {
            None => Ok(None),
            Some((bytes, identity)) => {
                Ok(Some(StoredManifest { manifest: Self::parse(name, &bytes)?, identity }))
            }
        }
    }

    async fn manifest_identity(&self, name: &str) -> Result<Option<FileIdentity>, StorageError> {
        Ok(self.state.manifests.lock().unwrap().get(name).map(|(_, id)| *id))
    }

    async fn update_manifest<F>(
        &self,
        name: &str,
        update: F,
    ) -> Result<Option<StoredManifest>, StorageError>
    where
        F: FnOnce(Option<Manifest>) -> Result<Option<Manifest>, RegistryError> + Send + 'static,
    {
        let _guard = self.state.lock.lock().await;
        let current = self.state.manifests.lock().unwrap().get(name).cloned();
        let current = match current {
            None => None,
            Some((bytes, identity)) => {
                Some(StoredManifest { manifest: Self::parse(name, &bytes)?, identity })
            }
        };
        match update(current.as_ref().map(|c| c.manifest.clone())).map_err(StorageError::Aborted)? {
            None => Ok(current),
            Some(m) => {
                let bytes = m.to_compact_vec();
                let identity = self.identity(bytes.len());
                self.state.manifests.lock().unwrap().insert(name.to_owned(), (bytes, identity));
                self.state.manifest_writes.fetch_add(1, Ordering::SeqCst);
                Ok(Some(StoredManifest { manifest: m, identity }))
            }
        }
    }

    async fn open_tarball(
        &self,
        name: &str,
        filename: &str,
    ) -> Result<Option<TarballBody>, StorageError> {
        Ok(self.tarball(name, filename).map(|bytes| {
            let len = bytes.len() as u64;
            let stream = futures_util::stream::iter([Ok(Bytes::from(bytes))]);
            TarballBody::new(Box::pin(stream), Some(len))
        }))
    }

    async fn create_tarball(&self, name: &str, filename: &str) -> Result<FakeWriter, StorageError> {
        self.state.creates.fetch_add(1, Ordering::SeqCst);
        Ok(FakeWriter {
            state: Arc::clone(&self.state),
            key: (name.into(), filename.into()),
            buf: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------- uplinks

#[derive(Clone)]
pub enum ManifestAnswer {
    Ok(Manifest, Option<String>),
    NotModified,
    Err(UplinkError),
}

#[derive(Clone)]
pub struct TarballAnswer {
    pub chunks: Vec<&'static [u8]>,
    pub content_length: Option<u64>,
    pub status: Option<UplinkError>,
    pub chunk_delay: Duration,
}

impl TarballAnswer {
    pub fn ok(chunks: Vec<&'static [u8]>) -> Self {
        let len = chunks.iter().map(|c| c.len() as u64).sum();
        Self { chunks, content_length: Some(len), status: None, chunk_delay: Duration::ZERO }
    }
}

#[derive(Default)]
pub struct UplinkState {
    pub manifests: Mutex<HashMap<String, ManifestAnswer>>,
    pub manifest_calls: AtomicUsize,
    pub etags_sent: Mutex<Vec<Option<String>>>,
    pub manifest_delay: Mutex<Duration>,
    pub tarballs: Mutex<HashMap<String, TarballAnswer>>,
    pub tarball_calls: AtomicUsize,
}

pub struct FakeUplink {
    pub name: String,
    pub url: String,
    pub maxage: Duration,
    pub cache: bool,
    pub state: Arc<UplinkState>,
}

impl FakeUplink {
    pub fn new(name: &str, url: &str) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            maxage: Duration::from_secs(120),
            cache: true,
            state: Arc::default(),
        }
    }
}

impl UplinkState {
    pub fn answer(&self, package: &str, answer: ManifestAnswer) {
        self.manifests.lock().unwrap().insert(package.into(), answer);
    }

    pub fn tarball(&self, url: &str, answer: TarballAnswer) {
        self.tarballs.lock().unwrap().insert(url.into(), answer);
    }

    pub fn delay(&self, d: Duration) {
        *self.manifest_delay.lock().unwrap() = d;
    }

    pub fn manifest_calls(&self) -> usize {
        self.manifest_calls.load(Ordering::SeqCst)
    }

    pub fn tarball_calls(&self) -> usize {
        self.tarball_calls.load(Ordering::SeqCst)
    }

    pub fn etags(&self) -> Vec<Option<String>> {
        self.etags_sent.lock().unwrap().clone()
    }
}

/// Tarballs served by throw-away uplinks ([`Uplink::for_foreign_url`]), keyed by URL.
pub fn foreign_tarballs() -> &'static Mutex<HashMap<String, (TarballAnswer, usize)>> {
    static FOREIGN: OnceLock<Mutex<HashMap<String, (TarballAnswer, usize)>>> = OnceLock::new();
    FOREIGN.get_or_init(Mutex::default)
}

fn serve_tarball(answer: Option<TarballAnswer>) -> Result<TarballBody, UplinkError> {
    let answer = answer.ok_or(UplinkError::NotFound)?;
    if let Some(e) = answer.status {
        return Err(e);
    }
    let total: u64 = answer.chunks.iter().map(|c| c.len() as u64).sum();
    let mismatch = answer.content_length.is_some_and(|l| l != total);
    let delay = answer.chunk_delay;
    let chunks = futures_util::stream::iter(answer.chunks).then(move |c| async move {
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(Bytes::from_static(c))
    });
    let tail = futures_util::stream::iter(
        mismatch.then(|| Err(io::Error::other("content length mismatch"))),
    );
    Ok(TarballBody::new(Box::pin(chunks.chain(tail)), answer.content_length))
}

impl Uplink for FakeUplink {
    fn name(&self) -> &str {
        &self.name
    }

    fn maxage(&self) -> Duration {
        self.maxage
    }

    fn cache_tarballs(&self) -> bool {
        self.cache
    }

    fn serves_url(&self, tarball_url: &str) -> bool {
        let base =
            if self.url.ends_with('/') { self.url.clone() } else { format!("{}/", self.url) };
        tarball_url.starts_with(&base)
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn fetch_manifest(
        &self,
        name: &str,
        etag: Option<&str>,
        _remote_address: Option<&str>,
    ) -> Result<ManifestFetch, UplinkError> {
        self.state.manifest_calls.fetch_add(1, Ordering::SeqCst);
        self.state.etags_sent.lock().unwrap().push(etag.map(str::to_owned));
        let delay = *self.state.manifest_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let answer = self.state.manifests.lock().unwrap().get(name).cloned();
        match answer.unwrap_or(ManifestAnswer::Err(UplinkError::NotFound)) {
            ManifestAnswer::Ok(manifest, etag) => Ok(ManifestFetch::Fetched { manifest, etag }),
            ManifestAnswer::NotModified => Ok(ManifestFetch::NotModified),
            ManifestAnswer::Err(e) => Err(e),
        }
    }

    async fn fetch_tarball(&self, tarball_url: &str) -> Result<TarballBody, UplinkError> {
        self.state.tarball_calls.fetch_add(1, Ordering::SeqCst);
        if self.name.starts_with("verdaccio-") {
            let mut foreign = foreign_tarballs().lock().unwrap();
            let entry = foreign.get_mut(tarball_url);
            let answer = entry.map(|(a, calls)| {
                *calls += 1;
                a.clone()
            });
            return serve_tarball(answer);
        }
        let answer = self.state.tarballs.lock().unwrap().get(tarball_url).cloned();
        serve_tarball(answer)
    }

    fn for_foreign_url(
        package: &str,
        tarball_url: &str,
        _config: &Config,
    ) -> Result<Self, UplinkBuildError> {
        if !tarball_url.contains("://") {
            return Err(UplinkBuildError::InvalidUrl {
                uplink: package.into(),
                url: tarball_url.into(),
            });
        }
        Ok(Self {
            name: format!("verdaccio-{package}"),
            url: tarball_url.into(),
            maxage: Duration::from_secs(120),
            cache: true,
            state: Arc::default(),
        })
    }
}

// ---------------------------------------------------------------- config

pub fn uplink_config(name: &str, url: &str) -> UplinkConfig {
    UplinkConfig {
        name: name.into(),
        url: url.into(),
        timeout: Duration::from_secs(30),
        maxage: Duration::from_secs(120),
        max_fails: 2,
        fail_timeout: Duration::from_secs(300),
        retry: 2,
        cache: true,
        headers: IndexMap::new(),
        auth: None,
        http_proxy: None,
        https_proxy: None,
        no_proxy: None,
    }
}

pub fn rule(pattern: &str, proxy: &[&str]) -> PackageRule {
    PackageRule {
        pattern: pattern.into(),
        proxy: proxy.iter().map(|s| (*s).to_owned()).collect(),
        storage: None,
    }
}

pub fn config(uplinks: &[(&str, &str)], packages: Vec<PackageRule>) -> Config {
    Config {
        config_path: PathBuf::from("/tmp/fastopm-store-test/config.yaml"),
        storage: PathBuf::from("/tmp/fastopm-store-test/storage"),
        listen: Vec::new(),
        url_prefix: None,
        uplinks: uplinks.iter().map(|(n, u)| ((*n).to_owned(), uplink_config(n, u))).collect(),
        packages,
        log: LogConfig::default(),
        http_proxy: None,
        https_proxy: None,
        no_proxy: Vec::new(),
        user_agent: "hidden".into(),
        server_id: "test".into(),
        public_url: None,
        forwarded_proto_header: "x-forwarded-proto".into(),
        warnings: Vec::new(),
    }
}

pub type TestStore = Store<FakeStorage, FakeUplink>;

/// A store whose packages proxy to `proxy` (all uplinks share URL patterns from `uplinks`).
pub fn store(uplinks: Vec<FakeUplink>, proxy: &[&str]) -> TestStore {
    store_with(uplinks, vec![rule("**", proxy)], StoreOptions::default())
}

pub fn store_with(
    uplinks: Vec<FakeUplink>,
    packages: Vec<PackageRule>,
    options: StoreOptions,
) -> TestStore {
    let pairs: Vec<(String, String)> =
        uplinks.iter().map(|u| (u.name.clone(), u.url.clone())).collect();
    let refs: Vec<(&str, &str)> = pairs.iter().map(|(n, u)| (n.as_str(), u.as_str())).collect();
    let cfg = Arc::new(config(&refs, packages));
    let map: IndexMap<String, FakeUplink> =
        uplinks.into_iter().map(|u| (u.name.clone(), u)).collect();
    Store::new(cfg, FakeStorage::default(), map, options)
}

pub async fn collect(body: TarballBody) -> Result<Vec<u8>, io::Error> {
    let mut out = Vec::new();
    let mut stream = body.stream;
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk?);
    }
    Ok(out)
}
