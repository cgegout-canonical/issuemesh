use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::store::Store;
use issuemesh::util::extract_references;
use std::collections::BTreeMap;
use tempfile::tempdir;

fn sample_evidence(key: &str, title: &str, body: &str, state: &str) -> Evidence {
    let r = ObjectRef::from_key(key).expect("valid key");
    Evidence {
        r#ref: r,
        title: title.to_string(),
        snippet: title.to_string(),
        body: Some(body.to_string()),
        url: format!("https://example.com/{}", key),
        container: "canonical/project".to_string(),
        author: "cgegout".to_string(),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-01T12:00:00Z".to_string()),
        state: state.to_string(),
        native: BTreeMap::new(),
    }
}

#[tokio::test]
async fn test_store_objects_upsert_revisions_and_retrieval() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test_store.db");
    let store = Store::open(&db_path).unwrap();

    let obj1 = sample_evidence(
        "jira:issue:OPEN-4679",
        "BGP support in Sunbeam",
        "Initial description pointing to LP#2167315",
        "In Progress",
    );

    // Initial insert
    let changed = store.upsert_object(obj1.clone()).await.unwrap();
    assert!(changed);

    let retrieved = store
        .get_object(&obj1.r#ref)
        .await
        .unwrap()
        .expect("exists");
    assert_eq!(retrieved.title, "BGP support in Sunbeam");
    assert_eq!(retrieved.state, "In Progress");

    // Second upsert identical -> changed is false, no revision created
    let changed_same = store.upsert_object(obj1.clone()).await.unwrap();
    assert!(!changed_same);

    let stats1 = store.get_stats().await.unwrap();
    assert_eq!(stats1.object_count, 1);
    assert_eq!(stats1.revision_count, 0);

    // Third upsert with changed state and body -> changed is true, revision created
    let mut obj1_updated = obj1.clone();
    obj1_updated.state = "Done".to_string();
    obj1_updated.body = Some("Fixed and released in canonical/project#912".to_string());

    let changed_diff = store.upsert_object(obj1_updated.clone()).await.unwrap();
    assert!(changed_diff);

    let stats2 = store.get_stats().await.unwrap();
    assert_eq!(stats2.object_count, 1);
    assert_eq!(stats2.revision_count, 1);

    let retrieved_updated = store
        .get_object(&obj1.r#ref)
        .await
        .unwrap()
        .expect("exists");
    assert_eq!(retrieved_updated.state, "Done");
}

#[tokio::test]
async fn test_relations_deduplication_and_confidence() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test_relations.db");
    let store = Store::open(&db_path).unwrap();

    let rel1 = Relation {
        from: ObjectRef::new("jira", "issue", "OPEN-4679"),
        to: ObjectRef::new("launchpad", "bug", "2167315"),
        relation_type: "references".to_string(),
        detected_by: "id_pattern".to_string(),
        confidence: 0.90,
    };

    store.insert_relation(rel1.clone()).await.unwrap();

    // Insert same relation with higher confidence (e.g. verified native link)
    let rel2 = Relation {
        from: ObjectRef::new("jira", "issue", "OPEN-4679"),
        to: ObjectRef::new("launchpad", "bug", "2167315"),
        relation_type: "references".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.00,
    };

    store.insert_relation(rel2).await.unwrap();

    let rels = store
        .get_relations_for_key("jira:issue:OPEN-4679")
        .await
        .unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].confidence, 1.00);
    assert_eq!(rels[0].detected_by, "native");

    // Also check query by target key
    let rels_target = store
        .get_relations_for_key("launchpad:bug:2167315")
        .await
        .unwrap();
    assert_eq!(rels_target.len(), 1);
}

#[tokio::test]
async fn test_unresolved_refs_lifecycle_and_extraction() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("test_unresolved.db");
    let store = Store::open(&db_path).unwrap();

    let text = "This issue relates to OPEN-5000 and LP#1999999 and fixes canonical/project#888. Ignore CVE-2024-1111.";
    let self_ref = ObjectRef::new("jira", "issue", "OPEN-4679");
    let extracted = extract_references(text, Some(&self_ref));

    assert_eq!(extracted.len(), 3);

    for target in &extracted {
        store
            .record_unresolved_ref(target, &self_ref.to_key())
            .await
            .unwrap();
    }

    let stats = store.get_stats().await.unwrap();
    assert_eq!(stats.unresolved_count, 3);

    let pending = store.get_unresolved_refs(10).await.unwrap();
    assert_eq!(pending.len(), 3);

    // Now upsert one of the targets as a discovered object
    let obj_resolved = sample_evidence(
        "jira:issue:OPEN-5000",
        "Subordinate task",
        "Details here",
        "Open",
    );
    store.upsert_object(obj_resolved).await.unwrap();

    // Verify unresolved_count decremented to 2
    let stats_after = store.get_stats().await.unwrap();
    assert_eq!(stats_after.unresolved_count, 2);

    let pending_after = store.get_unresolved_refs(10).await.unwrap();
    assert_eq!(pending_after.len(), 2);
    let pending_keys: Vec<String> = pending_after.into_iter().map(|(r, _)| r.to_key()).collect();
    assert!(!pending_keys.contains(&"jira:issue:OPEN-5000".to_string()));
}
