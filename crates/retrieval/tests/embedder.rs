use retrieval::{Embedder, MockEmbedder};

#[tokio::test]
async fn mock_embedder_is_deterministic_and_right_dim() {
    let e = MockEmbedder::new(64);
    assert_eq!(e.dim(), 64);
    let a = e.embed(&["create github issue".to_string()]).await.unwrap();
    let b = e.embed(&["create github issue".to_string()]).await.unwrap();
    assert_eq!(a, b, "same text -> same vector");
    assert_eq!(a[0].len(), 64);
}

#[tokio::test]
async fn mock_embedder_shared_tokens_score_higher_cosine() {
    let e = MockEmbedder::new(64);
    let v = e
        .embed(&[
            "send a slack message".to_string(),              // query
            "post a message to a slack channel".to_string(), // related
            "get the weather forecast".to_string(),          // unrelated
        ])
        .await
        .unwrap();
    let cos = |x: &[f32], y: &[f32]| -> f32 {
        let dot: f32 = x.iter().zip(y).map(|(a, b)| a * b).sum();
        let nx: f32 = x.iter().map(|a| a * a).sum::<f32>().sqrt();
        let ny: f32 = y.iter().map(|a| a * a).sum::<f32>().sqrt();
        dot / (nx * ny)
    };
    assert!(
        cos(&v[0], &v[1]) > cos(&v[0], &v[2]),
        "related text must be closer than unrelated"
    );
}

#[tokio::test]
async fn mock_embedder_failing_returns_err() {
    let e = MockEmbedder::failing(64);
    assert!(e.embed(&["x".to_string()]).await.is_err());
}
