use axum::http::StatusCode;
use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef};
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::web::build_router;
use issuemesh::AppState;
use tempfile::tempdir;
use tower::ServiceExt;

async fn setup_app() -> (axum::Router, AppState, tempfile::TempDir) {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    paths.ensure_dirs().unwrap();

    let mut config = Config::load_or_default(&paths.config_file).unwrap();
    config.onboarding_completed = true;
    let secrets = Secrets::default();
    let store = Store::open(&paths.db_file).unwrap();

    // Seed some test data into store
    let obj = Evidence {
        r#ref: ObjectRef::new("github", "issue", "org/repo#42"),
        title: "Test Issue 42".to_string(),
        snippet: "A sample issue snippet".to_string(),
        body: Some("Detailed body text for issue 42".to_string()),
        url: "https://github.com/org/repo/issues/42".to_string(),
        container: "org/repo".to_string(),
        author: "alice".to_string(),
        created_at: Some("2026-03-01T12:00:00Z".to_string()),
        updated_at: Some("2026-03-02T12:00:00Z".to_string()),
        state: "open".to_string(),
        native: std::collections::BTreeMap::new(),
    };
    store.upsert_object(obj).await.unwrap();

    let state = AppState::new(paths, config, secrets, store);
    (build_router(state.clone()), state, tmp)
}

#[tokio::test]
async fn test_pages_render() {
    let (app, _state, _tmp) = setup_app().await;

    // Test GET /memory
    let req = axum::http::Request::builder()
        .uri("/memory")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("Local Memory &amp; Datastore")
            || body_str.contains("Local Memory & Datastore")
    );

    // Test GET /sources
    let req = axum::http::Request::builder()
        .uri("/sources")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("Connected Sources &amp; Repositories")
            || body_str.contains("Connected Sources & Repositories")
    );

    // Test GET /tutorial
    let req = axum::http::Request::builder()
        .uri("/tutorial")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("Tutorial &amp; Beginner&#39;s Guide")
            || body_str.contains("Tutorial & Beginner's Guide")
            || body_str.contains("Tutorial")
    );

    // Test GET /introspection
    let req = axum::http::Request::builder()
        .uri("/introspection")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("Introspection"));

    // Test GET /search/xyz/run
    let req = axum::http::Request::builder()
        .uri("/search/xyz/run")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("Search Run Inspection"));
}

#[tokio::test]
async fn test_memory_diagnostics_and_maintenance_apis() {
    let (app, state, _tmp) = setup_app().await;

    // 1. GET /api/memory
    let req = axum::http::Request::builder()
        .uri("/api/memory")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let diag: issuemesh::store::MemoryDiagnostics = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(diag.total_objects, 1);
    assert_eq!(*diag.objects_by_source.get("github").unwrap_or(&0), 1);

    // 2. GET /api/sources
    let req = axum::http::Request::builder()
        .uri("/api/sources")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let sources: Vec<issuemesh::store::SourceContainerSummary> =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source, "github");
    assert_eq!(sources[0].container, "org/repo");
    assert_eq!(sources[0].object_count, 1);

    // 3. POST /api/memory/integrity
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/memory/integrity")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["ok"], true);

    // 4. POST /api/memory/optimize
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/memory/optimize")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 5. POST /api/memory/rebuild-fts
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/memory/rebuild-fts")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 6. Save a search run to verify search run retrieval & clear-history
    state
        .store
        .save_search_run(
            "run-123",
            "test query",
            r#"{"query":"test query"}"#,
            r#"{"duration_ms":100}"#,
            Some("Answer markdown"),
            Some("v1.0"),
            Some("local-heuristic"),
            &[("github:issue:org/repo#42".to_string(), 0.95)],
        )
        .await
        .unwrap();

    // GET /api/search/run-123/run
    let req = axum::http::Request::builder()
        .uri("/api/search/run-123/run")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let run_details: issuemesh::store::SearchRunDetails = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(run_details.run.id, "run-123");
    assert_eq!(run_details.objects.len(), 1);
    assert_eq!(
        run_details.objects[0].object_key,
        "github:issue:org/repo#42"
    );

    // GET /api/search/run-123/result
    let req = axum::http::Request::builder()
        .uri("/api/search/run-123/result")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let res_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(res_json["query"], "test query");
    assert_eq!(res_json["answer"], "Answer markdown");
    assert_eq!(res_json["ranked_results"].as_array().unwrap().len(), 1);

    // 7. POST /api/memory/clear-history
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/memory/clear-history")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Run-123 should now be deleted
    let req = axum::http::Request::builder()
        .uri("/api/search/run-123/run")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
