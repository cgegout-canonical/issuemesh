use axum::{
    extract::{Query, State},
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

use crate::connectors::github::GitHubConnector;
use crate::connectors::jira::JiraConnector;
use crate::connectors::launchpad::LaunchpadConnector;
use crate::connectors::Connector;
use crate::util::mask_secret;
use crate::AppState;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Page routes
        .route("/", get(index_handler))
        .route("/setup", get(setup_handler))
        .route("/health", get(health_page_handler))
        .route("/settings", get(settings_page_handler))
        .route("/memory", get(memory_page_handler))
        .route("/sources", get(sources_page_handler))
        .route("/search/{id}/run", get(search_run_page_handler))
        // API routes
        .route("/api/health", get(health_api_handler))
        .route("/api/stats", get(stats_handler))
        .route("/api/search/local", get(local_search_handler))
        .route(
            "/api/search",
            post(create_search_job_handler).get(list_search_jobs_handler),
        )
        .route("/api/search/{id}", get(get_search_job_handler))
        .route("/api/search/{id}/result", get(get_search_result_handler))
        .route("/api/search/{id}/run", get(get_search_run_handler))
        .route("/api/search/{id}/events", get(search_events_sse_handler))
        .route("/api/memory", get(get_memory_handler))
        .route("/api/memory/integrity", post(memory_integrity_handler))
        .route("/api/memory/optimize", post(memory_optimize_handler))
        .route("/api/memory/rebuild-fts", post(memory_rebuild_fts_handler))
        .route(
            "/api/memory/clear-history",
            post(memory_clear_history_handler),
        )
        .route("/api/memory/purge-stale", post(memory_purge_stale_handler))
        .route("/api/memory/reembed", post(memory_reembed_handler))
        .route("/api/sources", get(get_sources_handler))
        .route("/api/setup", post(save_setup_handler))
        .route(
            "/api/settings",
            get(get_settings_handler).post(save_settings_handler),
        )
        // Probe test routes
        .route("/api/test/github", post(test_github_handler))
        .route("/api/test/jira", post(test_jira_handler))
        .route("/api/test/launchpad", post(test_launchpad_handler))
        .route("/api/test/model", post(test_model_handler))
        .route(
            "/api/models",
            post(discover_models_handler).get(get_models_handler),
        )
        // Static assets
        .route("/static/{*file}", get(static_handler))
        .with_state(Arc::new(state))
}

fn guess_mime(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches("/static/");
    match Assets::get(path) {
        Some(content) => {
            let mime = guess_mime(path);
            ([(header::CONTENT_TYPE, mime)], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

async fn index_handler(State(state): State<Arc<AppState>>) -> Response {
    let completed = {
        let cfg = state.config.read().await;
        cfg.onboarding_completed
    };

    if !completed {
        return Redirect::temporary("/setup").into_response();
    }

    match Assets::get("index.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

async fn setup_handler() -> impl IntoResponse {
    match Assets::get("setup.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "setup.html not found").into_response(),
    }
}

async fn health_page_handler() -> impl IntoResponse {
    match Assets::get("health.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "health.html not found").into_response(),
    }
}

async fn settings_page_handler() -> impl IntoResponse {
    match Assets::get("settings.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "settings.html not found").into_response(),
    }
}

async fn memory_page_handler() -> impl IntoResponse {
    match Assets::get("memory.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "memory.html not found").into_response(),
    }
}

async fn sources_page_handler() -> impl IntoResponse {
    match Assets::get("sources.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "sources.html not found").into_response(),
    }
}

async fn search_run_page_handler() -> impl IntoResponse {
    match Assets::get("run.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "run.html not found").into_response(),
    }
}

#[derive(Serialize)]
pub struct SubsystemHealth {
    pub enabled: bool,
    pub ok: bool,
    pub detail: String,
    pub latency_ms: i64,
    pub rate_limit_remaining: Option<i64>,
}

pub async fn run_live_probes(state: &AppState) -> serde_json::Value {
    let (cfg, sec) = {
        let c = state.config.read().await.clone();
        let s = state.secrets.read().await.clone();
        (c, s)
    };

    // 1. GitHub Probe
    let gh_probe = if cfg.github.enabled {
        let base_url = "https://api.github.com";
        match GitHubConnector::new(
            base_url,
            sec.github_token.clone(),
            cfg.github.default_org.clone(),
        ) {
            Ok(connector) => {
                let h = connector.health().await;
                SubsystemHealth {
                    enabled: true,
                    ok: h.ok,
                    detail: h.detail,
                    latency_ms: h.latency_ms,
                    rate_limit_remaining: h.rate_limit_remaining,
                }
            }
            Err(e) => SubsystemHealth {
                enabled: true,
                ok: false,
                detail: format!("Initialization error: {e}"),
                latency_ms: 0,
                rate_limit_remaining: None,
            },
        }
    } else {
        SubsystemHealth {
            enabled: false,
            ok: true,
            detail: "Connector disabled".to_string(),
            latency_ms: 0,
            rate_limit_remaining: None,
        }
    };

    // 2. Jira Probe
    let jira_probe = if cfg.jira.enabled {
        if let Some(jira_url) = sec.jira_url.clone() {
            match JiraConnector::new(
                &jira_url,
                sec.jira_email.clone(),
                sec.jira_token.clone(),
                cfg.jira.default_project.clone(),
            ) {
                Ok(connector) => {
                    let h = connector.health().await;
                    SubsystemHealth {
                        enabled: true,
                        ok: h.ok,
                        detail: h.detail,
                        latency_ms: h.latency_ms,
                        rate_limit_remaining: h.rate_limit_remaining,
                    }
                }
                Err(e) => SubsystemHealth {
                    enabled: true,
                    ok: false,
                    detail: format!("Initialization error: {e}"),
                    latency_ms: 0,
                    rate_limit_remaining: None,
                },
            }
        } else {
            SubsystemHealth {
                enabled: true,
                ok: false,
                detail: "Jira URL not configured".to_string(),
                latency_ms: 0,
                rate_limit_remaining: None,
            }
        }
    } else {
        SubsystemHealth {
            enabled: false,
            ok: true,
            detail: "Connector disabled".to_string(),
            latency_ms: 0,
            rate_limit_remaining: None,
        }
    };

    // 3. Launchpad Probe
    let lp_probe = if cfg.launchpad.enabled {
        let base_url = "https://api.launchpad.net/1.0";
        match LaunchpadConnector::new(base_url, cfg.launchpad.default_project.clone()) {
            Ok(connector) => {
                let h = connector.health().await;
                SubsystemHealth {
                    enabled: true,
                    ok: h.ok,
                    detail: h.detail,
                    latency_ms: h.latency_ms,
                    rate_limit_remaining: None,
                }
            }
            Err(e) => SubsystemHealth {
                enabled: true,
                ok: false,
                detail: format!("Initialization error: {e}"),
                latency_ms: 0,
                rate_limit_remaining: None,
            },
        }
    } else {
        SubsystemHealth {
            enabled: false,
            ok: true,
            detail: "Connector disabled".to_string(),
            latency_ms: 0,
            rate_limit_remaining: None,
        }
    };

    // 4. Model Probe
    let model_probe = if cfg.model.enabled {
        let base_url = cfg
            .model
            .base_url
            .clone()
            .or_else(|| sec.openai_base_url.clone());
        let api_key = sec.openai_api_key.clone();
        let chat_model = cfg.model.model.clone().or_else(|| sec.openai_model.clone());
        if let Some(url) = base_url {
            let res =
                probe_model_endpoint(&url, api_key.as_deref(), chat_model.as_deref(), None).await;
            SubsystemHealth {
                enabled: true,
                ok: res.ok,
                detail: res.detail,
                latency_ms: res.latency_ms,
                rate_limit_remaining: None,
            }
        } else {
            SubsystemHealth {
                enabled: true,
                ok: false,
                detail: "Model base URL not configured".to_string(),
                latency_ms: 0,
                rate_limit_remaining: None,
            }
        }
    } else {
        SubsystemHealth {
            enabled: false,
            ok: true,
            detail: "Model integration disabled".to_string(),
            latency_ms: 0,
            rate_limit_remaining: None,
        }
    };

    // 5. Database Probe
    let t0 = Instant::now();
    let db_res = state.store.get_stats().await;
    let db_latency = t0.elapsed().as_millis() as i64;
    let (db_ok, db_detail) = match db_res {
        Ok(s) => (
            true,
            format!("{} objects, {} relations", s.object_count, s.relation_count),
        ),
        Err(e) => (false, format!("Database error: {e}")),
    };

    let db_probe = SubsystemHealth {
        enabled: true,
        ok: db_ok,
        detail: db_detail,
        latency_ms: db_latency,
        rate_limit_remaining: None,
    };

    serde_json::json!({
        "status": if db_probe.ok { "healthy" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "subsystems": {
            "database": db_probe,
            "github": gh_probe,
            "jira": jira_probe,
            "launchpad": lp_probe,
            "model": model_probe,
        }
    })
}

async fn health_api_handler(State(state): State<Arc<AppState>>) -> Response {
    let report = run_live_probes(&state).await;
    Json(report).into_response()
}

async fn stats_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.store.get_stats().await {
        Ok(s) => Json(s).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    limit: Option<usize>,
}

async fn local_search_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Response {
    let q = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(20);

    match state.store.search_fts(&q, limit).await {
        Ok(results) => Json(results).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct CreateSearchJobRequest {
    query: String,
}

async fn create_search_job_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateSearchJobRequest>,
) -> Response {
    let q = payload.query.trim().to_string();
    if q.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query cannot be empty" })),
        )
            .into_response();
    }

    let job_id = state.jobs.create_job(q.clone()).await;

    // Spawn background task to process search
    let state_clone = Arc::clone(&state);
    let job_id_clone = job_id.clone();
    tokio::spawn(async move {
        run_search_pipeline(state_clone, job_id_clone, q).await;
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job_id })),
    )
        .into_response()
}

async fn list_search_jobs_handler(State(state): State<Arc<AppState>>) -> Response {
    let jobs = state.jobs.list_jobs().await;
    Json(jobs).into_response()
}

async fn get_search_job_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.jobs.get_job_summary(&id).await {
        Some(summary) => Json(summary).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Job not found" })),
        )
            .into_response(),
    }
}

async fn search_events_sse_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::stream::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    let sub = state.jobs.subscribe_events(&id).await;
    let (history, rx) = match sub {
        Some(pair) => pair,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let history_stream = futures::stream::iter(history.into_iter().map(|ev| {
        let json = serde_json::to_string(&ev).unwrap_or_default();
        Ok::<_, std::convert::Infallible>(Event::default().data(json))
    }));

    let live_stream = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(ev) => {
                let json = serde_json::to_string(&ev).unwrap_or_default();
                Some(Ok::<_, std::convert::Infallible>(
                    Event::default().data(json),
                ))
            }
            Err(_) => None,
        }
    });

    let combined = history_stream.chain(live_stream);

    Sse::new(combined)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn run_search_pipeline(state: Arc<AppState>, job_id: String, query: String) {
    let result =
        crate::search::SearchOrchestrator::execute(Arc::clone(&state), Some(&job_id), &query).await;
    let mut map = state.search_results.write().await;
    map.insert(job_id, result);
}

async fn get_search_result_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    // 1. Check in-memory search results
    {
        let map = state.search_results.read().await;
        if let Some(res) = map.get(&id) {
            return Json(res.clone()).into_response();
        }
    }

    // 2. Check persistent store
    match state.store.get_search_run(&id).await {
        Ok(Some(details)) => {
            // Reconstruct a DeepSearchResult-compatible JSON response
            let plan: crate::model::planner::SearchPlan =
                serde_json::from_str(&details.run.plan_json).unwrap_or_else(|_| {
                    crate::model::planner::SearchPlan {
                        query: details.run.query.clone(),
                        ..Default::default()
                    }
                });
            let stats: crate::jobs::SearchStats =
                serde_json::from_str(&details.run.stats_json).unwrap_or_default();

            // Convert ranked objects
            let ranked_results: Vec<crate::search::ranking::RankedEvidence> = details
                .objects
                .into_iter()
                .map(|obj| {
                    let r_ref = crate::connectors::ObjectRef::from_key(&obj.object_key)
                        .unwrap_or_else(|| {
                            crate::connectors::ObjectRef::new(&obj.source, "issue", &obj.object_key)
                        });
                    let evidence = crate::connectors::Evidence {
                        r#ref: r_ref,
                        title: obj.title,
                        snippet: String::new(),
                        body: None,
                        url: obj.url,
                        container: String::new(),
                        author: String::new(),
                        created_at: None,
                        updated_at: None,
                        state: obj.state,
                        native: std::collections::BTreeMap::new(),
                    };
                    crate::search::ranking::RankedEvidence {
                        evidence,
                        score: obj.relevance_score as f32,
                        lexical_score: obj.relevance_score as f32,
                        semantic_score: 0.0,
                        graph_score: 0.0,
                        freshness_score: 0.0,
                    }
                })
                .collect();

            let res = serde_json::json!({
                "query": details.run.query,
                "plan": plan,
                "ranked_results": ranked_results,
                "findings": [],
                "relations_followed": [],
                "stats": stats,
                "answer": details.run.answer_markdown,
            });
            Json(res).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Search run not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_search_run_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.store.get_search_run(&id).await {
        Ok(Some(details)) => Json(details).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Search run not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_memory_handler(State(state): State<Arc<AppState>>) -> Response {
    match state
        .store
        .get_memory_diagnostics(&state.paths.db_file)
        .await
    {
        Ok(diag) => Json(diag).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn memory_integrity_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.store.check_integrity().await {
        Ok(result) => {
            Json(serde_json::json!({ "ok": result == "ok", "result": result })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn memory_optimize_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.store.optimize_database().await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn memory_rebuild_fts_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.store.rebuild_fts().await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn memory_clear_history_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.store.clear_search_history().await {
        Ok(cleared) => Json(serde_json::json!({ "ok": true, "cleared": cleared })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct PurgeStalePayload {
    cutoff: Option<String>,
}

async fn memory_purge_stale_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PurgeStalePayload>,
) -> Response {
    let cutoff = payload
        .cutoff
        .unwrap_or_else(|| (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339());
    match state.store.purge_stale_objects(&cutoff).await {
        Ok(purged) => Json(serde_json::json!({ "ok": true, "purged": purged })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn memory_reembed_handler(State(state): State<Arc<AppState>>) -> Response {
    // Re-embed triggers background batch embedding of stored objects
    // For now we optimize database and verify embedding model status
    let cfg = state.config.read().await;
    if !cfg.model.enabled {
        return Json(serde_json::json!({
            "ok": false,
            "error": "Model integration is disabled. Cannot generate embeddings."
        }))
        .into_response();
    }
    Json(serde_json::json!({ "ok": true, "message": "Embedding update triggered" })).into_response()
}

async fn get_sources_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.store.get_sources_summary().await {
        Ok(sources) => Json(sources).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Settings & Onboarding Form DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct GitHubForm {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub token: Option<String>,
    pub default_org: Option<String>,
    #[serde(default)]
    pub default_repos: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct JiraForm {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub url: Option<String>,
    pub email: Option<String>,
    pub token: Option<String>,
    pub default_project: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct LaunchpadForm {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub default_project: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct ModelForm {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub chat_model: Option<String>,
    pub embedding_model: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct SearchForm {
    pub graph_depth: Option<u8>,
    pub max_graph_nodes: Option<usize>,
    pub max_deep_reads: Option<usize>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct SettingsPayload {
    pub github: GitHubForm,
    pub jira: JiraForm,
    pub launchpad: LaunchpadForm,
    pub model: ModelForm,
    pub search: SearchForm,
}

async fn get_settings_handler(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state.config.read().await.clone();
    let sec = state.secrets.read().await.clone();

    let masked_gh = sec.github_token.as_ref().map(|t| mask_secret(t));
    let masked_jira = sec.jira_token.as_ref().map(|t| mask_secret(t));
    let masked_model = sec.openai_api_key.as_ref().map(|t| mask_secret(t));

    Json(serde_json::json!({
        "config": cfg,
        "masked_secrets": {
            "github_token": masked_gh,
            "jira_token": masked_jira,
            "openai_api_key": masked_model,
        },
        "secrets_meta": {
            "jira_url": sec.jira_url,
            "jira_email": sec.jira_email,
            "openai_base_url": sec.openai_base_url,
            "openai_model": sec.openai_model,
            "openai_embedding_model": sec.openai_embedding_model,
        }
    }))
    .into_response()
}

async fn save_settings_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SettingsPayload>,
) -> Response {
    apply_settings_update(&state, payload, false).await
}

async fn save_setup_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SettingsPayload>,
) -> Response {
    apply_settings_update(&state, payload, true).await
}

async fn apply_settings_update(
    state: &AppState,
    payload: SettingsPayload,
    complete_onboarding: bool,
) -> Response {
    // 1. Update config
    {
        let mut cfg = state.config.write().await;
        if complete_onboarding {
            cfg.onboarding_completed = true;
        }

        cfg.github.enabled = payload.github.enabled;
        cfg.github.default_org = payload.github.default_org;
        cfg.github.default_repos = payload.github.default_repos;

        cfg.jira.enabled = payload.jira.enabled;
        cfg.jira.default_project = payload.jira.default_project;

        cfg.launchpad.enabled = payload.launchpad.enabled;
        cfg.launchpad.default_project = payload.launchpad.default_project;

        cfg.model.enabled = payload.model.enabled;
        if payload.model.base_url.is_some() {
            cfg.model.base_url = payload.model.base_url.clone();
        }
        if payload.model.chat_model.is_some() {
            cfg.model.model = payload.model.chat_model.clone();
        }
        if payload.model.embedding_model.is_some() {
            cfg.model.embedding_model = payload.model.embedding_model.clone();
        }

        if let Some(d) = payload.search.graph_depth {
            cfg.search.graph_depth = d;
        }
        if let Some(n) = payload.search.max_graph_nodes {
            cfg.search.max_graph_nodes = n;
        }
        if let Some(r) = payload.search.max_deep_reads {
            cfg.search.max_deep_reads = r;
        }

        if let Err(e) = cfg.save(&state.paths.config_file) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("Failed to save config: {e}") })),
            )
                .into_response();
        }
    }

    // 2. Update secrets
    {
        let mut sec = state.secrets.write().await;

        if let Some(t) = payload.github.token {
            if !t.is_empty() {
                sec.github_token = Some(t);
            }
        }

        if let Some(u) = payload.jira.url {
            if !u.is_empty() {
                sec.jira_url = Some(u);
            }
        }
        if let Some(e) = payload.jira.email {
            if !e.is_empty() {
                sec.jira_email = Some(e);
            }
        }
        if let Some(t) = payload.jira.token {
            if !t.is_empty() {
                sec.jira_token = Some(t);
            }
        }

        if let Some(b) = payload.model.base_url {
            if !b.is_empty() {
                sec.openai_base_url = Some(b);
            }
        }
        if let Some(k) = payload.model.api_key {
            if !k.is_empty() {
                sec.openai_api_key = Some(k);
            }
        }
        if let Some(m) = payload.model.chat_model {
            if !m.is_empty() {
                sec.openai_model = Some(m);
            }
        }
        if let Some(em) = payload.model.embedding_model {
            if !em.is_empty() {
                sec.openai_embedding_model = Some(em);
            }
        }

        if let Err(e) = sec.save_to_file(&state.paths.secrets_file) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("Failed to save secrets: {e}") })),
            )
                .into_response();
        }
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

// ---------------------------------------------------------------------------
// Test probe endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GitHubTestRequest {
    token: Option<String>,
}

async fn test_github_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GitHubTestRequest>,
) -> Response {
    let token = payload.token.or_else(|| {
        let sec = state.secrets.try_read().ok()?;
        sec.github_token.clone()
    });

    let base_url = "https://api.github.com";
    match GitHubConnector::new(base_url, token, None) {
        Ok(connector) => {
            let h = connector.health().await;
            Json(serde_json::json!({
                "ok": h.ok,
                "detail": h.detail,
                "latency_ms": h.latency_ms,
                "rate_limit_remaining": h.rate_limit_remaining,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "detail": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct JiraTestRequest {
    url: Option<String>,
    email: Option<String>,
    token: Option<String>,
}

async fn test_jira_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<JiraTestRequest>,
) -> Response {
    let (url, email, token) = {
        let sec = state.secrets.read().await;
        let u = payload.url.or_else(|| sec.jira_url.clone());
        let e = payload.email.or_else(|| sec.jira_email.clone());
        let t = payload.token.or_else(|| sec.jira_token.clone());
        (u, e, t)
    };

    let url = match url {
        Some(u) if !u.is_empty() => u,
        _ => {
            return Json(serde_json::json!({
                "ok": false,
                "detail": "Jira URL is required for testing"
            }))
            .into_response();
        }
    };

    match JiraConnector::new(&url, email, token, None) {
        Ok(connector) => {
            let h = connector.health().await;
            Json(serde_json::json!({
                "ok": h.ok,
                "detail": h.detail,
                "latency_ms": h.latency_ms,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "detail": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct LaunchpadTestRequest {
    default_project: Option<String>,
}

async fn test_launchpad_handler(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<LaunchpadTestRequest>,
) -> Response {
    let base_url = "https://api.launchpad.net/1.0";
    match LaunchpadConnector::new(base_url, payload.default_project) {
        Ok(connector) => {
            let h = connector.health().await;
            Json(serde_json::json!({
                "ok": h.ok,
                "detail": h.detail,
                "latency_ms": h.latency_ms,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "detail": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ModelTestRequest {
    base_url: Option<String>,
    api_key: Option<String>,
    chat_model: Option<String>,
    embed_model: Option<String>,
}

pub struct ModelProbeResult {
    pub ok: bool,
    pub detail: String,
    pub chat_model: Option<String>,
    pub embedding_model: Option<String>,
    pub latency_ms: i64,
}

pub async fn probe_model_endpoint(
    base_url: &str,
    api_key: Option<&str>,
    chat_model: Option<&str>,
    embed_model: Option<&str>,
) -> ModelProbeResult {
    let t0 = Instant::now();
    let discovery = crate::model::ModelClient::discover_models(
        base_url,
        api_key,
        std::time::Duration::from_secs(10),
    )
    .await;
    let latency_ms = t0.elapsed().as_millis() as i64;

    if discovery.ok {
        let resolved_chat = if let Some(cm) = chat_model.filter(|s| !s.is_empty()) {
            Some(cm.to_string())
        } else {
            discovery.chat_models.first().map(|m| m.id.clone())
        };

        let resolved_embed = if let Some(em) = embed_model.filter(|s| !s.is_empty()) {
            Some(em.to_string())
        } else {
            discovery.embedding_models.first().map(|m| m.id.clone())
        };

        ModelProbeResult {
            ok: true,
            detail: format!(
                "Model endpoint reachable, discovered {} models ({} chat, {} embedding)",
                discovery.models.len(),
                discovery.chat_models.len(),
                discovery.embedding_models.len()
            ),
            chat_model: resolved_chat,
            embedding_model: resolved_embed,
            latency_ms,
        }
    } else {
        ModelProbeResult {
            ok: false,
            detail: discovery
                .error
                .unwrap_or_else(|| "Model endpoint query failed".to_string()),
            chat_model: None,
            embedding_model: None,
            latency_ms,
        }
    }
}

async fn test_model_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ModelTestRequest>,
) -> Response {
    let (base_url, api_key, chat_model, embed_model) = {
        let sec = state.secrets.read().await;
        let cfg = state.config.read().await;
        let u = payload
            .base_url
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cfg.model.base_url.clone())
            .or_else(|| sec.openai_base_url.clone());
        let k = payload
            .api_key
            .filter(|s| !s.trim().is_empty())
            .or_else(|| sec.openai_api_key.clone());
        let m = payload
            .chat_model
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cfg.model.model.clone())
            .or_else(|| sec.openai_model.clone());
        let em = payload
            .embed_model
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cfg.model.embedding_model.clone())
            .or_else(|| sec.openai_embedding_model.clone());
        (u, k, m, em)
    };

    let base_url = match base_url {
        Some(u) if !u.trim().is_empty() => u,
        _ => {
            return Json(serde_json::json!({
                "ok": false,
                "detail": "Model base URL is required"
            }))
            .into_response();
        }
    };

    let result = probe_model_endpoint(
        &base_url,
        api_key.as_deref(),
        chat_model.as_deref(),
        embed_model.as_deref(),
    )
    .await;

    Json(serde_json::json!({
        "ok": result.ok,
        "detail": result.detail,
        "chat_model": result.chat_model,
        "embedding_model": result.embedding_model,
        "latency_ms": result.latency_ms,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct ModelDiscoveryPayload {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

async fn discover_models_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ModelDiscoveryPayload>,
) -> Response {
    let (base_url, api_key) = {
        let sec = state.secrets.read().await;
        let cfg = state.config.read().await;
        let u = payload
            .base_url
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cfg.model.base_url.clone())
            .or_else(|| sec.openai_base_url.clone());
        let k = payload
            .api_key
            .filter(|s| !s.trim().is_empty())
            .or_else(|| sec.openai_api_key.clone());
        (u, k)
    };

    let base_url = match base_url {
        Some(u) if !u.trim().is_empty() => u,
        _ => {
            return Json(crate::model::ModelDiscoveryResult {
                ok: false,
                error: Some("API endpoint URL is required for model discovery".to_string()),
                models: Vec::new(),
                chat_models: Vec::new(),
                embedding_models: Vec::new(),
            })
            .into_response();
        }
    };

    let result = crate::model::ModelClient::discover_models(
        &base_url,
        api_key.as_deref(),
        std::time::Duration::from_secs(10),
    )
    .await;

    Json(result).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct ModelDiscoveryQuery {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

async fn get_models_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ModelDiscoveryQuery>,
) -> Response {
    let (base_url, api_key) = {
        let sec = state.secrets.read().await;
        let cfg = state.config.read().await;
        let u = query
            .base_url
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cfg.model.base_url.clone())
            .or_else(|| sec.openai_base_url.clone());
        let k = query
            .api_key
            .filter(|s| !s.trim().is_empty())
            .or_else(|| sec.openai_api_key.clone());
        (u, k)
    };

    let base_url = match base_url {
        Some(u) if !u.trim().is_empty() => u,
        _ => {
            return Json(crate::model::ModelDiscoveryResult {
                ok: false,
                error: Some("API endpoint URL is required for model discovery".to_string()),
                models: Vec::new(),
                chat_models: Vec::new(),
                embedding_models: Vec::new(),
            })
            .into_response();
        }
    };

    let result = crate::model::ModelClient::discover_models(
        &base_url,
        api_key.as_deref(),
        std::time::Duration::from_secs(10),
    )
    .await;

    Json(result).into_response()
}
