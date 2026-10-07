use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::store::Store;
use std::collections::BTreeMap;
use tempfile::tempdir;

fn sample_evidence(key: &str, title: &str, body: &str, container: &str) -> Evidence {
    let r = ObjectRef::from_key(key).expect("valid key");
    Evidence {
        r#ref: r,
        title: title.to_string(),
        snippet: title.to_string(),
        body: Some(body.to_string()),
        url: format!("https://example.com/{}", key),
        container: container.to_string(),
        author: "canonical-eng".to_string(),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-01T12:00:00Z".to_string()),
        state: "Open".to_string(),
        native: BTreeMap::new(),
    }
}

#[tokio::test]
async fn test_fts_search_and_ranking() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test_fts.db");
    let store = Store::open(&db_path).unwrap();

    let doc1 = sample_evidence(
        "jira:issue:OPEN-100",
        "Investigate BGP routing in multi-site Sunbeam",
        "Detailed logs show BGP daemon fails when connecting to spine switch",
        "openstack",
    );
    let doc2 = sample_evidence(
        "launchpad:bug:2000001",
        "OVN chassis connectivity failure",
        "Packets dropped on geneve tunnels between chassis",
        "ovn",
    );
    let doc3 = sample_evidence(
        "github:issue:canonical/sunbeam#45",
        "Release notes for 2026.1",
        "Includes BGP improvements and updated multi-site topology support",
        "canonical/sunbeam",
    );

    store.upsert_object(doc1).await.unwrap();
    store.upsert_object(doc2).await.unwrap();
    store.upsert_object(doc3).await.unwrap();

    // Search for "BGP multi-site"
    let results = store.search_fts("BGP multi-site", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    let keys: Vec<String> = results.iter().map(|r| r.evidence.r#ref.to_key()).collect();
    assert!(keys.contains(&"jira:issue:OPEN-100".to_string()));
    assert!(keys.contains(&"github:issue:canonical/sunbeam#45".to_string()));
    assert!(!keys.contains(&"launchpad:bug:2000001".to_string()));

    // Search with no results
    let none = store.search_fts("nonexistentkeywordxyz", 10).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn test_graph_traversal_fixture_cycle_detection_and_budgets() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test_traversal.db");
    let store = Store::open(&db_path).unwrap();

    // Fixture graph:
    // OPEN-100 --(implements)--> github#200 --(fixes)--> LP#3000000
    // LP#3000000 --(references)--> OPEN-100 (creates a cycle)
    // OPEN-100 --(blocks)--> OPEN-101
    let obj_jira = sample_evidence("jira:issue:OPEN-100", "Jira Bug", "Desc", "jira");
    let obj_gh = sample_evidence(
        "github:issue:canonical/repo#200",
        "GH PR",
        "PR body",
        "canonical/repo",
    );
    let obj_lp = sample_evidence("launchpad:bug:3000000", "LP Bug", "LP body", "ubuntu");
    let obj_jira2 = sample_evidence("jira:issue:OPEN-101", "Sub Bug", "Sub body", "jira");

    store.upsert_object(obj_jira).await.unwrap();
    store.upsert_object(obj_gh).await.unwrap();
    store.upsert_object(obj_lp).await.unwrap();
    store.upsert_object(obj_jira2).await.unwrap();

    let rels = vec![
        Relation {
            from: ObjectRef::new("jira", "issue", "OPEN-100"),
            to: ObjectRef::new("github", "issue", "canonical/repo#200"),
            relation_type: "implements".to_string(),
            detected_by: "native".to_string(),
            confidence: 1.0,
        },
        Relation {
            from: ObjectRef::new("github", "issue", "canonical/repo#200"),
            to: ObjectRef::new("launchpad", "bug", "3000000"),
            relation_type: "fixes".to_string(),
            detected_by: "native".to_string(),
            confidence: 1.0,
        },
        Relation {
            from: ObjectRef::new("launchpad", "bug", "3000000"),
            to: ObjectRef::new("jira", "issue", "OPEN-100"),
            relation_type: "references".to_string(),
            detected_by: "id_pattern".to_string(),
            confidence: 0.9,
        },
        Relation {
            from: ObjectRef::new("jira", "issue", "OPEN-100"),
            to: ObjectRef::new("jira", "issue", "OPEN-101"),
            relation_type: "blocks".to_string(),
            detected_by: "native".to_string(),
            confidence: 1.0,
        },
    ];

    store.insert_relations(rels).await.unwrap();

    let seed = ObjectRef::new("jira", "issue", "OPEN-100");

    // 1. Depth 0: only seed node
    let res_d0 = store
        .traverse_graph(vec![seed.clone()], 0, 50)
        .await
        .unwrap();
    assert_eq!(res_d0.nodes.len(), 1);
    assert_eq!(res_d0.nodes[0].r#ref, seed);
    assert!(!res_d0.budget_exhausted);

    // 2. Depth 1: seed + immediate 1-hop neighbors (github#200, LP#3000000, OPEN-101)
    let res_d1 = store
        .traverse_graph(vec![seed.clone()], 1, 50)
        .await
        .unwrap();
    let node_keys_d1: Vec<String> = res_d1.nodes.iter().map(|n| n.r#ref.to_key()).collect();
    assert!(node_keys_d1.contains(&"jira:issue:OPEN-100".to_string()));
    assert!(node_keys_d1.contains(&"github:issue:canonical/repo#200".to_string()));
    assert!(node_keys_d1.contains(&"jira:issue:OPEN-101".to_string()));
    assert!(node_keys_d1.contains(&"launchpad:bug:3000000".to_string())); // reached via cycle edge LP->OPEN-100
    assert_eq!(res_d1.nodes.len(), 4);

    // 3. Cycle handling at Depth 2: visited set prevents infinite loop, all 4 nodes found once
    let res_d2 = store
        .traverse_graph(vec![seed.clone()], 2, 50)
        .await
        .unwrap();
    assert_eq!(res_d2.nodes.len(), 4);
    assert!(!res_d2.budget_exhausted);

    // 4. Budget limit enforcement
    let res_budget = store
        .traverse_graph(vec![seed.clone()], 2, 2)
        .await
        .unwrap();
    assert_eq!(res_budget.nodes.len(), 2);
    assert!(res_budget.budget_exhausted);
}
