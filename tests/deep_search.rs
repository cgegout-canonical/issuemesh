use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::tempdir;

use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::paths::Paths;
use issuemesh::search::{
    analyze_consistency, SearchOrchestrator, Severity, DEFAULT_ANALYSIS_ENGINE_PROMPT,
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

    let app_state = Arc::new(AppState::new(paths, cfg, Secrets::default(), store));

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

#[test]
fn test_default_analysis_engine_prompt_content() {
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("You are the analysis engine for **IssueMesh**, an engineering search system covering **Jira, GitHub and Launchpad**."));
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("## Search behaviour"));
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("## Evidence rules"));
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("Never invent:"));
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("\"I could not find sufficient evidence in Jira, GitHub or Launchpad to answer this reliably.\""));
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT.contains("## Answer quality"));
    assert!(DEFAULT_ANALYSIS_ENGINE_PROMPT
        .contains("The complete answer must be **less than 250 words**."));
}

#[tokio::test]
async fn test_deep_search_synthesis_with_analysis_engine_prompt() {
    use axum::{routing::post, Json, Router};
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    let received_messages: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let rec_clone = received_messages.clone();

    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(payload): Json<serde_json::Value>| {
            let rec = rec_clone.clone();
            async move {
                let msgs = payload["messages"].as_array().cloned().unwrap_or_default();
                let is_synthesis = msgs
                    .first()
                    .and_then(|m| m["content"].as_str())
                    .is_some_and(|c| c.contains("analysis engine"));

                if is_synthesis {
                    let mut lock = rec.lock().unwrap();
                    *lock = msgs;
                    let resp = json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "The root cause in OPEN-100 is high-availability BGP routing failure in Sunbeam. Fix is in canonical/sunbeam#200."
                            }
                        }]
                    });
                    Json(resp)
                } else {
                    let plan = json!({
                        "query": "Sunbeam BGP HA",
                        "sources": ["jira"],
                        "projects": ["OPEN"],
                        "repositories": [],
                        "people": [],
                        "semantic_queries": ["Sunbeam BGP HA"],
                        "graph_depth": 2,
                        "max_remote_results": 50,
                        "max_deep_reads": 30,
                        "max_graph_nodes": 100,
                        "relation_types": ["references", "fixes", "implements"]
                    });
                    let resp = json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": plan.to_string()
                            }
                        }]
                    });
                    Json(resp)
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    let store = Store::open(&paths.db_file).unwrap();

    let jira = make_evidence(
        "jira:issue:OPEN-100",
        "Implement high-availability BGP HA routing in Sunbeam",
        "Done",
        Some("Core implementation for Sunbeam BGP high availability"),
        None,
    );
    store.upsert_object(jira.clone()).await.unwrap();

    let mut cfg = Config::default();
    cfg.github.enabled = false;
    cfg.jira.enabled = false;
    cfg.launchpad.enabled = false;
    cfg.model.enabled = true;
    cfg.model.base_url = Some(base_url);

    let app_state = Arc::new(AppState::new(paths, cfg, Secrets::default(), store));
    let job_id = app_state
        .jobs
        .create_job("Sunbeam BGP HA".to_string())
        .await;

    let result =
        SearchOrchestrator::execute(app_state.clone(), Some(&job_id), "Sunbeam BGP HA").await;

    // Verify model synthesis was called
    assert!(result.answer.is_some());
    let answer = result.answer.unwrap();
    assert!(answer.contains("OPEN-100"));

    // Verify the system prompt sent to model matches DEFAULT_ANALYSIS_ENGINE_PROMPT
    let msgs = received_messages.lock().unwrap().clone();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], DEFAULT_ANALYSIS_ENGINE_PROMPT);

    // Verify user prompt contains formatted query and retrieved evidence
    assert_eq!(msgs[1]["role"], "user");
    let user_prompt = msgs[1]["content"].as_str().unwrap();
    assert!(user_prompt.contains("User Question: Sunbeam BGP HA"));
    assert!(user_prompt.contains("Identifier: OPEN-100"));
}

#[tokio::test]
async fn test_deep_search_synthesis_with_custom_system_prompt_override() {
    use axum::{routing::post, Json, Router};
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    let received_system_prompt = Arc::new(Mutex::new(String::new()));
    let prompt_clone = received_system_prompt.clone();

    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(payload): Json<serde_json::Value>| {
            let prompt_rec = prompt_clone.clone();
            async move {
                let msgs = payload["messages"].as_array().cloned().unwrap_or_default();
                let is_synthesis = msgs
                    .first()
                    .and_then(|m| m["content"].as_str())
                    .is_some_and(|c| c.contains("Custom prompt override"));

                if is_synthesis {
                    let mut lock = prompt_rec.lock().unwrap();
                    *lock = msgs[0]["content"].as_str().unwrap_or_default().to_string();
                    let resp = json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Custom answer from override prompt."
                            }
                        }]
                    });
                    Json(resp)
                } else {
                    let plan = json!({
                        "query": "Sunbeam BGP HA",
                        "sources": ["jira"],
                        "projects": ["OPEN"],
                        "repositories": [],
                        "people": [],
                        "semantic_queries": ["Sunbeam BGP HA"],
                        "graph_depth": 2,
                        "max_remote_results": 50,
                        "max_deep_reads": 30,
                        "max_graph_nodes": 100,
                        "relation_types": ["references"]
                    });
                    Json(json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": plan.to_string()
                            }
                        }]
                    }))
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    let store = Store::open(&paths.db_file).unwrap();

    let jira = make_evidence(
        "jira:issue:OPEN-100",
        "Implement high-availability BGP HA routing in Sunbeam",
        "Done",
        Some("Core implementation for Sunbeam BGP high availability"),
        None,
    );
    store.upsert_object(jira).await.unwrap();

    let mut cfg = Config::default();
    cfg.github.enabled = false;
    cfg.jira.enabled = false;
    cfg.launchpad.enabled = false;
    cfg.model.enabled = true;
    cfg.model.base_url = Some(base_url);
    cfg.model.system_prompt = Some("Custom prompt override: answer concisely.".to_string());

    let app_state = Arc::new(AppState::new(paths, cfg, Secrets::default(), store));
    let job_id = app_state
        .jobs
        .create_job("Sunbeam BGP HA".to_string())
        .await;

    let result =
        SearchOrchestrator::execute(app_state.clone(), Some(&job_id), "Sunbeam BGP HA").await;

    assert_eq!(
        result.answer.as_deref(),
        Some("Custom answer from override prompt.")
    );
    let captured = received_system_prompt.lock().unwrap().clone();
    assert_eq!(captured, "Custom prompt override: answer concisely.");
}

#[tokio::test]
async fn test_top_3_issues_of_sunbeam_relevance() {
    use axum::{routing::post, Json, Router};
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    let received_user_prompt = Arc::new(Mutex::new(String::new()));
    let prompt_clone = received_user_prompt.clone();

    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(payload): Json<serde_json::Value>| {
            let user_rec = prompt_clone.clone();
            async move {
                let msgs = payload["messages"].as_array().cloned().unwrap_or_default();
                let is_synthesis = msgs
                    .first()
                    .and_then(|m| m["content"].as_str())
                    .is_some_and(|c| c.contains("Evidence rules"));

                if is_synthesis {
                    let mut lock = user_rec.lock().unwrap();
                    *lock = msgs.get(1).and_then(|m| m["content"].as_str()).unwrap_or_default().to_string();
                    let resp = json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "1. PF-6540 multiple hostname support for RGW\n2. LP#2166145 placement-k8s units stuck\n3. PF-6538 Sunbeam HA Support"
                            }
                        }]
                    });
                    Json(resp)
                } else {
                    // Planning prompt
                    let plan = json!({
                        "query": "Sunbeam",
                        "sources": ["jira", "launchpad", "github"],
                        "projects": ["Sunbeam"],
                        "repositories": [],
                        "people": [],
                        "semantic_queries": ["Sunbeam"],
                        "graph_depth": 2,
                        "max_remote_results": 10,
                        "max_deep_reads": 10,
                        "max_graph_nodes": 50,
                        "relation_types": ["references", "fixes"]
                    });
                    Json(json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": plan.to_string()
                            }
                        }]
                    }))
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    let store = Store::open(&paths.db_file).unwrap();

    // 1. Insert real Sunbeam issues
    let doc1 = make_evidence(
        "jira:issue:PF-6540",
        "Enable multiple hostname support for RGW in Charmed MicroCeph and Sunbeam",
        "Open",
        Some("Detailed multi-hostname support specification for Sunbeam deployment"),
        None,
    );
    let doc2 = make_evidence(
        "launchpad:bug:2166145",
        "placement-k8s units stuck in waiting after sunbeam cluster resize",
        "Confirmed",
        Some("Units stuck waiting for workload placement api in Sunbeam cluster"),
        None,
    );
    let doc3 = make_evidence(
        "jira:issue:PF-6538",
        "Sunbeam HA Support for Active/Passive Storage Drivers",
        "In Progress",
        Some("High availability architecture for Sunbeam storage backends"),
        None,
    );

    // 2. Insert noise PR with giant body containing the exact question words
    let noise_doc = make_evidence(
        "github:pr:random/repo#758",
        "CI pipeline run #758",
        "Closed",
        Some("what are the top 3 issues of sunbeam? This log contains what are the top 3 issues of sunbeam repeated."),
        None,
    );

    store.upsert_object(doc1).await.unwrap();
    store.upsert_object(doc2).await.unwrap();
    store.upsert_object(doc3).await.unwrap();
    store.upsert_object(noise_doc).await.unwrap();

    let mut cfg = Config::default();
    cfg.github.enabled = false;
    cfg.jira.enabled = false;
    cfg.launchpad.enabled = false;
    cfg.model.enabled = true;
    cfg.model.base_url = Some(base_url);

    let app_state = Arc::new(AppState::new(paths, cfg, Secrets::default(), store));
    let job_id = app_state
        .jobs
        .create_job("what are the top 3 issues of Sunbeam?".to_string())
        .await;

    let result = SearchOrchestrator::execute(
        app_state.clone(),
        Some(&job_id),
        "what are the top 3 issues of Sunbeam?",
    )
    .await;

    assert!(result.answer.is_some());
    let answer = result.answer.unwrap();
    assert!(answer.contains("PF-6540"));
    assert!(answer.contains("LP#2166145"));
    assert!(answer.contains("PF-6538"));

    let user_prompt = received_user_prompt.lock().unwrap().clone();
    assert!(user_prompt.contains("Identifier: PF-6540"));
    assert!(user_prompt.contains("Identifier: LP#2166145"));
    assert!(user_prompt.contains("Identifier: PF-6538"));
}

#[tokio::test]
#[ignore]
async fn test_live_user_sunbeam_search() {
    let paths = Paths::resolve(None, None).unwrap();
    let config = Config::load_or_default(&paths.config_file).unwrap();
    let secrets = Secrets::load_from_file(&paths.secrets_file).unwrap();
    let store = Store::open(&paths.db_file).unwrap();
    let app_state = Arc::new(AppState::new(paths, config, secrets, store));
    let job_id = app_state
        .jobs
        .create_job("what are the top 3 issues of Sunbeam?".to_string())
        .await;
    let result = SearchOrchestrator::execute(
        app_state.clone(),
        Some(&job_id),
        "what are the top 3 issues of Sunbeam?",
    )
    .await;
    println!("=== RESULTS COUNT: {} ===", result.ranked_results.len());
    for (i, r) in result.ranked_results.iter().take(10).enumerate() {
        println!(
            "{}. [{:.2}] {} ({}) - {}",
            i + 1,
            r.score,
            r.evidence.r#ref.to_key(),
            r.evidence.state,
            r.evidence.title
        );
    }
    println!("=== SYNTHESIS ANSWER ===");
    if let Some(ans) = &result.answer {
        println!("{}", ans);
    } else {
        println!("<NO ANSWER GENERATED>");
    }
}
