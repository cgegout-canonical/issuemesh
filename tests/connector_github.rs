use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use issuemesh::connectors::github::GitHubConnector;
use issuemesh::connectors::{Connector, HttpError, ObjectRef, ReadOnlyHttpClient, SearchQuery};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

struct MockState {
    request_count: AtomicUsize,
    gql_fail: std::sync::atomic::AtomicBool,
}

#[tokio::test]
async fn test_github_read_only_violation() {
    let state = Arc::new(MockState {
        request_count: AtomicUsize::new(0),
        gql_fail: std::sync::atomic::AtomicBool::new(false),
    });

    let app = Router::new()
        .route(
            "/api/write",
            post(|_: Json<Value>| async { (StatusCode::OK, "written") }),
        )
        .route(
            "/graphql",
            post(|_: Json<Value>| async { Json(json!({"data": {}})) }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = ReadOnlyHttpClient::new(
        &format!("http://{}", addr),
        Some("dummy_tok".into()),
        &["/graphql"],
        "test/1.0",
        4,
        0,
    )
    .unwrap();

    // POST to non-allowlisted endpoint
    let res = client
        .post_search("/api/write", &json!({"foo": "bar"}))
        .await;
    match res {
        Err(HttpError::ReadOnlyViolation(p)) => {
            assert_eq!(p, "/api/write");
        }
        _ => panic!("Expected ReadOnlyViolation error!"),
    }
    assert_eq!(state.request_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn test_github_connector_mock_suite() {
    let state = Arc::new(MockState {
        request_count: AtomicUsize::new(0),
        gql_fail: std::sync::atomic::AtomicBool::new(true), // test GQL fallback
    });
    let s_clone = state.clone();

    let app = Router::new()
        .route(
            "/user",
            get(|| async {
                Json(json!({
                    "login": "testuser"
                }))
            }),
        )
        .route(
            "/rate_limit",
            get(|| async {
                Json(json!({
                    "resources": {
                        "core": { "remaining": 4999 }
                    }
                }))
            }),
        )
        .route(
            "/search/issues",
            get({
                let s = s_clone.clone();
                move |Query(params): Query<HashMap<String, String>>| {
                    let s = s.clone();
                    async move {
                        s.request_count.fetch_add(1, Ordering::Relaxed);
                        let q = params.get("q").cloned().unwrap_or_default();

                        // Test retry with first 3 words
                        if q.contains("wordone wordtwo wordthree wordfour") {
                            return Json(json!({ "items": [] }));
                        }
                        if q.contains("wordone wordtwo wordthree") {
                            if q.contains("is:issue") {
                                return Json(json!({
                                    "items": [{
                                        "number": 101,
                                        "title": "Fallback match",
                                        "body": "Found by 3 words",
                                        "html_url": "https://github.com/org/repo/issues/101",
                                        "state": "open",
                                        "user": { "login": "alice" },
                                        "repository_url": "https://api.github.com/repos/org/repo",
                                        "updated_at": "2026-10-01T12:00:00Z"
                                    }]
                                }));
                            } else {
                                return Json(json!({ "items": [] }));
                            }
                        }

                        // Normal double query testing
                        if q.contains("is:issue") {
                            Json(json!({
                                "items": [{
                                    "number": 1,
                                    "title": "Issue #1",
                                    "body": "An issue body mentioning LP#2167315",
                                    "html_url": "https://github.com/canonical/project/issues/1",
                                    "state": "open",
                                    "user": { "login": "bob" },
                                    "repository_url": "https://api.github.com/repos/canonical/project",
                                    "updated_at": "2026-10-05T12:00:00Z"
                                }]
                            }))
                        } else if q.contains("is:pull-request") {
                            Json(json!({
                                "items": [{
                                    "number": 2,
                                    "title": "PR #2",
                                    "body": "A PR body mentioning OPEN-4679",
                                    "html_url": "https://github.com/canonical/project/pull/2",
                                    "state": "open",
                                    "pull_request": {},
                                    "user": { "login": "charlie" },
                                    "repository_url": "https://api.github.com/repos/canonical/project",
                                    "updated_at": "2026-10-06T12:00:00Z"
                                }]
                            }))
                        } else {
                            Json(json!({ "items": [] }))
                        }
                    }
                }
            }),
        )
        .route(
            "/graphql",
            post({
                let s = s_clone.clone();
                move |Json(_): Json<Value>| {
                    let s = s.clone();
                    async move {
                        if s.gql_fail.load(Ordering::Relaxed) {
                            Json(json!({
                                "errors": [{ "message": "Simulated GraphQL error" }]
                            }))
                        } else {
                            Json(json!({
                                "data": {
                                    "repository": {
                                        "pullRequest": {
                                            "number": 2,
                                            "title": "PR #2 GraphQL",
                                            "bodyText": "GraphQL PR body",
                                            "url": "https://github.com/canonical/project/pull/2",
                                            "state": "MERGED",
                                            "merged": true,
                                            "author": { "login": "charlie" },
                                            "closingIssuesReferences": { "nodes": [] }
                                        }
                                    }
                                }
                            }))
                        }
                    }
                }
            }),
        )
        .route(
            "/repos/canonical/project/pulls/2",
            get(|| async {
                Json(json!({
                    "number": 2,
                    "title": "PR #2 REST Fallback",
                    "body": "PR description from REST fallback",
                    "html_url": "https://github.com/canonical/project/pull/2",
                    "state": "closed",
                    "merged": true,
                    "user": { "login": "charlie" }
                }))
            }),
        )
        .route(
            "/repos/canonical/project/issues/1",
            get(|| async {
                Json(json!({
                    "number": 1,
                    "title": "Issue #1",
                    "body": "Issue description",
                    "html_url": "https://github.com/canonical/project/issues/1",
                    "state": "open",
                    "user": { "login": "bob" }
                }))
            }),
        )
        .route(
            "/repos/canonical/project/issues/1/comments",
            get(|| async { Json(json!([])) }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let connector = GitHubConnector::new(
        &format!("http://{}", addr),
        Some("test_token".into()),
        Some("canonical".into()),
    )
    .unwrap();

    // 1. Health test
    let health = connector.health().await;
    assert!(health.ok);
    assert_eq!(health.auth, "ok");
    assert_eq!(health.rate_limit_remaining, Some(4999));

    // 2. Double query test (no is:issue or is:pr provided)
    let sq = SearchQuery {
        query: "project".into(),
        max_results: 10,
        ..Default::default()
    };
    let items = connector.search(&sq).await.unwrap();
    assert_eq!(items.len(), 2);
    // PR #2 was updated 2026-10-06, Issue #1 was 2026-10-05, so PR #2 comes first
    assert_eq!(items[0].r#ref.id, "canonical/project#2");
    assert_eq!(items[1].r#ref.id, "canonical/project#1");

    // 3. Retry with first 3 words test
    let sq_words = SearchQuery {
        query: "wordone wordtwo wordthree wordfour".into(),
        max_results: 10,
        ..Default::default()
    };
    let items_words = connector.search(&sq_words).await.unwrap();
    assert_eq!(items_words.len(), 1);
    assert_eq!(items_words[0].title, "Fallback match");

    // 4. GraphQL errors -> REST fallback test
    let pr_ref = ObjectRef::new("github", "pr", "canonical/project#2");
    let ev = connector.get_object(&pr_ref).await.unwrap().unwrap();
    assert_eq!(ev.title, "PR #2 REST Fallback");
    assert_eq!(ev.state, "merged");
}
