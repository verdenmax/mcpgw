//! The `Embedder` abstraction: turn texts into vectors. The HTTP-backed provider lives in
//! the separate `embedder` crate; this module only defines the trait, errors, and a
//! deterministic `MockEmbedder` (behind the `testkit` feature) for tests.

use async_trait::async_trait;

/// Errors from embedding. Kept provider-agnostic so `retrieval` needs no HTTP dependency.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding provider error: {0}")]
    Provider(String),
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    Dimension { expected: usize, got: usize },
}

/// Turns a batch of texts into one vector each (same order). All-or-nothing per call.
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// Expected embedding dimension (used for sanity checks).
    fn dim(&self) -> usize;
}

#[cfg(feature = "testkit")]
mod mock {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Deterministic test embedder. Each token (split on non-alphanumeric, lowercased) is
    /// hashed into one of `dim` buckets and adds 1.0 there — so texts sharing tokens have
    /// higher cosine similarity. Records call count + texts seen for cache assertions.
    pub struct MockEmbedder {
        dim: usize,
        fail: bool,
        pub calls: Arc<AtomicUsize>,
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    impl MockEmbedder {
        pub fn new(dim: usize) -> Self {
            debug_assert!(
                dim > 0,
                "MockEmbedder dim must be > 0 (zero yields degenerate vectors)"
            );
            Self {
                dim,
                fail: false,
                calls: Arc::new(AtomicUsize::new(0)),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
        /// An embedder whose `embed` always errors (drives degradation tests).
        pub fn failing(dim: usize) -> Self {
            Self {
                fail: true,
                ..Self::new(dim)
            }
        }
        fn vec_for(&self, text: &str) -> Vec<f32> {
            let mut v = vec![0.0f32; self.dim];
            for tok in crate::tokenize(text) {
                // FNV-1a over the token bytes -> bucket.
                let mut h: u64 = 0xcbf29ce484222325;
                for b in tok.as_bytes() {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                // Do the modulo in u64 before the `as usize` cast so the bucket is identical
                // on 32- and 64-bit targets (width-independent).
                v[(h % self.dim as u64) as usize] += 1.0;
            }
            v
        }
    }

    #[async_trait]
    impl Embedder for MockEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(EmbedError::Provider("mock failure".into()));
            }
            self.seen.lock().unwrap().extend(texts.iter().cloned());
            Ok(texts.iter().map(|t| self.vec_for(t)).collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }
}

#[cfg(feature = "testkit")]
pub use mock::MockEmbedder;
