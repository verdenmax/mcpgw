//! Online config edit subsystem (M5): GET/PUT the live `mcpgw.toml`, Bearer-gated (mounted on the
//! M4 admin sub-router). GET returns the current file text; PUT (Task 4) validates + persists +
//! hot-reloads upstreams.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::api::AppState;

#[derive(Serialize)]
pub struct ConfigView {
    pub path: String,
    pub content: String,
}

/// `GET /api/admin/config` — current config file text. 404 when serve was started without `--config`.
pub async fn get_config(State(s): State<Arc<AppState>>) -> Response {
    let Some(path) = s.config_path.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match std::fs::read_to_string(path) {
        Ok(content) => Json(ConfigView {
            path: path.display().to_string(),
            content,
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read config: {e}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::seeded_state;

    #[tokio::test]
    async fn get_config_404_without_path() {
        let st = std::sync::Arc::new(seeded_state().await); // config_path: None
        let r = get_config(State(st)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_config_returns_file_content() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-get-{}.toml", std::process::id()));
        std::fs::write(&p, "[retrieval]\nstrategy = \"bm25\"\n").unwrap();
        let mut state = seeded_state().await;
        state.config_path = Some(p.clone());
        let r = get_config(State(std::sync::Arc::new(state))).await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["content"], "[retrieval]\nstrategy = \"bm25\"\n");
        let _ = std::fs::remove_file(&p);
    }
}
