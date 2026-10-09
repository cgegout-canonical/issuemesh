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

#[tokio::test]
async fn test_model_discovery_api() {
    let (app, _state, _tmp) = setup_app_unonboarded().await;

    // 1. GET /api/models without any base_url configured fails cleanly with empty models
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/api/models")
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
    assert_eq!(json_val["ok"], false);
    assert!(json_val["models"].as_array().unwrap().is_empty());
    assert!(json_val["error"]
        .as_str()
        .unwrap()
        .contains("API endpoint URL is required for model discovery"));

    // 2. Start mock OpenAI endpoint
    use axum::routing::get;
    use axum::Json;
    use serde_json::json;
    use tokio::net::TcpListener;

    let mock_app = axum::Router::new().route(
        "/v1/models",
        get(|headers: axum::http::HeaderMap| async move {
            let auth = headers.get("authorization").and_then(|h| h.to_str().ok());
            if auth == Some("Bearer invalid-key") {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": {"message": "Invalid API key"}})),
                );
            }
            if auth == Some("Bearer malformed") {
                return (StatusCode::OK, Json(json!("not an object or list")));
            }
            (
                StatusCode::OK,
                Json(json!({
                    "data": [
                        {"id": "openai/gpt-4o", "context_length": 128000},
                        {"id": "text-embedding-3-small"}
                    ]
                })),
            )
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let base_url = format!("http://{}/v1", addr);

    // 3. POST /api/models with valid endpoint and key
    let payload = json!({
        "base_url": base_url,
        "api_key": "valid-key"
    });
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/models")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json_val["ok"], true);
    assert_eq!(json_val["models"].as_array().unwrap().len(), 2);
    assert_eq!(json_val["chat_models"].as_array().unwrap().len(), 1);
    assert_eq!(json_val["chat_models"][0]["id"], "openai/gpt-4o");
    assert_eq!(json_val["embedding_models"].as_array().unwrap().len(), 1);
    assert_eq!(
        json_val["embedding_models"][0]["id"],
        "text-embedding-3-small"
    );

    // 4. POST /api/models with 401 Unauthorized
    let payload = json!({
        "base_url": base_url,
        "api_key": "invalid-key"
    });
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/models")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json_val["ok"], false);
    assert!(json_val["error"]
        .as_str()
        .unwrap()
        .contains("Authentication failed (HTTP 401 Unauthorized)"));
    assert!(json_val["models"].as_array().unwrap().is_empty());

    // 5. POST /api/models with unreachable host
    let payload = json!({
        "base_url": "http://127.0.0.1:9",
        "api_key": "key"
    });
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/models")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json_val["ok"], false);
    assert!(json_val["models"].as_array().unwrap().is_empty());
    assert!(json_val["error"]
        .as_str()
        .unwrap()
        .contains("Endpoint unreachable or invalid"));

    // 6. POST /api/models with empty base_url
    let payload = json!({
        "base_url": "",
        "api_key": ""
    });
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/models")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json_val["ok"], false);
    assert!(json_val["models"].as_array().unwrap().is_empty());
    assert_eq!(
        json_val["error"].as_str().unwrap(),
        "API endpoint URL is required for model discovery"
    );

    // 7. GET /api/models with query params
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("GET")
            .uri(format!(
                "/api/models?base_url={}&api_key=valid-key",
                urlencoding_encode(&base_url)
            ))
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
    assert_eq!(json_val["ok"], true);
    assert_eq!(json_val["models"].as_array().unwrap().len(), 2);
}

fn urlencoding_encode(s: &str) -> String {
    s.replace(":", "%3A").replace("/", "%2F")
}
