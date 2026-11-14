//! Memory accounting: how much RAM a parsed manifest takes, and a byte budget that bounds the
//! transient memory of concurrent heavy work.
//!
//! # Sizes
//!
//! A parsed manifest is a `serde_json::Value` tree with `preserve_order` (`IndexMap`): every
//! string, key, object and array is a separate heap block, so the tree takes from ~1.7× (long
//! readmes) to ~9× (many small strings) the size of the JSON text. [`value_heap_bytes`] walks a
//! tree and adds up its heap blocks the way glibc malloc sizes them (16-byte rounding, 8-byte
//! header, 32-byte minimum chunk); it matches allocator counts within ~1% on real packuments
//! (`typescript`: 15.2 MB of JSON → 122 MB). [`predict_heap_bytes`] guesses the same number from
//! the JSON text before parsing (one pass counting `"`), slightly above the real value.
//!
//! # Budget
//!
//! [`MemoryBudget`] is an async byte semaphore. Work that needs memory takes a [`Reservation`]
//! ([`MemoryBudget::reserve`]), grows it as the need becomes known ([`Reservation::grow_to`],
//! e.g. while a response body streams in), shrinks it when memory is freed, and releases it on
//! drop. Rules, chosen so the total stays bounded and nothing deadlocks:
//!
//! - a new reservation starts when it fits (`used + n <= capacity`), in FIFO order, and only
//!   when no running reservation is waiting to grow; when nothing else is running it starts
//!   whatever its size (one item larger than the whole budget still runs, alone);
//! - growing waits until the extra bytes fit, except for the *oldest* running reservation,
//!   which never waits (so the oldest always finishes and frees its memory). The total can
//!   exceed the capacity only by what that oldest reservation took beyond it.
//!
//! A task must not wait for a second reservation while it holds one (that could wait forever
//! on itself); grow the one it holds instead. Memory that outlives the work that produced it
//! (a parsed manifest too large for the cache, still used by requests) stays counted as a
//! [`PassiveHold`]: it counts in `used`, but it is not running work, so it neither blocks a
//! reservation from starting alone nor ever grows. Holding one while waiting is allowed.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use tokio::sync::Notify;

/// Size of a `serde_json::Value` (with `preserve_order`), the unit of arrays and map entries.
const VALUE_SIZE: usize = std::mem::size_of::<Value>();
/// One `IndexMap` entry: hash, `String` key, value.
const ENTRY_SIZE: usize = 8 + 24 + VALUE_SIZE;

/// Heap bytes glibc malloc uses for a request of `n` bytes (0 for no allocation).
pub fn malloc_chunk(n: usize) -> usize {
    if n == 0 { 0 } else { ((n + 8 + 15) & !15).max(32) }
}

/// `(buckets, entries capacity)` of an `IndexMap` built by `len` inserts from empty.
fn map_layout(len: usize) -> (usize, usize) {
    if len == 0 {
        return (0, 0);
    }
    let (mut buckets, mut cap) = (4usize, 3usize);
    while cap < len {
        buckets *= 2;
        cap = buckets / 8 * 7;
    }
    (buckets, cap)
}

/// Heap bytes of a JSON tree as `serde_json` builds it when parsing (strings and keys with
/// exact capacity, arrays and maps grown by pushes), glibc malloc sizes. Trees built otherwise
/// (clones, merges) are within a few percent.
pub fn value_heap_bytes(value: &Value) -> usize {
    match value {
        Value::String(s) => malloc_chunk(s.len()),
        Value::Array(items) => {
            let cap = if items.is_empty() { 0 } else { items.len().next_power_of_two().max(4) };
            malloc_chunk(cap * VALUE_SIZE) + items.iter().map(value_heap_bytes).sum::<usize>()
        }
        Value::Object(map) => map_heap_bytes(map),
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

/// [`value_heap_bytes`] of an object.
pub fn map_heap_bytes(map: &serde_json::Map<String, Value>) -> usize {
    let (buckets, cap) = map_layout(map.len());
    let indices = if buckets == 0 { 0 } else { malloc_chunk(buckets * 8 + buckets + 16) };
    indices
        + malloc_chunk(cap * ENTRY_SIZE)
        + map.iter().map(|(k, v)| malloc_chunk(k.len()) + value_heap_bytes(v)).sum::<usize>()
}

/// Heap bytes per JSON string (key or value) used by [`predict_heap_bytes`] (string block plus
/// its share of the `Value` slot and map entry).
const BYTES_PER_STRING: usize = 110;
/// Heap bytes per JSON object beyond its strings (hash index and entry table).
const BYTES_PER_OBJECT: usize = 250;

/// Upper estimate of [`value_heap_bytes`] for the tree `serde_json` would build from `json`,
/// without parsing it: the text size plus a cost per string (`"` pairs) and per object (`{`).
/// On real packuments (`vite`, `@firebase/firestore`, `typescript`, `@types/node`, `rollup`,
/// `zod`, `express`) it is 2–32% above the real value.
pub fn predict_heap_bytes(json: &[u8]) -> usize {
    let (mut quotes, mut braces) = (0usize, 0usize);
    for &b in json {
        quotes += usize::from(b == b'"');
        braces += usize::from(b == b'{');
    }
    json.len() + quotes / 2 * BYTES_PER_STRING + braces * BYTES_PER_OBJECT
}

/// Upper bound of the tree-to-file ratio, to size a reservation before reading a manifest file
/// whose tree size is not known yet (real packuments: 1.7–9.1; dense synthetic ones up to ~11).
pub const TREE_BYTES_PER_FILE_BYTE: u64 = 12;

/// Bytes of a manifest file `len` bytes long while it is read and parsed: the file contents
/// plus an upper bound of the tree.
pub fn file_load_bytes(len: u64) -> usize {
    usize::try_from(len.saturating_mul(TREE_BYTES_PER_FILE_BYTE + 1)).unwrap_or(usize::MAX)
}

/// See the module docs.
#[derive(Debug)]
pub struct MemoryBudget {
    capacity: usize,
    state: Mutex<State>,
    notify: Notify,
}

#[derive(Debug, Default)]
struct State {
    used: usize,
    peak: usize,
    next_ticket: u64,
    /// Running reservations, oldest first.
    active: BTreeSet<u64>,
    /// Reservations waiting to start, in arrival order.
    queue: VecDeque<u64>,
    /// Running reservations waiting to grow.
    growing: usize,
}

impl State {
    fn add(&mut self, n: usize) {
        self.used = self.used.saturating_add(n);
        self.peak = self.peak.max(self.used);
    }
}

impl MemoryBudget {
    /// A budget of `capacity` bytes (`usize::MAX`: never waits).
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self { capacity, state: Mutex::default(), notify: Notify::new() })
    }

    /// A budget that never makes anyone wait (still counts).
    pub fn unlimited() -> Arc<Self> {
        Self::new(usize::MAX)
    }

    /// Capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes reserved now.
    pub fn used(&self) -> usize {
        self.lock().used
    }

    /// Highest [`MemoryBudget::used`] seen so far.
    pub fn peak(&self) -> usize {
        self.lock().peak
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Count `bytes` already in use (memory that left a cache but is still referenced) without
    /// waiting, until the returned hold is dropped.
    pub fn count_in_use(self: &Arc<Self>, bytes: usize) -> PassiveHold {
        self.lock().add(bytes);
        PassiveHold { budget: Arc::clone(self), held: bytes }
    }

    /// Wait until `bytes` can be reserved (see the module docs), then reserve them.
    pub async fn reserve(self: &Arc<Self>, bytes: usize) -> Reservation {
        let ticket = {
            let mut st = self.lock();
            let ticket = st.next_ticket;
            st.next_ticket += 1;
            if self.capacity == usize::MAX
                || (st.queue.is_empty() && st.growing == 0 && st.used + bytes <= self.capacity)
            {
                st.active.insert(ticket);
                st.add(bytes);
                return Reservation { budget: Arc::clone(self), ticket, held: bytes };
            }
            st.queue.push_back(ticket);
            ticket
        };
        let mut queued = Queued { budget: self, ticket, waiting: true };
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = self.lock();
                let fits = st.used.saturating_add(bytes) <= self.capacity || st.active.is_empty();
                if st.queue.front() == Some(&ticket) && st.growing == 0 && fits {
                    st.queue.pop_front();
                    st.active.insert(ticket);
                    st.add(bytes);
                    queued.waiting = false;
                    drop(st);
                    // the next one in the queue may fit too
                    self.notify.notify_waiters();
                    return Reservation { budget: Arc::clone(self), ticket, held: bytes };
                }
            }
            notified.await;
        }
    }
}

/// Removes a cancelled [`MemoryBudget::reserve`] from the queue.
struct Queued<'a> {
    budget: &'a MemoryBudget,
    ticket: u64,
    waiting: bool,
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        if self.waiting {
            self.budget.lock().queue.retain(|t| *t != self.ticket);
            self.budget.notify.notify_waiters();
        }
    }
}

/// Counts a reservation waiting in [`Reservation::grow_to`]; undone on drop (also on cancel).
struct Growing<'a> {
    budget: &'a MemoryBudget,
    counted: bool,
}

impl Drop for Growing<'_> {
    fn drop(&mut self) {
        if self.counted {
            self.budget.lock().growing -= 1;
            self.budget.notify.notify_waiters();
        }
    }
}

/// Bytes taken from a [`MemoryBudget`], given back on drop.
#[derive(Debug)]
pub struct Reservation {
    budget: Arc<MemoryBudget>,
    ticket: u64,
    held: usize,
}

impl Reservation {
    /// Bytes held.
    pub fn held(&self) -> usize {
        self.held
    }

    /// The budget this comes from.
    pub fn budget(&self) -> &Arc<MemoryBudget> {
        &self.budget
    }

    /// Hold at least `total` bytes, waiting for the difference unless this is the oldest
    /// running reservation (see the module docs). No-op when `total <= held`.
    pub async fn grow_to(&mut self, total: usize) {
        if total <= self.held {
            return;
        }
        let extra = total - self.held;
        let budget = Arc::clone(&self.budget);
        let mut growing = Growing { budget: &budget, counted: false };
        loop {
            let notified = budget.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = budget.lock();
                let oldest = st.active.first() == Some(&self.ticket);
                if oldest || st.used.saturating_add(extra) <= budget.capacity {
                    st.add(extra);
                    self.held = total;
                    return;
                }
                if !growing.counted {
                    st.growing += 1;
                    growing.counted = true;
                }
            }
            notified.await;
        }
    }

    /// Grow by `extra` bytes ([`Reservation::grow_to`]).
    pub async fn grow(&mut self, extra: usize) {
        self.grow_to(self.held.saturating_add(extra)).await;
    }

    /// Move `bytes` (at most what is held) into a new reservation of the same budget, without
    /// waiting. The new one counts as just started (youngest).
    pub fn split(&mut self, bytes: usize) -> Reservation {
        let bytes = bytes.min(self.held);
        self.held -= bytes;
        let ticket = {
            let mut st = self.budget.lock();
            let ticket = st.next_ticket;
            st.next_ticket += 1;
            st.active.insert(ticket);
            ticket
        };
        Reservation { budget: Arc::clone(&self.budget), ticket, held: bytes }
    }

    /// Keep the bytes counted, but as memory at rest rather than running work (see the module
    /// docs).
    pub fn into_passive(mut self) -> PassiveHold {
        let held = std::mem::take(&mut self.held);
        let budget = Arc::clone(&self.budget);
        // the drop gives back 0 bytes and removes the ticket from the running set
        drop(self);
        PassiveHold { budget, held }
    }

    /// Give back everything above `total` bytes. No-op when `total >= held`.
    pub fn shrink_to(&mut self, total: usize) {
        if total >= self.held {
            return;
        }
        let freed = self.held - total;
        self.held = total;
        self.budget.lock().used -= freed;
        self.budget.notify.notify_waiters();
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        {
            let mut st = self.budget.lock();
            st.used -= self.held;
            st.active.remove(&self.ticket);
        }
        self.budget.notify.notify_waiters();
    }
}

/// Bytes kept counted in a [`MemoryBudget`] for memory at rest; given back on drop.
#[derive(Debug)]
pub struct PassiveHold {
    budget: Arc<MemoryBudget>,
    held: usize,
}

impl PassiveHold {
    /// Bytes held.
    pub fn held(&self) -> usize {
        self.held
    }
}

impl Drop for PassiveHold {
    fn drop(&mut self) {
        self.budget.lock().used -= self.held;
        self.budget.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn heap_estimate_counts_every_block() {
        assert_eq!(value_heap_bytes(&Value::Null), 0);
        assert_eq!(value_heap_bytes(&Value::String(String::new())), 0);
        // 5 bytes + 8 header → 32-byte minimum chunk
        assert_eq!(value_heap_bytes(&Value::String("hello".into())), 32);
        // 40 bytes + 8 → 48, already a multiple of 16
        assert_eq!(value_heap_bytes(&Value::String("x".repeat(40))), 48);
        // an array of 1 → capacity 4 values
        let one = Value::Array(vec![Value::Bool(true)]);
        assert_eq!(value_heap_bytes(&one), malloc_chunk(4 * VALUE_SIZE));
        // an object of 1 short key → 4 buckets, 3 entries
        let obj: Value = serde_json::from_str(r#"{"a":1}"#).unwrap();
        let expected = malloc_chunk(4 * 8 + 4 + 16) + malloc_chunk(3 * ENTRY_SIZE) + 32;
        assert_eq!(value_heap_bytes(&obj), expected);
        assert_eq!(map_layout(3), (4, 3));
        assert_eq!(map_layout(4), (8, 7));
        assert_eq!(map_layout(8), (16, 14));
        assert_eq!(map_layout(15), (32, 28));
    }

    #[test]
    fn prediction_is_an_upper_estimate_of_the_tree() {
        // a packument-like document: many small strings, nested objects
        let mut versions = serde_json::Map::new();
        for i in 0..500 {
            versions.insert(
                format!("1.{i}.0"),
                serde_json::json!({
                    "name": "pkg", "version": format!("1.{i}.0"),
                    "dependencies": {"a": "^1.0.0", "b": "~2.3.4", "c": "*"},
                    "dist": {"shasum": "0123456789abcdef0123456789abcdef01234567",
                             "tarball": format!("https://registry.npmjs.org/pkg/-/pkg-1.{i}.0.tgz")},
                    "maintainers": [{"name": "someone", "email": "someone@example.com"}],
                }),
            );
        }
        let doc = serde_json::json!({"name": "pkg", "versions": versions});
        let text = serde_json::to_vec(&doc).unwrap();
        let parsed: Value = serde_json::from_slice(&text).unwrap();
        let real = value_heap_bytes(&parsed);
        let predicted = predict_heap_bytes(&text);
        assert!(predicted >= real, "predicted {predicted} < real {real}");
        assert!(predicted < real * 13 / 10, "predicted {predicted} too far above {real}");
        let ratio = real as f64 / text.len() as f64;
        assert!(ratio < TREE_BYTES_PER_FILE_BYTE as f64, "tree/file ratio {ratio}");
    }

    #[tokio::test]
    async fn reserve_waits_until_it_fits() {
        let budget = MemoryBudget::new(100);
        let a = budget.reserve(60).await;
        assert_eq!(budget.used(), 60);
        let waiting = tokio::spawn({
            let budget = Arc::clone(&budget);
            async move { budget.reserve(50).await.held() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(a);
        assert_eq!(waiting.await.unwrap(), 50);
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.peak(), 60);
    }

    #[tokio::test]
    async fn oversized_reservation_runs_alone() {
        let budget = MemoryBudget::new(100);
        let small = budget.reserve(10).await;
        let big = tokio::spawn({
            let budget = Arc::clone(&budget);
            async move {
                let r = budget.reserve(500).await;
                (r.held(), budget.used())
            }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!big.is_finished(), "must wait for the running reservation");
        // FIFO: a small one behind the big one waits too
        let behind = tokio::spawn({
            let budget = Arc::clone(&budget);
            async move { budget.reserve(1).await.held() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!behind.is_finished());
        drop(small);
        assert_eq!(big.await.unwrap(), (500, 500));
        assert_eq!(behind.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn oldest_grows_without_waiting_younger_waits() {
        let budget = MemoryBudget::new(100);
        let mut old = budget.reserve(40).await;
        let mut young = budget.reserve(40).await;
        // the oldest may exceed the capacity: it is never blocked
        old.grow_to(90).await;
        assert_eq!(budget.used(), 130);
        let budget2 = Arc::clone(&budget);
        let grow = tokio::spawn(async move {
            young.grow(30).await;
            let used = budget2.used();
            (young, used)
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!grow.is_finished(), "younger reservation must wait");
        // new reservations wait while someone waits to grow
        let starter = tokio::spawn({
            let budget = Arc::clone(&budget);
            async move { budget.reserve(1).await.held() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!starter.is_finished());
        old.shrink_to(10);
        // 10 + 40 + 30 fits
        let (young, used) = grow.await.unwrap();
        assert_eq!(young.held(), 70);
        assert_eq!(used, 80);
        drop(old);
        assert_eq!(starter.await.unwrap(), 1);
        drop(young);
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn passive_holds_count_but_do_not_block_running_alone() {
        let budget = MemoryBudget::new(100);
        let mut r = budget.reserve(90).await;
        let part = r.split(80);
        assert_eq!((r.held(), part.held(), budget.used()), (10, 80, 90));
        drop(r);
        let hold = part.into_passive();
        assert_eq!((hold.held(), budget.used()), (80, 80));
        // does not fit, but nothing is running: starts alone
        let next = tokio::time::timeout(Duration::from_secs(1), budget.reserve(50)).await.unwrap();
        assert_eq!(budget.used(), 130);
        // while it runs, others wait
        let waiting = tokio::spawn({
            let budget = Arc::clone(&budget);
            async move { budget.reserve(30).await.held() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(hold);
        drop(next);
        assert_eq!(waiting.await.unwrap(), 30);
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn cancelled_waits_do_not_block_others() {
        let budget = MemoryBudget::new(100);
        let a = budget.reserve(90).await;
        let cancelled = tokio::spawn({
            let budget = Arc::clone(&budget);
            async move { budget.reserve(50).await.held() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancelled.abort();
        let _ = cancelled.await;
        // the aborted waiter left the queue: a fitting reservation starts at once
        let b = tokio::time::timeout(Duration::from_secs(1), budget.reserve(10)).await;
        assert!(b.is_ok());
        drop(a);
    }

    #[tokio::test]
    async fn many_concurrent_tasks_stay_within_capacity() {
        let budget = MemoryBudget::new(1000);
        let mut tasks = Vec::new();
        for i in 0..64usize {
            let budget = Arc::clone(&budget);
            tasks.push(tokio::spawn(async move {
                let mut r = budget.reserve(10).await;
                for _ in 0..5 {
                    r.grow(10 + i % 7 * 10).await;
                    tokio::task::yield_now().await;
                }
                r.shrink_to(5);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(budget.used(), 0);
        // only the oldest may exceed the capacity, by at most what it grew past it
        assert!(budget.peak() <= 1000 + 5 * 70, "peak {}", budget.peak());
    }
}
