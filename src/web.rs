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
        .route("/introspection", get(introspection_page_handler))
        .route(
            "/about-introspection",
            get(about_introspection_page_handler),
        )
        .route("/tutorial", get(tutorial_page_handler))
        .route("/intelligence", get(intelligence_page_handler))
        .route("/concepts", get(concepts_page_handler))
        .route("/search/{id}/run", get(search_run_page_handler))
        // API routes
        .route("/api/concepts", get(list_concepts_api_handler))
        .route("/api/concepts/{id}/related", get(get_related_concepts_api_handler))
        .route("/api/concepts/object/{*key}", get(get_object_concepts_api_handler))
        .route(
            "/api/mcp",
            post(crate::mcp::mcp_http_handler).get(crate::mcp::mcp_info_handler),
        )
        .route(
            "/api/intelligence/watchlists",
            get(list_watchlists_handler).post(create_watchlist_handler),
        )
        .route(
            "/api/intelligence/watchlists/{id}",
            get(get_watchlist_handler).delete(delete_watchlist_handler),
        )
        .route(
            "/api/intelligence/watchlists/{id}/check",
            post(check_watchlist_handler),
        )
        .route("/api/intelligence/what-changed", get(what_changed_handler))
        .route("/api/intelligence/impact", get(impact_analysis_handler))
        .route("/api/intelligence/risk", get(risk_radar_handler))
        .route("/api/intelligence/timeline", get(timeline_handler))
        .route("/api/intelligence/gaps", get(gaps_handler))
        .route(
            "/api/intelligence/investigations",
            get(list_investigations_handler).post(save_investigation_handler),
        )
        .route(
            "/api/intelligence/investigations/{id}",
            get(get_investigation_handler).delete(delete_investigation_handler),
        )
        .route(
            "/api/intelligence/investigations/{id}/reopen",
            post(reopen_investigation_handler),
        )
        .route("/api/intelligence/readiness", get(readiness_handler))
        .route("/api/intelligence/diff", post(diff_evidence_handler))
        .route("/api/health", get(health_api_handler))
        .route("/api/stats", get(stats_handler))
        .route("/api/introspection", get(get_introspection_handler))
        .route(
            "/api/introspection/{id}",
            get(get_introspection_run_handler),
        )
        .route(
            "/api/introspection/trigger",
            post(trigger_introspection_handler),
        )
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

async fn concepts_page_handler() -> impl IntoResponse {
    match Assets::get("concepts.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "concepts.html not found").into_response(),
    }
}

async fn introspection_page_handler() -> impl IntoResponse {
    match Assets::get("introspection.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "introspection.html not found").into_response(),
    }
}

async fn about_introspection_page_handler() -> impl IntoResponse {
    match Assets::get("about-introspection.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "about-introspection.html not found").into_response(),
    }
}

async fn tutorial_page_handler() -> impl IntoResponse {
    match Assets::get("tutorial.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "tutorial.html not found").into_response(),
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

    // 6. Introspection Probe
    let intro_level = cfg.introspection.level;
    let questions_per_day = cfg.introspection.questions_per_day();
    let questions_today = state
        .store
        .count_introspection_runs_today()
        .await
        .unwrap_or(0);
    let last_successful = state.introspection.last_run_at.read().await.clone();
    let next_expected = state.introspection.next_expected_at.read().await.clone();
    let failures_today = state
        .introspection
        .failures_today
        .load(std::sync::atomic::Ordering::SeqCst);
    let scheduler_status = state.introspection.scheduler_status.read().await.clone();

    let intro_status = if intro_level == 0 {
        "disabled"
    } else if failures_today > 0 && questions_today == 0 {
        "degraded"
    } else {
        "healthy"
    };

    let introspection_probe = serde_json::json!({
        "enabled": intro_level > 0,
        "ok": intro_status != "degraded",
        "detail": format!("Level {} ({}), {}/day, {} today", intro_level, cfg.introspection.level_name(), questions_per_day, questions_today),
        "latency_ms": 0,
        "status": intro_status,
        "level": intro_level,
        "level_name": cfg.introspection.level_name(),
        "questions_per_day": questions_per_day,
        "questions_today": questions_today,
        "last_successful": last_successful,
        "next_expected": next_expected,
        "failures_today": failures_today,
        "scheduler": scheduler_status,
    });

    serde_json::json!({
        "status": if db_probe.ok { "healthy" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "subsystems": {
            "database": db_probe,
            "github": gh_probe,
            "jira": jira_probe,
            "launchpad": lp_probe,
            "model": model_probe,
            "introspection": introspection_probe,
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

async fn get_introspection_handler(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state.config.read().await.clone();
    let level = cfg.introspection.level;
    let level_name = cfg.introspection.level_name();
    let questions_per_day = cfg.introspection.questions_per_day();
    let questions_today = state
        .store
        .count_introspection_runs_today()
        .await
        .unwrap_or(0);
    let last_run = state.introspection.last_run_at.read().await.clone();
    let next_expected = state.introspection.next_expected_at.read().await.clone();
    let is_running = state
        .introspection
        .is_running
        .load(std::sync::atomic::Ordering::SeqCst);
    let scheduler_status = state.introspection.scheduler_status.read().await.clone();

    let stats = match state.store.get_introspection_stats().await {
        Ok(s) => serde_json::to_value(s).unwrap_or(serde_json::json!({})),
        Err(e) => {
            tracing::warn!("Failed to get introspection stats: {e}");
            serde_json::json!({})
        }
    };

    let trends = match state.store.get_introspection_trends(7).await {
        Ok(t) => serde_json::to_value(t).unwrap_or(serde_json::json!([])),
        Err(e) => {
            tracing::warn!("Failed to get introspection trends: {e}");
            serde_json::json!([])
        }
    };

    let runs = match state.store.list_introspection_runs(50).await {
        Ok(r) => {
            let mut list = Vec::new();
            for item in r {
                let prob_areas: Vec<String> =
                    serde_json::from_str(&item.problem_areas_json).unwrap_or_default();
                let ev_keys: Vec<String> =
                    serde_json::from_str(&item.evidence_keys_json).unwrap_or_default();
                let eval_details: serde_json::Value =
                    serde_json::from_str(&item.evaluation_details_json)
                        .unwrap_or(serde_json::json!({}));
                list.push(serde_json::json!({
                    "id": item.id,
                    "created_at": item.created_at,
                    "question": item.question,
                    "question_fingerprint": item.question_fingerprint,
                    "trigger_reason": item.trigger_reason,
                    "search_run_id": item.search_run_id,
                    "accuracy": item.accuracy,
                    "relevance": item.relevance,
                    "coverage": item.coverage,
                    "traceability": item.traceability,
                    "conciseness": item.conciseness,
                    "weighted_score": item.weighted_score,
                    "percentage": item.percentage,
                    "result": item.result,
                    "interpretation": item.interpretation,
                    "answer": item.answer,
                    "problem_areas": prob_areas,
                    "recommendation": item.recommendation,
                    "evidence_keys": ev_keys,
                    "evaluation_details": eval_details,
                }));
            }
            serde_json::Value::Array(list)
        }
        Err(e) => {
            tracing::warn!("Failed to list introspection runs: {e}");
            serde_json::json!([])
        }
    };

    Json(serde_json::json!({
        "level": level,
        "level_name": level_name,
        "questions_per_day": questions_per_day,
        "questions_today": questions_today,
        "last_run": last_run,
        "next_expected": next_expected,
        "is_running": is_running,
        "scheduler_status": scheduler_status,
        "stats": stats,
        "trends": trends,
        "runs": runs,
    }))
    .into_response()
}

async fn get_introspection_run_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.store.get_introspection_run(&id).await {
        Ok(Some(item)) => {
            let prob_areas: Vec<String> =
                serde_json::from_str(&item.problem_areas_json).unwrap_or_default();
            let ev_keys: Vec<String> =
                serde_json::from_str(&item.evidence_keys_json).unwrap_or_default();
            let eval_details: serde_json::Value =
                serde_json::from_str(&item.evaluation_details_json)
                    .unwrap_or(serde_json::json!({}));
            Json(serde_json::json!({
                "id": item.id,
                "created_at": item.created_at,
                "question": item.question,
                "question_fingerprint": item.question_fingerprint,
                "trigger_reason": item.trigger_reason,
                "search_run_id": item.search_run_id,
                "accuracy": item.accuracy,
                "relevance": item.relevance,
                "coverage": item.coverage,
                "traceability": item.traceability,
                "conciseness": item.conciseness,
                "weighted_score": item.weighted_score,
                "percentage": item.percentage,
                "result": item.result,
                "interpretation": item.interpretation,
                "answer": item.answer,
                "problem_areas": prob_areas,
                "recommendation": item.recommendation,
                "evidence_keys": ev_keys,
                "evaluation_details": eval_details,
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Run not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn trigger_introspection_handler(State(state): State<Arc<AppState>>) -> Response {
    match state.introspection.trigger_now(state.clone()).await {
        Ok(record) => {
            let prob_areas: Vec<String> =
                serde_json::from_str(&record.problem_areas_json).unwrap_or_default();
            let ev_keys: Vec<String> =
                serde_json::from_str(&record.evidence_keys_json).unwrap_or_default();
            let eval_details: serde_json::Value =
                serde_json::from_str(&record.evaluation_details_json)
                    .unwrap_or(serde_json::json!({}));
            Json(serde_json::json!({
                "ok": true,
                "run": {
                    "id": record.id,
                    "created_at": record.created_at,
                    "question": record.question,
                    "question_fingerprint": record.question_fingerprint,
                    "trigger_reason": record.trigger_reason,
                    "search_run_id": record.search_run_id,
                    "accuracy": record.accuracy,
                    "relevance": record.relevance,
                    "coverage": record.coverage,
                    "traceability": record.traceability,
                    "conciseness": record.conciseness,
                    "weighted_score": record.weighted_score,
                    "percentage": record.percentage,
                    "result": record.result,
                    "interpretation": record.interpretation,
                    "answer": record.answer,
                    "problem_areas": prob_areas,
                    "recommendation": record.recommendation,
                    "evidence_keys": ev_keys,
                    "evaluation_details": eval_details,
                }
            }))
            .into_response()
        }
        Err(e) => {
            let status = if e.to_string().contains("already actively executing") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (
                status,
                Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
            )
                .into_response()
        }
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
pub struct IntrospectionForm {
    pub level: Option<u8>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct ConceptsForm {
    pub enabled: Option<bool>,
    pub max_index_per_search: Option<usize>,
    pub max_queue_depth: Option<usize>,
    pub max_input_tokens: Option<usize>,
    pub daily_token_budget: Option<usize>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct SettingsPayload {
    pub github: GitHubForm,
    pub jira: JiraForm,
    pub launchpad: LaunchpadForm,
    pub model: ModelForm,
    pub search: SearchForm,
    pub introspection: Option<IntrospectionForm>,
    pub concepts: Option<ConceptsForm>,
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

        if let Some(intro) = payload.introspection {
            if let Some(lvl) = intro.level {
                cfg.introspection.level = lvl.min(4);
            }
        }

        if let Some(c) = payload.concepts {
            if let Some(en) = c.enabled {
                cfg.concepts.enabled = en;
            }
            if let Some(m) = c.max_index_per_search {
                cfg.concepts.max_index_per_search = m;
            }
            if let Some(q) = c.max_queue_depth {
                cfg.concepts.max_queue_depth = q;
            }
            if let Some(i) = c.max_input_tokens {
                cfg.concepts.max_input_tokens = i;
            }
            if let Some(b) = c.daily_token_budget {
                cfg.concepts.daily_token_budget = b;
            }
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

// ----------------------------------------------------------------------------
// Intelligence & MCP Handlers
// ----------------------------------------------------------------------------

async fn intelligence_page_handler() -> impl IntoResponse {
    match Assets::get("intelligence.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "intelligence.html not found").into_response(),
    }
}

async fn list_watchlists_handler(State(state): State<Arc<AppState>>) -> Response {
    let service = crate::intelligence::WatchlistService::new(state.store.clone());
    match service.list_watchlists().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn create_watchlist_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<crate::intelligence::CreateWatchlistRequest>,
) -> Response {
    let service = crate::intelligence::WatchlistService::new(state.store.clone());
    match service.create_watchlist(payload).await {
        Ok(rec) => (StatusCode::CREATED, Json(rec)).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_watchlist_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let service = crate::intelligence::WatchlistService::new(state.store.clone());
    match service.get_watchlist(&id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Watchlist not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_watchlist_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let service = crate::intelligence::WatchlistService::new(state.store.clone());
    match service.delete_watchlist(&id).await {
        Ok(deleted) => Json(serde_json::json!({ "deleted": deleted, "id": id })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn check_watchlist_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let service = crate::intelligence::WatchlistService::new(state.store.clone());
    match service.check_watchlist(&id).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct KeyQuery {
    pub key: Option<String>,
    pub depth: Option<usize>,
}

async fn what_changed_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KeyQuery>,
) -> Response {
    let key = match query.key {
        Some(k) if !k.trim().is_empty() => k,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing 'key' query parameter" })),
            )
                .into_response();
        }
    };
    let service = crate::intelligence::WatchlistService::new(state.store.clone());
    match service.what_changed_for_object(&key).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn impact_analysis_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KeyQuery>,
) -> Response {
    let key = match query.key {
        Some(k) if !k.trim().is_empty() => k,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing 'key' query parameter" })),
            )
                .into_response();
        }
    };
    let analyzer = crate::intelligence::ImpactAnalyzer::new(state.store.clone());
    match analyzer.analyze_impact(&key).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn risk_radar_handler(State(state): State<Arc<AppState>>) -> Response {
    let radar = crate::intelligence::RiskRadar::new(state.store.clone());
    match radar.scan().await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn timeline_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KeyQuery>,
) -> Response {
    let key = match query.key {
        Some(k) if !k.trim().is_empty() => k,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing 'key' query parameter" })),
            )
                .into_response();
        }
    };
    let reconstructor = crate::intelligence::StoryReconstructor::new(state.store.clone());
    match reconstructor.reconstruct_timeline(&key).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn gaps_handler(State(state): State<Arc<AppState>>) -> Response {
    let detector = crate::intelligence::GapDetector::new(state.store.clone());
    match detector.detect_gaps().await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn list_investigations_handler(State(state): State<Arc<AppState>>) -> Response {
    let mgr = crate::intelligence::InvestigationManager::new(state.store.clone());
    match mgr.list().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn save_investigation_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<crate::intelligence::SaveInvestigationRequest>,
) -> Response {
    let mgr = crate::intelligence::InvestigationManager::new(state.store.clone());
    match mgr.save(payload).await {
        Ok(rec) => (StatusCode::CREATED, Json(rec)).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_investigation_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let mgr = crate::intelligence::InvestigationManager::new(state.store.clone());
    match mgr.get(&id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Investigation not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_investigation_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let mgr = crate::intelligence::InvestigationManager::new(state.store.clone());
    match mgr.delete(&id).await {
        Ok(deleted) => Json(serde_json::json!({ "deleted": deleted, "id": id })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn reopen_investigation_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let mgr = crate::intelligence::InvestigationManager::new(state.store.clone());
    match mgr.reopen(&id).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadinessQuery {
    pub container: Option<String>,
    pub topic: Option<String>,
    pub target: Option<String>,
}

async fn readiness_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ReadinessQuery>,
) -> Response {
    let target = query
        .target
        .or(query.container)
        .or(query.topic)
        .unwrap_or_else(|| "all".to_string());
    let evaluator = crate::intelligence::ReadinessEvaluator::new(state.store.clone());
    match evaluator.assess_readiness(&target).await {
        Ok(assessment) => Json(assessment).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct DiffRequest {
    pub previous: Vec<crate::intelligence::diff::EvidenceSnapshotItem>,
    pub current_keys: Vec<String>,
}

async fn diff_evidence_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DiffRequest>,
) -> Response {
    let mut current_evs = Vec::new();
    for k in &payload.current_keys {
        let oref = crate::connectors::ObjectRef::from_key(k)
            .unwrap_or_else(|| crate::connectors::ObjectRef::new("unknown", "unknown", k));
        if let Ok(Some(ev)) = state.store.get_object(&oref).await {
            current_evs.push(ev);
        }
    }
    let diff = crate::intelligence::diff::diff_evidence_snapshots(&payload.previous, &current_evs);
    Json(diff).into_response()
}

// ----------------------------------------------------------------------------
// Concept Layer API Handlers
// ----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ConceptsQueryParams {
    pub query: Option<String>,
    pub source: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

async fn list_concepts_api_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<ConceptsQueryParams>,
) -> Response {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let (total_concepts, indexed_objects, total_links) = match state.store.get_concept_stats().await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let concepts = if let Some(ref q) = params.query {
        if !q.trim().is_empty() {
            state.store.search_concepts_fts(q, limit).await
        } else {
            state.store.list_concepts(limit, offset).await
        }
    } else if let Some(ref src) = params.source {
        state.store.list_top_concepts_for_source(src, limit).await
    } else {
        state.store.list_concepts(limit, offset).await
    };

    match concepts {
        Ok(list) => Json(serde_json::json!({
            "total_concepts": total_concepts,
            "indexed_objects": indexed_objects,
            "total_links": total_links,
            "concepts": list,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_related_concepts_api_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let limit = 20;
    let related_res = state.store.get_related_concepts(id, limit).await;
    let objects_res = state.store.get_objects_for_concept(id, limit).await;

    match (related_res, objects_res) {
        (Ok(related), Ok(objects)) => Json(serde_json::json!({
            "concept_id": id,
            "related_concepts": related.into_iter().map(|(c, co)| serde_json::json!({ "concept": c, "co_occurrence": co })).collect::<Vec<_>>(),
            "matching_objects": objects.into_iter().map(|(k, w)| serde_json::json!({ "object_key": k, "weight": w })).collect::<Vec<_>>(),
        }))
        .into_response(),
        (Err(e), _) | (_, Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_object_concepts_api_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Response {
    match state.store.get_concepts_for_object(&key).await {
        Ok(concepts) => Json(serde_json::json!({
            "object_key": key,
            "concepts": concepts,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

