//! Online config edit subsystem (M5): GET/PUT the live `mcpgw.toml`, Bearer-gated (mounted on the
//! M4 admin sub-router). GET returns the current file text; PUT (Task 4) validates + persists +
//! hot-reloads upstreams.

use std::io::Write;
use std::path::Path;
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

#[derive(serde::Serialize)]
pub struct ApplyResult {
    pub upstreams: gateway::ReconcileSummary,
    pub needs_restart: Vec<&'static str>,
}

/// Non-upstream sections of `new` that differ from the boot baseline → need a restart to take effect.
fn restart_diff(boot: &config::Config, new: &config::Config) -> Vec<&'static str> {
    // Destructure new so adding a top-level Config section forces a compile error here.
    let config::Config {
        retrieval,
        upstreams: _,
        server,
        audit,
        dashboard,
    } = new;
    let mut v = Vec::new();
    if &boot.retrieval != retrieval {
        v.push("retrieval");
    }
    if &boot.server != server {
        v.push("server");
    }
    if &boot.audit != audit {
        v.push("audit");
    }
    if &boot.dashboard != dashboard {
        v.push("dashboard");
    }
    v
}

/// Best-effort atomic write: backup current to `<path>.bak`, then temp → fsync → rename.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        if let Err(e) = std::fs::copy(path, std::path::PathBuf::from(&bak)) {
            tracing::warn!(path = %path.display(), error = %e, "config .bak backup failed");
        }
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    let result = write_then_rename(&tmp, path, content);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp); // don't leak a temp on failure
    }
    result
}

fn write_then_rename(tmp: &Path, path: &Path, content: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

/// `PUT /api/admin/config` — validate (structure + env) → atomic persist(+.bak) → hot-reload
/// upstreams → report reconcile result + needs-restart sections.
pub async fn put_config(State(s): State<Arc<AppState>>, body: String) -> Response {
    let Some(path) = s.config_path.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let _guard = s.config_write_lock.lock().await; // serialize config writes

    if body.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty config body".to_string()).into_response();
    }

    let new_cfg = match (s.config_validator)(&body) {
        Ok(c) => c,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };

    let w_path = path.clone();
    let w_body = body.clone();
    let write_res = tokio::task::spawn_blocking(move || atomic_write(&w_path, &w_body))
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
    if let Err(e) = write_res {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("persist config: {e}"),
        )
            .into_response();
    }

    let old_ups = s.applied_upstreams.lock().unwrap().clone();
    let summary = s
        .gateway
        .reconcile_upstreams(&old_ups, &new_cfg.upstreams, s.rebuild_trigger.clone())
        .await;
    // Baseline = upstreams that actually (re)connected (exclude connect failures), so a re-PUT of
    // the same config retries the still-failed ones rather than treating them as "unchanged".
    let failed: std::collections::HashSet<&str> = summary
        .connect_failures
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    let applied: Vec<config::UpstreamConfig> = new_cfg
        .upstreams
        .iter()
        .filter(|u| !failed.contains(u.name.as_str()))
        .cloned()
        .collect();
    *s.applied_upstreams.lock().unwrap() = applied;

    let needs_restart = restart_diff(&s.boot_config, &new_cfg);
    Json(ApplyResult {
        upstreams: summary,
        needs_restart,
    })
    .into_response()
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

    #[tokio::test]
    async fn put_config_404_without_path() {
        let st = std::sync::Arc::new(seeded_state().await);
        let r = put_config(State(st), "[retrieval]\nstrategy=\"bm25\"\n".into()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_config_400_on_invalid_toml_does_not_write() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-bad-{}.toml", std::process::id()));
        std::fs::write(&p, "[retrieval]\nstrategy = \"bm25\"\n").unwrap();
        let mut state = seeded_state().await;
        state.config_path = Some(p.clone());
        let r = put_config(State(std::sync::Arc::new(state)), "not = = toml".into()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // file untouched
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "[retrieval]\nstrategy = \"bm25\"\n"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn put_config_persists_with_bak_and_reports_needs_restart() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-put-{}.toml", std::process::id()));
        let old = "[retrieval]\nstrategy = \"bm25\"\n";
        std::fs::write(&p, old).unwrap();
        let mut state = seeded_state().await; // boot_config = default (dashboard.enabled = false)
        state.config_path = Some(p.clone());
        let st = std::sync::Arc::new(state);

        let new = "[dashboard]\nenabled = true\n"; // differs from boot in [dashboard] -> needs restart
        let r = put_config(State(st), new.to_string()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), new); // persisted verbatim

        let mut bak = p.clone().into_os_string();
        bak.push(".bak");
        assert_eq!(
            std::fs::read_to_string(std::path::PathBuf::from(bak)).unwrap(),
            old
        ); // .bak = old

        let body = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let nr: Vec<String> = serde_json::from_value(v["needs_restart"].clone()).unwrap();
        assert!(nr.contains(&"dashboard".to_string()), "got {nr:?}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn atomic_write_creates_bak_and_leaves_no_temp() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-aw-{}.toml", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        atomic_write(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let mut bak = p.clone().into_os_string();
        bak.push(".bak");
        assert_eq!(
            std::fs::read_to_string(std::path::PathBuf::from(&bak)).unwrap(),
            "old"
        );
        // no temp left
        let mut tmp = p.clone().into_os_string();
        tmp.push(format!(".tmp.{}", std::process::id()));
        assert!(!std::path::PathBuf::from(tmp).exists());
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(std::path::PathBuf::from(bak));
    }

    #[tokio::test]
    async fn put_config_400_on_empty_body_does_not_write() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-empty-{}.toml", std::process::id()));
        std::fs::write(&p, "[retrieval]\nstrategy = \"bm25\"\n").unwrap();
        let mut state = seeded_state().await;
        state.config_path = Some(p.clone());
        let r = put_config(State(std::sync::Arc::new(state)), "   \n".into()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "[retrieval]\nstrategy = \"bm25\"\n"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn put_config_removes_upstream_via_reconcile_and_updates_baseline() {
        use crate::api::tests::{gateway_with_mock, make_state};
        let (gw, join) = gateway_with_mock("mock").await; // connected + rebuilt -> mock__echo present
        assert!(gw.snapshot().catalog().get("mock__echo").is_some());
        let mut state = make_state(gw);
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-rm-{}.toml", std::process::id()));
        std::fs::write(
            &p,
            "[[upstream]]\nname = \"mock\"\ntransport = \"stdio\"\ncommand = \"x\"\n",
        )
        .unwrap();
        state.config_path = Some(p.clone());
        *state.applied_upstreams.lock().unwrap() = config::Config::from_toml_str(
            "[[upstream]]\nname = \"mock\"\ntransport = \"stdio\"\ncommand = \"x\"\n",
        )
        .unwrap()
        .upstreams;
        let st = std::sync::Arc::new(state);
        // PUT a config with NO upstreams -> reconcile removes "mock", rebuild drops its tools.
        let r = put_config(
            State(st.clone()),
            "[retrieval]\nstrategy = \"bm25\"\n".into(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(st.gateway.snapshot().catalog().get("mock__echo").is_none());
        assert!(st.applied_upstreams.lock().unwrap().is_empty()); // baseline updated
        join.abort();
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn put_config_excludes_connect_failure_from_baseline_so_re_put_retries() {
        // A stdio upstream with an unspawnable command PASSES validation (commands aren't
        // spawn-checked there) but FAILS connect during reconcile: it lands in connect_failures and
        // is EXCLUDED from the applied baseline, so an identical re-PUT re-attempts it (symmetric
        // with boot-skipped seeding) instead of treating it as "unchanged".
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-cf-{}.toml", std::process::id()));
        std::fs::write(&p, "[retrieval]\nstrategy = \"bm25\"\n").unwrap();
        let mut state = seeded_state().await; // real GatewayState + real validator, empty baseline
        state.config_path = Some(p.clone());
        let st = std::sync::Arc::new(state);

        let body =
            "[[upstream]]\nname = \"bad\"\ntransport = \"stdio\"\ncommand = \"/nonexistent-mcpgw-bin\"\n";

        let r = put_config(State(st.clone()), body.to_string()).await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let cf: Vec<(String, String)> =
            serde_json::from_value(v["upstreams"]["connect_failures"].clone()).unwrap();
        assert!(
            cf.iter().any(|(n, _)| n == "bad"),
            "connect_failures must contain the unspawnable upstream, got {cf:?}"
        );
        // Baseline EXCLUDES the connect-failed upstream -> a re-PUT won't see it as "unchanged".
        assert!(
            st.applied_upstreams
                .lock()
                .unwrap()
                .iter()
                .all(|u| u.name != "bad"),
            "applied_upstreams must not contain the connect-failed upstream"
        );

        // Second identical PUT: "bad" is (re)attempted (still planned as added + in
        // connect_failures), proving the exclusion makes it recoverable rather than a silent no-op.
        let r2 = put_config(State(st.clone()), body.to_string()).await;
        assert_eq!(r2.status(), StatusCode::OK);
        let bytes2 = axum::body::to_bytes(r2.into_body(), usize::MAX)
            .await
            .unwrap();
        let v2: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();
        let added2: Vec<String> = serde_json::from_value(v2["upstreams"]["added"].clone()).unwrap();
        let cf2: Vec<(String, String)> =
            serde_json::from_value(v2["upstreams"]["connect_failures"].clone()).unwrap();
        assert!(
            added2.contains(&"bad".to_string()),
            "re-PUT must re-attempt the failed upstream, not treat it as unchanged"
        );
        assert!(cf2.iter().any(|(n, _)| n == "bad"));

        let mut bak = p.clone().into_os_string();
        bak.push(".bak");
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(std::path::PathBuf::from(bak));
    }

    #[tokio::test]
    async fn config_routes_are_gated() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        for method in ["GET", "PUT"] {
            let st = std::sync::Arc::new(seeded_state().await); // admin_token None
            let r = crate::build_dashboard_router(st, false)
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/api/admin/config")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                r.status(),
                StatusCode::NOT_FOUND,
                "{method} unconfigured -> 404"
            );
        }
        for method in ["GET", "PUT"] {
            let mut s = seeded_state().await;
            s.admin_token = Some(std::sync::Arc::from("sek"));
            let r = crate::build_dashboard_router(std::sync::Arc::new(s), false)
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/api/admin/config")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                r.status(),
                StatusCode::UNAUTHORIZED,
                "{method} no-bearer -> 401"
            );
        }
    }
}
