use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use issuemesh::model::{ModelClient, QueryPlanner};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

async fn handle_chat_completion(
    State(state): State<Arc<AtomicUsize>>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    state.fetch_add(1, Ordering::SeqCst);
    let messages = payload["messages"].as_array();
    let user_msg = messages
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default();

    if user_msg.contains("trigger_error") {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Model runtime internal failure"})),
        )
            .into_response();
    }

    let plan = json!({
        "query": "bgp routing crash",
        "sources": ["github", "jira"],
        "projects": ["OPEN"],
        "repositories": ["canonical/net"],
        "people": ["alice"],
        "semantic_queries": ["bgp routing crash", "bgp flap core dump"],
        "graph_depth": 99,
        "max_remote_results": 999,
        "max_deep_reads": 500,
        "max_graph_nodes": 1000,
        "relation_types": ["references", "fixes"]
    });

    let resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": plan.to_string()
            }
        }]
    });

    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_embeddings(
    State(state): State<Arc<AtomicUsize>>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    state.fetch_add(1, Ordering::SeqCst);
    let input = payload["input"].as_array().cloned().unwrap_or_default();
    let data: Vec<serde_json::Value> = input
        .iter()
        .enumerate()
        .map(|(i, _)| {
            json!({
                "embedding": vec![0.1f32, 0.2f32, 0.3f32],
                "index": i
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "data": data }))).into_response()
}

async fn start_mock_model_server() -> (String, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completion))
        .route("/embeddings", post(handle_embeddings))
        .with_state(count.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (base_url, count)
}

#[tokio::test]
async fn test_deterministic_query_planner() {
    let raw = "repo:canonical/net project:OPEN author:bob bgp routing issue";
    let plan = QueryPlanner::parse_deterministic(raw);

    assert_eq!(plan.query, "bgp routing issue");
    assert_eq!(plan.repositories, vec!["canonical/net".to_string()]);
    assert_eq!(plan.projects, vec!["OPEN".to_string()]);
    assert_eq!(plan.people, vec!["bob".to_string()]);
    assert_eq!(plan.graph_depth, 2);
    assert_eq!(plan.max_remote_results, 50);
}

#[tokio::test]
async fn test_model_planning_and_sanitization() {
    let (base_url, req_count) = start_mock_model_server().await;

    let client = ModelClient::new(
        &base_url,
        Some("test-key".to_string()),
        "test-model",
        "test-embed",
        Duration::from_secs(2),
    )
    .unwrap();

    // 1. Model successful plan with out-of-bound values clamped
    let plan = QueryPlanner::plan("bgp crash", Some(&client)).await;
    assert_eq!(req_count.load(Ordering::SeqCst), 1);
    assert_eq!(plan.query, "bgp routing crash");
    assert_eq!(plan.repositories, vec!["canonical/net".to_string()]);
    assert_eq!(plan.projects, vec!["OPEN".to_string()]);
    // Clamping verification per gate: depth 99 -> max (3), remote results 999 -> 100, nodes -> 200, reads -> 50
    assert_eq!(plan.graph_depth, 3);
    assert_eq!(plan.max_remote_results, 100);
    assert_eq!(plan.max_deep_reads, 50);
    assert_eq!(plan.max_graph_nodes, 200);

    // 2. Model down / error -> graceful fallback to deterministic without user error
    let plan_fallback = QueryPlanner::plan("trigger_error critical issue", Some(&client)).await;
    assert_eq!(plan_fallback.query, "trigger_error critical issue");
    assert_eq!(plan_fallback.graph_depth, 2);
    assert_eq!(plan_fallback.max_remote_results, 50);

    // 3. None client -> deterministic fallback
    let plan_none = QueryPlanner::plan("repo:canonical/snapcraft package build", None).await;
    assert_eq!(plan_none.query, "package build");
    assert_eq!(
        plan_none.repositories,
        vec!["canonical/snapcraft".to_string()]
    );
}

#[tokio::test]
async fn test_model_embeddings_client() {
    let (base_url, _) = start_mock_model_server().await;

    let client = ModelClient::new(
        &base_url,
        Some("test-key".to_string()),
        "test-model",
        "test-embed",
        Duration::from_secs(2),
    )
    .unwrap();

    let texts = vec!["hello world".to_string(), "foo bar".to_string()];
    let embeddings = client.embed(&texts).await.unwrap();

    assert_eq!(embeddings.len(), 2);
    assert_eq!(embeddings[0].len(), 3);
    assert_eq!(embeddings[1].len(), 3);
}

#[tokio::test]
async fn test_natural_language_query_cleaning_and_planning() {
    use issuemesh::model::planner::clean_natural_language_query;

    let (q, limit, proj) = clean_natural_language_query("what are the top 3 issues of Sunbeam?");
    assert_eq!(q, "Sunbeam");
    assert_eq!(limit, Some(3));
    assert_eq!(proj, Some("Sunbeam".to_string()));

    let (q2, limit2, proj2) = clean_natural_language_query("show me the latest 5 bugs in Nova");
    assert_eq!(q2, "Nova");
    assert_eq!(limit2, Some(5));
    assert_eq!(proj2, Some("Nova".to_string()));

    // Deterministic plan end-to-end
    let plan = QueryPlanner::plan("what are the top 3 issues of Sunbeam?", None).await;
    assert_eq!(plan.query, "Sunbeam");
    assert_eq!(plan.projects, vec!["Sunbeam".to_string()]);
    assert_eq!(plan.max_remote_results, 10);
}

#[tokio::test]
async fn test_model_client_openrouter_normalization() {
    let client = ModelClient::new(
        "https://openrouter.ai/api/v1",
        Some("test-key".to_string()),
        "gpt-sol-latest",
        "gpt-sol-latest",
        Duration::from_secs(5),
    )
    .unwrap();

    // Verify it builds and the internal model is mapped properly
    // Client can format requests without 404 on OpenRouter
    assert!(format!("{:?}", client).contains("openai/gpt-4o-mini"));
    assert!(format!("{:?}", client).contains("openai/text-embedding-3-small"));
}
