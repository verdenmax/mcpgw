use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use catalog::Catalog;
use clap::{Parser, Subcommand};
use config::Config;
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
            let mut strat = build_strategy(&cfg.retrieval.strategy).map_err(|e| e.to_string())?;
            strat.index(&catalog);
            let k = top_k.unwrap_or(cfg.retrieval.top_k);
            let hits = strat.search(&query, k);
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
    let state =
        Arc::new(gateway::GatewayState::new(&cfg.retrieval.strategy).map_err(|e| e.to_string())?);
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

    if !cfg.server.stdio {
        return Err("only the stdio server is supported in this build".to_string());
    }
    let (state, rx) = prepare_state(&cfg).await?;

    // list_changed-driven rebuild worker.
    tokio::spawn(gateway::run_rebuild_worker((*state).clone(), rx));

    let server = downstream::GatewayServer::new(state.clone(), cfg.retrieval.top_k);
    let service = server.serve(stdio()).await.map_err(|e| e.to_string())?;
    service.waiting().await.map_err(|e| e.to_string())?;

    // Best-effort graceful shutdown of upstream children.
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            if let Ok(h) = Arc::try_unwrap(handle) {
                h.shutdown().await;
            }
        }
    }
    Ok(())
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
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5).is_empty());
    }
}
