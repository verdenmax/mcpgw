//! `CachingEmbedder`: an `Embedder` decorator that memoizes vectors by text content.
//!
//! Bounded by a two-generation scheme (`current` + `previous`, each capped at
//! `CACHE_GEN_CAP`) so memory cannot grow without bound when arbitrary query texts are
//! embedded. Frequently-seen texts (e.g. tool descriptions re-embedded each rebuild) stay
//! warm via promote-on-hit. Only cache-miss texts are forwarded to `inner`. Keyed on the text
//! itself, so two distinct texts can never collide onto one cache slot.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::embedder::{EmbedError, Embedder};

/// Per-generation entry cap. Total resident entries are bounded by ~`2 * CACHE_GEN_CAP`.
const CACHE_GEN_CAP: usize = 2048;

/// Two-generation bounded cache keyed on the text. Lookups check `current`, then `previous`
/// (promoting a `previous` hit back into `current`). When `current` reaches `CACHE_GEN_CAP`, it
/// rotates into `previous` (dropping the old `previous`) and a fresh `current` starts.
struct GenCache {
    current: HashMap<String, Arc<[f32]>>,
    previous: HashMap<String, Arc<[f32]>>,
}

impl GenCache {
    fn new() -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
        }
    }

    /// Look up `key`, promoting a `previous`-generation hit into `current`.
    fn get(&mut self, key: &str) -> Option<Arc<[f32]>> {
        if let Some(v) = self.current.get(key) {
            return Some(v.clone());
        }
        if let Some(v) = self.previous.remove(key) {
            self.insert(key.to_string(), v.clone());
            return Some(v);
        }
        None
    }

    /// Insert `key`, rotating generations first if `current` is full.
    fn insert(&mut self, key: String, value: Arc<[f32]>) {
        if self.current.len() >= CACHE_GEN_CAP && !self.current.contains_key(&key) {
            self.previous = std::mem::take(&mut self.current);
        }
        self.current.insert(key, value);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }
}

/// Memoizes embeddings by text content, bounded by a two-generation cache.
pub struct CachingEmbedder {
    inner: Arc<dyn Embedder>,
    cache: Mutex<GenCache>,
}

impl CachingEmbedder {
    pub fn new(inner: Arc<dyn Embedder>) -> Self {
        Self {
            inner,
            cache: Mutex::new(GenCache::new()),
        }
    }
}

#[async_trait]
impl Embedder for CachingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        // First pass: pull cached vectors (promoting previous-gen hits) into a local map and
        // collect the unique miss texts. The lock is held only for synchronous map ops.
        let mut resolved: HashMap<String, Arc<[f32]>> = HashMap::new();
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_seen: HashSet<&str> = HashSet::new();
        {
            let mut cache = self.cache.lock().unwrap();
            for t in texts {
                if resolved.contains_key(t) {
                    continue;
                }
                if let Some(v) = cache.get(t) {
                    resolved.insert(t.clone(), v);
                } else if miss_seen.insert(t.as_str()) {
                    miss_texts.push(t.clone());
                }
            }
        }

        // Embed only the misses (skip the call entirely if everything was cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            // Defend the reassembly invariant: a conforming Embedder returns one vector per input.
            // A short/long return would otherwise leave a miss unresolved and panic below.
            if embedded.len() != miss_texts.len() {
                return Err(EmbedError::Provider(format!(
                    "embedder returned {} vectors for {} inputs",
                    embedded.len(),
                    miss_texts.len()
                )));
            }
            let mut cache = self.cache.lock().unwrap();
            for (t, v) in miss_texts.into_iter().zip(embedded) {
                let arc: Arc<[f32]> = Arc::from(v.into_boxed_slice());
                cache.insert(t.clone(), arc.clone());
                resolved.insert(t, arc);
            }
        }

        // Reassemble in original input order from the local `resolved` map (NOT the bounded
        // cache, which may have evicted entries within an oversized batch). Every text is either a
        // first-pass hit or was just embedded+inserted, so the lookup cannot miss — unless the
        // inner embedder returned fewer vectors than inputs (a contract violation the only
        // production `Embedder` rejects as `Err`).
        Ok(texts
            .iter()
            .map(|t| resolved.get(t).expect("text resolved above").to_vec())
            .collect())
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Local FNV-1a, used only to derive deterministic test vectors (equal text -> equal vector).
    fn text_hash(text: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in text.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Embedder that counts how many texts it was asked to embed and returns a deterministic
    /// vector derived from each text (so equal texts -> equal vectors, distinct texts differ).
    struct CountingEmbedder {
        calls: AtomicUsize,
        dim: usize,
    }
    impl CountingEmbedder {
        fn new(dim: usize) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                dim,
            }
        }
    }
    #[async_trait]
    impl Embedder for CountingEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.calls.fetch_add(texts.len(), Ordering::Relaxed);
            Ok(texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0f32; self.dim];
                    v[0] = text_hash(t) as f32;
                    v
                })
                .collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn caches_hits_and_only_embeds_misses() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        let a = c.embed(&["x".into()]).await.unwrap();
        let b = c.embed(&["x".into()]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(
            inner.calls.load(Ordering::Relaxed),
            1,
            "second embed of the same text must hit the cache (no new inner call)"
        );
    }

    #[tokio::test]
    async fn distinct_texts_get_distinct_vectors() {
        // Collision-freedom is now structural (the key IS the text); this guards the hit path:
        // distinct inputs keep distinct vectors, and a fully-cached re-read returns each text's
        // own vector (a swapped/wrong mapping would fail `assert_eq!(first, again)`).
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner);
        let first = c.embed(&["alpha".into(), "beta".into()]).await.unwrap();
        let again = c.embed(&["alpha".into(), "beta".into()]).await.unwrap();
        assert_eq!(first, again);
        assert_ne!(
            first[0], first[1],
            "different texts must map to different vectors"
        );
    }

    #[tokio::test]
    async fn memory_is_bounded_under_many_distinct_texts() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner);
        for i in 0..(CACHE_GEN_CAP * 3) {
            c.embed(&[format!("q{i}")]).await.unwrap();
        }
        let cache = c.cache.lock().unwrap();
        assert!(
            cache.len() <= 2 * CACHE_GEN_CAP,
            "cache must stay bounded at ~2*CAP, got {}",
            cache.len()
        );
    }

    #[tokio::test]
    async fn promote_on_hit_prevents_re_embedding_a_hot_key() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        c.embed(&["hot".into()]).await.unwrap();
        // Churn >2 full generations of distinct keys, re-touching "hot" within each generation
        // window so promote-on-hit keeps it resident.
        for i in 0..(CACHE_GEN_CAP * 3) {
            c.embed(&[format!("k{i}")]).await.unwrap();
            if i % (CACHE_GEN_CAP / 2) == 0 {
                c.embed(&["hot".into()]).await.unwrap();
            }
        }
        // Inner embedded "hot" once + each of the 3*CAP distinct k's once, nothing more. Without
        // promote-on-hit "hot" would fall out of `previous` on a later rotation and be re-embedded
        // once, making this 3*CAP + 2 — so this exact count actually guards promotion.
        assert_eq!(
            inner.calls.load(Ordering::Relaxed),
            CACHE_GEN_CAP * 3 + 1,
            "promotion must prevent any re-embed of the periodically-touched hot key"
        );
    }

    #[tokio::test]
    async fn single_oversized_batch_stays_bounded_and_returns_all_vectors() {
        // The headline fix: one embed() of a batch larger than 2*CAP rotates the cache several
        // times mid-call, yet every input must still get its correct vector (reassembly reads the
        // local `resolved` map, not the evicting cache) and the cache stays bounded.
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        let batch: Vec<String> = (0..(CACHE_GEN_CAP * 2 + 7))
            .map(|i| format!("t{i}"))
            .collect();
        let out = c.embed(&batch).await.unwrap();

        assert_eq!(out.len(), batch.len(), "one output vector per input");
        for (t, v) in batch.iter().zip(&out) {
            let mut expected = vec![0.0f32; 2];
            expected[0] = text_hash(t) as f32;
            assert_eq!(
                v, &expected,
                "vector for {t:?} must be correct despite mid-batch eviction"
            );
        }
        assert!(
            c.cache.lock().unwrap().len() <= 2 * CACHE_GEN_CAP,
            "persistent cache must stay bounded even for an oversized batch"
        );
    }

    /// An embedder that returns FEWER vectors than inputs (a contract violation) must surface as
    /// an `Err`, not a panic in the reassembly step.
    struct ShortEmbedder {
        dim: usize,
    }
    #[async_trait]
    impl Embedder for ShortEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            // One fewer vector than asked for.
            Ok(texts
                .iter()
                .skip(1)
                .map(|_| vec![0.0f32; self.dim])
                .collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn short_inner_return_errors_instead_of_panicking() {
        let c = CachingEmbedder::new(Arc::new(ShortEmbedder { dim: 2 }));
        let r = c.embed(&["a".into(), "b".into()]).await;
        assert!(
            matches!(r, Err(EmbedError::Provider(_))),
            "a short inner return must be an Err, not a panic"
        );
    }
}
