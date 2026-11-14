//! In-memory manifest cache: LRU bounded by the RAM the entries really take.
//!
//! An entry has two parts:
//!
//! - [`Meta`]: what the hot path needs to decide that the local copy can be served as is
//!   (file identity, generation, `_uplinks`, whether a sync would rewrite it) — a few hundred
//!   bytes;
//! - the parsed trees ([`Loaded`]), kept only when they take at most half the capacity. A
//!   package whose tree is not kept is still served from the render cache through its meta;
//!   the tree is read again from disk when a body has to be rendered or the package synced.
//!
//! Costs are [`fastopm_core::memory::value_heap_bytes`] estimates (the tree as allocated), not
//! file sizes: a 16 MB `package.json` is ~120 MB of tree. Replaced and evicted entries are
//! handed back to the caller, so the (possibly large) trees are freed outside the cache lock.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use fastopm_core::Manifest;
use fastopm_core::memory::{MemoryBudget, PassiveHold, map_heap_bytes};
use fastopm_storage::FileIdentity;
use serde_json::{Map, Value};

use crate::PackageSnapshot;

/// Fixed cost of an entry besides its strings and trees (slot, order index, `Arc`s).
const ENTRY_OVERHEAD: u64 = 256;

/// The small facts about a local manifest the hot path decides on.
#[derive(Debug)]
pub(crate) struct Meta {
    /// Identity of the file this was read from (or just written).
    pub identity: FileIdentity,
    /// Generation of the served form.
    pub generation: u64,
    /// `_uplinks` of the local manifest.
    pub uplinks: Map<String, Value>,
    /// `would_change_self(local)`: a sync stopping at a `maxage` hit may still write.
    pub changes_self: bool,
    /// Heap bytes of the trees (`local`, plus `served` when it is a separate copy).
    pub tree_bytes: u64,
}

/// A `package.json` as loaded from disk, in the two forms the store needs.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub meta: Arc<Meta>,
    /// `normalizePackage(file)`: what `getPackageLocalMetadata` returns, the input of an uplink
    /// sync.
    pub local: Arc<Manifest>,
    /// What `getPackage` serves from it (`normalizeDistTags` applied once more). Usually the same
    /// allocation as `local`.
    pub served: Arc<Manifest>,
    /// Keeps the trees counted in the work budget while they are not in the cache.
    pub hold: Mutex<Option<PassiveHold>>,
    /// Whether the cache took the trees when this was made.
    pub cached: bool,
}

impl Loaded {
    pub(crate) fn new(
        meta: Arc<Meta>,
        local: Arc<Manifest>,
        served: Arc<Manifest>,
        hold: Option<PassiveHold>,
        cached: bool,
    ) -> Self {
        Self { meta, local, served, hold: Mutex::new(hold), cached }
    }

    /// Whether the trees are counted in the work budget (they are not in the cache).
    pub(crate) fn holds_memory(&self) -> bool {
        self.hold.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub(crate) fn snapshot(self: &Arc<Self>) -> PackageSnapshot {
        PackageSnapshot {
            manifest: Arc::clone(&self.served),
            generation: self.meta.generation,
            file_len: self.meta.identity.len,
            cached: self.cached,
            _keep: self.holds_memory().then(|| Arc::clone(self)),
        }
    }
}

/// What the cache holds for a package.
#[derive(Debug, Clone)]
pub(crate) struct Resident {
    pub meta: Arc<Meta>,
    /// `None` when the trees were too large to keep.
    pub tree: Option<Arc<Loaded>>,
}

/// A removed entry; drop it outside the cache lock.
#[derive(Debug)]
pub(crate) struct Evicted(Resident);

impl Evicted {
    /// Free the entry. Trees still used elsewhere (a request rendering them) stay counted in
    /// `budget` until their last user is done, so memory that left the cache is never
    /// invisible to the budget.
    pub(crate) fn release(self, budget: &Arc<MemoryBudget>) {
        if let Some(tree) = self.0.tree {
            // ours plus at least one other
            if Arc::strong_count(&tree) > 1 && !tree.holds_memory() {
                let bytes = usize::try_from(tree.meta.tree_bytes).unwrap_or(usize::MAX);
                *tree.hold.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(budget.count_in_use(bytes));
            }
        }
    }
}

struct Slot {
    resident: Resident,
    meta_cost: u64,
    meta_tick: u64,
    /// Cost and LRU tick of the trees, when kept.
    tree: Option<(u64, u64)>,
}

/// LRU keyed by package name, in two parts sharing one budget: the metas (at most a quarter of
/// it, evicted with their whole entry) and the trees (the rest, evicted on their own, leaving
/// the meta). See the module docs.
pub(crate) struct ManifestCache {
    capacity: u64,
    meta_used: u64,
    tree_used: u64,
    next_tick: u64,
    slots: HashMap<String, Slot>,
    meta_order: BTreeMap<u64, String>,
    tree_order: BTreeMap<u64, String>,
}

/// Cost of an entry without its trees.
fn meta_cost(name: &str, meta: &Meta) -> u64 {
    ENTRY_OVERHEAD + 2 * name.len() as u64 + map_heap_bytes(&meta.uplinks) as u64
}

impl ManifestCache {
    pub(crate) fn new(capacity: u64) -> Self {
        Self {
            capacity,
            meta_used: 0,
            tree_used: 0,
            next_tick: 0,
            slots: HashMap::new(),
            meta_order: BTreeMap::new(),
            tree_order: BTreeMap::new(),
        }
    }

    /// Whether the trees of `bytes` are kept (at most half the capacity, so one package cannot
    /// flush everything else).
    pub(crate) fn keeps_tree(&self, bytes: u64) -> bool {
        bytes <= self.capacity / 2
    }

    fn meta_capacity(&self) -> u64 {
        self.capacity / 4
    }

    fn tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }

    /// The entry for `name`, marked as most recently used.
    pub(crate) fn get(&mut self, name: &str) -> Option<Resident> {
        let meta_tick = self.tick();
        let tree_tick = self.tick();
        let slot = self.slots.get_mut(name)?;
        self.meta_order.remove(&slot.meta_tick);
        slot.meta_tick = meta_tick;
        self.meta_order.insert(meta_tick, name.to_owned());
        if let Some((_, tick)) = slot.tree.as_mut() {
            self.tree_order.remove(tick);
            *tick = tree_tick;
            self.tree_order.insert(tree_tick, name.to_owned());
        }
        Some(slot.resident.clone())
    }

    /// The entry for `name` without touching the LRU order.
    pub(crate) fn peek(&self, name: &str) -> Option<Resident> {
        self.slots.get(name).map(|s| s.resident.clone())
    }

    pub(crate) fn remove(&mut self, name: &str) -> Option<Evicted> {
        let slot = self.slots.remove(name)?;
        self.meta_order.remove(&slot.meta_tick);
        self.meta_used -= slot.meta_cost;
        if let Some((cost, tick)) = slot.tree {
            self.tree_order.remove(&tick);
            self.tree_used -= cost;
        }
        Some(Evicted(slot.resident))
    }

    /// Store `loaded` (replacing any entry for `name`), without its trees when they are too
    /// large, then evict: whole entries while the metas take more than a quarter of the
    /// capacity, then trees (least recently used first) until everything fits. Returns whether
    /// the trees were kept, and the removed entries and trees.
    pub(crate) fn insert(&mut self, name: &str, loaded: &Arc<Loaded>) -> (bool, Vec<Evicted>) {
        let mut evicted: Vec<Evicted> = self.remove(name).into_iter().collect();
        let meta = Arc::clone(&loaded.meta);
        let base = meta_cost(name, &meta);
        if self.capacity == 0 || base > self.meta_capacity() {
            return (false, evicted);
        }
        let keep = self.keeps_tree(meta.tree_bytes);
        let meta_tick = self.tick();
        self.meta_order.insert(meta_tick, name.to_owned());
        self.meta_used += base;
        let tree = keep.then(|| {
            let tick = self.tick();
            self.tree_order.insert(tick, name.to_owned());
            self.tree_used += meta.tree_bytes;
            (meta.tree_bytes, tick)
        });
        let resident = Resident { meta, tree: keep.then(|| Arc::clone(loaded)) };
        self.slots.insert(name.to_owned(), Slot { resident, meta_cost: base, meta_tick, tree });
        while self.meta_used > self.meta_capacity() {
            let Some((_, oldest)) = self.meta_order.first_key_value() else { break };
            let oldest = oldest.clone();
            evicted.extend(self.remove(&oldest));
        }
        while self.meta_used + self.tree_used > self.capacity {
            let Some((_, oldest)) = self.tree_order.pop_first() else { break };
            if let Some(slot) = self.slots.get_mut(&oldest)
                && let Some((cost, _)) = slot.tree.take()
            {
                self.tree_used -= cost;
                let tree = slot.resident.tree.take();
                evicted.push(Evicted(Resident { meta: Arc::clone(&slot.resident.meta), tree }));
            }
        }
        let kept = self.slots.get(name).is_some_and(|s| s.tree.is_some());
        (kept, evicted)
    }

    pub(crate) fn used(&self) -> u64 {
        self.meta_used + self.tree_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn loaded(tree_bytes: u64) -> Arc<Loaded> {
        let m = Arc::new(Manifest::template("x"));
        let meta = Meta {
            identity: FileIdentity {
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                len: 1,
            },
            generation: 1,
            uplinks: Map::new(),
            changes_self: false,
            tree_bytes,
        };
        Arc::new(Loaded::new(Arc::new(meta), Arc::clone(&m), m, None, true))
    }

    fn base(name: &str) -> u64 {
        ENTRY_OVERHEAD + 2 * name.len() as u64
    }

    #[test]
    fn cost_is_tree_bytes_not_file_size() {
        let mut c = ManifestCache::new(10_000);
        let (kept, _) = c.insert("a", &loaded(1_000));
        assert!(kept);
        assert_eq!(c.used(), base("a") + 1_000);
        assert!(c.peek("a").unwrap().tree.is_some());
    }

    #[test]
    fn evicts_least_recently_used_trees_keeping_their_meta() {
        // the meta quarter holds four entries, the rest three trees of 1 000 bytes
        let mut c = ManifestCache::new(16 * base("a"));
        c.insert("a", &loaded(1_000));
        c.insert("b", &loaded(1_000));
        c.insert("c", &loaded(1_000));
        assert!(c.get("a").is_some()); // b is now the oldest
        let (_, evicted) = c.insert("d", &loaded(1_000));
        assert_eq!(evicted.len(), 1);
        // b lost its tree, not its meta
        let b = c.peek("b").unwrap();
        assert!(b.tree.is_none());
        assert_eq!(b.meta.tree_bytes, 1_000);
        for name in ["a", "c", "d"] {
            assert!(c.peek(name).unwrap().tree.is_some(), "{name}");
        }
        assert_eq!(c.used(), 4 * base("a") + 3_000);
    }

    #[test]
    fn metas_take_at_most_a_quarter() {
        let mut c = ManifestCache::new(4 * (2 * base("a")));
        for name in ["a", "b", "c"] {
            c.insert(name, &loaded(10));
        }
        // the quarter holds two metas: the least recently used entry goes, tree included
        assert!(c.peek("a").is_none());
        assert!(c.peek("b").is_some() && c.peek("c").is_some());
    }

    #[test]
    fn large_trees_keep_only_the_meta() {
        let mut c = ManifestCache::new(10_000);
        let (kept, _) = c.insert("big", &loaded(6_000));
        assert!(!kept);
        let resident = c.peek("big").unwrap();
        assert!(resident.tree.is_none());
        assert_eq!(resident.meta.tree_bytes, 6_000);
        assert_eq!(c.used(), base("big"));
        // half the capacity still fits
        assert!(c.insert("half", &loaded(5_000)).0);
    }

    #[test]
    fn evicted_trees_in_use_stay_counted() {
        let budget = MemoryBudget::new(1 << 20);
        // room for two metas and one tree of 1 000 bytes (at most half the capacity each)
        let mut c = ManifestCache::new(8 * base("a") + 100);
        let a = loaded(1_000);
        c.insert("a", &a);
        let unused = loaded(1_000);
        let (_, evicted) = c.insert("b", &unused);
        drop(unused);
        // `a` is still used by the test: counted after eviction, until dropped
        for e in evicted {
            e.release(&budget);
        }
        assert_eq!(budget.used(), 1_000);
        assert!(a.holds_memory());
        drop(a);
        assert_eq!(budget.used(), 0);
        // `b`'s tree has no other user: nothing counted
        let (_, evicted) = c.insert("c", &loaded(10));
        for e in evicted {
            e.release(&budget);
        }
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn zero_capacity_keeps_nothing_and_replacing_updates_cost() {
        let mut z = ManifestCache::new(0);
        assert!(!z.insert("a", &loaded(1)).0);
        assert!(z.peek("a").is_none());
        let mut c = ManifestCache::new(10_000);
        c.insert("a", &loaded(300));
        let (_, replaced) = c.insert("a", &loaded(500));
        assert_eq!(replaced.len(), 1);
        assert_eq!(c.used(), base("a") + 500);
        assert!(c.remove("a").is_some());
        assert_eq!(c.used(), 0);
    }
}
