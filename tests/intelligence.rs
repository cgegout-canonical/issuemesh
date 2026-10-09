use anyhow::Result;
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::intelligence::{
    calculate_confidence, diff_evidence_snapshots, CreateWatchlistRequest, EvidenceSnapshotItem,
    GapDetector, ImpactAnalyzer, InvestigationManager, ReadinessEvaluator, RiskRadar,
    SaveInvestigationRequest, StoryReconstructor, WatchlistService,
};
use issuemesh::store::Store;
use tempfile::NamedTempFile;

async fn create_test_store() -> Result<Store> {
    let tmp = NamedTempFile::new()?;
    let path = tmp.path().to_path_buf();
    let store = Store::open(&path)?;
    Ok(store)
}

fn make_evidence(
    source: &str,
    r#type: &str,
    id: &str,
    title: &str,
    state: &str,
    container: &str,
    author: &str,
) -> Evidence {
    Evidence {
        r#ref: ObjectRef::new(source, r#type, id),
        title: title.to_string(),
        state: state.to_string(),
        container: container.to_string(),
        author: author.to_string(),
        snippet: format!("Detailed description for {} {} {}", source, id, title),
        body: Some(format!(
            "Detailed description for {} {} {}",
            source, id, title
        )),
        url: format!("https://{}.example.com/{}/{}", source, r#type, id),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-08T12:00:00Z".to_string()),
        native: std::collections::BTreeMap::new(),
    }
}

#[tokio::test]
async fn test_watchlist_and_what_changed() -> Result<()> {
    let store = create_test_store().await?;
    let service = WatchlistService::new(store.clone());

    // 1. Insert initial evidence
    let mut ev1 = make_evidence(
        "jira",
        "issue",
        "OPEN-100",
        "Initial Title",
        "In Progress",
        "OPEN",
        "alice",
    );
    store.upsert_object(ev1.clone()).await?;

    // 2. Create watchlist
    let wl = service
        .create_watchlist(CreateWatchlistRequest {
            title: "BGP Watchlist".to_string(),
            target_type: "issue".to_string(),
            target_value: "jira:issue:OPEN-100".to_string(),
        })
        .await?;

    assert_eq!(wl.title, "BGP Watchlist");
    assert_eq!(wl.target_value, "jira:issue:OPEN-100");

    // 3. Check immediately -> no changes
    let report1 = service.check_watchlist(&wl.id).await?.expect("Report 1");
    assert!(report1.diff.state_changes.is_empty());
    assert!(report1.diff.modified.is_empty());

    // 4. Update evidence to trigger changes
    ev1.state = "Done".to_string();
    ev1.title = "Updated BGP Title".to_string();
    store.upsert_object(ev1.clone()).await?;

    // 5. Check again -> should report changes
    let report2 = service.check_watchlist(&wl.id).await?.expect("Report 2");
    assert!(!report2.diff.state_changes.is_empty() || !report2.diff.modified.is_empty());

    // 6. What changed for object directly
    let obj_diff = service
        .what_changed_for_object("jira:issue:OPEN-100")
        .await?;
    assert_eq!(obj_diff.object_key, "jira:issue:OPEN-100");
    assert!(!obj_diff.revision_history.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_impact_analysis_traversal_and_severity() -> Result<()> {
    let store = create_test_store().await?;
    let analyzer = ImpactAnalyzer::new(store.clone());

    // Root issue
    let root = make_evidence(
        "jira",
        "issue",
        "CORE-1",
        "Core Engine Block",
        "Open",
        "CORE",
        "alice",
    );
    // Downstream blocked issue
    let down1 = make_evidence(
        "jira",
        "issue",
        "NET-50",
        "Network Router Stalled",
        "Blocked",
        "NET",
        "bob",
    );
    // Downstream PR implementing NET-50
    let pr = make_evidence(
        "github",
        "pull",
        "canonical/net#12",
        "Implement BGP Route",
        "open",
        "canonical/net",
        "charlie",
    );
    // Downstream Launchpad bug
    let bug = make_evidence(
        "launchpad",
        "bug",
        "200001",
        "Production BGP Packet Drop",
        "Confirmed",
        "ubuntu",
        "dan",
    );

    store.upsert_object(root.clone()).await?;
    store.upsert_object(down1.clone()).await?;
    store.upsert_object(pr.clone()).await?;
    store.upsert_object(bug.clone()).await?;

    // Relations: CORE-1 blocks NET-50, NET-50 relates to pr, NET-50 relates to bug
    store
        .insert_relations(vec![
            Relation {
                from: root.r#ref.clone(),
                to: down1.r#ref.clone(),
                relation_type: "blocks".to_string(),
                detected_by: "jira".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: down1.r#ref.clone(),
                to: pr.r#ref.clone(),
                relation_type: "relates_to".to_string(),
                detected_by: "github".to_string(),
                confidence: 0.95,
            },
            Relation {
                from: down1.r#ref.clone(),
                to: bug.r#ref.clone(),
                relation_type: "causes".to_string(),
                detected_by: "launchpad".to_string(),
                confidence: 0.9,
            },
        ])
        .await?;

    let impact = analyzer.analyze_impact("jira:issue:CORE-1").await?;
    assert_eq!(impact.target_key, "jira:issue:CORE-1");
    assert!(impact.total_impacted >= 3);
    assert!(!impact.items.is_empty());

    // Check evidence path exposure
    let net_item = impact
        .items
        .iter()
        .find(|item| item.object_key == "jira:issue:NET-50");
    assert!(net_item.is_some());
    assert!(!net_item.unwrap().evidence_path.is_empty());

    // Blast radius overall severity should be High or Critical
    assert!(impact.overall_severity == "High" || impact.overall_severity == "Critical");

    Ok(())
}

#[tokio::test]
async fn test_risk_radar_categories() -> Result<()> {
    let store = create_test_store().await?;
    let radar = RiskRadar::new(store.clone());

    // 1. Merged PR with Open Jira item
    let jira_open = make_evidence(
        "jira",
        "issue",
        "RISK-10",
        "Unresolved Jira",
        "Open",
        "RISK",
        "alice",
    );
    let pr_merged = make_evidence(
        "github",
        "pull",
        "canonical/pkg#99",
        "Merged Feature",
        "merged",
        "canonical/pkg",
        "bob",
    );
    store.upsert_object(jira_open.clone()).await?;
    store.upsert_object(pr_merged.clone()).await?;

    store
        .insert_relations(vec![Relation {
            from: jira_open.r#ref.clone(),
            to: pr_merged.r#ref.clone(),
            relation_type: "implements".to_string(),
            detected_by: "github".to_string(),
            confidence: 1.0,
        }])
        .await?;

    // 2. Missing Owner
    let no_owner = make_evidence(
        "jira",
        "issue",
        "RISK-20",
        "Orphaned Task",
        "In Progress",
        "RISK",
        "",
    );
    store.upsert_object(no_owner).await?;

    let report = radar.scan().await?;
    assert!(report.total_risks >= 2);

    let has_merged_unresolved = report
        .items
        .iter()
        .any(|r| r.category == "merged_pr_unresolved_jira");
    assert!(
        has_merged_unresolved,
        "Should detect merged PR with unresolved Jira"
    );

    let has_missing_owner = report.items.iter().any(|r| r.category == "missing_owner");
    assert!(has_missing_owner, "Should detect missing owner");

    Ok(())
}

#[tokio::test]
async fn test_story_reconstruction() -> Result<()> {
    let store = create_test_store().await?;
    let reconstructor = StoryReconstructor::new(store.clone());

    let root = make_evidence(
        "jira",
        "issue",
        "STORY-1",
        "Authentication Overhaul",
        "Done",
        "AUTH",
        "alice",
    );
    let pr = make_evidence(
        "github",
        "pull",
        "canonical/auth#42",
        "OAuth2 Implementation",
        "merged",
        "canonical/auth",
        "bob",
    );

    store.upsert_object(root.clone()).await?;
    store.upsert_object(pr.clone()).await?;

    store
        .insert_relations(vec![Relation {
            from: root.r#ref.clone(),
            to: pr.r#ref.clone(),
            relation_type: "implements".to_string(),
            detected_by: "jira".to_string(),
            confidence: 1.0,
        }])
        .await?;

    let timeline = reconstructor
        .reconstruct_timeline("jira:issue:STORY-1")
        .await?;
    assert_eq!(timeline.root_key, "jira:issue:STORY-1");
    assert!(!timeline.events.is_empty());
    assert!(!timeline.story_narrative.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_gap_detection() -> Result<()> {
    let store = create_test_store().await?;
    let detector = GapDetector::new(store.clone());

    // Active Jira without PR
    let active_jira = make_evidence(
        "jira",
        "issue",
        "GAP-1",
        "Feature Missing Code",
        "In Progress",
        "GAP",
        "alice",
    );
    store.upsert_object(active_jira).await?;

    // Merged PR without Release
    let merged_pr = make_evidence(
        "github",
        "pull",
        "canonical/repo#88",
        "Merged Without Release",
        "merged",
        "canonical/repo",
        "bob",
    );
    store.upsert_object(merged_pr).await?;

    let gaps = detector.detect_gaps().await?;
    assert!(gaps.total_gaps >= 1);

    let has_missing_pr = gaps.items.iter().any(|g| g.category == "jira_without_pr");
    assert!(has_missing_pr, "Should detect Jira item with no PR");

    Ok(())
}

#[tokio::test]
async fn test_saved_investigations_lifecycle() -> Result<()> {
    let store = create_test_store().await?;
    let mgr = InvestigationManager::new(store.clone());

    let ev = make_evidence(
        "jira",
        "issue",
        "INV-1",
        "Investigation Subject",
        "In Progress",
        "INV",
        "alice",
    );
    store.upsert_object(ev.clone()).await?;

    let saved = mgr
        .save(SaveInvestigationRequest {
            title: "Sunbeam BGP Readiness".to_string(),
            query: "Sunbeam BGP".to_string(),
            conclusions_markdown: "Initial finding: 1 blocker remaining.".to_string(),
            evidence_keys: vec!["jira:issue:INV-1".to_string()],
            tags: Some("bgp,sunbeam".to_string()),
        })
        .await?;

    assert_eq!(saved.title, "Sunbeam BGP Readiness");

    // Reopen investigation
    let reopened = mgr.reopen(&saved.id).await?.expect("Reopened report");
    assert_eq!(reopened.investigation.id, saved.id);
    assert!(reopened.deltas.new_evidence.is_empty());

    // Now update evidence and reopen to verify delta tracking
    let mut updated_ev = ev.clone();
    updated_ev.state = "Done".to_string();
    store.upsert_object(updated_ev).await?;

    let reopened2 = mgr.reopen(&saved.id).await?.expect("Reopened report 2");
    assert!(
        !reopened2.deltas.changed_evidence.is_empty()
            || !reopened2.deltas.resolved_evidence.is_empty()
    );

    Ok(())
}

#[tokio::test]
async fn test_readiness_assessment() -> Result<()> {
    let store = create_test_store().await?;
    let evaluator = ReadinessEvaluator::new(store.clone());

    // 1 blocker, 1 done issue
    let blocker = make_evidence(
        "jira",
        "issue",
        "REL-1",
        "Critical Release Blocker",
        "Open",
        "REL",
        "alice",
    );
    let resolved = make_evidence(
        "jira",
        "issue",
        "REL-2",
        "Completed Migration",
        "Done",
        "REL",
        "bob",
    );

    store.upsert_object(blocker.clone()).await?;
    store.upsert_object(resolved.clone()).await?;

    store
        .insert_relations(vec![Relation {
            from: blocker.r#ref.clone(),
            to: resolved.r#ref.clone(),
            relation_type: "blocks".to_string(),
            detected_by: "jira".to_string(),
            confidence: 1.0,
        }])
        .await?;

    let assessment = evaluator.assess_readiness("REL").await?;
    assert_eq!(assessment.target, "REL");
    assert_eq!(assessment.total_items, 2);
    assert_eq!(assessment.blockers.len(), 1);
    assert_ne!(assessment.verdict, "READY");

    Ok(())
}

#[tokio::test]
async fn test_evidence_diff_snapshots() {
    let prev = vec![EvidenceSnapshotItem {
        object_key: "jira:issue:DIFF-1".to_string(),
        source: "jira".to_string(),
        object_type: "issue".to_string(),
        external_id: "DIFF-1".to_string(),
        title: "Original Title".to_string(),
        state: "Open".to_string(),
        content_hash: None,
        updated_at: Some("2026-10-01T00:00:00Z".to_string()),
    }];

    let current = vec![Evidence {
        r#ref: ObjectRef::new("jira", "issue", "DIFF-1"),
        title: "Original Title".to_string(),
        state: "Done".to_string(),
        container: "DIFF".to_string(),
        author: "alice".to_string(),
        snippet: "DIFF-1 body".to_string(),
        body: Some("DIFF-1 body".to_string()),
        url: "https://jira.example.com/issue/DIFF-1".to_string(),
        created_at: None,
        updated_at: Some("2026-10-09T00:00:00Z".to_string()),
        native: std::collections::BTreeMap::new(),
    }];

    let diff = diff_evidence_snapshots(&prev, &current);
    assert_eq!(diff.total_before, 1);
    assert_eq!(diff.total_after, 1);
    assert_eq!(diff.state_changes.len(), 1);
    assert_eq!(diff.state_changes[0].field_diffs[0].old_val, "Open");
    assert_eq!(diff.state_changes[0].field_diffs[0].new_val, "Done");
}

#[tokio::test]
async fn test_evidence_based_confidence_calculation() {
    let ev1 = make_evidence(
        "jira", "issue", "CONF-1", "Issue 1", "Done", "CONF", "alice",
    );
    let ev2 = make_evidence(
        "github",
        "pull",
        "canonical/conf#1",
        "PR 1",
        "merged",
        "canonical/conf",
        "bob",
    );

    let relations = vec![Relation {
        from: ev1.r#ref.clone(),
        to: ev2.r#ref.clone(),
        relation_type: "implements".to_string(),
        detected_by: "github".to_string(),
        confidence: 1.0,
    }];

    // High confidence case: fresh, multi-source, native links, zero penalties
    let conf = calculate_confidence(&[ev1.clone(), ev2.clone()], &relations, 0, 0);
    assert!(conf.score >= 0.75);
    assert_eq!(conf.level, "High");
    assert!(conf.corroboration_bonus > 0.0);

    // Penalized case: unresolved refs & unavailable sources
    let conf_penalized = calculate_confidence(&[ev1, ev2], &relations, 3, 2);
    assert!(conf_penalized.score < conf.score);
    assert!(conf_penalized.unresolved_penalty > 0.0);
    assert!(conf_penalized.source_health_penalty > 0.0);
}
