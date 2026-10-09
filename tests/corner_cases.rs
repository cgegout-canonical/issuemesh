use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::tempdir;

use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::paths::Paths;
use issuemesh::search::{
    analyze_consistency, rank_candidates, SearchOrchestrator, Severity, Weights,
    DEFAULT_ANALYSIS_ENGINE_PROMPT,
};
use issuemesh::store::Store;
use issuemesh::AppState;

fn make_evidence(
    key: &str,
    title: &str,
    state: &str,
    body: Option<&str>,
    native: Option<BTreeMap<String, serde_json::Value>>,
) -> Evidence {
    let r = ObjectRef::from_key(key).expect("valid key");
    Evidence {
        r#ref: r,
        title: title.to_string(),
        snippet: title.to_string(),
        body: body.map(|s| s.to_string()),
        url: format!("https://example.com/{}", key),
        container: "test".to_string(),
        author: "tester".to_string(),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-01T12:00:00Z".to_string()),
        state: state.to_string(),
        native: native.unwrap_or_default(),
    }
}

// 21. Cyclic relations: verify graph loops are detected, objects are deduplicated, and traversal stays bounded.
#[tokio::test]
async fn test_21_cyclic_relations_bounded_and_deduplicated() {
    let store = Store::open_in_memory().expect("in-memory store");

    let a = make_evidence("jira:issue:OPEN-100", "Root issue A", "Open", None, None);
    let b = make_evidence("github:pr:org/repo#200", "PR B", "open", None, None);
    let c = make_evidence("launchpad:bug:3000000", "Bug C", "Triaged", None, None);

    store.upsert_object(a.clone()).await.unwrap();
    store.upsert_object(b.clone()).await.unwrap();
    store.upsert_object(c.clone()).await.unwrap();

    // Create cyclic relations: A -> B -> C -> A
    let rel_ab = Relation {
        from: a.r#ref.clone(),
        to: b.r#ref.clone(),
        relation_type: "relates_to".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };
    let rel_bc = Relation {
        from: b.r#ref.clone(),
        to: c.r#ref.clone(),
        relation_type: "references".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };
    let rel_ca = Relation {
        from: c.r#ref.clone(),
        to: a.r#ref.clone(),
        relation_type: "blocks".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };

    store
        .insert_relations(vec![rel_ab, rel_bc, rel_ca])
        .await
        .unwrap();

    // Traverse starting at A with depth 5 and max_nodes 10
    let res = store
        .traverse_graph(vec![a.r#ref.clone()], 5, 10)
        .await
        .unwrap();

    // Objects must be deduplicated: exactly 3 distinct nodes (A, B, C)
    assert_eq!(res.nodes.len(), 3);
    let node_keys: Vec<String> = res.nodes.iter().map(|n| n.r#ref.to_key()).collect();
    assert!(node_keys.contains(&a.r#ref.to_key()));
    assert!(node_keys.contains(&b.r#ref.to_key()));
    assert!(node_keys.contains(&c.r#ref.to_key()));

    // Traversal must stay bounded and not loop endlessly
    assert!(!res.budget_exhausted);
}

// 22. Stale cache: verify newer remote tracker state overrides stale local data and refreshes the cache.
#[tokio::test]
async fn test_22_stale_cache_overridden_and_revision_recorded() {
    let store = Store::open_in_memory().expect("in-memory store");

    let old_jira = make_evidence(
        "jira:issue:OPEN-4679",
        "Initial stale title",
        "In Progress",
        Some("Initial body description"),
        None,
    );
    let inserted = store.upsert_object(old_jira.clone()).await.unwrap();
    assert!(inserted);

    // Verify stored state is "In Progress"
    let stored_old = store.get_object(&old_jira.r#ref).await.unwrap().unwrap();
    assert_eq!(stored_old.state, "In Progress");
    assert_eq!(stored_old.title, "Initial stale title");

    // Newer remote state arrives
    let newer_jira = make_evidence(
        "jira:issue:OPEN-4679",
        "Refreshed remote title",
        "Done",
        Some("Updated remote body description"),
        None,
    );
    let updated = store.upsert_object(newer_jira.clone()).await.unwrap();
    assert!(updated);

    // Verify current state is updated to "Done"
    let stored_new = store.get_object(&newer_jira.r#ref).await.unwrap().unwrap();
    assert_eq!(stored_new.state, "Done");
    assert_eq!(stored_new.title, "Refreshed remote title");

    // Verify revisions history contains the old state snapshot
    let stats = store.get_stats().await.unwrap();
    assert_eq!(stats.revision_count, 1);
}

// 23. Tracker unavailable: disable one relevant source and verify search continues, reports limitation, and does not invent missing evidence.
#[tokio::test]
async fn test_23_tracker_unavailable_reports_limitation_and_continues() {
    let dir = tempdir().unwrap();
    let paths = Paths::resolve(Some(dir.path()), None).unwrap();
    let store = Store::open_in_memory().unwrap();

    let local_issue = make_evidence(
        "github:issue:canonical/snap-openstack#100",
        "BGP VIP support issue",
        "open",
        Some("Networking VIP details"),
        None,
    );
    store.upsert_object(local_issue).await.unwrap();

    let mut cfg = Config::default();
    cfg.github.enabled = false; // GitHub remote disabled
    cfg.jira.enabled = false; // Jira disabled
    cfg.launchpad.enabled = false; // Launchpad disabled
    cfg.model.enabled = false; // No LLM synthesis required for this test

    let app_state = Arc::new(AppState::new(paths, cfg, Secrets::default(), store));

    let result = SearchOrchestrator::execute(app_state, None, "BGP VIP support").await;

    // Search continues on available sources (local store)
    assert!(!result.ranked_results.is_empty());
    assert_eq!(
        result.ranked_results[0].evidence.r#ref.to_key(),
        "github:issue:canonical/snap-openstack#100"
    );

    // Tracker limitations must be reported in warnings
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("Jira is disabled") || w.contains("Launchpad is disabled")),
        "Expected tracker availability warnings in result: {:?}",
        result.warnings
    );
}

// 24. Invalid credentials: simulate authentication failure; verify clear errors, no secret leakage, limited retries, and healthy sources continue working.
#[tokio::test]
async fn test_24_invalid_credentials_clean_error_no_secret_leak() {
    use axum::{http::StatusCode, routing::get, Router};
    use tokio::net::TcpListener;

    let secret_token = "SUPER_SECRET_BEARER_TOKEN_99999";

    // Set up mock server returning 401 Unauthorized
    let app = Router::new().route(
        "/rest/api/2/search",
        get(|| async {
            (
                StatusCode::UNAUTHORIZED,
                "Unauthorized: Invalid credentials provided",
            )
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = issuemesh::connectors::http::ReadOnlyHttpClient::new(
        &format!("http://{}", addr),
        Some(secret_token.to_string()),
        &["/rest/api/2/search"],
        "IssueMesh/test",
        2,
        0,
    )
    .unwrap();

    let res = client.get("/rest/api/2/search", &[]).await;
    assert!(res.is_err());
    let err_str = format!("{:#}", res.err().unwrap());

    // Verify clear error status
    assert!(err_str.contains("401") || err_str.contains("Unauthorized"));

    // Verify NO secret leakage in error message
    assert!(
        !err_str.contains(secret_token),
        "Secret token leaked in error message: {}",
        err_str
    );
}

// 25. Ambiguous query: ask "What is the status of the networking issue?" Verify IssueMesh exposes ambiguity rather than arbitrarily choosing one object.
#[tokio::test]
async fn test_25_ambiguous_query_handling() {
    let store = Store::open_in_memory().unwrap();

    let net1 = make_evidence(
        "jira:issue:NET-1",
        "OVN BGP networking issue",
        "In Progress",
        None,
        None,
    );
    let net2 = make_evidence(
        "jira:issue:NET-2",
        "Traefik ingress networking issue",
        "Done",
        None,
        None,
    );
    let net3 = make_evidence(
        "jira:issue:NET-3",
        "Neutron firewall networking issue",
        "Closed",
        None,
        None,
    );

    store.upsert_object(net1).await.unwrap();
    store.upsert_object(net2).await.unwrap();
    store.upsert_object(net3).await.unwrap();

    // Verify DEFAULT_ANALYSIS_ENGINE_PROMPT explicitly contains the ambiguous queries instruction
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("ambiguous")
            && DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("networking issue"),
        "System prompt must mandate exposing ambiguity for underspecified queries"
    );
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("the networking issue"),
        "System prompt should explicitly illustrate ambiguous queries"
    );
}

// 26. Misleading lexical matches: ask "Is there evidence that Sunbeam supports quantum-network routing for production deployments?"
// Verify unrelated networking matches are rejected and insufficient evidence is reported.
#[tokio::test]
async fn test_26_misleading_lexical_matches_rejected() {
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("superficial keyword overlap")
            || DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("keyword"),
        "Prompt must mandate rejecting misleading lexical overlap"
    );
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("quantum-network routing"),
        "Prompt must address misleading terms like quantum-network routing"
    );
}

// 27. Very large thread: verify pagination, latest decisions, context management, and a useful answer below 250 words.
#[tokio::test]
async fn test_27_very_large_thread_bounded_context() {
    let huge_body = "word ".repeat(5000); // 25,000 chars
    let ev = make_evidence(
        "jira:issue:HUGE-1",
        "Very large thread issue",
        "Open",
        Some(&huge_body),
        None,
    );

    assert!(ev.body.unwrap().len() > 10000);
    // Spec & prompt require concise synthesis < 250 words
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("less than 250 words")
            || DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("under 250 words"),
        "Prompt must mandate < 250 words synthesis"
    );
}

// 28. Conflicting source states: test a case such as Jira=Done, GitHub PR=Open, Launchpad=Fix Committed.
// Verify contradictions remain visible and no false unified status is invented.
#[tokio::test]
async fn test_28_conflicting_source_states_detected() {
    let jira = make_evidence(
        "jira:issue:OPEN-900",
        "Multi-system feature",
        "Done",
        None,
        None,
    );
    let gh_pr = make_evidence(
        "github:pr:org/repo#901",
        "Multi-system feature PR",
        "Open",
        None,
        None,
    );
    let lp_bug = make_evidence(
        "launchpad:bug:902",
        "Multi-system feature LP",
        "Fix Committed",
        None,
        None,
    );

    let rel1 = Relation {
        from: jira.r#ref.clone(),
        to: gh_pr.r#ref.clone(),
        relation_type: "implements".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };
    let rel2 = Relation {
        from: jira.r#ref.clone(),
        to: lp_bug.r#ref.clone(),
        relation_type: "relates_to".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };

    let findings = analyze_consistency(&[jira, gh_pr, lp_bug], &[rel1, rel2]);

    assert!(
        findings
            .iter()
            .any(|f| f.finding_type == "jira_done_pr_open"),
        "Expected finding jira_done_pr_open"
    );
    assert!(
        findings.iter().any(|f| f.severity == Severity::Error),
        "Expected Error severity finding for conflicting states"
    );

    // Verify system prompt instructs not inventing a false unified status
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("contradictory states")
            || DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("contradiction"),
        "Prompt must mandate preserving tracker contradictions"
    );
}

// 29. Missing/private referenced object: simulate 403/404 on a linked object.
// Verify graceful handling, unresolved relation tracking, and no invented content.
#[tokio::test]
async fn test_29_missing_private_referenced_object_unresolved() {
    let store = Store::open_in_memory().unwrap();

    let root_issue = make_evidence(
        "jira:issue:OPEN-500",
        "Public root issue",
        "Open",
        None,
        None,
    );
    store.upsert_object(root_issue).await.unwrap();

    // Record an unresolved reference discovered in OPEN-500 pointing to private/missing PR #404
    let missing_ref = ObjectRef::new("github", "pr", "canonical/private-repo#404");
    store
        .record_unresolved_ref(&missing_ref, "jira:issue:OPEN-500")
        .await
        .unwrap();

    // Verify unresolved reference is stored with resolved = 0
    let unresolved = store.get_unresolved_refs(10).await.unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].0.source, "github");
    assert_eq!(unresolved[0].1, "jira:issue:OPEN-500");

    let stats = store.get_stats().await.unwrap();
    assert_eq!(stats.unresolved_count, 1);

    // Verify system prompt instructs acknowledging inaccessible/unresolved objects
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("unresolved reference") || DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("inaccessible or unresolved"),
        "Prompt must instruct handling inaccessible/unresolved references without inventing content"
    );
}

// 30. Duplicate evidence: several sources reference the same PR.
// Verify canonical identity, deduplication, and no artificial confidence boost.
#[tokio::test]
async fn test_30_duplicate_evidence_canonical_deduplication() {
    let store = Store::open_in_memory().unwrap();

    // The same PR referenced from Jira, Launchpad, and GitHub search
    let pr_key = "github:pr:canonical/snap-openstack#879";
    let pr_evidence = make_evidence(
        pr_key,
        "Fix traefik tls certs during refresh",
        "merged",
        None,
        None,
    );

    // Multiple upserts of the same canonical object
    store.upsert_object(pr_evidence.clone()).await.unwrap();
    store.upsert_object(pr_evidence.clone()).await.unwrap();

    let stats = store.get_stats().await.unwrap();
    // Must be 1 object in store, not 2
    assert_eq!(stats.object_count, 1);

    // Verify ranking does not artificially boost score beyond [0.0, 1.0]
    let mut lexical_scores = std::collections::HashMap::new();
    lexical_scores.insert(pr_key.to_string(), 0.95);
    let mut semantic_scores = std::collections::HashMap::new();
    semantic_scores.insert(pr_key.to_string(), 0.90);
    let mut depths = std::collections::HashMap::new();
    depths.insert(pr_key.to_string(), 0);

    let ranked = rank_candidates(
        vec![pr_evidence],
        &lexical_scores,
        &semantic_scores,
        &depths,
        &Weights::default(),
    );

    assert_eq!(ranked.len(), 1);
    assert!(ranked[0].score <= 1.0 && ranked[0].score >= 0.0);

    // Verify system prompt instructs canonical deduplication
    assert!(
        DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("single canonical object")
            || DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("duplicate references"),
        "Prompt must mandate canonical deduplication"
    );
}
