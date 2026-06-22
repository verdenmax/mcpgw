use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use catalog::Catalog;
use clap::{Parser, Subcommand};
use config::Config;
use config::UpstreamTransport;
use retrieval::{build_strategy, Backends};

/// Upper bound on how long shutdown waits for the audit writer to drain + fsync.
const AUDIT_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Upper bound on how long shutdown waits for the HTTP server to drain in-flight requests.
const HTTP_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Upper bound on how long shutdown waits for the dashboard server to drain.
const DASHBOARD_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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
            let mut strat = build_strategy(&cfg.retrieval.strategy, &Backends::default())
                .map_err(|e| e.to_string())?;
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

/// Resolve every `[[server.http.api_key]]` secret from its env var. Fail-fast on any missing
/// env, or a set-but-empty/whitespace-only value (returns the offending field/env name, never
/// the value).
fn resolve_api_keys(cfg: &config::Config) -> Result<Vec<String>, String> {
    let Some(http) = cfg.server.http.as_ref().filter(|h| h.enabled) else {
        return Ok(Vec::new());
    };
    let mut keys = Vec::with_capacity(http.api_keys.len());
    for k in &http.api_keys {
        let secret = std::env::var(&k.env)
            .map_err(|_| format!("api_key {:?}: env {:?} is not set", k.name, k.env))?;
        if secret.trim().is_empty() {
            return Err(format!(
                "api_key {:?}: env {:?} is set but empty",
                k.name, k.env
            ));
        }
        keys.push(secret);
    }
    Ok(keys)
}

/// Resolve the optional dashboard admin Bearer token from `[dashboard].admin_token_env`. Returns
/// `Ok(None)` when the dashboard is disabled or the env ref is unset; fails fast if the referenced
/// env var is missing or blank (so a misconfigured admin token surfaces at startup). Mirrors
/// `resolve_api_keys`: a secret for a switched-off subsystem isn't validated.
fn resolve_admin_token(cfg: &config::Config) -> Result<Option<Arc<str>>, String> {
    if !cfg.dashboard.enabled {
        return Ok(None);
    }
    let Some(env_name) = cfg.dashboard.admin_token_env.as_deref() else {
        return Ok(None);
    };
    let token = std::env::var(env_name)
        .map_err(|_| format!("[dashboard].admin_token_env: env {env_name:?} is not set"))?;
    if token.trim().is_empty() {
        return Err(format!(
            "[dashboard].admin_token_env: env {env_name:?} is set but empty"
        ));
    }
    Ok(Some(Arc::from(token)))
}

/// True when an HTTP server with NO api keys is bound to a non-loopback address (reachable off
/// this host) — an unauthenticated exposure worth a loud warning. A bind that doesn't parse as a
/// `SocketAddr` (a `host:port` form; DNS is not resolved here) warns conservatively, except the
/// well-known `localhost` hostname, which is loopback.
fn unauthenticated_public_bind(bind: &str, has_keys: bool) -> bool {
    if has_keys {
        return false;
    }
    match bind.parse::<std::net::SocketAddr>() {
        Ok(addr) => !addr.ip().is_loopback(),
        Err(_) => {
            // Can't prove it's loopback without DNS; treat only the literal `localhost` as safe.
            let host = bind.rsplit_once(':').map_or(bind, |(h, _)| h);
            host != "localhost"
        }
    }
}

/// Map an upstream transport variant to its short string label for the dashboard's upstream list.
fn transport_str(t: &UpstreamTransport) -> String {
    match t {
        UpstreamTransport::Stdio { .. } => "stdio".into(),
        UpstreamTransport::Http { .. } => "http".into(),
    }
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

/// Build the retrieval backends from config: an `embedder` for vector/hybrid, a `chat` model for
/// subagent, or nothing for bm25. Reads API keys from their env vars (fail-fast); the embedder is
/// wrapped in a content-hash cache shared across snapshot rebuilds.
fn build_backends(cfg: &config::Config) -> Result<retrieval::Backends, String> {
    let mut backends = retrieval::Backends::default();
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
            backends.embedder = Some(Arc::new(retrieval::CachingEmbedder::new(Arc::new(openai))));
        }
        "subagent" => {
            let s = cfg
                .retrieval
                .subagent
                .as_ref()
                .ok_or("strategy=\"subagent\" requires [retrieval.subagent]")?;
            let api_key = std::env::var(&s.api_key_env)
                .map_err(|_| format!("[retrieval.subagent]: env {:?} is not set", s.api_key_env))?;
            let openai = chat::OpenAiChat::new(
                s.base_url.clone(),
                s.model.clone(),
                api_key,
                s.timeout_ms.map(std::time::Duration::from_millis),
            );
            backends.chat = Some(Arc::new(openai));
            backends.subagent_candidates = s.candidates;
        }
        _ => {}
    }
    Ok(backends)
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
    let backends = build_backends(cfg)?;
    let disabled = Arc::new(gateway::DisableSet::load_or_new(
        cfg.dashboard
            .disabled_state_path
            .as_ref()
            .map(std::path::PathBuf::from),
    ));
    let state = Arc::new(
        gateway::GatewayState::with_backends(&cfg.retrieval.strategy, backends)
            .map_err(|e| e.to_string())?
            .with_disabled(disabled),
    );
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
    let admin_token = resolve_admin_token(&cfg)?;
    validate_upstream_http_env(&cfg)?;

    let (state, rx) = prepare_state(&cfg).await?;
    tokio::spawn(gateway::run_rebuild_worker((*state).clone(), rx));

    // Observation sinks shared by both stdio and http transports. Default = TracingSink;
    // when [audit].enabled, additionally append a JsonlSink backed by a background writer.
    let mut sink_vec: Vec<Arc<dyn observe::CallSink>> =
        vec![Arc::new(observe::TracingSink) as Arc<dyn observe::CallSink>];
    let audit_writer = if cfg.audit.enabled {
        let (sink, writer) = observe::spawn_writer(
            std::path::Path::new(&cfg.audit.path),
            observe::AUDIT_CHANNEL_CAPACITY,
        )
        .map_err(|e| format!("open audit file {:?}: {e}", cfg.audit.path))?;
        tracing::info!(path = %cfg.audit.path, "audit log enabled");
        sink_vec.push(Arc::new(sink));
        Some(writer)
    } else {
        None
    };
    // Dashboard's metrics sink (only when enabled) joins the CallSink fan-out.
    let dashboard_metrics = if cfg.dashboard.enabled {
        let m = Arc::new(dashboard::MetricsSink::new());
        sink_vec.push(m.clone() as Arc<dyn observe::CallSink>);
        Some(m)
    } else {
        None
    };
    // Per-call ring for the dashboard Calls drill-down (only when dashboard enabled). Fed via the
    // CONTENT channel (CallContentSink) so it carries args/result; bounded by [dashboard].call_buffer.
    let dashboard_calls = if cfg.dashboard.enabled {
        Some(Arc::new(dashboard::CallRingSink::new(
            cfg.dashboard.call_buffer,
        )))
    } else {
        None
    };
    let content_sinks: Arc<[Arc<dyn observe::CallContentSink>]> = match &dashboard_calls {
        Some(c) => Arc::from(vec![c.clone() as Arc<dyn observe::CallContentSink>]),
        None => Arc::from(Vec::new()),
    };
    let payload_max_bytes = cfg.dashboard.payload_max_bytes;
    let sinks: Arc<[Arc<dyn observe::CallSink>]> = sink_vec.into();

    // Opt-in discovery capture (query -> tools). Ring buffer for live; optional JSONL for history.
    let (discovery_ring, discovery_writer) = if cfg.dashboard.enabled && cfg.dashboard.trace_queries
    {
        let (ring, writer) = dashboard::DiscoveryRingSink::spawn(
            cfg.dashboard.trace_buffer,
            cfg.dashboard
                .trace_path
                .as_deref()
                .map(std::path::Path::new),
        )
        .map_err(|e| format!("open discovery trace file: {e}"))?;
        (Some(Arc::new(ring)), writer)
    } else {
        (None, None)
    };
    let discovery_sinks: Arc<[Arc<dyn observe::DiscoverySink>]> = match &discovery_ring {
        Some(r) => Arc::from(vec![r.clone() as Arc<dyn observe::DiscoverySink>]),
        None => Arc::from(Vec::new()),
    };

    // Pre-bind the HTTP listener (fail-fast on bind errors) before entering select!.
    let http_bound = if http_enabled {
        let h = cfg.server.http.as_ref().unwrap();
        let listener = tokio::net::TcpListener::bind(&h.bind)
            .await
            .map_err(|e| format!("bind {:?}: {e}", h.bind))?;
        tracing::info!(bind = %h.bind, path = %h.path, auth = !api_keys.is_empty(), "http server listening");
        if unauthenticated_public_bind(&h.bind, !api_keys.is_empty()) {
            tracing::warn!(
                bind = %h.bind,
                "HTTP server is UNAUTHENTICATED and bound to a non-loopback address; \
                 configure [[server.http.api_key]] or bind to localhost"
            );
        }
        let router = downstream::http::build_router(
            state.clone(),
            cfg.retrieval.top_k,
            &h.path,
            api_keys,
            sinks.clone(),
            discovery_sinks.clone(),
            content_sinks.clone(),
            payload_max_bytes,
        );
        Some((listener, router))
    } else {
        None
    };

    // Pre-bind the dashboard listener (fail-fast on bind errors) BEFORE spawning any serve task,
    // so a dashboard bind failure can't orphan an already-running HTTP task or skip upstream teardown
    // (symmetric to the HTTP listener pre-bind above).
    let dash_listener = if cfg.dashboard.enabled {
        let listener = tokio::net::TcpListener::bind(&cfg.dashboard.bind)
            .await
            .map_err(|e| format!("bind dashboard {:?}: {e}", cfg.dashboard.bind))?;
        tracing::info!(bind = %cfg.dashboard.bind, "dashboard listening");
        if unauthenticated_public_bind(&cfg.dashboard.bind, false) {
            tracing::warn!(
                bind = %cfg.dashboard.bind,
                "dashboard is UNAUTHENTICATED and bound to a non-loopback address; bind to localhost"
            );
        }
        Some(listener)
    } else {
        None
    };

    let stdio_enabled = cfg.server.stdio;
    let state_for_stdio = state.clone();
    let top_k = cfg.retrieval.top_k;

    // Run HTTP as a background task with graceful shutdown driven by a oneshot, so on shutdown its
    // keep-alive sessions close and release their GatewayServer/JsonlSink clones promptly (instead
    // of being orphaned and forcing the audit drain to wait out its timeout).
    let (http_shutdown_tx, http_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut http_task = http_bound.map(|(listener, router)| {
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = http_shutdown_rx.await;
                })
                .await
                .map_err(|e| e.to_string())
        })
    });

    // Run the read-only dashboard as a separate task on its own port (only when enabled), with
    // graceful shutdown driven by its own oneshot so it releases its AppState (and thereby its
    // DiscoveryRingSink clone) promptly during the shutdown sequence below.
    let (dash_shutdown_tx, dash_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let dashboard_enabled = cfg.dashboard.enabled;
    let mut dash_self_terminated = false;
    let mut dash_task = if let Some(listener) = dash_listener {
        let app_state = Arc::new(dashboard::AppState {
            gateway: state.clone(),
            metrics: dashboard_metrics
                .clone()
                .expect("metrics present when dashboard enabled"),
            discovery: discovery_ring.clone(),
            calls: dashboard_calls.clone(),
            upstreams: cfg
                .upstreams
                .iter()
                .map(|u| dashboard::UpstreamInfo {
                    name: u.name.clone(),
                    transport: transport_str(&u.transport),
                })
                .collect(),
            strategy: cfg.retrieval.strategy.clone(),
            audit_path: cfg.audit.enabled.then(|| PathBuf::from(&cfg.audit.path)),
            discovery_path: cfg.dashboard.trace_path.as_ref().map(PathBuf::from),
            started_at: std::time::Instant::now(),
            about: dashboard::AboutInfo::from_config(
                &cfg,
                dashboard::VersionInfo {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    git_sha: env!("MCPGW_GIT_SHA").to_string(),
                    build_time: env!("MCPGW_BUILD_TIME").to_string(),
                },
            ),
            admin_token: admin_token.clone(),
        });
        // Enforce a local Host header only when bound to loopback (non-loopback is an explicit,
        // already-warned operator exposure that they front themselves).
        let enforce_loopback_host = !unauthenticated_public_bind(&cfg.dashboard.bind, false);
        let router = dashboard::build_dashboard_router(app_state, enforce_loopback_host);
        Some(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = dash_shutdown_rx.await;
                })
                .await
                .map_err(|e| e.to_string())
        }))
    } else {
        None
    };

    // Wait for the first shutdown trigger: stdio client disconnect, ctrl_c, or the HTTP serve task
    // ending on its own (a serve error; axum doesn't return without a shutdown signal or error).
    let mut http_self_terminated = false;
    let outcome: Result<(), String> = tokio::select! {
        res = async {
            let server = downstream::GatewayServer::new(
                state_for_stdio,
                top_k,
                sinks.clone(),
                discovery_sinks.clone(),
                content_sinks.clone(),
                payload_max_bytes,
            );
            let service = server.serve(stdio()).await.map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())
        }, if stdio_enabled => {
            if res.is_ok() {
                tracing::info!("stdio client disconnected; shutting down");
            }
            res.map(|_| ())
        }
        res = async {
            match http_task.as_mut() {
                Some(t) => t.await.map_err(|e| e.to_string()).and_then(|r| r),
                // Unreachable: this arm is gated by `if http_enabled`, and http_task is Some
                // exactly when http_enabled. `pending()` is the safe no-op for the type-checker.
                None => std::future::pending().await,
            }
        }, if http_enabled => {
            http_self_terminated = true;
            res
        }
        _ = async {
            // The dashboard is a non-critical, read-only diagnostic subsystem. If its serve task
            // ends (only on a serve/accept error — graceful shutdown is via its own oneshot), log
            // and keep the gateway running rather than triggering a global shutdown.
            if let Some(t) = dash_task.as_mut() {
                match t.await {
                    Ok(Ok(())) => tracing::info!("dashboard server stopped"),
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "dashboard server error; gateway continues")
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "dashboard task panicked; gateway continues")
                    }
                }
                dash_self_terminated = true;
            }
            std::future::pending::<()>().await
        }, if dashboard_enabled => {
            Ok(())
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c; shutting down");
            Ok(())
        }
    };

    // Signal graceful shutdown and await the HTTP drain (bounded), UNLESS the HTTP task already
    // ended (its JoinHandle is then consumed and must not be awaited again). Draining here closes
    // keep-alive sessions and releases their sink clones before the audit drain below.
    let _ = http_shutdown_tx.send(());
    if !http_self_terminated {
        if let Some(task) = http_task {
            if tokio::time::timeout(HTTP_SHUTDOWN_TIMEOUT, task)
                .await
                .is_err()
            {
                tracing::warn!("http server graceful shutdown timed out");
            }
        }
    }

    // Signal the dashboard to drain and await it (bounded) before dropping the sinks, so its
    // AppState clone of the DiscoveryRingSink is released ahead of the discovery writer drain.
    let _ = dash_shutdown_tx.send(());
    if !dash_self_terminated {
        if let Some(task) = dash_task {
            if tokio::time::timeout(DASHBOARD_SHUTDOWN_TIMEOUT, task)
                .await
                .is_err()
            {
                tracing::warn!("dashboard graceful shutdown timed out");
            }
        }
    }

    // Drain the audit writer (if any). With the HTTP sessions now closed and the stdio server
    // dropped, `drop(sinks)` releases the last JsonlSink clone, disconnecting the channel so the
    // writer FIFO-drains, flushes, fsyncs, and exits — promptly, not at the timeout.
    drop(sinks);
    if let Some(writer) = audit_writer {
        if tokio::time::timeout(
            AUDIT_DRAIN_TIMEOUT,
            tokio::task::spawn_blocking(move || writer.join()),
        )
        .await
        .is_err()
        {
            tracing::warn!("audit writer drain timed out; some records may be unflushed");
        }
    }

    // Release every DiscoveryRingSink clone (downstream sinks dropped with `sinks`, the dashboard
    // task already joined) so the writer's channel disconnects, then drain it.
    drop(discovery_sinks);
    drop(discovery_ring);
    if let Some(writer) = discovery_writer {
        if tokio::time::timeout(
            AUDIT_DRAIN_TIMEOUT,
            tokio::task::spawn_blocking(move || writer.join()),
        )
        .await
        .is_err()
        {
            tracing::warn!("discovery writer drain timed out; some traces may be unflushed");
        }
    }

    // Graceful shutdown of upstream children (runs on clean exit AND error). If we own the only
    // reference, await a full graceful cancel; otherwise (rebuild worker / in-flight call still
    // holds a clone) cancel via the service token so the upstream is never silently left running.
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            match Arc::try_unwrap(handle) {
                Ok(h) => h.shutdown().await,
                Err(shared) => shared.cancel(),
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
    fn build_backends_empty_for_bm25() {
        let cfg = config::Config::default_from_empty();
        let b = build_backends(&cfg).unwrap();
        assert!(b.embedder.is_none() && b.chat.is_none() && b.subagent_candidates.is_none());
    }

    #[test]
    fn build_backends_fails_fast_on_missing_vector_key() {
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_NO_KEY\"\n",
        )
        .unwrap();
        assert!(build_backends(&cfg).is_err());
    }

    #[test]
    fn build_backends_embedder_for_vector_with_key() {
        std::env::set_var("MCPGW_M2_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_KEY\"\n",
        )
        .unwrap();
        let b = build_backends(&cfg).unwrap();
        assert!(b.embedder.is_some() && b.chat.is_none());
    }

    #[test]
    fn build_backends_embedder_for_hybrid_with_key() {
        std::env::set_var("MCPGW_M2B_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"hybrid\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2B_KEY\"\n",
        )
        .unwrap();
        let b = build_backends(&cfg).unwrap();
        assert!(b.embedder.is_some() && b.chat.is_none());
    }

    #[test]
    fn build_backends_chat_for_subagent_with_key() {
        std::env::set_var("MCPGW_M2T5_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"subagent\"\n[retrieval.subagent]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2T5_KEY\"\ncandidates=15\n",
        )
        .unwrap();
        let b = build_backends(&cfg).unwrap();
        assert!(b.chat.is_some() && b.embedder.is_none());
        assert_eq!(b.subagent_candidates, Some(15));
    }

    #[test]
    fn resolve_api_keys_rejects_set_but_empty_env() {
        std::env::set_var("MCPGW_AUDIT_EMPTY_KEY", "");
        let cfg = config::Config::from_toml_str(
            "[server.http]\nenabled = true\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_AUDIT_EMPTY_KEY\"\n",
        )
        .unwrap();
        let err = resolve_api_keys(&cfg).unwrap_err();
        assert!(
            err.contains("empty"),
            "error must explain the empty secret: {err}"
        );
        assert!(
            !err.contains("MCPGW_AUDIT_EMPTY_KEY="),
            "error must not leak the value"
        );
    }

    #[test]
    fn resolve_admin_token_none_when_unconfigured() {
        let cfg = config::Config::from_toml_str("").unwrap();
        assert!(resolve_admin_token(&cfg).unwrap().is_none());
    }

    #[test]
    fn resolve_admin_token_none_when_dashboard_disabled() {
        std::env::set_var("MCPGW_T8_OFF", "tok");
        // `enabled` omitted => dashboard disabled; the admin token must not be validated/required.
        let cfg =
            config::Config::from_toml_str("[dashboard]\nadmin_token_env = \"MCPGW_T8_OFF\"\n")
                .unwrap();
        assert!(resolve_admin_token(&cfg).unwrap().is_none());
    }

    #[test]
    fn resolve_admin_token_reads_env_and_fails_fast() {
        std::env::set_var("MCPGW_T8_ADMIN", "s3cr3t");
        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_ADMIN\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve_admin_token(&cfg).unwrap().as_deref(),
            Some("s3cr3t")
        );

        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_MISSING\"\n",
        )
        .unwrap();
        assert!(resolve_admin_token(&cfg).is_err());

        std::env::set_var("MCPGW_T8_EMPTY", "");
        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_EMPTY\"\n",
        )
        .unwrap();
        assert!(resolve_admin_token(&cfg).is_err(), "empty env -> fail-fast");

        std::env::set_var("MCPGW_T8_WS", "   ");
        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_WS\"\n",
        )
        .unwrap();
        assert!(
            resolve_admin_token(&cfg).is_err(),
            "whitespace-only env -> fail-fast"
        );
    }

    #[test]
    fn unauthenticated_public_bind_flags_only_public_no_key() {
        use super::unauthenticated_public_bind as f;
        assert!(f("0.0.0.0:9000", false), "public bind + no key -> warn");
        assert!(!f("0.0.0.0:9000", true), "public bind WITH key -> ok");
        assert!(!f("127.0.0.1:8970", false), "loopback v4 -> ok");
        assert!(!f("[::1]:9000", false), "loopback v6 -> ok");
        assert!(f("[::]:9000", false), "v6 all-interfaces -> warn");
        assert!(f("203.0.113.5:9000", false), "routable public IP -> warn");
        assert!(
            !f("localhost:9000", false),
            "localhost hostname is loopback -> ok"
        );
        assert!(
            f("example.com:9000", false),
            "unparseable non-localhost host + no key -> conservatively warn"
        );
    }
}
