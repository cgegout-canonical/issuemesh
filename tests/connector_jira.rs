use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use issuemesh::connectors::jira::JiraConnector;
use issuemesh::connectors::{Connector, ObjectRef, SearchQuery};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

struct MockState {
    search_count: AtomicUsize,
}

#[tokio::test]
async fn test_jira_connector_mock_suite() {
    let state = Arc::new(MockState {
        search_count: AtomicUsize::new(0),
    });
    let s_clone = state.clone();

    let app = Router::new()
        // Health check endpoint
        .route(
            "/rest/api/3/myself",
            get(|| async {
                Json(json!({
                    "accountId": "jira-account-123",
                    "displayName": "Jira Admin",
                    "emailAddress": "admin@example.com"
                }))
            }),
        )
        // Search endpoint: /rest/api/3/search/jql
        .route(
            "/rest/api/3/search/jql",
            post({
                let s = s_clone.clone();
                move |Json(body): Json<Value>| {
                    let s = s.clone();
                    async move {
                        s.search_count.fetch_add(1, Ordering::Relaxed);
                        let jql = body.get("jql").and_then(|x| x.as_str()).unwrap_or("");
                        let page_token = body.get("nextPageToken").and_then(|x| x.as_str());

                        if jql.contains("OPEN-4679") {
                            return Json(json!({
                                "total": 1,
                                "issues": [{
                                    "key": "OPEN-4679",
                                    "fields": {
                                        "summary": "Fix broken relation indexing",
                                        "description": {
                                            "type": "doc",
                                            "version": 1,
                                            "content": [{
                                                "type": "paragraph",
                                                "content": [{
                                                    "type": "text",
                                                    "text": "Mentions LP#2167315 and canonical/project#1"
                                                }]
                                            }]
                                        },
                                        "status": { "name": "In Progress" },
                                        "reporter": { "displayName": "Alice Engineer" },
                                        "created": "2026-02-01T10:00:00.000+0000",
                                        "updated": "2026-02-02T10:00:00.000+0000",
                                        "project": { "key": "OPEN" }
                                    }
                                }]
                            }));
                        }

                        // Pagination test: page 1 returns token, page 2 finishes
                        if page_token.is_none() {
                            Json(json!({
                                "total": 2,
                                "nextPageToken": "page-2-token",
                                "issues": [{
                                    "key": "OPEN-101",
                                    "fields": {
                                        "summary": "First Issue",
                                        "status": { "name": "Open" },
                                        "project": { "key": "OPEN" }
                                    }
                                }]
                            }))
                        } else {
                            Json(json!({
                                "total": 2,
                                "issues": [{
                                    "key": "OPEN-102",
                                    "fields": {
                                        "summary": "Second Issue",
                                        "status": { "name": "Done" },
                                        "project": { "key": "OPEN" }
                                    }
                                }]
                            }))
                        }
                    }
                }
            }),
        )
        // Issue detail endpoint: /rest/api/3/issue/OPEN-4679
        .route(
            "/rest/api/3/issue/OPEN-4679",
            get(|| async {
                Json(json!({
                    "key": "OPEN-4679",
                    "fields": {
                        "summary": "Fix broken relation indexing",
                        "description": {
                            "type": "doc",
                            "version": 1,
                            "content": [{
                                "type": "paragraph",
                                "content": [{
                                    "type": "text",
                                    "text": "Detailed ADF description with LP#2167315"
                                }]
                            }]
                        },
                        "status": { "name": "In Progress" },
                        "reporter": { "displayName": "Alice Engineer" },
                        "created": "2026-02-01T10:00:00.000+0000",
                        "updated": "2026-02-02T10:00:00.000+0000",
                        "project": { "key": "OPEN" },
                        "parent": { "key": "OPEN-1000" },
                        "issuelinks": [{
                            "type": { "outward": "blocks" },
                            "outwardIssue": { "key": "OPEN-5000" }
                        }]
                    },
                    "changelog": {
                        "histories": [{
                            "created": "2026-02-02T09:00:00.000+0000",
                            "author": { "displayName": "Bob Tech" },
                            "items": [{
                                "field": "status",
                                "fromString": "Open",
                                "toString": "In Progress"
                            }]
                        }]
                    }
                }))
            }),
        )
        // Issue comments endpoint
        .route(
            "/rest/api/3/issue/OPEN-4679/comment",
            get(|| async {
                Json(json!({
                    "total": 1,
                    "comments": [{
                        "author": { "displayName": "Charlie QA" },
                        "created": "2026-02-02T11:00:00.000+0000",
                        "body": {
                            "type": "doc",
                            "version": 1,
                            "content": [{
                                "type": "paragraph",
                                "content": [{
                                    "type": "text",
                                    "text": "Tested and verified against https://github.com/canonical/project/pull/2"
                                }]
                            }]
                        }
                    }]
                }))
            }),
        )
        // Remote link endpoint
        .route(
            "/rest/api/3/issue/OPEN-4679/remotelink",
            get(|| async {
                Json(json!([
                    {
                        "object": {
                            "url": "https://github.com/canonical/project/pull/2",
                            "title": "PR #2"
                        }
                    },
                    {
                        "object": {
                            "url": "https://bugs.launchpad.net/bugs/2167315",
                            "title": "Bug 2167315"
                        }
                    }
                ]))
            }),
        )
        // 404 issue
        .route(
            "/rest/api/3/issue/NONEXIST-999",
            get(|| async {
                (StatusCode::NOT_FOUND, "Issue does not exist")
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let connector = JiraConnector::new(
        &base_url,
        Some("admin@example.com".into()),
        Some("secret-token".into()),
        Some("OPEN".into()),
    )
    .unwrap();

    // 1. Health check
    let health = connector.health().await;
    assert!(health.ok);
    assert_eq!(health.auth, "ok");
    assert!(health.detail.contains("Connected as Jira Admin"));

    // 2. Search with pagination (fetches OPEN-101 and OPEN-102)
    let sq = SearchQuery {
        query: "".into(),
        projects: vec!["OPEN".into()],
        max_results: 10,
        ..Default::default()
    };
    let items = connector.search(&sq).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].r#ref.id, "OPEN-101");
    assert_eq!(items[1].r#ref.id, "OPEN-102");

    // 3. get_object for OPEN-4679 with ADF description
    let issue_ref = ObjectRef::new("jira", "issue", "OPEN-4679");
    let ev = connector
        .get_object(&issue_ref)
        .await
        .unwrap()
        .expect("issue found");
    assert_eq!(ev.title, "Fix broken relation indexing");
    assert_eq!(ev.state, "In Progress");
    assert_eq!(ev.author, "Alice Engineer");
    assert!(ev
        .body
        .as_ref()
        .unwrap()
        .contains("Detailed ADF description with LP#2167315"));

    // 4. get_thread for OPEN-4679 comments (returns issue body + comments)
    let thread = connector
        .get_thread(&issue_ref, 10)
        .await
        .unwrap()
        .expect("thread found");
    assert_eq!(thread.comments.len(), 2);
    assert_eq!(thread.comments[0].author, "Alice Engineer");
    assert_eq!(thread.comments[1].author, "Charlie QA");
    assert!(thread.comments[1].body.contains("Tested and verified"));

    // 5. get_related for parent, issue link, remote links, and text patterns
    let relations = connector.get_related(&issue_ref, None).await.unwrap();
    let to_ids: Vec<_> = relations.iter().map(|r| r.to.id.as_str()).collect();
    // Parent
    assert!(to_ids.contains(&"OPEN-1000"));
    // Issuelink blocks
    assert!(to_ids.contains(&"OPEN-5000"));
    // Remotelink GitHub PR
    assert!(to_ids.contains(&"canonical/project#2"));
    // Remotelink Launchpad bug
    assert!(to_ids.contains(&"2167315"));

    // 6. 404 returns Ok(None)
    let not_found_ref = ObjectRef::new("jira", "issue", "NONEXIST-999");
    let not_found = connector.get_object(&not_found_ref).await.unwrap();
    assert!(not_found.is_none());
}
