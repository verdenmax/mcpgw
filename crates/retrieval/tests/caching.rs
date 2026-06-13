use retrieval::{CachingEmbedder, Embedder, MockEmbedder};
use std::sync::Arc;

#[tokio::test]
async fn caches_and_only_embeds_new_texts() {
    let mock = MockEmbedder::new(32);
    let seen = mock.seen.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));

    let v1 = caching.embed(&["a".into(), "b".into()]).await.unwrap();
    let v2 = caching.embed(&["a".into(), "c".into()]).await.unwrap(); // "a" cached, "c" new

    // Inner saw each unique text exactly once, in first-seen order.
    assert_eq!(*seen.lock().unwrap(), vec!["a", "b", "c"]);
    // Cached vector for "a" is identical across calls.
    assert_eq!(v1[0], v2[0]);
}

#[tokio::test]
async fn preserves_input_order_and_dedups_within_a_call() {
    let mock = MockEmbedder::new(16);
    let seen = mock.seen.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));
    assert_eq!(caching.dim(), 16);

    let v = caching
        .embed(&["x".into(), "y".into(), "x".into()])
        .await
        .unwrap();
    assert_eq!(v.len(), 3);
    assert_eq!(v[0], v[2]); // same text -> same vector, original order preserved
    assert_eq!(*seen.lock().unwrap(), vec!["x", "y"]); // "x" embedded once
}

#[tokio::test]
async fn all_cached_second_call_skips_inner() {
    let mock = MockEmbedder::new(8);
    let calls = mock.calls.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));
    caching.embed(&["a".into()]).await.unwrap();
    caching.embed(&["a".into()]).await.unwrap(); // fully cached
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn propagates_inner_error() {
    let caching = CachingEmbedder::new(Arc::new(MockEmbedder::failing(8)));
    assert!(caching.embed(&["x".into()]).await.is_err());
}
