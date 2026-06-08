use std::path::PathBuf;
use std::process::ExitCode;

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

fn run(cli: Cli) -> Result<(), String> {
    let json = std::fs::read_to_string(&cli.catalog)
        .map_err(|e| format!("read catalog {:?}: {e}", cli.catalog))?;
    let catalog = Catalog::from_json_str(&json).map_err(|e| e.to_string())?;
    let cfg = load_config(&cli.config)?;

    match cli.command {
        Command::Search { query, top_k } => {
            let mut strat = build_strategy(&cfg.retrieval).map_err(|e| e.to_string())?;
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
        Command::GetDetails { name } => match catalog.get(&name) {
            Some(tool) => println!("{}", serde_json::to_string_pretty(tool).unwrap()),
            None => return Err(format!("no such tool: {name}")),
        },
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
