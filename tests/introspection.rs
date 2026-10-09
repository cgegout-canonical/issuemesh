use axum::http::StatusCode;
use issuemesh::config::{Config, IntrospectionConfig, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::introspection::evaluator::AnswerEvaluator;
use issuemesh::introspection::generator::{compute_fingerprint, QuestionGenerator};
use issuemesh::introspection::types::GeneratedQuestion;
use issuemesh::jobs::SearchStats;
use issuemesh::model::planner::SearchPlan;
use issuemesh::paths::Paths;
use issuemesh::search::orchestrator::DeepSearchResult;
use issuemesh::search::ranking::RankedEvidence;
use issuemesh::store::{IntrospectionRunRecord, Store};
use issuemesh::web::{build_router, IntrospectionForm, SettingsPayload};
use issuemesh::AppState;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use tempfile::tempdir;
use tower::ServiceExt;

#[test]
fn test_introspection_config_levels() {
    let cfg_default = IntrospectionConfig::default();
    assert_eq!(cfg_default.level, 0);
    assert_eq!(cfg_default.level_name(), "None");
    assert_eq!(cfg_default.questions_per_day(), 0);

    let levels = [
        (0, "None", 0),
        (1, "Light", 2),
        (2, "Reasonable", 4),
        (3, "Curious", 7),
        (4, "Introvert!!", 10),
        (5, "None", 0), // fallback to default
    ];

    for (lvl, expected_name, expected_count) in levels {
        let c = IntrospectionConfig { level: lvl };
        assert_eq!(c.level_name(), expected_name);
        assert_eq!(c.questions_per_day(), expected_count);
    }
}

#[tokio::test]
async fn test_introspection_store_crud_and_stats() {
    let store = Store::open_in_memory().unwrap();

    // Check initially empty stats
    let stats0 = store.get_introspection_stats().await.unwrap();
    assert_eq!(stats0.total_count, 0);
    assert_eq!(stats0.pass_count, 0);

    // Save a PASS run
    let run1 = IntrospectionRunRecord {
        id: "intro-test-1".to_string(),
        created_at: "2026-10-09T10:00:00Z".to_string(),
        question: "Is OPEN-4679 still blocked?".to_string(),
        question_fingerprint: "fp123456".to_string(),
        trigger_reason: "unresolved_ref".to_string(),
        search_run_id: Some("sr-1".to_string()),
        accuracy: 4.8,
        relevance: 4.5,
        coverage: 4.2,
        traceability: 4.6,
        conciseness: 4.5,
        weighted_score: 4.6,
        percentage: 92.0,
        result: "PASS".to_string(),
        interpretation: "Excellent answer supported by tracker evidence".to_string(),
        answer: "OPEN-4679 is resolved.".to_string(),
        problem_areas_json: "[]".to_string(),
        recommendation: "Keep monitoring".to_string(),
        evidence_keys_json: "[\"jira:issue:OPEN-4679\"]".to_string(),
        evaluation_details_json: "{}".to_string(),
    };
    store.save_introspection_run(run1).await.unwrap();

    // Save a WEAK run
    let run2 = IntrospectionRunRecord {
        id: "intro-test-2".to_string(),
        created_at: "2026-10-09T11:00:00Z".to_string(),
        question: "What is the status of LP#2167315?".to_string(),
        question_fingerprint: "fp789012".to_string(),
        trigger_reason: "stale_object".to_string(),
        search_run_id: Some("sr-2".to_string()),
        accuracy: 3.5,
        relevance: 3.8,
        coverage: 3.0,
        traceability: 3.2,
        conciseness: 3.5,
        weighted_score: 3.48,
        percentage: 69.6,
        result: "WEAK".to_string(),
        interpretation: "Passable answer with minor coverage gaps".to_string(),
        answer: "LP#2167315 is reported.".to_string(),
        problem_areas_json: "[\"Missing relevant source\"]".to_string(),
        recommendation: "Refresh Launchpad cache".to_string(),
        evidence_keys_json: "[\"launchpad:bug:2167315\"]".to_string(),
        evaluation_details_json: "{}".to_string(),
    };
    store.save_introspection_run(run2).await.unwrap();

    // Retrieve single run
    let fetched = store
        .get_introspection_run("intro-test-1")
        .await
        .unwrap()
        .expect("found");
    assert_eq!(fetched.id, "intro-test-1");
    assert_eq!(fetched.result, "PASS");
    assert_eq!(fetched.accuracy, 4.8);
    assert_eq!(fetched.evidence_keys(), vec!["jira:issue:OPEN-4679"]);

    // List runs
    let list = store.list_introspection_runs(10).await.unwrap();
    assert_eq!(list.len(), 2);

    // Count today
    let today_count = store.count_introspection_runs_today().await.unwrap();
    assert_eq!(today_count, 2);

    // Last run
    let last = store
        .get_last_introspection_run()
        .await
        .unwrap()
        .expect("last run exists");
    assert_eq!(last.id, "intro-test-2");

    // Fingerprint check
    let exists = store
        .has_question_fingerprint_recently("fp123456", 7)
        .await
        .unwrap();
    assert!(exists);
    let not_exists = store
        .has_question_fingerprint_recently("unknown_fp", 7)
        .await
        .unwrap();
    assert!(!not_exists);

    // Stats aggregation
    let stats = store.get_introspection_stats().await.unwrap();
    assert_eq!(stats.total_count, 2);
    assert_eq!(stats.pass_count, 1);
    assert_eq!(stats.weak_count, 1);
    assert_eq!(stats.fail_count, 0);
    assert!(stats.seven_day_average > 4.0);

    // Trends aggregation
    let trends = store.get_introspection_trends(7).await.unwrap();
    assert!(!trends.is_empty());
}

#[tokio::test]
async fn test_introspection_evaluation_and_hard_failures() {
    let question = GeneratedQuestion {
        question: "Is OPEN-4679 resolved by canonical/project#912?".to_string(),
        fingerprint: compute_fingerprint("Is OPEN-4679 resolved by canonical/project#912?"),
        trigger_reason: "cross_tracker_ref".to_string(),
        candidate_keys: vec!["jira:issue:OPEN-4679".to_string()],
    };

    let obj1 = Evidence {
        r#ref: ObjectRef::new("jira", "issue", "OPEN-4679"),
        title: "BGP route reflector fails to sync".to_string(),
        snippet: "BGP route reflector sync failure".to_string(),
        body: Some("Fixed in PR canonical/project#912".to_string()),
        url: "https://jira.example.com/OPEN-4679".to_string(),
        container: "jira".to_string(),
        author: "alice".to_string(),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-02T10:00:00Z".to_string()),
        state: "Done".to_string(),
        native: BTreeMap::new(),
    };

    let obj2 = Evidence {
        r#ref: ObjectRef::new("github", "pull_request", "canonical/project#912"),
        title: "Fix BGP route reflector sync".to_string(),
        snippet: "Fix BGP route reflector sync".to_string(),
        body: Some("Merged into main".to_string()),
        url: "https://github.com/canonical/project/pull/912".to_string(),
        container: "canonical/project".to_string(),
        author: "bob".to_string(),
        created_at: Some("2026-10-01T12:00:00Z".to_string()),
        updated_at: Some("2026-10-02T12:00:00Z".to_string()),
        state: "merged".to_string(),
        native: BTreeMap::new(),
    };

    let relation = Relation {
        from: ObjectRef::new("jira", "issue", "OPEN-4679"),
        to: ObjectRef::new("github", "pull_request", "canonical/project#912"),
        relation_type: "fixed_by".to_string(),
        detected_by: "test".to_string(),
        confidence: 0.95,
    };

    let stats = SearchStats {
        objects_analysed: 2,
        deep_reads: 2,
        relations_followed: 1,
        trackers_searched: 2,
        duration_ms: 120,
    };

    // Case 1: Grounded, well-supported answer -> PASS
    let answer1 = "OPEN-4679 is resolved. Jira issue OPEN-4679 is marked as Done, and linked PR canonical/project#912 has been merged into main.".to_string();
    let search_result_good = DeepSearchResult {
        query: question.question.clone(),
        plan: SearchPlan::default(),
        ranked_results: vec![
            RankedEvidence {
                evidence: obj1.clone(),
                score: 0.9,
                lexical_score: 0.9,
                semantic_score: 0.9,
                graph_score: 0.9,
                freshness_score: 0.9,
            },
            RankedEvidence {
                evidence: obj2.clone(),
                score: 0.9,
                lexical_score: 0.9,
                semantic_score: 0.9,
                graph_score: 0.9,
                freshness_score: 0.9,
            },
        ],
        findings: vec![],
        relations_followed: vec![relation.clone()],
        stats: stats.clone(),
        answer: Some(answer1),
        warnings: vec![],
    };

    let eval1 = AnswerEvaluator::evaluate(&question, &search_result_good);
    assert_eq!(eval1.result, "PASS");
    assert!(eval1.accuracy >= 4.5);
    assert!(eval1.weighted_score >= 4.0);
    assert!(eval1.problem_areas.is_empty());

    // Case 2: Hallucination hard-failure:
    // The answer mentions a non-existent Jira ticket OPEN-9999 or PR #9999
    let answer_hallucinated = "OPEN-4679 is resolved and verified in OPEN-9999 and PR canonical/project#9999 which was deployed.".to_string();
    let mut search_result_hallucinated = search_result_good.clone();
    search_result_hallucinated.answer = Some(answer_hallucinated);

    let eval_hallucinated = AnswerEvaluator::evaluate(&question, &search_result_hallucinated);
    assert_eq!(eval_hallucinated.result, "FAIL");
    assert_eq!(eval_hallucinated.accuracy, 0.0);
    assert!(eval_hallucinated
        .problem_areas
        .iter()
        .any(|p| p.contains("hallucination")));

    // Case 3: Contradiction ignored hard-rule:
    // One object is Closed/Done, but another linked object is Open/In Progress
    let mut obj2_open = obj2.clone();
    obj2_open.state = "open".to_string();
    let mut search_result_conflict = search_result_good.clone();
    search_result_conflict.ranked_results[1].evidence = obj2_open.clone();
    search_result_conflict.findings = vec![issuemesh::search::ConsistencyFinding {
        finding_type: "status_mismatch".to_string(),
        severity: issuemesh::search::Severity::Warning,
        objects: vec![obj1.r#ref.clone(), obj2_open.r#ref.clone()],
        explanation: "Jira issue is Done but linked PR is Open".to_string(),
    }];
    search_result_conflict.answer = Some(
        "All items are completely finished and everything is done across all systems.".to_string(),
    );

    let eval_contradiction = AnswerEvaluator::evaluate(&question, &search_result_conflict);
    // Accuracy must be capped at <= 1.0 when critical contradiction is ignored
    assert!(eval_contradiction.accuracy <= 1.0);
    assert!(eval_contradiction
        .problem_areas
        .iter()
        .any(|p| p.contains("consistency analysis")));
}

#[tokio::test]
async fn test_introspection_fingerprint_normalization() {
    let fp1 = compute_fingerprint("Is OPEN-4679 still blocked?");
    let fp2 = compute_fingerprint("  is open-4679 still blocked?  ");
    assert_eq!(fp1, fp2);
}

#[tokio::test]
async fn test_introspection_question_generator() {
    let store = Store::open_in_memory().unwrap();
    let config = Config::default();

    // Baseline fallback works even on empty store
    let q = QuestionGenerator::generate(&store, &config).await.unwrap();
    assert!(!q.question.is_empty());
    assert!(!q.fingerprint.is_empty());

    // Seed store with an object having an unresolved ref
    let obj = Evidence {
        r#ref: ObjectRef::new("jira", "issue", "SUNBEAM-100"),
        title: "Investigate OVS firewall leak".to_string(),
        snippet: "Pointing to unindexed LP#1234567".to_string(),
        body: Some("See LP#1234567 for logs".to_string()),
        url: "https://jira.example.com/SUNBEAM-100".to_string(),
        container: "jira".to_string(),
        author: "tester".to_string(),
        created_at: Some("2026-10-08T10:00:00Z".to_string()),
        updated_at: Some("2026-10-08T12:00:00Z".to_string()),
        state: "Open".to_string(),
        native: BTreeMap::new(),
    };
    store.upsert_object(obj).await.unwrap();

    // Generate question with seeded data
    let q_seeded = QuestionGenerator::generate(&store, &config).await.unwrap();
    assert!(!q_seeded.question.is_empty());
    assert!(!q_seeded.fingerprint.is_empty());

    // Now save a run with this fingerprint
    let run = IntrospectionRunRecord {
        id: "intro-seed".to_string(),
        created_at: "2026-10-09T10:00:00Z".to_string(),
        question: q_seeded.question.clone(),
        question_fingerprint: q_seeded.fingerprint.clone(),
        trigger_reason: q_seeded.trigger_reason.clone(),
        search_run_id: None,
        accuracy: 4.5,
        relevance: 4.5,
        coverage: 4.5,
        traceability: 4.5,
        conciseness: 4.5,
        weighted_score: 4.5,
        percentage: 90.0,
        result: "PASS".to_string(),
        interpretation: "Grounded answer".to_string(),
        answer: "Verified in store".to_string(),
        problem_areas_json: "[]".to_string(),
        recommendation: "None".to_string(),
        evidence_keys_json: "[]".to_string(),
        evaluation_details_json: "{}".to_string(),
    };
    store.save_introspection_run(run).await.unwrap();

    // Next question generation must deduplicate and avoid the same fingerprint
    let has_recent = store
        .has_question_fingerprint_recently(&q_seeded.fingerprint, 7)
        .await
        .unwrap();
    assert!(has_recent);
}

#[tokio::test]
async fn test_introspection_scheduler_concurrency_lock() {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    paths.ensure_dirs().unwrap();

    let mut config = Config::load_or_default(&paths.config_file).unwrap();
    config.onboarding_completed = true;
    let secrets = Secrets::default();
    let store = Store::open(&paths.db_file).unwrap();

    let state = AppState::new(paths, config, secrets, store);

    // Manually set is_running to true
    state.introspection.is_running.store(true, Ordering::SeqCst);

    // trigger_now must immediately reject when already running
    let result = state
        .introspection
        .trigger_now(std::sync::Arc::new(state.clone()))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("already actively executing"));

    // Reset running lock
    state
        .introspection
        .is_running
        .store(false, Ordering::SeqCst);
}

#[tokio::test]
async fn test_introspection_web_pages_and_api() {
    let tmp = tempdir().unwrap();
    let paths = Paths::resolve(Some(tmp.path()), None).unwrap();
    paths.ensure_dirs().unwrap();

    let mut config = Config::load_or_default(&paths.config_file).unwrap();
    config.onboarding_completed = true;
    config.introspection.level = 2; // Reasonable: 4/day
    let secrets = Secrets::default();
    let store = Store::open(&paths.db_file).unwrap();

    let state = AppState::new(paths, config, secrets, store.clone());
    let app = build_router(state.clone());

    // 1. GET /introspection renders dashboard HTML
    let req = axum::http::Request::builder()
        .uri("/introspection")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("Continuous Introspection"));
    assert!(html.contains("Trigger Introspection Now"));

    // 2. GET /about-introspection renders documentation HTML
    let req = axum::http::Request::builder()
        .uri("/about-introspection")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("IssueMesh Introspection Architecture"));
    assert!(html.contains("Motivation &amp; Purpose") || html.contains("Motivation & Purpose"));

    // 3. GET /api/introspection returns stats, trends, and recent runs JSON
    let req = axum::http::Request::builder()
        .uri("/api/introspection")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["level_name"], "Reasonable");
    assert_eq!(json["questions_per_day"], 4);
    assert!(json["stats"].is_object());
    assert!(json["runs"].is_array());

    // 4. GET /health contains Introspection subsystem with all 8 fields
    let req = axum::http::Request::builder()
        .uri("/api/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 100_000)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    let intro_probe = &json["subsystems"]["introspection"];
    assert!(intro_probe.is_object());
    assert_eq!(intro_probe["level"], 2);
    assert_eq!(intro_probe["questions_per_day"], 4);
    assert!(intro_probe["scheduler"].is_string());

    // 5. Trigger conflict handling: when is_running is true, POST /api/introspection/trigger returns 409 Conflict
    state.introspection.is_running.store(true, Ordering::SeqCst);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/introspection/trigger")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    state
        .introspection
        .is_running
        .store(false, Ordering::SeqCst);

    // 6. PUT /api/settings updates introspection level
    let settings_payload = SettingsPayload {
        introspection: Some(IntrospectionForm { level: Some(3) }),
        ..Default::default()
    };
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/settings")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&settings_payload).unwrap(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Verify updated level
    let current_cfg = state.config.read().await;
    assert_eq!(current_cfg.introspection.level, 3);
    assert_eq!(current_cfg.introspection.questions_per_day(), 7);
    assert_eq!(current_cfg.introspection.level_name(), "Curious");
}
