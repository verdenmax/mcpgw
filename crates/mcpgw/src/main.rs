use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use catalog::Catalog;
use clap::{Parser, Subcommand};
use config::Config;
use config::UpstreamTransport;
use retrieval::build_strategy;

/// mcpgw retrieval-core CLI: query a tool catalog with the configured strategy.
#[derive(Parser)]
#[command(name = "mcpgw", version)]
struct Cli {
    /// Path to a catalog JSON file (array of tools).
    #[arg(long, global = true, default_value = "tests/fixtures/tools.json")]
    catalog: PathBuf,
    /// Optional path to a TOML config file; defaults are used if omitted.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search for tools matching a natural-language query.
    Search {
        /// The query text.
        query: String,
        /// Override the configured top_k.
        #[arg(long)]
        top_k: Option<usize>,
    },
    /// Print the full definition of one tool by qualified name.
    GetDetails {
        /// Qualified name, e.g. "github__create_issue".
        name: String,
    },
    /// Run the live MCP gateway server (stdio): aggregate upstreams, expose the 3 meta-tools.
    Serve,
}

fn load_config(path: &Option<PathBuf>) -> Result<Config, String> {
    match path {
        None => Ok(Config::default_from_empty()),
        Some(p) => {
            let s = std::fs::read_to_string(p).map_err(|e| format!("read config {p:?}: {e}"))?;
            Config::from_toml_str(&s).map_err(|e| e.to_string())
        }
    }
}

fn load_catalog(path: &std::path::Path) -> Result<Catalog, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("read catalog {path:?}: {e}"))?;
    Catalog::from_json_str(&json).map_err(|e| e.to_string())
}

fn run(cli: Cli) -> Result<(), String> {
    let cfg = load_config(&cli.config)?;

    match cli.command {
        Command::Search { query, top_k } => {
            let catalog = load_catalog(&cli.catalog)?;
            let mut strat =
                build_strategy(&cfg.retrieval.strategy, None).map_err(|e| e.to_string())?;
            let k = top_k.unwrap_or(cfg.retrieval.top_k);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            let hits = rt.block_on(async {
                strat.index(&catalog).await;
                strat.search(&query, k).await
            });
            let out: Vec<_> = hits
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "name": h.qualified_name,
                        "description": h.description,
                        "score": h.score,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        Command::GetDetails { name } => {
            let catalog = load_catalog(&cli.catalog)?;
            match catalog.get(&name) {
                Some(tool) => println!("{}", serde_json::to_string_pretty(tool).unwrap()),
                None => return Err(format!("no such tool: {name}")),
            }
        }
        Command::Serve => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(run_serve(cfg))?;
        }
    }
    Ok(())
}

/// Resolve every `[[server.http.api_key]]` secret from its env var. Fail-fast on any
/// missing env (returns the offending field/env name, never the value).
fn resolve_api_keys(cfg: &config::Config) -> Result<Vec<String>, String> {
    let Some(http) = cfg.server.http.as_ref().filter(|h| h.enabled) else {
        return Ok(Vec::new());
    };
    let mut keys = Vec::with_capacity(http.api_keys.len());
    for k in &http.api_keys {
        let secret = std::env::var(&k.env)
            .map_err(|_| format!("api_key {:?}: env {:?} is not set", k.name, k.env))?;
        keys.push(secret);
    }
    Ok(keys)
}

/// Verify every env referenced by an HTTP upstream (bearer + headers) is present, so a
/// missing credential fails startup rather than silently degrading to a 401 loop.
fn validate_upstream_http_env(cfg: &config::Config) -> Result<(), String> {
    for u in &cfg.upstreams {
        if let UpstreamTransport::Http {
            bearer_env,
            headers,
            ..
        } = &u.transport
        {
            if let Some(env_name) = bearer_env {
                if std::env::var(env_name).is_err() {
                    return Err(format!(
                        "upstream {:?}: bearer_env {:?} is not set",
                        u.name, env_name
                    ));
                }
            }
            for (hname, env_name) in headers {
                if std::env::var(env_name).is_err() {
                    return Err(format!(
                        "upstream {:?}: header {:?} env {:?} is not set",
                        u.name, hname, env_name
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Build the retrieval embedder from config (vector/hybrid). Returns `None` for bm25.
/// Reads the API key from its env var (fail-fast) and wraps the provider in a content-hash
/// cache shared across snapshot rebuilds.
fn build_embedder(cfg: &config::Config) -> Result<Option<Arc<dyn retrieval::Embedder>>, String> {
    match cfg.retrieval.strategy.as_str() {
        "vector" | "hybrid" => {
            let v = cfg.retrieval.vector.as_ref().ok_or_else(|| {
                format!(
                    "strategy={:?} requires [retrieval.vector]",
                    cfg.retrieval.strategy
                )
            })?;
            let api_key = std::env::var(&v.api_key_env)
                .map_err(|_| format!("[retrieval.vector]: env {:?} is not set", v.api_key_env))?;
            let openai = embedder::OpenAiEmbedder::new(
                v.base_url.clone(),
                v.model.clone(),
                api_key,
                v.dim,
                v.timeout_ms.map(std::time::Duration::from_millis),
            );
            Ok(Some(Arc::new(retrieval::CachingEmbedder::new(Arc::new(
                openai,
            )))))
        }
        _ => Ok(None),
    }
}

/// Build gateway state, connect upstreams, build the initial snapshot, and return the
/// state plus the rebuild-trigger receiver for the worker. Split out so it is unit-testable.
async fn prepare_state(
    cfg: &config::Config,
) -> Result<
    (
        Arc<gateway::GatewayState>,
        tokio::sync::mpsc::Receiver<String>,
    ),
    String,
> {
    let state = match build_embedder(cfg)? {
        Some(embedder) => Arc::new(
            gateway::GatewayState::with_embedder(&cfg.retrieval.strategy, embedder)
                .map_err(|e| e.to_string())?,
        ),
        None => Arc::new(
            gateway::GatewayState::new(&cfg.retrieval.strategy).map_err(|e| e.to_string())?,
        ),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let csum = upstream::connect::connect_all(state.registry(), &cfg.upstreams, tx).await;
    tracing::info!(connected = ?csum.connected, skipped = ?csum.skipped, "upstreams connected");
    let rsum = state.rebuild_snapshot().await.map_err(|e| e.to_string())?;
    tracing::info!(ingested = ?rsum.ingested, skipped = ?rsum.skipped, "initial snapshot built");
    Ok((state, rx))
}

async fn run_serve(cfg: config::Config) -> Result<(), String> {
    use rmcp::transport::stdio;
    use rmcp::ServiceExt;

    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let http_enabled = cfg.server.http.as_ref().is_some_and(|h| h.enabled);
    if !cfg.server.stdio && !http_enabled {
        return Err(
            "no server transport enabled (set [server].stdio or [server.http].enabled)".into(),
        );
    }

    // Fail-fast: resolve/verify every env-referenced secret before connecting anything.
    let api_keys = resolve_api_keys(&cfg)?;
    validate_upstream_http_env(&cfg)?;

    let (state, rx) = prepare_state(&cfg).await?;
    tokio::spawn(gateway::run_rebuild_worker((*state).clone(), rx));

    // Pre-bind the HTTP listener (fail-fast on bind errors) before entering select!.
    let http_bound = if http_enabled {
        let h = cfg.server.http.as_ref().unwrap();
        let listener = tokio::net::TcpListener::bind(&h.bind)
            .await
            .map_err(|e| format!("bind {:?}: {e}", h.bind))?;
        tracing::info!(bind = %h.bind, path = %h.path, auth = !api_keys.is_empty(), "http server listening");
        let router =
            downstream::http::build_router(state.clone(), cfg.retrieval.top_k, &h.path, api_keys);
        Some((listener, router))
    } else {
        None
    };

    let stdio_enabled = cfg.server.stdio;
    let state_for_stdio = state.clone();
    let top_k = cfg.retrieval.top_k;

    let outcome: Result<(), String> = tokio::select! {
        res = async {
            let server = downstream::GatewayServer::new(state_for_stdio, top_k);
            let service = server.serve(stdio()).await.map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())
        }, if stdio_enabled => {
            if res.is_ok() {
                tracing::info!("stdio client disconnected; shutting down");
            }
            res.map(|_| ())
        }
        res = async {
            let (listener, router) = http_bound.unwrap();
            axum::serve(listener, router).await.map_err(|e| e.to_string())
        }, if http_enabled => {
            res
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c; shutting down");
            Ok(())
        }
    };

    // Best-effort graceful shutdown of upstream children (runs on clean exit AND error).
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            if let Ok(h) = Arc::try_unwrap(handle) {
                h.shutdown().await;
            }
        }
    }
    outcome
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_parses_serve_subcommand() {
        let cli = Cli::try_parse_from(["mcpgw", "serve"]).unwrap();
        assert!(matches!(cli.command, Command::Serve));
    }

    #[tokio::test]
    async fn run_serve_builds_initial_snapshot_with_no_upstreams() {
        // Empty config: 0 upstreams, stdio default on. prepare_state must succeed and yield
        // a usable (empty) snapshot.
        let cfg = config::Config::default_from_empty();
        let (state, _rx) = prepare_state(&cfg).await.expect("prepare ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }

    #[test]
    fn resolve_api_keys_reads_env_and_fails_fast_on_missing() {
        std::env::set_var("MCPGW_T5_KEY", "abc");
        let cfg = config::Config::from_toml_str(
            "[server.http]\nenabled = true\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_T5_KEY\"\n",
        )
        .unwrap();
        assert_eq!(resolve_api_keys(&cfg).unwrap(), vec!["abc".to_string()]);

        let cfg = config::Config::from_toml_str(
            "[server.http]\nenabled = true\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_T5_MISSING\"\n",
        )
        .unwrap();
        assert!(resolve_api_keys(&cfg).is_err());
    }

    #[test]
    fn resolve_api_keys_empty_when_no_http() {
        let cfg = config::Config::default_from_empty();
        assert!(resolve_api_keys(&cfg).unwrap().is_empty());
    }

    #[test]
    fn resolve_api_keys_empty_when_http_disabled_even_with_keys() {
        let cfg = config::Config::from_toml_str(
            "[server.http]\nenabled = false\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_CLEANUP_MISSING\"\n",
        )
        .unwrap();
        assert!(resolve_api_keys(&cfg).unwrap().is_empty());
    }

    #[test]
    fn validate_upstream_http_env_fails_fast_on_missing_bearer() {
        let cfg = config::Config::from_toml_str(
            "[[upstream]]\nname=\"r\"\ntransport=\"http\"\nurl=\"http://x/mcp\"\nbearer_env=\"MCPGW_T5_NO_SUCH\"\n",
        )
        .unwrap();
        assert!(validate_upstream_http_env(&cfg).is_err());
    }

    #[test]
    fn build_embedder_none_for_bm25() {
        let cfg = config::Config::default_from_empty();
        assert!(build_embedder(&cfg).unwrap().is_none());
    }

    #[test]
    fn build_embedder_fails_fast_on_missing_key() {
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_NO_KEY\"\n",
        )
        .unwrap();
        assert!(build_embedder(&cfg).is_err());
    }

    #[test]
    fn build_embedder_some_for_vector_with_key() {
        std::env::set_var("MCPGW_M2_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_KEY\"\n",
        )
        .unwrap();
        assert!(build_embedder(&cfg).unwrap().is_some());
    }

    #[test]
    fn build_embedder_some_for_hybrid_with_key() {
        std::env::set_var("MCPGW_M2B_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"hybrid\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2B_KEY\"\n",
        )
        .unwrap();
        assert!(build_embedder(&cfg).unwrap().is_some());
    }
}
