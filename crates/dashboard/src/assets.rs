use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The built Svelte UI (`ui/dist/`), embedded into the binary at compile time. `dist/` is committed,
/// so `cargo build` needs no node toolchain.
#[derive(RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

/// Serve an embedded UI asset by request path: `/` -> `index.html`, `/assets/x` -> that asset.
/// Unknown paths fall back to `index.html` (harmless — the SPA uses hash routing, so real requests
/// are only `/` and `/assets/*`; this just makes a stray deep-link refresh load the app).
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(content) => (
            [(
                header::CONTENT_TYPE,
                content.metadata.mimetype().to_string(),
            )],
            content.data,
        )
            .into_response(),
        None => match Assets::get("index.html") {
            Some(index) => ([(header::CONTENT_TYPE, "text/html")], index.data).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_index_present_and_has_mount_point() {
        let idx = Assets::get("index.html").expect("ui/dist/index.html embedded");
        let html = std::str::from_utf8(idx.data.as_ref()).unwrap();
        assert!(
            html.contains("id=\"app\""),
            "index.html has the Svelte mount point"
        );
    }

    #[test]
    fn embedded_dist_has_a_js_asset() {
        let has_js = Assets::iter().any(|p| p.starts_with("assets/") && p.ends_with(".js"));
        assert!(has_js, "a hashed JS asset is embedded under assets/");
    }

    // Security guard (replaces the deleted app.js escape test): Svelte auto-escapes `{expr}`, but
    // `{@html ...}` bypasses it. Forbid `{@html}` across the UI source so untrusted /api fields
    // (query text, tool/upstream names, error reasons) can never be injected as raw HTML.
    #[test]
    fn no_svelte_component_uses_raw_html() {
        fn scan(dir: &std::path::Path, offenders: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, offenders);
                } else if p.extension().is_some_and(|x| x == "svelte") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        if s.contains("{@html") {
                            offenders.push(p.display().to_string());
                        }
                    }
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/src");
        let mut offenders = Vec::new();
        scan(&dir, &mut offenders);
        assert!(
            offenders.is_empty(),
            "no {{@html}} allowed in UI source (XSS risk): {offenders:?}"
        );
    }
}
