use std::sync::{Arc, Mutex};

use axum::{extract::State, routing::post, Json, Router};
use embedder::OpenAiEmbedder;
use retrieval::Embedder;
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<Value>>>;

async fn embeddings_stub(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
    // Return a 3-dim embedding per input, with index, intentionally OUT of order to
    // verify the client sorts by `index`.
    let inputs = body["input"].as_array().cloned().unwrap_or_default();
    let mut data: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, _)| json!({"object":"embedding","index": i, "embedding":[i as f32, 0.0, 1.0]}))
        .collect();
    data.reverse();
    Json(json!({"object":"list","data": data, "model":"stub"}))
}

async fn spawn_stub() -> (String, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/embeddings", post(embeddings_stub))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

#[tokio::test]
async fn embeds_via_openai_compatible_endpoint() {
    let (base, seen) = spawn_stub().await;
    let e = OpenAiEmbedder::new(
        base,
        "text-embedding-3-small".into(),
        "sk-test".into(),
        Some(3),
        None,
    );

    let out = e.embed(&["alpha".into(), "beta".into()]).await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].len(), 3);
    // index-sorted: input 0 -> [0,0,1], input 1 -> [1,0,1]
    assert_eq!(out[0], vec![0.0, 0.0, 1.0]);
    assert_eq!(out[1], vec![1.0, 0.0, 1.0]);

    // request body carried model + input[].
    let body = seen.lock().unwrap()[0].clone();
    assert_eq!(body["model"], "text-embedding-3-small");
    assert_eq!(body["input"], json!(["alpha", "beta"]));
}

#[tokio::test]
async fn dimension_mismatch_is_error() {
    let (base, _) = spawn_stub().await;
    // stub returns dim 3; configure expected dim 99 -> Dimension error.
    let e = OpenAiEmbedder::new(base, "m".into(), "sk".into(), Some(99), None);
    assert!(matches!(
        e.embed(&["x".into()]).await,
        Err(retrieval::EmbedError::Dimension { .. })
    ));
}
