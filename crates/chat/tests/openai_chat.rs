use std::sync::{Arc, Mutex};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use chat::OpenAiChat;
use retrieval::{ChatError, ChatModel};
use serde_json::{json, Value};

#[derive(Default)]
struct Captured {
    bodies: Vec<Value>,
    auth: Vec<String>,
}

type Seen = Arc<Mutex<Captured>>;

async fn chat_stub(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    {
        let mut g = seen.lock().unwrap();
        g.bodies.push(body.clone());
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        g.auth.push(auth);
    }
    Json(json!({"choices":[{"message":{"role":"assistant","content":"[\"a__b\"]"}}]}))
}

async fn spawn(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn returns_content_and_sends_expected_request() {
    let seen: Seen = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route("/chat/completions", post(chat_stub))
        .with_state(seen.clone());
    let base = spawn(app).await;
    let c = OpenAiChat::new(base, "gpt-4o-mini".into(), "sk-x".into(), None);
    let out = c.complete("sys", "usr").await.expect("ok");
    assert_eq!(out, "[\"a__b\"]");
    let g = seen.lock().unwrap();
    let body = &g.bodies[0];
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "sys");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "usr");
    assert_eq!(g.auth[0], "Bearer sk-x");
}

#[tokio::test]
async fn non_2xx_is_provider_error() {
    async fn bad() -> (StatusCode, Json<Value>) {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"bad model xyz"})),
        )
    }
    let base = spawn(Router::new().route("/chat/completions", post(bad))).await;
    let c = OpenAiChat::new(base, "m".into(), "k".into(), None);
    match c.complete("s", "u").await {
        Err(ChatError::Provider(msg)) => {
            assert!(msg.contains("400"), "should carry status: {msg}");
            assert!(
                msg.contains("bad model xyz"),
                "should carry body snippet: {msg}"
            );
        }
        other => panic!("expected Provider, got {other:?}"),
    }
}

#[tokio::test]
async fn empty_choices_is_empty_error() {
    async fn empty() -> Json<Value> {
        Json(json!({"choices": []}))
    }
    let base = spawn(Router::new().route("/chat/completions", post(empty))).await;
    let c = OpenAiChat::new(base, "m".into(), "k".into(), None);
    assert!(matches!(c.complete("s", "u").await, Err(ChatError::Empty)));
}

#[tokio::test]
async fn blank_content_is_empty_error() {
    // A present choice whose content is whitespace-only must collapse to Empty (the
    // `!s.trim().is_empty()` guard), not be returned as a blank "success".
    async fn blank() -> Json<Value> {
        Json(json!({"choices":[{"message":{"role":"assistant","content":"   "}}]}))
    }
    let base = spawn(Router::new().route("/chat/completions", post(blank))).await;
    let c = OpenAiChat::new(base, "m".into(), "k".into(), None);
    assert!(matches!(c.complete("s", "u").await, Err(ChatError::Empty)));
}
