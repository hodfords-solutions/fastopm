//! On-disk storage with Verdaccio's layout.
//!
//! ```text
//! <storage>[/<packages.*.storage>]/<name>/package.json   compact JSON manifest
//! <storage>[/<packages.*.storage>]/<name>/<file>.tgz      tarballs
//! ```
//!
//! `<name>` is the package name as a path (`@scope/pkg` is two directory levels). Writes go to a
//! temporary file in the same directory followed by `rename`, so readers (including a Verdaccio
//! process sharing the folder) never see a partial file. Temporary names follow Verdaccio:
//! `package.json.tmp<digits>` and `<file>.tmp-<digits>`.
//!
//! [`PackageStorage`] is the seam `fastopm-store` depends on; [`FsStorage`] is the real
//! implementation. Tests of the store use an in-memory implementation of the same trait.
//!
//! Verdaccio reference: `packages/plugins/local-storage/src/local-fs.ts` (file operations) and
//! `local-database.ts` (`getPackageStorage`: name → folder).

mod lock;
mod path;

use std::fs::File as StdFile;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use fastopm_config::Config;
use fastopm_core::{Manifest, RegistryError, TarballBody, messages};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_util::io::ReaderStream;

use crate::lock::LockMap;

/// Manifest file name inside a package folder.
const PACKAGE_JSON: &str = "package.json";
/// Read buffer for tarball streams: each read is one blocking-pool hop.
const TARBALL_READ_BUFFER: usize = 256 * 1024;
/// Write buffer for tarball writers.
const TARBALL_WRITE_BUFFER: usize = 256 * 1024;
/// Attempts at finding a free temporary file name.
const TEMP_ATTEMPTS: usize = 16;

/// What identifies one version of `package.json` on disk. When either field changes the file
/// was rewritten (by fastopm or another process such as Verdaccio) and cached copies are stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    /// Modification time.
    pub modified: SystemTime,
    /// Size in bytes.
    pub len: u64,
}

/// A manifest read from disk together with the identity of the file it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredManifest {
    /// Parsed content, keys in file order.
    pub manifest: Manifest,
    /// Identity of the file that was read (or just written).
    pub identity: FileIdentity,
}

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The package name or file name does not map to a path inside the storage root
    /// (`resolveSafePath` failed, or the folder is not exactly the name).
    #[error("invalid storage path for {0}")]
    InvalidPath(String),
    /// The `storage` value of the matching `packages` rule (the payload) is not a path inside
    /// the storage root (`resolveSafePath` failed on it).
    #[error("invalid packages storage path {0}")]
    InvalidRuleStorage(String),
    /// `package.json` exists but is not a JSON object.
    #[error("corrupt package.json for {name}: {source}")]
    Corrupt {
        /// Package name.
        name: String,
        /// Parser error.
        source: serde_json::Error,
    },
    /// Any other file system error.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The update closure of [`PackageStorage::update_manifest`] returned this error; nothing was
    /// written.
    #[error(transparent)]
    Aborted(RegistryError),
}

impl StorageError {
    /// The error a client receives, following Verdaccio:
    ///
    /// - [`StorageError::InvalidPath`] → `500 package-specific storage path is not under the
    ///   configured storage directory or is invalid`;
    /// - [`StorageError::InvalidRuleStorage`] → `500 access-specific storage path is not under
    ///   the configured storage directory or is invalid`;
    /// - [`StorageError::Corrupt`] and [`StorageError::Io`] → `500 internal server error`
    ///   (Verdaccio rethrows the raw error, which the error middleware turns into this);
    /// - [`StorageError::Aborted`] → the wrapped error.
    pub fn to_registry_error(&self) -> RegistryError {
        match self {
            Self::InvalidPath(_) => RegistryError::internal(
                "package-specific storage path is not under the configured storage directory or is invalid",
            ),
            Self::InvalidRuleStorage(_) => RegistryError::internal(
                "access-specific storage path is not under the configured storage directory or is invalid",
            ),
            Self::Corrupt { .. } | Self::Io(_) => {
                RegistryError::internal(messages::INTERNAL_SERVER_ERROR)
            }
            Self::Aborted(err) => err.clone(),
        }
    }
}

/// A tarball being written. Bytes go to a temporary file; [`TarballWrite::commit`] renames it
/// to the final name. Dropping the writer without committing removes the temporary file
/// (best effort), as does [`TarballWrite::abort`].
pub trait TarballWrite: Send + 'static {
    /// Append a chunk.
    fn write(&mut self, chunk: Bytes) -> impl Future<Output = io::Result<()>> + Send;

    /// Flush, close and rename the temporary file to its final name. On a rename error the
    /// temporary file is removed and the error returned (the final file stays absent or
    /// unchanged). An existing final file is replaced.
    fn commit(self) -> impl Future<Output = io::Result<()>> + Send;

    /// Discard the temporary file. Never fails; a missing file is fine.
    fn abort(self) -> impl Future<Output = ()> + Send;
}

/// The storage operations `fastopm-store` needs.
///
/// `name` is always a package name already accepted by [`fastopm_core::validate_package`] and
/// `filename` a name accepted by [`fastopm_core::validate_name`]; implementations still refuse
/// anything that would escape the package folder ([`StorageError::InvalidPath`]).
pub trait PackageStorage: Send + Sync + 'static {
    /// Writer returned by [`PackageStorage::create_tarball`].
    type Writer: TarballWrite;

    /// Read and parse `package.json`. `Ok(None)` when the file (or the package folder) does not
    /// exist. No lock is taken: the file is only ever replaced by `rename`.
    fn read_manifest(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<StoredManifest>, StorageError>> + Send;

    /// `stat` of `package.json` without reading it. `Ok(None)` when absent. Lets the store check
    /// whether its in-memory copy is still current.
    fn manifest_identity(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<FileIdentity>, StorageError>> + Send;

    /// Read-modify-write `package.json` under a per-package lock.
    ///
    /// Steps: take the lock for `name` (one writer per package inside this process; readers are
    /// not blocked) → read the current manifest (`None` if absent) → call `update` → if it
    /// returns `Ok(Some(m))`, write `m` as compact JSON to a temporary file in the package folder
    /// (creating folders as needed) and rename it over `package.json` → release the lock.
    ///
    /// The manifest is written exactly as returned: bumping `_rev`, normalizing and merging are
    /// the caller's job. `update` runs on a blocking-capable thread and may be expensive.
    ///
    /// Returns the manifest on disk after the call: the written one, the unchanged current one
    /// when `update` returned `Ok(None)`, or `None` when there was none and nothing was written.
    /// When `update` returns `Err(e)`, nothing is written and the result is
    /// [`StorageError::Aborted`]`(e)`.
    ///
    /// The lock is in-process only. Verdaccio writes cached manifests without any lock, so there
    /// is nothing to coordinate with across processes.
    fn update_manifest<F>(
        &self,
        name: &str,
        update: F,
    ) -> impl Future<Output = Result<Option<StoredManifest>, StorageError>> + Send
    where
        F: FnOnce(Option<Manifest>) -> Result<Option<Manifest>, RegistryError> + Send + 'static;

    /// Open a tarball for reading. `Ok(None)` when the file does not exist. The body carries the
    /// file size as `content_length`, taken from the open file descriptor.
    fn open_tarball(
        &self,
        name: &str,
        filename: &str,
    ) -> impl Future<Output = Result<Option<TarballBody>, StorageError>> + Send;

    /// Start writing a tarball: create the package folder if needed and open a temporary file
    /// next to the final one.
    fn create_tarball(
        &self,
        name: &str,
        filename: &str,
    ) -> impl Future<Output = Result<Self::Writer, StorageError>> + Send;
}

/// Folder of a package (`getPackageStorage`).
///
/// `root` is the storage root, `rule_storage` the `storage` value of the first `packages` rule
/// matching the package (if any). The result is `root/rule_storage/name`. Both joins go through
/// `resolveSafePath` semantics: the part is percent-decoded, must not be absolute and must stay
/// inside the base after normalization; additionally the folder relative to its base must be
/// exactly `name` (no normalization of `.` / `..`). Otherwise [`StorageError::InvalidPath`]
/// ([`StorageError::InvalidRuleStorage`] when `rule_storage` itself is rejected).
///
/// Paths are resolved lexically like Node's `path.resolve`: symbolic links are not followed.
pub fn package_dir(
    root: &Path,
    rule_storage: Option<&str>,
    name: &str,
) -> Result<PathBuf, StorageError> {
    let invalid = || StorageError::InvalidPath(name.to_owned());
    let root = path::absolute(root).ok_or_else(invalid)?;
    let base = match rule_storage {
        Some(storage) => path::resolve_safe_path(&root, storage)
            .ok_or_else(|| StorageError::InvalidRuleStorage(storage.to_owned()))?,
        None => root,
    };
    let dir = path::resolve_safe_path(&base, name).ok_or_else(invalid)?;
    if path::relative_slash(&base, &dir).as_deref() != Some(name) {
        return Err(invalid());
    }
    Ok(dir)
}

/// Path of `filename` inside a package folder: `path.join(dir, sanitize(filename))`
/// (`local-fs.ts:_getStorage`). A name that sanitizes to nothing would be the folder itself and
/// is refused.
fn file_in(dir: &Path, name: &str, filename: &str) -> Result<PathBuf, StorageError> {
    let sanitized = path::sanitize_filename(filename);
    if sanitized.is_empty() {
        return Err(StorageError::InvalidPath(format!("{name}/{filename}")));
    }
    Ok(dir.join(sanitized))
}

/// Decimal digits for temporary names, like Verdaccio's `String(Math.random()).slice(2)`.
fn random_digits() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let value = getrandom::u64().unwrap_or_else(|_| {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        nanos ^ (u64::from(std::process::id()) << 32) ^ COUNTER.fetch_add(1, Ordering::Relaxed)
    });
    value.to_string()
}

/// Create a new temporary file with a name from `make_path`, retrying on a name collision.
fn create_temp(make_path: impl Fn(&str) -> PathBuf) -> io::Result<(PathBuf, StdFile)> {
    let mut last_err = None;
    for _ in 0..TEMP_ATTEMPTS {
        let temp = make_path(&random_digits());
        match StdFile::options().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => last_err = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("no free temporary file name")))
}

fn identity_of(meta: &std::fs::Metadata) -> io::Result<FileIdentity> {
    Ok(FileIdentity { modified: meta.modified()?, len: meta.len() })
}

/// Read and parse `dir/package.json`; identity and content come from the same descriptor.
fn read_manifest_sync(dir: &Path, name: &str) -> Result<Option<StoredManifest>, StorageError> {
    let mut file = match StdFile::open(dir.join(PACKAGE_JSON)) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let meta = file.metadata()?;
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0).saturating_add(1));
    file.read_to_end(&mut bytes)?;
    let manifest = Manifest::from_slice(&bytes)
        .map_err(|source| StorageError::Corrupt { name: name.to_owned(), source })?;
    Ok(Some(StoredManifest { manifest, identity: identity_of(&meta)? }))
}

/// Write `manifest` as compact JSON to `dir/package.json.tmp<digits>` and rename it over
/// `dir/package.json` (`local-fs.ts:writeTempFileAndRename`). Returns the identity of the
/// written file (rename keeps size and modification time).
fn write_manifest_sync(dir: &Path, manifest: &Manifest) -> io::Result<FileIdentity> {
    let bytes = manifest.to_compact_vec();
    std::fs::create_dir_all(dir)?;
    let target = dir.join(PACKAGE_JSON);
    let (temp, mut file) = create_temp(|digits| dir.join(format!("{PACKAGE_JSON}.tmp{digits}")))?;
    let written = file.write_all(&bytes).and_then(|()| file.metadata());
    drop(file);
    let result = written.and_then(|meta| {
        std::fs::rename(&temp, &target)?;
        identity_of(&meta)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Run blocking work on the blocking pool; a panic in it resumes on the caller.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
        Err(err) => Err(StorageError::Io(io::Error::other(err))),
    }
}

async fn read_manifest_in(
    dir: PathBuf,
    name: &str,
) -> Result<Option<StoredManifest>, StorageError> {
    let name = name.to_owned();
    blocking(move || read_manifest_sync(&dir, &name)).await
}

async fn manifest_identity_in(dir: &Path) -> Result<Option<FileIdentity>, StorageError> {
    match tokio::fs::metadata(dir.join(PACKAGE_JSON)).await {
        Ok(meta) => Ok(Some(identity_of(&meta)?)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn update_manifest_in<F>(
    locks: &Arc<LockMap>,
    dir: PathBuf,
    name: &str,
    update: F,
) -> Result<Option<StoredManifest>, StorageError>
where
    F: FnOnce(Option<Manifest>) -> Result<Option<Manifest>, RegistryError> + Send + 'static,
{
    let lock = locks.lock(dir.clone()).await;
    let name = name.to_owned();
    // The lock moves into the blocking task: if the caller stops waiting, it is still held
    // until the write is finished.
    blocking(move || {
        let _lock = lock;
        let current = read_manifest_sync(&dir, &name)?;
        match update(current.as_ref().map(|stored| stored.manifest.clone())) {
            Ok(Some(manifest)) => {
                let identity = write_manifest_sync(&dir, &manifest)?;
                Ok(Some(StoredManifest { manifest, identity }))
            }
            Ok(None) => Ok(current),
            Err(err) => Err(StorageError::Aborted(err)),
        }
    })
    .await
}

async fn open_tarball_in(
    dir: &Path,
    name: &str,
    filename: &str,
) -> Result<Option<TarballBody>, StorageError> {
    let path = file_in(dir, name, filename)?;
    let opened = blocking(move || match StdFile::open(&path) {
        Ok(file) => {
            let meta = file.metadata()?;
            Ok(meta.is_file().then_some((file, meta.len())))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    })
    .await?;
    Ok(opened.map(|(file, len)| {
        let mut file = tokio::fs::File::from_std(file);
        file.set_max_buf_size(TARBALL_READ_BUFFER);
        let stream = ReaderStream::with_capacity(file, TARBALL_READ_BUFFER);
        TarballBody::new(Box::pin(stream), Some(len))
    }))
}

async fn create_tarball_in(
    dir: PathBuf,
    name: &str,
    filename: &str,
) -> Result<FsTarballWriter, StorageError> {
    let final_path = file_in(&dir, name, filename)?;
    let filename = filename.to_owned();
    let (temp_path, file) = blocking(move || {
        std::fs::create_dir_all(&dir)?;
        let temp = create_temp(|digits| {
            dir.join(path::sanitize_filename(&format!("{filename}.tmp-{digits}")))
        })?;
        Ok(temp)
    })
    .await?;
    let file = BufWriter::with_capacity(TARBALL_WRITE_BUFFER, tokio::fs::File::from_std(file));
    Ok(FsTarballWriter { temp_path, final_path, file: Some(file), finished: false })
}

/// [`PackageStorage`] on the local file system.
#[derive(Debug, Clone)]
pub struct FsStorage {
    config: Arc<Config>,
    locks: Arc<LockMap>,
}

impl FsStorage {
    /// Storage rooted at [`Config::storage`]; `packages.*.storage` is looked up per package
    /// with [`Config::package_rule`].
    pub fn new(config: Arc<Config>) -> Self {
        Self { config, locks: Arc::default() }
    }

    /// Folder of `name` under this storage (see [`package_dir`]).
    pub fn package_dir(&self, name: &str) -> Result<PathBuf, StorageError> {
        let rule_storage = self.config.package_rule(name).and_then(|rule| rule.storage.as_deref());
        package_dir(&self.config.storage, rule_storage, name)
    }
}

/// [`TarballWrite`] for [`FsStorage`].
pub struct FsTarballWriter {
    temp_path: PathBuf,
    final_path: PathBuf,
    file: Option<BufWriter<tokio::fs::File>>,
    finished: bool,
}

impl std::fmt::Debug for FsTarballWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsTarballWriter")
            .field("temp_path", &self.temp_path)
            .field("final_path", &self.final_path)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl FsTarballWriter {
    fn closed() -> io::Error {
        io::Error::other("tarball writer already closed")
    }
}

impl TarballWrite for FsTarballWriter {
    async fn write(&mut self, chunk: Bytes) -> io::Result<()> {
        self.file.as_mut().ok_or_else(Self::closed)?.write_all(&chunk).await
    }

    async fn commit(mut self) -> io::Result<()> {
        let mut file = self.file.take().ok_or_else(Self::closed)?;
        // `flush` also waits for the write the tokio file still has in flight.
        let flushed = file.flush().await;
        drop(file);
        let result = match flushed {
            Ok(()) => tokio::fs::rename(&self.temp_path, &self.final_path).await,
            Err(err) => Err(err),
        };
        if result.is_err() {
            let _ = tokio::fs::remove_file(&self.temp_path).await;
        }
        self.finished = true;
        result
    }

    async fn abort(mut self) {
        drop(self.file.take());
        let _ = tokio::fs::remove_file(&self.temp_path).await;
        self.finished = true;
    }
}

impl Drop for FsTarballWriter {
    fn drop(&mut self) {
        if !self.finished {
            drop(self.file.take());
            let _ = std::fs::remove_file(&self.temp_path);
        }
    }
}

impl PackageStorage for FsStorage {
    type Writer = FsTarballWriter;

    async fn read_manifest(&self, name: &str) -> Result<Option<StoredManifest>, StorageError> {
        read_manifest_in(self.package_dir(name)?, name).await
    }

    async fn manifest_identity(&self, name: &str) -> Result<Option<FileIdentity>, StorageError> {
        manifest_identity_in(&self.package_dir(name)?).await
    }

    async fn update_manifest<F>(
        &self,
        name: &str,
        update: F,
    ) -> Result<Option<StoredManifest>, StorageError>
    where
        F: FnOnce(Option<Manifest>) -> Result<Option<Manifest>, RegistryError> + Send + 'static,
    {
        update_manifest_in(&self.locks, self.package_dir(name)?, name, update).await
    }

    async fn open_tarball(
        &self,
        name: &str,
        filename: &str,
    ) -> Result<Option<TarballBody>, StorageError> {
        open_tarball_in(&self.package_dir(name)?, name, filename).await
    }

    async fn create_tarball(
        &self,
        name: &str,
        filename: &str,
    ) -> Result<FsTarballWriter, StorageError> {
        create_tarball_in(self.package_dir(name)?, name, filename).await
    }
}

#[cfg(test)]
mod tests;
