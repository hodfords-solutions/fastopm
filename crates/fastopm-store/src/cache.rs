//! In-memory manifest cache: LRU bounded by the size of the manifest files it holds.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use fastopm_core::Manifest;
use fastopm_storage::FileIdentity;

use crate::PackageSnapshot;

/// A `package.json` as loaded from disk, in the two forms the store needs.
#[derive(Debug)]
pub(crate) struct Loaded {
    /// Identity of the file this was read from (or just written).
    pub identity: FileIdentity,
    /// `normalizePackage(file)`: what `getPackageLocalMetadata` returns, the input of an uplink
    /// sync.
    pub local: Arc<Manifest>,
    /// What `getPackage` serves from it (`normalizeDistTags` applied once more). Usually the same
    /// allocation as `local`.
    pub served: Arc<Manifest>,
    /// Generation of `served`.
    pub generation: u64,
}

impl Loaded {
    pub(crate) fn snapshot(&self) -> PackageSnapshot {
        PackageSnapshot { manifest: Arc::clone(&self.served), generation: self.generation }
    }
}

struct Slot {
    loaded: Arc<Loaded>,
    size: u64,
    tick: u64,
}

/// LRU keyed by package name. Entries larger than the capacity are not kept.
pub(crate) struct ManifestCache {
    capacity: u64,
    used: u64,
    next_tick: u64,
    slots: HashMap<String, Slot>,
    order: BTreeMap<u64, String>,
}

impl ManifestCache {
    pub(crate) fn new(capacity: u64) -> Self {
        Self { capacity, used: 0, next_tick: 0, slots: HashMap::new(), order: BTreeMap::new() }
    }

    fn tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }

    /// The entry for `name`, marked as most recently used.
    pub(crate) fn get(&mut self, name: &str) -> Option<Arc<Loaded>> {
        let tick = self.tick();
        let slot = self.slots.get_mut(name)?;
        self.order.remove(&slot.tick);
        slot.tick = tick;
        self.order.insert(tick, name.to_owned());
        Some(Arc::clone(&slot.loaded))
    }

    /// The entry for `name` without touching the LRU order.
    pub(crate) fn peek(&self, name: &str) -> Option<Arc<Loaded>> {
        self.slots.get(name).map(|s| Arc::clone(&s.loaded))
    }

    pub(crate) fn remove(&mut self, name: &str) {
        if let Some(slot) = self.slots.remove(name) {
            self.order.remove(&slot.tick);
            self.used -= slot.size;
        }
    }

    /// Store `loaded` (replacing any entry for `name`), then evict least recently used entries
    /// until the total size fits. `size` is the manifest file size in bytes.
    pub(crate) fn insert(&mut self, name: &str, loaded: Arc<Loaded>, size: u64) {
        self.remove(name);
        if size > self.capacity {
            return;
        }
        let tick = self.tick();
        self.slots.insert(name.to_owned(), Slot { loaded, size, tick });
        self.order.insert(tick, name.to_owned());
        self.used += size;
        while self.used > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            if let Some(slot) = self.slots.remove(&oldest) {
                self.used -= slot.size;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn used(&self) -> u64 {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn loaded(len: u64) -> Arc<Loaded> {
        let m = Arc::new(Manifest::template("x"));
        Arc::new(Loaded {
            identity: FileIdentity {
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                len,
            },
            local: Arc::clone(&m),
            served: m,
            generation: 1,
        })
    }

    #[test]
    fn evicts_least_recently_used_by_size() {
        let mut c = ManifestCache::new(100);
        c.insert("a", loaded(40), 40);
        c.insert("b", loaded(40), 40);
        assert!(c.get("a").is_some()); // b is now the oldest
        c.insert("c", loaded(40), 40);
        assert!(c.peek("b").is_none());
        assert!(c.peek("a").is_some() && c.peek("c").is_some());
        assert_eq!(c.used(), 80);
    }

    #[test]
    fn oversized_and_zero_capacity_are_not_kept() {
        let mut c = ManifestCache::new(10);
        c.insert("a", loaded(5), 5);
        c.insert("a", loaded(11), 11);
        assert!(c.peek("a").is_none());
        assert_eq!(c.used(), 0);
        let mut z = ManifestCache::new(0);
        z.insert("a", loaded(1), 1);
        assert!(z.peek("a").is_none());
    }

    #[test]
    fn replacing_updates_size() {
        let mut c = ManifestCache::new(100);
        c.insert("a", loaded(30), 30);
        c.insert("a", loaded(50), 50);
        assert_eq!(c.used(), 50);
        c.remove("a");
        assert_eq!(c.used(), 0);
    }
}
