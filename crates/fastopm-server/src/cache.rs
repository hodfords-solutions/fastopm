//! Rendered response bodies, keyed by `(name, generation, form, base URL)`.
//!
//! A new generation of a package replaces all its entries. The cache is bounded by an
//! approximate byte budget (body plus room for one encoded copy); the packages inserted first
//! are evicted first.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
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

/// A rendered body and its encoded copies, computed on first use.
#[derive(Debug)]
pub(crate) struct CachedBody {
    pub(crate) rendered: Rendered,
    gzip: OnceLock<Bytes>,
    deflate: OnceLock<Bytes>,
    brotli: OnceLock<Bytes>,
}

impl CachedBody {
    pub(crate) fn new(rendered: Rendered) -> Self {
        Self { rendered, gzip: OnceLock::new(), deflate: OnceLock::new(), brotli: OnceLock::new() }
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
}

impl RenderCache {
    pub(crate) fn new(budget: usize) -> Self {
        Self { inner: Mutex::default(), budget }
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

    pub(crate) fn insert(
        &self,
        name: &str,
        generation: u64,
        form: Form,
        base: Option<&str>,
        body: Arc<CachedBody>,
    ) {
        let cost = body.rendered.body.len() * 2 + 256;
        if self.budget == 0 || cost > self.budget {
            return;
        }
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
            entry.items.clear();
            entry.bytes = 0;
            entry.generation = generation;
        }
        let key = ItemKey { form, base: base.map(str::to_owned) };
        if let Some(old) = entry.items.insert(key, body) {
            let old_cost = old.rendered.body.len() * 2 + 256;
            entry.bytes -= old_cost;
            inner.bytes -= old_cost;
        }
        entry.bytes += cost;
        inner.bytes += cost;
        while inner.bytes > self.budget {
            let Some(oldest) = inner.order.pop_front() else { break };
            if let Some(evicted) = inner.by_name.remove(&oldest) {
                inner.bytes -= evicted.bytes;
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
        let cache = RenderCache::new(1 << 20);
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
        let cache = RenderCache::new(2 * (100 * 2 + 256));
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
    fn zero_budget_disables() {
        let cache = RenderCache::new(0);
        let full = Form::Manifest(Variant::Full);
        cache.insert("a", 1, full.clone(), None, body("x"));
        assert!(cache.get("a", 1, &full, None).is_none());
    }
}
