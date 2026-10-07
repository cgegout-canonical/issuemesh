use axum::http::StatusCode;
use issuemesh::config::{Config, Secrets};
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::web::{build_router, SettingsPayload};
use issuemesh::AppState;
use tempfile::tempdir;

async fn setup_app_unonboarded() -> (axum::Router, AppState, tempfile::TempDir) {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    paths.ensure_dirs().unwrap();

    let config = Config::load_or_default(&paths.config_file).unwrap();
    let secrets = Secrets::default();
    let store = Store::open_in_memory().unwrap();

    let state = AppState::new(paths, config, secrets, store);

    (build_router(state.clone()), state, tmp)
}

#[tokio::test]
async fn test_onboarding_redirect_and_setup_pages() {
    let (app, _state, _tmp) = setup_app_unonboarded().await;

    // 1. Root redirects to /setup when not onboarded
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(resp.headers().get("location").unwrap(), "/setup");

    // 2. GET /setup renders onboarding wizard
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/setup")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("IssueMesh - Onboarding Setup"));
    assert!(body.contains("Welcome to IssueMesh"));

    // 3. GET /health renders health page
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4. GET /settings renders settings page
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/settings")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_onboarding_wizard_submission_and_redirection() {
    let (app, state, _tmp) = setup_app_unonboarded().await;

    // Verify initial state
    assert!(!state.config.read().await.onboarding_completed);

    let mut payload = SettingsPayload::default();
    payload.github.enabled = true;
    payload.github.token = Some("ghp_mocktesttoken123456789".to_string());
    payload.github.default_org = Some("canonical".to_string());
    payload.jira.enabled = false;
    payload.launchpad.enabled = true;
    payload.search.graph_depth = Some(3);
    payload.search.max_graph_nodes = Some(150);

    let req_body = serde_json::to_vec(&payload).unwrap();

    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/setup")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(req_body))
            .unwrap(),
    )
    .await
    .unwrap();

    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "Response error was: {}", text);

    // Verify state transitioned to completed
    assert!(state.config.read().await.onboarding_completed);
    assert_eq!(
        state.config.read().await.github.default_org.as_deref(),
        Some("canonical")
    );
    assert_eq!(state.config.read().await.search.graph_depth, 3);
    assert_eq!(
        state.secrets.read().await.github_token.as_deref(),
        Some("ghp_mocktesttoken123456789")
    );

    // Verify that GET / now returns OK (renders index.html) instead of redirecting
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_settings_api_and_secret_masking() {
    let (app, state, _tmp) = setup_app_unonboarded().await;

    // Prepopulate secret
    {
        let mut sec = state.secrets.write().await;
        sec.github_token = Some("github_pat_11AAAAAA_BBBBBBBBBB42fa".to_string());
        sec.jira_token = Some("secret_jira_token_abc999".to_string());
    }

    // Call GET /api/settings
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/api/settings")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Check that masked secrets are returned, NOT plain secrets
    let gh_masked = json_val["masked_secrets"]["github_token"].as_str().unwrap();
    assert!(gh_masked.contains("••••"));
    assert!(gh_masked.ends_with("42fa"));
    assert!(!gh_masked.contains("AAAAAA"));

    let jira_masked = json_val["masked_secrets"]["jira_token"].as_str().unwrap();
    assert!(jira_masked.contains("••••"));
    assert!(jira_masked.ends_with("c999"));
}

#[tokio::test]
async fn test_health_api_and_probes() {
    let (app, _state, _tmp) = setup_app_unonboarded().await;

    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/api/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Check subsystem probes presence
    assert!(json_val.get("subsystems").is_some());
    let sub = &json_val["subsystems"];
    assert!(sub.get("database").is_some());
    assert_eq!(sub["database"]["ok"], true);
    assert!(sub.get("github").is_some());
    assert!(sub.get("jira").is_some());
    assert!(sub.get("launchpad").is_some());
    assert!(sub.get("model").is_some());
}
