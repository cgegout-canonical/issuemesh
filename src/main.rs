#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

use issuemesh::config::{Config, Secrets};
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::AppState;

#[derive(Parser, Debug)]
#[command(
    name = "issuemesh",
    version,
    about = "Local engineering issue intelligence across GitHub, Jira and Launchpad"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Port to listen on (default: 8765)
    #[arg(long)]
    port: Option<u16>,

    /// Address to bind to (default: 127.0.0.1)
    #[arg(long)]
    bind: Option<String>,

    /// Path to data/home directory (default: $ISSUEMESH_HOME or ~/.issuemesh)
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Path to config.toml file
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Run Model Context Protocol (MCP) server over standard I/O (stdio)
    Mcp,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up tracing / logging to stderr
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let paths = Paths::resolve(cli.data_dir.as_deref(), cli.config.as_deref())?;
    paths.ensure_dirs()?;

    let config = Config::load_or_default(&paths.config_file)?;
    let secrets = Secrets::load_from_file(&paths.secrets_file)?;
    let store = Store::open(&paths.db_file)?;

    let app_state = AppState::new(paths.clone(), config.clone(), secrets, store);

    // If MCP subcommand is invoked, run stdio server and exit
    if let Some(Commands::Mcp) = cli.command {
        return issuemesh::mcp::run_stdio_server(Arc::new(app_state)).await;
    }

    let bind_addr = cli.bind.unwrap_or_else(|| config.server.bind.clone());
    let port = cli.port.unwrap_or(config.server.port);

    if bind_addr != "127.0.0.1" && bind_addr != "localhost" {
        eprintln!(
            "WARNING: Binding to {} exposes IssueMesh to the network. IssueMesh is intended for local use.",
            bind_addr
        );
    }

    // Start background introspection scheduler if enabled
    app_state.introspection.start(Arc::new(app_state.clone()));

    println!("IssueMesh {}", env!("CARGO_PKG_VERSION"));
    println!();
    if !config.onboarding_completed {
        println!("First-time setup required.");
        println!();
        println!("Open:");
        println!("http://{}:{}/setup", bind_addr, port);
    } else {
        println!("Database     {}", paths.db_file.display());
        println!(
            "GitHub       {}",
            if config.github.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "Jira         {}",
            if config.jira.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "Launchpad    {}",
            if config.launchpad.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "Model        {}",
            if config.model.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!("Introspection {}", config.introspection.level_name());
        println!();
        println!("Web UI:");
        println!("http://{}:{}", bind_addr, port);
    }

    let router = issuemesh::web::build_router(app_state);
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", bind_addr, port)).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
