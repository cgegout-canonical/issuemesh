use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use tower::ServiceExt;

use issuemesh::config::{Config, Secrets};
use issuemesh::jobs::{JobManager, SearchEvent, Source};
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::web::build_router;
use issuemesh::AppState;

async fn setup_app() -> (axum::Router, AppState, tempfile::TempDir) {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    paths.ensure_dirs().unwrap();

    let mut config = Config::load_or_default(&paths.config_file).unwrap();
    config.onboarding_completed = true;
    config.github.enabled = false;
    config.jira.enabled = false;
    config.launchpad.enabled = false;
    config.model.enabled = false;

    let store = Store::open_in_memory().unwrap();
    let jobs = JobManager::new();

    let state = AppState {
        paths,
        config: Arc::new(RwLock::new(config)),
        secrets: Arc::new(RwLock::new(Secrets::default())),
        store,
        jobs,
    };

    (build_router(state.clone()), state, tmp)
}

#[tokio::test]
async fn test_create_search_job_and_sse() {
    let (app, _state, _tmp) = setup_app().await;

    // 1. Create job
    let req = Request::builder()
        .method("POST")
        .uri("/api/search")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"query": "network timeout bug"}"#))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let job_id = json["job_id"].as_str().unwrap().to_string();
    assert!(!job_id.is_empty());

    // 2. Poll job status
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/search/{job_id}"))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 3. Request SSE stream
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/search/{job_id}/events"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok()),
        Some("text/event-stream")
    );
}

#[tokio::test]
async fn test_job_manager_direct_events() {
    let jm = JobManager::new();
    let id = jm.create_job("test query".to_string()).await;

    let (history, mut rx) = jm.subscribe_events(&id).await.unwrap();
    assert_eq!(history.len(), 0);

    jm.emit_event(&id, SearchEvent::PlanningStarted).await;
    jm.emit_event(
        &id,
        SearchEvent::SourceSearchStarted {
            source: Source::GitHub,
        },
    )
    .await;

    let ev1 = rx.recv().await.unwrap();
    match ev1 {
        SearchEvent::PlanningStarted => {}
        _ => panic!("Expected PlanningStarted"),
    }

    let ev2 = rx.recv().await.unwrap();
    match ev2 {
        SearchEvent::SourceSearchStarted { source } => assert_eq!(source, Source::GitHub),
        _ => panic!("Expected SourceSearchStarted"),
    }

    let summary = jm.get_job_summary(&id).await.unwrap();
    assert_eq!(summary.status, issuemesh::jobs::JobStatus::Running);
}
