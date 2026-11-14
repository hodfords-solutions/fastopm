//! In-process per-package write locks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// One async mutex per package folder, created on demand and removed when nobody holds or
/// waits for it, so the map only contains packages being written right now.
#[derive(Debug, Default)]
pub(crate) struct LockMap {
    locks: Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>,
}

impl LockMap {
    fn map(&self) -> MutexGuard<'_, HashMap<PathBuf, Arc<AsyncMutex<()>>>> {
        self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wait for the lock of `key`. Cancel-safe: dropping the future gives up the place in line.
    pub(crate) async fn lock(self: &Arc<Self>, key: PathBuf) -> PackageLock {
        let mutex = Arc::clone(self.map().entry(key.clone()).or_default());
        let guard = mutex.lock_owned().await;
        PackageLock { map: Arc::clone(self), key, guard: Some(guard) }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map().len()
    }
}

/// A held package lock; released on drop.
#[derive(Debug)]
pub(crate) struct PackageLock {
    map: Arc<LockMap>,
    key: PathBuf,
    guard: Option<OwnedMutexGuard<()>>,
}

impl Drop for PackageLock {
    fn drop(&mut self) {
        // Holding the map lock while releasing: a new locker clones the Arc under the same map
        // lock, so a count of 1 (the map's own reference) means nobody holds or waits for it.
        let mut map = self.map.map();
        drop(self.guard.take());
        if map.get(&self.key).is_some_and(|m| Arc::strong_count(m) == 1) {
            map.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn serializes_holders_and_cleans_up() {
        let locks = Arc::new(LockMap::default());
        let first = locks.lock(PathBuf::from("/a")).await;
        let other = locks.lock(PathBuf::from("/b")).await;
        assert_eq!(locks.len(), 2);

        let waiter = {
            let locks = Arc::clone(&locks);
            tokio::spawn(async move { locks.lock(PathBuf::from("/a")).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "second locker must wait");

        drop(first);
        let second = waiter.await.unwrap();
        assert_eq!(locks.len(), 2, "entry kept while held");
        drop(second);
        drop(other);
        assert_eq!(locks.len(), 0);
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_leak() {
        let locks = Arc::new(LockMap::default());
        let held = locks.lock(PathBuf::from("/a")).await;
        let attempt =
            tokio::time::timeout(Duration::from_millis(10), locks.lock(PathBuf::from("/a"))).await;
        assert!(attempt.is_err());
        drop(held);
        assert_eq!(locks.len(), 0);
    }
}
