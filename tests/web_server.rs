use axum::http::StatusCode;
use issuemesh::config::{Config, Secrets};
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::web::build_router;
use issuemesh::AppState;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

async fn setup_app() -> axum::Router {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    paths.ensure_dirs().unwrap();

    let config = Config::load_or_default(&paths.config_file).unwrap();
    let secrets = Secrets::default();
    let store = Store::open_in_memory().unwrap();

    let state = AppState {
        paths,
        config: Arc::new(RwLock::new(config)),
        secrets: Arc::new(RwLock::new(secrets)),
        store,
    };

    build_router(state)
}

#[tokio::test]
async fn test_web_index_and_static_assets() {
    let app = setup_app().await;

    // Test GET /
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("IssueMesh"));
    assert!(body.contains("Search GitHub &middot; Jira &middot; Launchpad"));

    // Test GET /static/css/style.css
    let response_css = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/static/css/style.css")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response_css.status(), StatusCode::OK);
    let content_type = response_css
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/css"));

    // Test GET /static/js/app.js
    let response_js = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/static/js/app.js")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response_js.status(), StatusCode::OK);

    // Test 404 for missing static file
    let response_404 = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::builder()
            .uri("/static/missing.png")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response_404.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_web_health_and_stats_api() {
    let app = setup_app().await;

    // Test GET /api/health
    let res_health = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/api/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(res_health.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res_health.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));

    // Test GET /api/stats
    let res_stats = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/api/stats")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(res_stats.status(), StatusCode::OK);

    // Test GET /api/search/local
    let res_search = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::builder()
            .uri("/api/search/local?q=test")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(res_search.status(), StatusCode::OK);
}
