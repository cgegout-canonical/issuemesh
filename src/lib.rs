pub mod config;
pub mod connectors;
pub mod intelligence;
pub mod introspection;
pub mod jobs;
pub mod mcp;
pub mod model;
pub mod paths;
pub mod search;
pub mod store;
pub mod util;
pub mod web;

use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub paths: paths::Paths,
    pub config: Arc<RwLock<config::Config>>,
    pub secrets: Arc<RwLock<config::Secrets>>,
    pub store: store::Store,
    pub jobs: jobs::JobManager,
    pub introspection: introspection::IntrospectionScheduler,
    pub search_results:
        Arc<RwLock<std::collections::HashMap<String, search::orchestrator::DeepSearchResult>>>,
}

impl AppState {
    pub fn new(
        paths: paths::Paths,
        config: config::Config,
        secrets: config::Secrets,
        store: store::Store,
    ) -> Self {
        Self {
            paths,
            config: Arc::new(RwLock::new(config)),
            secrets: Arc::new(RwLock::new(secrets)),
            store,
            jobs: jobs::JobManager::new(),
            introspection: introspection::IntrospectionScheduler::new(),
            search_results: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
}
