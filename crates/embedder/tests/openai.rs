use std::sync::{Arc, Mutex};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use embedder::OpenAiEmbedder;
use retrieval::Embedder;
use serde_json::{json, Value};

#[derive(Default)]
struct Captured {
    bodies: Vec<Value>,
    auth: Vec<String>,
}

type Seen = Arc<Mutex<Captured>>;

async fn embeddings_stub(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    {
        let mut guard = seen.lock().unwrap();
        guard.bodies.push(body.clone());
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        guard.auth.push(auth);
    }
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

async fn bad_request_stub() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":{"message":"bad model xyz"}})),
    )
}

async fn duplicate_index_stub(Json(_body): Json<Value>) -> Json<Value> {
    // Two data items, both with index 0, for two inputs -> non-contiguous indices.
    let data = vec![
        json!({"object":"embedding","index":0,"embedding":[0.0,0.0,1.0]}),
        json!({"object":"embedding","index":0,"embedding":[1.0,0.0,1.0]}),
    ];
    Json(json!({"object":"list","data": data, "model":"stub"}))
}

async fn spawn_stub() -> (String, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Captured::default()));
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

async fn spawn_router(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
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
    let guard = seen.lock().unwrap();
    let body = guard.bodies[0].clone();
    assert_eq!(body["model"], "text-embedding-3-small");
    assert_eq!(body["input"], json!(["alpha", "beta"]));

    // secret transport: the server received the Bearer token.
    assert_eq!(guard.auth[0], "Bearer sk-test");
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

#[tokio::test]
async fn non_2xx_includes_body_snippet() {
    let app = Router::new().route("/embeddings", post(bad_request_stub));
    let base = spawn_router(app).await;
    let e = OpenAiEmbedder::new(base, "m".into(), "sk".into(), None, None);
    match e.embed(&["x".into()]).await {
        Err(retrieval::EmbedError::Provider(msg)) => {
            assert!(msg.contains("400"), "msg should contain status: {msg}");
            assert!(
                msg.contains("bad model xyz"),
                "msg should contain body snippet: {msg}"
            );
        }
        other => panic!("expected Provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn empty_input_returns_ok_without_request() {
    // Unroutable base_url; if a request were made it would fail.
    let e = OpenAiEmbedder::new(
        "http://127.0.0.1:1".into(),
        "m".into(),
        "sk".into(),
        None,
        None,
    );
    let out = e.embed(&[]).await.unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn non_contiguous_indices_are_rejected() {
    let app = Router::new().route("/embeddings", post(duplicate_index_stub));
    let base = spawn_router(app).await;
    let e = OpenAiEmbedder::new(base, "m".into(), "sk".into(), None, None);
    assert!(matches!(
        e.embed(&["a".into(), "b".into()]).await,
        Err(retrieval::EmbedError::Provider(_))
    ));
}
