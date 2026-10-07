pub mod config;
pub mod connectors;
pub mod paths;
pub mod store;
pub mod util;

use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub paths: paths::Paths,
    pub config: Arc<RwLock<config::Config>>,
    pub secrets: Arc<RwLock<config::Secrets>>,
    pub store: store::Store,
}
