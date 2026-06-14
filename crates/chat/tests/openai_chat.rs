use std::sync::{Arc, Mutex};

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use chat::OpenAiChat;
use retrieval::{ChatError, ChatModel};
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<Value>>>;

async fn chat_stub(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
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
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(chat_stub))
        .with_state(seen.clone());
    let base = spawn(app).await;
    let c = OpenAiChat::new(base, "gpt-4o-mini".into(), "sk-x".into(), None);
    let out = c.complete("sys", "usr").await.expect("ok");
    assert_eq!(out, "[\"a__b\"]");
    let body = &seen.lock().unwrap()[0];
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "sys");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "usr");
}

#[tokio::test]
async fn non_2xx_is_provider_error() {
    async fn bad() -> (StatusCode, Json<Value>) {
        (StatusCode::BAD_REQUEST, Json(json!({"error":"bad model"})))
    }
    let base = spawn(Router::new().route("/chat/completions", post(bad))).await;
    let c = OpenAiChat::new(base, "m".into(), "k".into(), None);
    assert!(matches!(
        c.complete("s", "u").await,
        Err(ChatError::Provider(_))
    ));
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
