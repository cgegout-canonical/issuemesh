use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use issuemesh::connectors::launchpad::LaunchpadConnector;
use issuemesh::connectors::{Connector, ObjectRef, SearchQuery};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

struct MockState {
    request_count: AtomicUsize,
}

#[tokio::test]
async fn test_launchpad_connector_mock_suite() {
    let state = Arc::new(MockState {
        request_count: AtomicUsize::new(0),
    });
    let s_clone = state.clone();

    let app = Router::new()
        // Health endpoint: /1.0/bugs/1
        .route(
            "/1.0/bugs/1",
            get(|| async {
                Json(json!({
                    "id": 1,
                    "title": "Bug #1 sample",
                    "description": "Sample bug for health"
                }))
            }),
        )
        // Search endpoint: /1.0/ubuntu with ws.op=searchTasks
        .route(
            "/1.0/ubuntu",
            get({
                let s = s_clone.clone();
                move |Query(params): Query<HashMap<String, String>>| {
                    let s = s.clone();
                    async move {
                        s.request_count.fetch_add(1, Ordering::Relaxed);
                        let text = params.get("search_text").cloned().unwrap_or_default();
                        let ws_start = params.get("ws.start").and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);

                        // Test retry with 3 words
                        if text == "one two three four" {
                            // Empty response triggers retry
                            return Json(json!({
                                "total_size": 0,
                                "start": 0,
                                "entries": []
                            }));
                        }
                        if text == "one two three" {
                            return Json(json!({
                                "total_size": 1,
                                "start": 0,
                                "entries": [{
                                    "web_link": "https://bugs.launchpad.net/ubuntu/+source/test/+bug/2000001",
                                    "title": "Task for bug 2000001",
                                    "status": "Fix Committed",
                                    "importance": "High",
                                    "bug_link": "https://api.launchpad.net/1.0/bugs/2000001",
                                    "date_created": "2026-09-01T10:00:00Z",
                                    "date_last_updated": "2026-09-02T10:00:00Z"
                                }]
                            }));
                        }

                        // Test pagination: 2 pages
                        if ws_start == 0 {
                            Json(json!({
                                "total_size": 2,
                                "start": 0,
                                "next_collection_link": "https://api.launchpad.net/1.0/ubuntu?ws.start=1",
                                "entries": [{
                                    "web_link": "https://bugs.launchpad.net/ubuntu/+source/pkg/+bug/2167315",
                                    "title": "Task for bug 2167315",
                                    "status": "Fix Released",
                                    "importance": "Medium",
                                    "bug_link": "https://api.launchpad.net/1.0/bugs/2167315",
                                    "date_created": "2026-01-01T00:00:00Z",
                                    "date_last_updated": "2026-01-02T00:00:00Z"
                                }]
                            }))
                        } else {
                            Json(json!({
                                "total_size": 2,
                                "start": 1,
                                "entries": [{
                                    "web_link": "https://bugs.launchpad.net/ubuntu/+source/pkg/+bug/2167316",
                                    "title": "Task for bug 2167316",
                                    "status": "Triaged",
                                    "importance": "Low",
                                    "bug_link": "https://api.launchpad.net/1.0/bugs/2167316",
                                    "date_created": "2026-01-03T00:00:00Z",
                                    "date_last_updated": "2026-01-04T00:00:00Z"
                                }]
                            }))
                        }
                    }
                }
            }),
        )
        // Bug get_object: /1.0/bugs/2167315
        .route(
            "/1.0/bugs/2167315",
            get(|| async {
                Json(json!({
                    "id": 2167315,
                    "title": "LP Bug 2167315 Title",
                    "description": "Fixes GH#canonical/project#1 and mentions OPEN-100",
                    "web_link": "https://bugs.launchpad.net/bugs/2167315",
                    "date_created": "2026-01-01T00:00:00Z",
                    "date_last_updated": "2026-01-02T00:00:00Z",
                    "tags": ["core24", "security"],
                    "owner_link": "https://api.launchpad.net/1.0/~user",
                    "bug_tasks_collection_link": "https://api.launchpad.net/1.0/bugs/2167315/bug_tasks",
                    "messages_collection_link": "https://api.launchpad.net/1.0/bugs/2167315/messages",
                    "activity_collection_link": "https://api.launchpad.net/1.0/bugs/2167315/activity"
                }))
            }),
        )
        // Bug tasks: /1.0/bugs/2167315/bug_tasks
        .route(
            "/1.0/bugs/2167315/bug_tasks",
            get(|| async {
                Json(json!({
                    "total_size": 1,
                    "start": 0,
                    "entries": [{
                        "bug_target_name": "ubuntu",
                        "status": "Fix Released",
                        "importance": "High",
                        "assignee_link": "https://api.launchpad.net/1.0/~dev"
                    }]
                }))
            }),
        )
        // Messages: /1.0/bugs/2167315/messages
        .route(
            "/1.0/bugs/2167315/messages",
            get(|| async {
                Json(json!({
                    "total_size": 2,
                    "start": 0,
                    "entries": [
                        {
                            "index": 0,
                            "content": "Initial description text",
                            "owner_link": "https://api.launchpad.net/1.0/~alice",
                            "date_created": "2026-01-01T00:00:00Z"
                        },
                        {
                            "index": 1,
                            "content": "A follow-up comment referencing https://github.com/foo/bar/pull/12",
                            "owner_link": "https://api.launchpad.net/1.0/~bob",
                            "date_created": "2026-01-01T01:00:00Z"
                        }
                    ]
                }))
            }),
        )
        // Activity: /1.0/bugs/2167315/activity
        .route(
            "/1.0/bugs/2167315/activity",
            get(|| async {
                Json(json!({
                    "total_size": 1,
                    "start": 0,
                    "entries": [{
                        "whodunnit_link": "https://api.launchpad.net/1.0/~charlie",
                        "datechanged": "2026-01-02T00:00:00Z",
                        "what_changed": "status: In Progress => Fix Released"
                    }]
                }))
            }),
        )
        // Non-existent bug 404 test
        .route(
            "/1.0/bugs/9999999",
            get(|| async {
                (StatusCode::NOT_FOUND, "Not found")
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}/1.0", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let connector = LaunchpadConnector::new(&base_url, Some("ubuntu".into())).unwrap();

    // 1. Health check
    let health = connector.health().await;
    assert!(health.ok);
    assert_eq!(health.auth, "none");

    // 2. Search with pagination (fetches page 0 and page 1)
    let sq = SearchQuery {
        query: "".into(),
        projects: vec!["ubuntu".into()],
        max_results: 10,
        ..Default::default()
    };
    let items = connector.search(&sq).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].r#ref.id, "2167315");
    assert_eq!(items[1].r#ref.id, "2167316");

    // 3. Quirk: Retry with first 3 words
    let sq_retry = SearchQuery {
        query: "one two three four".into(),
        projects: vec!["ubuntu".into()],
        max_results: 10,
        ..Default::default()
    };
    let retry_items = connector.search(&sq_retry).await.unwrap();
    assert_eq!(retry_items.len(), 1);
    assert_eq!(retry_items[0].r#ref.id, "2000001");

    // 4. get_object for bug 2167315
    let bug_ref = ObjectRef::new("launchpad", "bug", "2167315");
    let ev = connector
        .get_object(&bug_ref)
        .await
        .unwrap()
        .expect("bug found");
    assert_eq!(ev.title, "LP Bug 2167315 Title");
    assert_eq!(ev.state, "Fix Released");
    assert!(ev.body.as_ref().unwrap().contains("canonical/project#1"));

    // Check thread
    let thread = connector
        .get_thread(&bug_ref, 10)
        .await
        .unwrap()
        .expect("thread found");
    assert_eq!(thread.comments.len(), 2);
    assert_eq!(thread.comments[0].author, "alice");
    assert_eq!(thread.comments[1].author, "bob");

    // Check related
    let related = connector.get_related(&bug_ref, None).await.unwrap();
    let to_ids: Vec<_> = related.iter().map(|r| r.to.id.as_str()).collect();
    assert!(to_ids.contains(&"OPEN-100"));

    // 5. get_object 404 returns Ok(None)
    let not_found_ref = ObjectRef::new("launchpad", "bug", "9999999");
    let not_found = connector.get_object(&not_found_ref).await.unwrap();
    assert!(not_found.is_none());
}
