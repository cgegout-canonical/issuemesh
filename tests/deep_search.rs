use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::paths::Paths;
use issuemesh::search::{analyze_consistency, SearchOrchestrator, Severity};
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

#[test]
fn test_consistency_check_1_jira_done_pr_open() {
    let jira = make_evidence("jira:issue:OPEN-100", "Fix BGP", "Done", None, None);
    let gh_pr = make_evidence("github:pr:org/repo#10", "Fix BGP PR", "Open", None, None);

    let rel = Relation {
        from: jira.r#ref.clone(),
        to: gh_pr.r#ref.clone(),
        relation_type: "implements".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };

    // Triggered case: Jira Done + PR Open
    let findings = analyze_consistency(&[jira.clone(), gh_pr.clone()], std::slice::from_ref(&rel));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].finding_type, "jira_done_pr_open");
    assert_eq!(findings[0].severity, Severity::Error);

    // Negative case (not triggered): Jira In Progress + PR Open
    let mut jira_in_progress = jira;
    jira_in_progress.state = "In Progress".to_string();
    let no_findings = analyze_consistency(&[jira_in_progress, gh_pr], &[rel]);
    assert!(no_findings.is_empty());
}

#[test]
fn test_consistency_check_2_lp_released_pr_unmerged() {
    let lp = make_evidence(
        "launchpad:bug:3000000",
        "Crash on boot",
        "Fix Released",
        None,
        None,
    );
    let gh_pr = make_evidence("github:pr:org/repo#20", "PR for crash", "open", None, None);

    let rel = Relation {
        from: lp.r#ref.clone(),
        to: gh_pr.r#ref.clone(),
        relation_type: "fixes".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };

    // Triggered case: LP Fix Released + PR open
    let findings = analyze_consistency(&[lp.clone(), gh_pr.clone()], std::slice::from_ref(&rel));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].finding_type, "launchpad_released_pr_unmerged");
    assert_eq!(findings[0].severity, Severity::Error);

    // Negative case: LP Fix Released + PR merged
    let mut gh_pr_merged = gh_pr;
    gh_pr_merged.state = "merged".to_string();
    let no_findings = analyze_consistency(&[lp, gh_pr_merged], &[rel]);
    assert!(no_findings.is_empty());
}

#[test]
fn test_consistency_check_3_blocked_by_resolved() {
    let blocker = make_evidence("jira:issue:OPEN-50", "Prerequisite", "Closed", None, None);
    let blocked = make_evidence("jira:issue:OPEN-51", "Blocked task", "Open", None, None);

    let rel = Relation {
        from: blocked.r#ref.clone(),
        to: blocker.r#ref.clone(),
        relation_type: "blocked_by".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };

    // Triggered case: Blocked task is open while blocker is Closed
    let findings = analyze_consistency(
        &[blocked.clone(), blocker.clone()],
        std::slice::from_ref(&rel),
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].finding_type, "blocked_by_resolved_item");
    assert_eq!(findings[0].severity, Severity::Info);

    // Negative case: Blocker is still In Progress
    let mut blocker_open = blocker;
    blocker_open.state = "In Progress".to_string();
    let no_findings = analyze_consistency(&[blocked, blocker_open], &[rel]);
    assert!(no_findings.is_empty());
}

#[test]
fn test_consistency_check_4_unreciprocated_fix_reference() {
    // Triggered case: PR body contains "Fixes OPEN-123", but no relation in datastore
    let gh_pr = make_evidence(
        "github:pr:org/repo#30",
        "PR title",
        "open",
        Some("This PR Fixes OPEN-123 directly."),
        None,
    );

    let findings = analyze_consistency(std::slice::from_ref(&gh_pr), &[]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].finding_type, "unreciprocated_fix_reference");
    assert_eq!(findings[0].severity, Severity::Warning);

    // Negative case: Relation is confirmed
    let target = ObjectRef::new("jira", "issue", "OPEN-123");
    let confirmed_rel = Relation {
        from: gh_pr.r#ref.clone(),
        to: target,
        relation_type: "fixes".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };
    let no_findings = analyze_consistency(&[gh_pr], &[confirmed_rel]);
    assert!(no_findings.is_empty());
}

#[test]
fn test_consistency_check_5_release_version_with_open_items() {
    let mut native_released = BTreeMap::new();
    native_released.insert(
        "fixVersions".to_string(),
        serde_json::json!([{ "name": "1.0", "released": true }]),
    );

    let jira_released = make_evidence(
        "jira:issue:OPEN-200",
        "Release 1.0 tracker",
        "Released",
        None,
        Some(native_released),
    );
    let impl_task = make_evidence(
        "github:pr:org/repo#40",
        "Remaining work",
        "open",
        None,
        None,
    );

    let rel = Relation {
        from: jira_released.r#ref.clone(),
        to: impl_task.r#ref.clone(),
        relation_type: "implements".to_string(),
        detected_by: "native".to_string(),
        confidence: 1.0,
    };

    // Triggered case: Jira release is complete but related implementation PR remains open
    let findings = analyze_consistency(
        &[jira_released.clone(), impl_task.clone()],
        std::slice::from_ref(&rel),
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].finding_type, "release_version_with_open_items");
    assert_eq!(findings[0].severity, Severity::Error);

    // Negative case: Related implementation task is closed/merged
    let mut impl_task_closed = impl_task;
    impl_task_closed.state = "closed".to_string();
    let no_findings = analyze_consistency(&[jira_released, impl_task_closed], &[rel]);
    assert!(no_findings.is_empty());
}

#[tokio::test]
async fn test_deep_search_pipeline_end_to_end_on_fixture_graph() {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    let store = Store::open(&paths.db_file).unwrap();

    // Populate SPEC.md §65 Fixture Graph:
    // Jira OPEN-100 --(implements)--> GitHub org/repo#200 --(fixes)--> Launchpad #3000000
    let jira = make_evidence(
        "jira:issue:OPEN-100",
        "Implement high-availability BGP routing in Sunbeam",
        "Done",
        Some("Core implementation for Sunbeam BGP high availability"),
        None,
    );
    let gh_pr = make_evidence(
        "github:pr:canonical/sunbeam#200",
        "Sunbeam BGP HA implementation",
        "Open", // Intentionally open to test consistency finding during pipeline run!
        Some("Implements BGP HA. Fixes LP #3000000"),
        None,
    );
    let lp_bug = make_evidence(
        "launchpad:bug:3000000",
        "BGP daemon failover broken",
        "Confirmed",
        Some("Failover fails when peer switch reboots"),
        None,
    );

    store.upsert_object(jira.clone()).await.unwrap();
    store.upsert_object(gh_pr.clone()).await.unwrap();
    store.upsert_object(lp_bug.clone()).await.unwrap();

    store
        .insert_relations(vec![
            Relation {
                from: jira.r#ref.clone(),
                to: gh_pr.r#ref.clone(),
                relation_type: "implements".to_string(),
                detected_by: "native".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: gh_pr.r#ref.clone(),
                to: lp_bug.r#ref.clone(),
                relation_type: "fixes".to_string(),
                detected_by: "native".to_string(),
                confidence: 1.0,
            },
        ])
        .await
        .unwrap();

    let mut cfg = Config::default();
    cfg.github.enabled = false;
    cfg.jira.enabled = false;
    cfg.launchpad.enabled = false;
    cfg.model.enabled = false;

    let app_state = Arc::new(AppState {
        paths,
        config: Arc::new(RwLock::new(cfg)),
        secrets: Arc::new(RwLock::new(Secrets::default())),
        store,
        jobs: issuemesh::jobs::JobManager::new(),
    });

    let job_id = app_state
        .jobs
        .create_job("Sunbeam BGP HA".to_string())
        .await;

    // Run pipeline end-to-end starting with query matching OPEN-100
    let result =
        SearchOrchestrator::execute(app_state.clone(), Some(&job_id), "Sunbeam BGP HA").await;

    // Verify all graph components discovered
    assert!(result.ranked_results.len() >= 3);
    let keys: Vec<String> = result
        .ranked_results
        .iter()
        .map(|r| r.evidence.r#ref.to_key())
        .collect();
    assert!(keys.contains(&"jira:issue:OPEN-100".to_string()));
    assert!(keys.contains(&"github:pr:canonical/sunbeam#200".to_string()));
    assert!(keys.contains(&"launchpad:bug:3000000".to_string()));

    // Verify consistency engine identified the Jira Done vs PR Open inconsistency
    assert!(!result.findings.is_empty());
    assert!(result
        .findings
        .iter()
        .any(|f| f.finding_type == "jira_done_pr_open"));

    // Verify job status updated to Completed
    let summary = app_state.jobs.get_job_summary(&job_id).await.unwrap();
    assert_eq!(summary.status, issuemesh::jobs::JobStatus::Completed);
}
