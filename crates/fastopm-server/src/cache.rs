//! Rendered response bodies, keyed by `(name, generation, form, base URL)`.
//!
//! A new generation of a package replaces all its entries. The cache is bounded by an
//! approximate byte budget (body plus half of it for compressed copies: gzip, deflate and brotli
//! of JSON are each ~10–15% of it); the packages inserted first are evicted first. A body larger
//! than the budget is not kept: it holds its share of the work memory budget until it has been
//! sent ([`CachedBody::hold`]).
//!
//! [`RenderCache::in_flight`] lets concurrent requests missing the same body share one render.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use fastopm_core::memory::{MemoryBudget, PassiveHold};
use fastopm_render::{Rendered, Variant};

use crate::negotiate::Coding;

/// Which body of a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Form {
    /// `GET /:package`.
    Manifest(Variant),
    /// `GET /:package/:version` with the version or tag as requested.
    Version(String, Variant),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ItemKey {
    form: Form,
    base: Option<String>,
}

/// One render in progress: concurrent requests for the same body wait on the same cell.
pub(crate) type RenderCell = Arc<tokio::sync::OnceCell<Arc<CachedBody>>>;

/// Key of a render in progress: name, generation, form, base URL.
type FlightKey = (String, u64, Form, Option<String>);

/// A rendered body and its encoded copies, computed on first use.
#[derive(Debug)]
pub(crate) struct CachedBody {
    pub(crate) rendered: Rendered,
    gzip: OnceLock<Bytes>,
    deflate: OnceLock<Bytes>,
    brotli: OnceLock<Bytes>,
    /// For a body not kept in the cache: its memory, counted until the body is dropped.
    hold: Mutex<Option<PassiveHold>>,
}

/// Bytes a cached body is counted for: the body, room for compressed copies, bookkeeping.
pub(crate) fn body_cost(len: usize) -> usize {
    len + len / 2 + 256
}

impl CachedBody {
    pub(crate) fn new(rendered: Rendered) -> Self {
        Self {
            rendered,
            gzip: OnceLock::new(),
            deflate: OnceLock::new(),
            brotli: OnceLock::new(),
            hold: Mutex::new(None),
        }
    }

    /// Keep `hold` counted as long as this body lives.
    pub(crate) fn set_hold(&self, hold: PassiveHold) {
        *self.hold.lock().unwrap_or_else(|e| e.into_inner()) = Some(hold);
    }

    /// Whether this body holds work memory (it is not in the cache).
    pub(crate) fn holds_memory(&self) -> bool {
        self.hold.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// The body in `coding` (`identity` returns the body itself).
    pub(crate) fn encoded(&self, coding: Coding) -> Bytes {
        let cell = match coding {
            Coding::Identity => return self.rendered.body.clone(),
            Coding::Gzip => &self.gzip,
            Coding::Deflate => &self.deflate,
            Coding::Brotli => &self.brotli,
        };
        cell.get_or_init(|| crate::compress::encode(coding, &self.rendered.body)).clone()
    }
}

struct NameEntry {
    generation: u64,
    items: HashMap<ItemKey, Arc<CachedBody>>,
    bytes: usize,
}

#[derive(Default)]
struct Inner {
    by_name: HashMap<String, NameEntry>,
    order: VecDeque<String>,
    bytes: usize,
}

/// See the module docs.
pub(crate) struct RenderCache {
    inner: Mutex<Inner>,
    budget: usize,
    flights: Mutex<HashMap<FlightKey, RenderCell>>,
    /// Where evicted bodies still being sent are counted.
    memory: Arc<MemoryBudget>,
    /// Mirrors the bytes counted, for reports.
    gauge: Arc<AtomicUsize>,
}

impl RenderCache {
    pub(crate) fn new(budget: usize, memory: Arc<MemoryBudget>) -> Self {
        Self {
            inner: Mutex::default(),
            budget,
            flights: Mutex::default(),
            memory,
            gauge: Arc::default(),
        }
    }

    /// Report the bytes counted in `gauge`.
    pub(crate) fn with_gauge(mut self, gauge: Arc<AtomicUsize>) -> Self {
        self.gauge = gauge;
        self
    }

    /// The cell of the render of `(name, generation, form, base)`, shared by concurrent callers.
    /// Call [`RenderCache::land`] once it is filled (or abandoned).
    pub(crate) fn in_flight(
        &self,
        name: &str,
        generation: u64,
        form: &Form,
        base: Option<&str>,
    ) -> RenderCell {
        let key = (name.to_owned(), generation, form.clone(), base.map(str::to_owned));
        let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(flights.entry(key).or_default())
    }

    /// Forget the render `cell` of `(name, generation, form, base)`.
    pub(crate) fn land(
        &self,
        name: &str,
        generation: u64,
        form: &Form,
        base: Option<&str>,
        cell: &RenderCell,
    ) {
        let key = (name.to_owned(), generation, form.clone(), base.map(str::to_owned));
        let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
        if flights.get(&key).is_some_and(|c| Arc::ptr_eq(c, cell)) {
            flights.remove(&key);
        }
    }

    /// Whether a body of `len` bytes would be kept.
    pub(crate) fn would_keep(&self, len: usize) -> bool {
        self.budget > 0 && body_cost(len) <= self.budget
    }

    pub(crate) fn get(
        &self,
        name: &str,
        generation: u64,
        form: &Form,
        base: Option<&str>,
    ) -> Option<Arc<CachedBody>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = inner.by_name.get(name).filter(|e| e.generation == generation)?;
        let key = ItemKey { form: form.clone(), base: base.map(str::to_owned) };
        entry.items.get(&key).cloned()
    }

    /// Keep `body` unless it is larger than the budget ([`RenderCache::would_keep`]). Entries
    /// replaced or evicted are freed after the lock is released.
    pub(crate) fn insert(
        &self,
        name: &str,
        generation: u64,
        form: Form,
        base: Option<&str>,
        body: Arc<CachedBody>,
    ) {
        let cost = body_cost(body.rendered.body.len());
        if !self.would_keep(body.rendered.body.len()) {
            return;
        }
        let mut freed: Vec<NameEntry> = Vec::new();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &mut *guard;
        if !inner.by_name.contains_key(name) {
            inner.order.push_back(name.to_owned());
            let entry = NameEntry { generation, items: HashMap::new(), bytes: 0 };
            inner.by_name.insert(name.to_owned(), entry);
        }
        let Some(entry) = inner.by_name.get_mut(name) else { return };
        if entry.generation != generation {
            inner.bytes -= entry.bytes;
            let old = std::mem::take(&mut entry.items);
            freed.push(NameEntry { generation: entry.generation, items: old, bytes: 0 });
            entry.bytes = 0;
            entry.generation = generation;
        }
        let key = ItemKey { form, base: base.map(str::to_owned) };
        if let Some(old) = entry.items.insert(key.clone(), body) {
            let old_cost = body_cost(old.rendered.body.len());
            entry.bytes -= old_cost;
            inner.bytes -= old_cost;
            let mut items = HashMap::new();
            items.insert(key, old);
            freed.push(NameEntry { generation, items, bytes: 0 });
        }
        entry.bytes += cost;
        inner.bytes += cost;
        while inner.bytes > self.budget {
            let Some(oldest) = inner.order.pop_front() else { break };
            if let Some(evicted) = inner.by_name.remove(&oldest) {
                inner.bytes -= evicted.bytes;
                freed.push(evicted);
            }
        }
        self.gauge.store(inner.bytes, Ordering::Relaxed);
        drop(guard);
        // bodies still being sent stay counted in the work budget until they are dropped
        for entry in freed {
            for body in entry.items.into_values() {
                if Arc::strong_count(&body) > 1 && !body.holds_memory() {
                    let cost = body_cost(body.rendered.body.len());
                    body.set_hold(self.memory.count_in_use(cost));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(text: &str) -> Arc<CachedBody> {
        Arc::new(CachedBody::new(Rendered {
            body: Bytes::copy_from_slice(text.as_bytes()),
            etag: fastopm_render::etag(text.as_bytes()),
            content_type: fastopm_core::headers::CONTENT_TYPE_JSON,
        }))
    }

    #[test]
    fn keyed_by_generation_form_and_base() {
        let cache = RenderCache::new(1 << 20, MemoryBudget::unlimited());
        let full = Form::Manifest(Variant::Full);
        cache.insert("a", 1, full.clone(), Some("http://h/"), body("x"));
        assert!(cache.get("a", 1, &full, Some("http://h/")).is_some());
        assert!(cache.get("a", 1, &full, None).is_none());
        assert!(cache.get("a", 2, &full, Some("http://h/")).is_none());
        assert!(
            cache.get("a", 1, &Form::Manifest(Variant::Abbreviated), Some("http://h/")).is_none()
        );
        // a new generation drops the old entries
        cache.insert("a", 2, Form::Version("1.0.0".into(), Variant::Full), None, body("y"));
        assert!(cache.get("a", 1, &full, Some("http://h/")).is_none());
    }

    #[test]
    fn evicts_oldest_package_over_budget() {
        let cache = RenderCache::new(2 * body_cost(100), MemoryBudget::unlimited());
        let full = Form::Manifest(Variant::Full);
        let big = "z".repeat(100);
        cache.insert("a", 1, full.clone(), None, body(&big));
        cache.insert("b", 1, full.clone(), None, body(&big));
        cache.insert("c", 1, full.clone(), None, body(&big));
        assert!(cache.get("a", 1, &full, None).is_none());
        assert!(cache.get("b", 1, &full, None).is_some());
        assert!(cache.get("c", 1, &full, None).is_some());
    }

    #[test]
    fn evicted_bodies_in_use_stay_counted() {
        let memory = MemoryBudget::new(1 << 20);
        let cache = RenderCache::new(body_cost(100), Arc::clone(&memory));
        let full = Form::Manifest(Variant::Full);
        let sending = body(&"z".repeat(100));
        cache.insert("a", 1, full.clone(), None, Arc::clone(&sending));
        cache.insert("b", 1, full.clone(), None, body(&"y".repeat(100)));
        assert!(cache.get("a", 1, &full, None).is_none());
        assert_eq!(memory.used(), body_cost(100));
        drop(sending);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn oversized_bodies_are_not_kept() {
        let cache = RenderCache::new(body_cost(100), MemoryBudget::unlimited());
        assert!(cache.would_keep(100));
        assert!(!cache.would_keep(101));
        let full = Form::Manifest(Variant::Full);
        cache.insert("a", 1, full.clone(), None, body(&"z".repeat(101)));
        assert!(cache.get("a", 1, &full, None).is_none());
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_render() {
        let cache = Arc::new(RenderCache::new(1 << 20, MemoryBudget::unlimited()));
        let full = Form::Manifest(Variant::Full);
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (cache, full, renders) = (Arc::clone(&cache), full.clone(), Arc::clone(&renders));
            tasks.push(tokio::spawn(async move {
                let cell = cache.in_flight("a", 1, &full, None);
                let b = cell
                    .get_or_init(|| async {
                        renders.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        body("x")
                    })
                    .await
                    .clone();
                cache.land("a", 1, &full, None, &cell);
                b
            }));
        }
        let bodies: Vec<_> = futures_util::future::join_all(tasks).await;
        assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 1);
        let first = bodies[0].as_ref().unwrap();
        assert!(bodies.iter().all(|b| Arc::ptr_eq(b.as_ref().unwrap(), first)));
        assert!(cache.flights.lock().unwrap().is_empty());
    }

    #[test]
    fn zero_budget_disables() {
        let cache = RenderCache::new(0, MemoryBudget::unlimited());
        let full = Form::Manifest(Variant::Full);
        cache.insert("a", 1, full.clone(), None, body("x"));
        assert!(cache.get("a", 1, &full, None).is_none());
    }
}
