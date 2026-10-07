#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up tracing / logging
    tracing_subscriber::fmt()
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

    let bind_addr = cli.bind.unwrap_or_else(|| config.server.bind.clone());
    let port = cli.port.unwrap_or(config.server.port);

    if bind_addr != "127.0.0.1" && bind_addr != "localhost" {
        eprintln!(
            "WARNING: Binding to {} exposes IssueMesh to the network. IssueMesh is intended for local use.",
            bind_addr
        );
    }

    let _app_state = AppState {
        paths: paths.clone(),
        config: Arc::new(RwLock::new(config.clone())),
        secrets: Arc::new(RwLock::new(secrets)),
        store,
    };

    println!("IssueMesh 0.1.0");
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
        println!();
        println!("Web UI:");
        println!("http://{}:{}", bind_addr, port);
    }

    Ok(())
}
