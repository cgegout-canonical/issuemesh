pub mod config;
pub mod concepts;
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
    pub concept_worker: concepts::ConceptWorkerHandle,
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
        let config_arc = Arc::new(RwLock::new(config));
        let secrets_arc = Arc::new(RwLock::new(secrets));
        let max_q = 256;
        let (concept_worker, _) = concepts::ConceptWorker::start(
            store.clone(),
            config_arc.clone(),
            secrets_arc.clone(),
            max_q,
        );

        Self {
            paths,
            config: config_arc,
            secrets: secrets_arc,
            store,
            jobs: jobs::JobManager::new(),
            introspection: introspection::IntrospectionScheduler::new(),
            concept_worker,
            search_results: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
}
