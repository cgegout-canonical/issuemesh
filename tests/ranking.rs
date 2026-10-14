use tempfile::tempdir;

use issuemesh::connectors::{Evidence, ObjectRef};
use issuemesh::search::{
    compute_freshness_score, compute_graph_proximity_score, compute_hybrid_score,
    cosine_similarity, Weights,
};
use issuemesh::store::Store;

#[test]
fn test_cosine_similarity() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![1.0, 0.0, 0.0];
    assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);

    let c = vec![0.0, 1.0, 0.0];
    assert!((cosine_similarity(&a, &c) - 0.0).abs() < 1e-5);

    let d = vec![-1.0, 0.0, 0.0];
    assert!((cosine_similarity(&a, &d) - (-1.0)).abs() < 1e-5);

    // Empty or mismatched lengths
    assert_eq!(cosine_similarity(&[], &[]), 0.0);
    assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);

    // Zero vectors
    let zeros = vec![0.0, 0.0, 0.0];
    assert_eq!(cosine_similarity(&a, &zeros), 0.0);
}

#[test]
fn test_freshness_decay() {
    let now = chrono::Utc::now();
    // Identical now
    let score_now = compute_freshness_score(Some(now), now);
    assert!((score_now - 1.0).abs() < 1e-3);

    // 30 days ago -> half-life of 30 days => ~0.5
    let thirty_days_ago = now - chrono::Duration::days(30);
    let score_30 = compute_freshness_score(Some(thirty_days_ago), now);
    assert!((score_30 - 0.5).abs() < 0.05);

    // 60 days ago -> 2 half-lives => ~0.25
    let sixty_days_ago = now - chrono::Duration::days(60);
    let score_60 = compute_freshness_score(Some(sixty_days_ago), now);
    assert!((score_60 - 0.25).abs() < 0.05);

    // None timestamp defaults to base 0.2
    let score_none = compute_freshness_score(None, now);
    assert!((score_none - 0.2).abs() < 1e-4);
}

#[test]
fn test_graph_proximity_score() {
    assert_eq!(compute_graph_proximity_score(0), 1.0);
    assert_eq!(compute_graph_proximity_score(1), 0.8);
    assert_eq!(compute_graph_proximity_score(2), 0.5);
    assert_eq!(compute_graph_proximity_score(3), 0.2);
    assert_eq!(compute_graph_proximity_score(4), 0.05);
}

#[test]
fn test_hybrid_score_calculation() {
    let weights = Weights::default(); // 0.35, 0.35, 0.20, 0.10
    let score = compute_hybrid_score(&weights, 1.0, 1.0, 1.0, 1.0);
    assert!((score - 1.0).abs() < 1e-4);

    let partial = compute_hybrid_score(&weights, 1.0, 0.0, 0.0, 0.0);
    assert!((partial - 0.35).abs() < 1e-4);

    let partial2 = compute_hybrid_score(&weights, 0.0, 1.0, 0.0, 0.0);
    assert!((partial2 - 0.35).abs() < 1e-4);
}

#[tokio::test]
async fn test_store_embeddings_and_dimension_guard() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = Store::open(&db_path).unwrap();

    let obj1 = Evidence {
        r#ref: ObjectRef::new("github", "issue", "101"),
        title: "Test Issue 101".to_string(),
        snippet: "Snippet 101".to_string(),
        body: Some("Body 101".to_string()),
        url: "https://github.com/org/repo/issues/101".to_string(),
        container: "org/repo".to_string(),
        author: "alice".to_string(),
        created_at: Some("2026-01-01T00:00:00Z".to_string()),
        updated_at: Some("2026-01-02T00:00:00Z".to_string()),
        state: "open".to_string(),
        native: Default::default(),
    };
    store.upsert_object(obj1.clone()).await.unwrap();

    let vec1 = vec![0.1f32, 0.2f32, 0.3f32];
    store
        .upsert_embedding("github:issue:101", &vec1, "text-embedding-3-small")
        .await
        .unwrap();

    // Query valid embeddings
    let valid = store
        .get_valid_embeddings("text-embedding-3-small", 3)
        .await
        .unwrap();
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].0, "github:issue:101");
    assert_eq!(valid[0].1, vec1);

    // Dimension mismatch query returns empty
    let invalid_dim = store
        .get_valid_embeddings("text-embedding-3-small", 4)
        .await
        .unwrap();
    assert_eq!(invalid_dim.len(), 0);

    // Model mismatch query returns empty
    let invalid_model = store
        .get_valid_embeddings("text-embedding-ada-002", 3)
        .await
        .unwrap();
    assert_eq!(invalid_model.len(), 0);

    // Check and mark stale embeddings when active model switches
    let stale_count = store
        .check_and_mark_stale_embeddings("text-embedding-3-large", 1536)
        .await
        .unwrap();
    assert_eq!(stale_count, 1);

    // Now previous embedding is stale and won't be returned even with its old model name
    let valid_after = store
        .get_valid_embeddings("text-embedding-3-small", 3)
        .await
        .unwrap();
    assert_eq!(valid_after.len(), 0);
}

#[tokio::test]
async fn test_concept_overlap_and_pairwise_scoring() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test_rank_concepts.db");
    let store = Store::open(&db_path).unwrap();

    let cid1 = store.upsert_concept("bgp routing", "feature").await.unwrap();
    let cid2 = store.upsert_concept("frr daemon", "component").await.unwrap();
    let cid3 = store.upsert_concept("memory leak", "problem").await.unwrap();

    store.link_object_concept("jira:issue:A", cid1, 1.0, "title", "hA").await.unwrap();
    store.link_object_concept("jira:issue:A", cid2, 0.8, "body", "hA").await.unwrap();

    store.link_object_concept("github:pull:B", cid1, 0.9, "title", "hB").await.unwrap();
    store.link_object_concept("github:pull:B", cid2, 0.8, "body", "hB").await.unwrap();

    store.link_object_concept("launchpad:bug:C", cid3, 1.0, "title", "hC").await.unwrap();

    let ocs_a = store.get_concepts_for_object("jira:issue:A").await.unwrap();
    let ocs_b = store.get_concepts_for_object("github:pull:B").await.unwrap();
    let ocs_c = store.get_concepts_for_object("launchpad:bug:C").await.unwrap();

    // High overlap between A and B
    let overlap_ab = issuemesh::concepts::compute_pairwise_concept_overlap(&ocs_a, &ocs_b);
    assert!(overlap_ab >= 0.85);

    // Near zero overlap between A and C
    let overlap_ac = issuemesh::concepts::compute_pairwise_concept_overlap(&ocs_a, &ocs_c);
    assert_eq!(overlap_ac, 0.0);

    // Overlap with query concept set
    let mut query_set = std::collections::HashSet::new();
    query_set.insert(cid1);
    let q_score_a = issuemesh::concepts::compute_concept_overlap_score(&ocs_a, &query_set);
    assert!(q_score_a > 0.5);
}

