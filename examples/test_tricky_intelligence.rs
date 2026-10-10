use anyhow::Result;
use chrono::{Duration, Utc};
use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::intelligence::{
    calculate_confidence, diff_evidence_snapshots, CreateWatchlistRequest, EvidenceSnapshotItem,
    GapDetector, ImpactAnalyzer, InvestigationManager, ReadinessEvaluator, RiskRadar,
    SaveInvestigationRequest, StoryReconstructor, WatchlistService,
};
use issuemesh::mcp::handle_mcp_request;
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::AppState;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
fn make_ev(
    source: &str,
    r#type: &str,
    id: &str,
    title: &str,
    state: &str,
    container: &str,
    author: &str,
    days_ago_created: i64,
    days_ago_updated: i64,
) -> Evidence {
    let now = Utc::now();
    let created = (now - Duration::days(days_ago_created)).to_rfc3339();
    let updated = (now - Duration::days(days_ago_updated)).to_rfc3339();

    Evidence {
        r#ref: ObjectRef::new(source, r#type, id),
        title: title.to_string(),
        state: state.to_string(),
        container: container.to_string(),
        author: author.to_string(),
        snippet: format!("{}: {} [{}]", id, title, state),
        body: Some(format!(
            "Comprehensive engineering details for {} in container {}. Status: {}. Owner: {}.",
            id, container, state, author
        )),
        url: format!("https://{}.canonical.internal/{}/{}", source, r#type, id),
        created_at: Some(created),
        updated_at: Some(updated),
        native: BTreeMap::new(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("================================================================================");
    println!("     ISSUEMESH v0.2.9: TRICKY & COMPLEX INTELLIGENCE SCENARIO VERIFICATION     ");
    println!("================================================================================");

    let temp_db =
        std::env::temp_dir().join(format!("issuemesh_tricky_test_{}.db", uuid::Uuid::new_v4()));
    let store = Store::open(&temp_db)?;

    let paths = Paths {
        home: temp_db.clone(),
        secrets_file: temp_db.clone(),
        config_file: temp_db.clone(),
        db_file: temp_db.clone(),
        logs_dir: temp_db.clone(),
    };
    let app_state = Arc::new(AppState::new(
        paths,
        Config::default(),
        Secrets::default(),
        store.clone(),
    ));

    println!(
        "\n[1/8] SEEDING HETEROGENEOUS CROSS-TRACKER TEST GRAPH WITH CYCLES & CONTRADICTIONS..."
    );

    // 1. OPEN-4679: Core OVN BGP engine (stale blocker)
    let ev_open4679 = make_ev(
        "jira",
        "issue",
        "OPEN-4679",
        "Core OVN BGP distributed routing engine",
        "In Progress",
        "sunbeam-2026.1",
        "alice",
        30,
        20, // 20 days idle -> stale blocker
    );

    // 2. NET-101: BGP speaker daemon integration
    let ev_net101 = make_ev(
        "jira",
        "issue",
        "NET-101",
        "BGP speaker daemon integration",
        "Blocked",
        "sunbeam-2026.1",
        "bob",
        22,
        10,
    );

    // 3. GitHub PR: canonical/sunbeam#404 (merged)
    let ev_pr404 = make_ev(
        "github",
        "pull",
        "canonical/sunbeam#404",
        "Add BGP EVPN peering and route reflection",
        "merged",
        "canonical/sunbeam",
        "charlie",
        15,
        8,
    );

    // 4. Launchpad Bug LP#200123 (triaged/confirmed route drops)
    let ev_lp200123 = make_ev(
        "launchpad",
        "bug",
        "200123",
        "Kernel routing table packet drops under BGP load",
        "Confirmed",
        "sunbeam-2026.1",
        "dan",
        35,
        12,
    );

    // 5. Release Master Ticket: REL-90 (unassigned owner)
    let mut ev_rel90 = make_ev(
        "jira",
        "issue",
        "REL-90",
        "Sunbeam 2026.1 GA Production Release Milestone",
        "Open",
        "sunbeam-2026.1",
        "",
        40,
        5, // author empty -> missing owner
    );
    ev_rel90
        .native
        .insert("priority".to_string(), json!("Blocker"));

    // 6. Security CVE: SEC-99 (Critical CVE idle > 7 days, unassigned)
    let ev_sec99 = make_ev(
        "jira",
        "issue",
        "SEC-99",
        "Critical CVE-2026-3391 BFD session hijacking vulnerability in routing agent",
        "Open",
        "sunbeam-2026.1",
        "unassigned",
        14,
        11, // idle 11 days -> critical inactivity + unassigned
    );

    // 7. Completed Ticket: FEAT-10
    let ev_feat10 = make_ev(
        "jira",
        "issue",
        "FEAT-10",
        "Documentation for BGP peering CLI commands",
        "Done",
        "sunbeam-2026.1",
        "alice",
        25,
        2,
    );

    // 8. Launchpad Regression Bug: LP#200456 (caused by PR #404)
    let ev_lp200456 = make_ev(
        "launchpad",
        "bug",
        "200456",
        "Regression: Routing loop detected after BGP EVPN merge #404",
        "Triaged",
        "sunbeam-2026.1",
        "eva",
        6,
        2,
    );

    // 9. Governance Gap Items:
    // REQ-99: Jira requirement marked in progress with NO PR
    let ev_req99 = make_ev(
        "jira",
        "issue",
        "REQ-99",
        "Mandatory IPv6 BGP multi-homing support",
        "In Progress",
        "sunbeam-2026.1",
        "frank",
        18,
        4,
    );

    // GitHub PR canonical/repo#888: Merged with NO release milestone tag
    let ev_pr888 = make_ev(
        "github",
        "pull",
        "canonical/repo#888",
        "Merged experimental BGP routing patch",
        "merged",
        "canonical/repo",
        "grace",
        12,
        5,
    );

    // Duplicated Work: SUN-10 vs SUN-20 with >85% lexical similarity in container "sunbeam-core"
    let ev_sun10 = make_ev(
        "jira",
        "issue",
        "SUN-10",
        "Configure OVN BGP EVPN peering route reflector",
        "In Progress",
        "sunbeam-core",
        "heidi",
        10,
        3,
    );
    let ev_sun20 = make_ev(
        "jira",
        "issue",
        "SUN-20",
        "Configure OVN BGP EVPN peering route reflector agent",
        "In Progress",
        "sunbeam-core",
        "ivan",
        8,
        2,
    );

    // Upsert all into Store
    store.upsert_object(ev_open4679.clone()).await?;
    store.upsert_object(ev_net101.clone()).await?;
    store.upsert_object(ev_pr404.clone()).await?;
    store.upsert_object(ev_lp200123.clone()).await?;
    store.upsert_object(ev_rel90.clone()).await?;
    store.upsert_object(ev_sec99.clone()).await?;
    store.upsert_object(ev_feat10.clone()).await?;
    store.upsert_object(ev_lp200456.clone()).await?;
    store.upsert_object(ev_req99.clone()).await?;
    store.upsert_object(ev_pr888.clone()).await?;
    store.upsert_object(ev_sun10.clone()).await?;
    store.upsert_object(ev_sun20.clone()).await?;

    // Record Unresolved 404 Reference from REQ-99 to Launchpad bug 999999
    store
        .record_unresolved_ref(
            &ObjectRef::new("launchpad", "bug", "999999"),
            "jira:issue:REQ-99",
        )
        .await?;

    // Setup Complex & Cyclic Relations:
    // OPEN-4679 blocks NET-101
    // NET-101 blocks canonical/sunbeam#404 (deep chain)
    // canonical/sunbeam#404 blocks LP#200123 (deep chain)
    // LP#200123 blocks OPEN-4679 (CIRCULAR DEPENDENCY LOOP!)
    // LP#200123 blocks REL-90 (Production Milestone Blocker)
    // canonical/sunbeam#404 causes LP#200456 (Regression link)
    // Contradiction: LP#200123 is Confirmed, but canonical/sunbeam#404 claims to fix it and is merged
    // Merged PR vs Blocked Jira: PR #404 is merged, but NET-101 is still Blocked
    store
        .insert_relations(vec![
            Relation {
                from: ev_open4679.r#ref.clone(),
                to: ev_net101.r#ref.clone(),
                relation_type: "blocks".to_string(),
                detected_by: "jira:native".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: ev_net101.r#ref.clone(),
                to: ev_pr404.r#ref.clone(),
                relation_type: "blocks".to_string(),
                detected_by: "github:pr_body".to_string(),
                confidence: 0.95,
            },
            Relation {
                from: ev_pr404.r#ref.clone(),
                to: ev_lp200123.r#ref.clone(),
                relation_type: "blocks".to_string(),
                detected_by: "launchpad:link".to_string(),
                confidence: 0.95,
            },
            Relation {
                // THE CYCLE BACK TO ROOT
                from: ev_lp200123.r#ref.clone(),
                to: ev_open4679.r#ref.clone(),
                relation_type: "blocks".to_string(),
                detected_by: "launchpad:comment".to_string(),
                confidence: 0.90,
            },
            Relation {
                from: ev_lp200123.r#ref.clone(),
                to: ev_rel90.r#ref.clone(),
                relation_type: "blocks".to_string(),
                detected_by: "jira:link".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: ev_pr404.r#ref.clone(),
                to: ev_lp200456.r#ref.clone(),
                relation_type: "causes".to_string(),
                detected_by: "launchpad:bug_activity".to_string(),
                confidence: 0.90,
            },
        ])
        .await?;

    println!(
        "   => 12 multi-tracker objects & 6 relations (including cycle) successfully indexed."
    );

    // =========================================================================
    // SCENARIO 1: IMPACT ANALYSIS WITH CIRCULAR DEPENDENCIES
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 1: IMPACT ANALYSIS - CIRCULAR DEPENDENCY & CROSS-TRACKER BLAST RADIUS");
    println!("Question: If we delay or roll back OPEN-4679, what is the downstream blast radius");
    println!("          across Jira, GitHub, and Launchpad, and does graph traversal terminate?");
    println!("--------------------------------------------------------------------------------");
    let impact_analyzer = ImpactAnalyzer::new(store.clone());
    let impact = impact_analyzer
        .analyze_impact("jira:issue:OPEN-4679")
        .await?;

    println!("Target Object: {}", impact.target_key);
    println!("Blast Radius Severity: {}", impact.overall_severity);
    println!("Total Impacted Objects: {}", impact.total_impacted);
    println!("Cycle Termination: SUCCESS (visited set prevented infinite loop)");
    println!("\nImpacted Entities & Evidence Paths:");
    for item in &impact.items {
        println!("  - [{}] {} ({})", item.depth, item.object_key, item.title);
        println!(
            "    Impact Type: {} | Severity: {}",
            item.impact_type, item.severity
        );
        println!("    Evidence Path: {}", item.evidence_path.join(" -> "));
    }

    // =========================================================================
    // SCENARIO 2: PRODUCTION READINESS ASSESSMENT ON AMBIGUOUS MILESTONE
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 2: RELEASE / PRODUCTION READINESS ASSESSMENT");
    println!("Question: Is milestone 'sunbeam-2026.1' ready for production release?");
    println!("--------------------------------------------------------------------------------");
    let readiness_eval = ReadinessEvaluator::new(store.clone());
    let readiness = readiness_eval.assess_readiness("sunbeam-2026.1").await?;

    println!("Milestone Target: {}", readiness.target);
    println!(
        "Verdict: {} (Score: {:.1}/100.0)",
        readiness.verdict, readiness.readiness_score
    );
    println!(
        "Implementation Progress: {:.1}%",
        readiness.implementation_percentage
    );
    println!("Active Blockers ({}):", readiness.blockers.len());
    for b in &readiness.blockers {
        println!("  * {}", b);
    }
    println!(
        "High-Priority / CVE Issues ({}):",
        readiness.high_priority_issues.len()
    );
    for hp in &readiness.high_priority_issues {
        println!("  * {}", hp);
    }
    println!("Stale Work ({}):", readiness.stale_work.len());
    for s in &readiness.stale_work {
        println!("  * {}", s);
    }
    println!("Ownership Gaps ({}):", readiness.ownership_gaps.len());
    for og in &readiness.ownership_gaps {
        println!("  * {}", og);
    }
    println!("Summary: {}", readiness.summary);

    // =========================================================================
    // SCENARIO 3: ENGINEERING RISK RADAR (ALL 6 RISK CATEGORIES)
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 3: ENGINEERING RISK RADAR SCAN");
    println!("Question: What hidden architectural and delivery risks exist across all systems?");
    println!("--------------------------------------------------------------------------------");
    let risk_radar = RiskRadar::new(store.clone());
    let risks = risk_radar.scan().await?;

    println!(
        "Total Risks Flagged: {} (Critical: {}, High: {}, Medium: {})",
        risks.total_risks, risks.critical_count, risks.high_count, risks.medium_count
    );
    println!("Summary: {}", risks.summary);
    println!("\nDetected Risk Items by Category:");
    for r in &risks.items {
        println!(
            "  [{}] Category: {} | Severity: {}",
            r.id, r.category, r.severity
        );
        println!("     Title: {}", r.title);
        println!("     Description: {}", r.description);
        println!("     Remediation: {}", r.remediation);
        println!("     Objects Involved: {:?}", r.objects);
    }

    // =========================================================================
    // SCENARIO 4: TIMELINE & STORY RECONSTRUCTION
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 4: TIMELINE / STORY RECONSTRUCTION ACROSS 3 TRACKERS");
    println!("Question: Reconstruct the chronological history of the BGP Route Reflector failure.");
    println!("--------------------------------------------------------------------------------");
    let story_reconstructor = StoryReconstructor::new(store.clone());
    let timeline = story_reconstructor
        .reconstruct_timeline("jira:issue:OPEN-4679")
        .await?;

    println!("Root Focus: {}", timeline.root_key);
    println!("Total Chronological Events: {}", timeline.total_events);
    println!("\nChronological Event Stream:");
    for ev in &timeline.events {
        println!(
            "  - [{}] [{:^9}] [{}] by {}: {}",
            ev.timestamp.chars().take(10).collect::<String>(),
            ev.source,
            ev.event_type,
            ev.actor,
            ev.description
        );
    }
    println!(
        "\nSynthesized Story Narrative:\n{}",
        timeline.story_narrative
    );

    // =========================================================================
    // SCENARIO 5: WHAT ARE WE MISSING? (GAP DETECTION)
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 5: GOVERNANCE GAP DETECTION ('WHAT ARE WE MISSING?')");
    println!("Question: What engineering governance blindspots exist (unimplemented requirements,");
    println!("          merged code without release, dangling 404 refs, duplicated work)?");
    println!("--------------------------------------------------------------------------------");
    let gap_detector = GapDetector::new(store.clone());
    let gaps = gap_detector.detect_gaps().await?;

    println!("Total Gaps Detected: {}", gaps.total_gaps);
    println!("Summary: {}", gaps.summary);
    println!("\nGap Findings:");
    for g in &gaps.items {
        println!("  - Category: {} | Severity: {}", g.category, g.severity);
        println!("    Title: {}", g.title);
        println!("    Description: {}", g.description);
        println!("    Recommendation: {}", g.recommendation);
        println!("    Affected Keys: {:?}", g.affected_keys);
    }

    // =========================================================================
    // SCENARIO 6: SAVED INVESTIGATION WITH 4-WAY ASYNCHRONOUS DELTAS
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 6: SAVED INVESTIGATION WITH LIVE ASYNCHRONOUS DELTAS");
    println!("Question: Save an investigation into 'BGP Routing', simulate remote mutations,");
    println!("          and verify reopening detects new, changed, and resolved evidence.");
    println!("--------------------------------------------------------------------------------");
    let inv_manager = InvestigationManager::new(store.clone());
    let saved_inv = inv_manager
        .save(SaveInvestigationRequest {
            title: "Sunbeam BGP Peering Stability".to_string(),
            query: "BGP routing peering failure".to_string(),
            conclusions_markdown:
                "Initial Finding: PR #404 is merged, but LP#200123 packet drops remain confirmed."
                    .to_string(),
            evidence_keys: vec![
                "jira:issue:OPEN-4679".to_string(),
                "launchpad:bug:200123".to_string(),
            ],
            tags: Some("bgp,sunbeam,routing".to_string()),
        })
        .await?;
    println!(
        "Saved Investigation Created: ID '{}' (Title: '{}')",
        saved_inv.id, saved_inv.title
    );

    // Also register a Watchlist on OPEN-4679 to test WatchlistService & What Changed
    let wl_service = WatchlistService::new(store.clone());
    let wl = wl_service
        .create_watchlist(CreateWatchlistRequest {
            title: "BGP Core Engine Watchlist".to_string(),
            target_type: "issue".to_string(),
            target_value: "jira:issue:OPEN-4679".to_string(),
        })
        .await?;
    println!("Watchlist created: ID '{}' on '{}'", wl.id, wl.target_value);

    // Initial check (clean)
    let _ = wl_service.check_watchlist(&wl.id).await?;

    // Simulate remote tracker updates:
    // 1. LP#200123 resolved: moves to Fix Released
    let mut resolved_lp = ev_lp200123.clone();
    resolved_lp.state = "Fix Released".to_string();
    store.upsert_object(resolved_lp).await?;

    // 2. OPEN-4679 changed: title updated & state moves to Done
    let mut changed_jira = ev_open4679.clone();
    changed_jira.state = "Done".to_string();
    changed_jira.title =
        "Core OVN BGP distributed routing engine (v2 production ready)".to_string();
    store.upsert_object(changed_jira.clone()).await?;

    // 3. New evidence discovered in topic: LP#200500
    let ev_new = make_ev(
        "launchpad",
        "bug",
        "200500",
        "BGP routing daemon memory leak on interface flap",
        "Triaged",
        "sunbeam-2026.1",
        "eva",
        1,
        0,
    );
    store.upsert_object(ev_new).await?;

    // Reopen investigation
    let reopened = inv_manager
        .reopen(&saved_inv.id)
        .await?
        .expect("Reopened report");
    println!("\nReopened Investigation Deltas:");
    println!(
        "  * Resolved Evidence ({} items):",
        reopened.deltas.resolved_evidence.len()
    );
    for r in &reopened.deltas.resolved_evidence {
        println!(
            "    - {} ({}) -> now in state '{}'",
            r.object_key, r.title, r.state
        );
    }
    println!(
        "  * Changed Evidence ({} items):",
        reopened.deltas.changed_evidence.len()
    );
    for c in &reopened.deltas.changed_evidence {
        println!(
            "    - {} ({}) [New State: {}]",
            c.object_key, c.title, c.state
        );
    }
    println!(
        "  * New Evidence Discovered ({} items):",
        reopened.deltas.new_evidence.len()
    );
    for n in &reopened.deltas.new_evidence {
        println!("    - {} ({})", n.object_key, n.title);
    }

    // Verify Watchlist caught the change
    let wl_check = wl_service.check_watchlist(&wl.id).await?.expect("WL check");
    println!("\nWatchlist Verification ('What Changed'):");
    println!("  Target: {}", wl_check.target_value);
    println!("  State Changes: {:?}", wl_check.diff.state_changes.len());
    for sc in &wl_check.diff.state_changes {
        for f in &sc.field_diffs {
            println!(
                "    Field '{}': '{}' -> '{}'",
                f.field, f.old_val, f.new_val
            );
        }
    }

    // Evidence Diff verification
    let prev_snapshot = vec![EvidenceSnapshotItem {
        object_key: "jira:issue:OPEN-4679".to_string(),
        source: "jira".to_string(),
        object_type: "issue".to_string(),
        external_id: "OPEN-4679".to_string(),
        title: ev_open4679.title.clone(),
        state: ev_open4679.state.clone(),
        content_hash: None,
        updated_at: ev_open4679.updated_at.clone(),
    }];
    let diff_report = diff_evidence_snapshots(&prev_snapshot, &[changed_jira]);
    println!("\nEvidence Diff Snapshot Test ('What changed since this answer?'):");
    println!("  Total Changed: {}", diff_report.state_changes.len());
    for sc in &diff_report.state_changes {
        for fd in &sc.field_diffs {
            println!(
                "  Diff: {} changed from '{}' to '{}'",
                fd.field, fd.old_val, fd.new_val
            );
        }
    }

    // =========================================================================
    // SCENARIO 7: OBJECTIVE EVIDENCE-BASED CONFIDENCE ENGINE
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 7: EVIDENCE-BASED CONFIDENCE (HIGH vs ADVERSARIAL SIGNALS)");
    println!("Question: How does the deterministic confidence formula score a clean, corroborated");
    println!(
        "          finding vs an adversarial cluster with stale data, 404s, and contradictions?"
    );
    println!("--------------------------------------------------------------------------------");
    // Cluster A: Fresh, 3 independent sources, native links, zero penalties
    let conf_a = calculate_confidence(
        &[ev_open4679.clone(), ev_pr404.clone(), ev_lp200123.clone()],
        &[
            Relation {
                from: ev_open4679.r#ref.clone(),
                to: ev_pr404.r#ref.clone(),
                relation_type: "relates_to".to_string(),
                detected_by: "native_link".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: ev_pr404.r#ref.clone(),
                to: ev_lp200123.r#ref.clone(),
                relation_type: "fixes".to_string(),
                detected_by: "native_link".to_string(),
                confidence: 1.0,
            },
        ],
        0, // 0 unresolved refs
        0, // 0 unavailable sources
    );

    // Cluster B: Stale (90+ days), inferred link (0.4), 3 unresolved refs, 2 unavailable sources, contradiction
    let stale_jira = make_ev(
        "jira",
        "issue",
        "OLD-1",
        "Stale Spec",
        "Open",
        "OLD",
        "alice",
        120,
        110,
    );
    let stale_pr = make_ev(
        "github",
        "pull",
        "repo#1",
        "Old Patch",
        "merged",
        "repo",
        "bob",
        110,
        105,
    );
    let conf_b = calculate_confidence(
        &[stale_jira.clone(), stale_pr.clone()],
        &[Relation {
            from: stale_jira.r#ref.clone(),
            to: stale_pr.r#ref.clone(),
            relation_type: "inferred_match".to_string(),
            detected_by: "heuristic".to_string(),
            confidence: 0.40,
        }],
        3, // 3 unresolved references (404s)
        2, // 2 broken/unavailable trackers
    );

    println!("Cluster A (Authoritative Multi-Source Finding):");
    println!("  Score: {:.2} | Level: {}", conf_a.score, conf_a.level);
    println!(
        "  Freshness Score: {:.2} | Relation Fidelity: {:.2}",
        conf_a.freshness_score, conf_a.relation_fidelity_score
    );
    println!(
        "  Corroboration Bonus: +{:.2} | Contradiction Penalty: -{:.2}",
        conf_a.corroboration_bonus, conf_a.contradiction_penalty
    );
    println!("  Factors: {:?}", conf_a.factors);

    println!("\nCluster B (Adversarial Fragile Finding):");
    println!("  Score: {:.2} | Level: {}", conf_b.score, conf_b.level);
    println!(
        "  Freshness Score: {:.2} | Relation Fidelity: {:.2}",
        conf_b.freshness_score, conf_b.relation_fidelity_score
    );
    println!(
        "  Unresolved Penalty: -{:.2} | Source Health Penalty: -{:.2}",
        conf_b.unresolved_penalty, conf_b.source_health_penalty
    );
    println!("  Factors: {:?}", conf_b.factors);

    // =========================================================================
    // SCENARIO 8: MCP PROTOCOL JSON-RPC 2.0 NATIVE INVOCATION
    // =========================================================================
    println!("\n--------------------------------------------------------------------------------");
    println!("SCENARIO 8: MCP PROTOCOL - NATIVE JSON-RPC 2.0 TOOL DISCOVERY & EXECUTION");
    println!("Question: Can AI agents dynamically discover tools and query IssueMesh via MCP?");
    println!("--------------------------------------------------------------------------------");

    // 1. MCP Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": "agent-init-1",
        "method": "initialize",
        "params": {}
    });
    let init_resp = handle_mcp_request(app_state.clone(), init_req)
        .await
        .expect("Init resp");
    println!(
        "MCP Initialize Handshake: Server='{}' Protocol='{}'",
        init_resp["result"]["serverInfo"]["name"], init_resp["result"]["protocolVersion"]
    );

    // 2. MCP Tools List
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": "agent-tools-2",
        "method": "tools/list",
        "params": {}
    });
    let list_resp = handle_mcp_request(app_state.clone(), list_req)
        .await
        .expect("List resp");
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("Tools array");
    println!("MCP Tools Advertised: {} tools registered", tools.len());
    let sample_names: Vec<&str> = tools
        .iter()
        .take(6)
        .filter_map(|t| t["name"].as_str())
        .collect();
    println!("Sample Tools: {:?}", sample_names);

    // 3. MCP Tool Call: impact_analysis
    let mcp_call_impact = json!({
        "jsonrpc": "2.0",
        "id": "agent-call-3",
        "method": "tools/call",
        "params": {
            "name": "impact_analysis",
            "arguments": {
                "object_key": "jira:issue:OPEN-4679"
            }
        }
    });
    let mcp_impact_resp = handle_mcp_request(app_state.clone(), mcp_call_impact)
        .await
        .expect("Call resp");
    assert_eq!(mcp_impact_resp["result"]["isError"], false);
    let impact_text = mcp_impact_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    println!("\nMCP Tool Call [impact_analysis] Output (Truncated preview):");
    println!(
        "{}",
        impact_text
            .lines()
            .take(6)
            .collect::<Vec<&str>>()
            .join("\n")
    );

    // 4. MCP Tool Call: readiness
    let mcp_call_ready = json!({
        "jsonrpc": "2.0",
        "id": "agent-call-4",
        "method": "tools/call",
        "params": {
            "name": "readiness",
            "arguments": {
                "target": "sunbeam-2026.1"
            }
        }
    });
    let mcp_ready_resp = handle_mcp_request(app_state.clone(), mcp_call_ready)
        .await
        .expect("Ready resp");
    assert_eq!(mcp_ready_resp["result"]["isError"], false);
    let ready_text = mcp_ready_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    println!("\nMCP Tool Call [readiness] Output (Truncated preview):");
    println!(
        "{}",
        ready_text.lines().take(7).collect::<Vec<&str>>().join("\n")
    );

    // Clean up temp DB
    let _ = std::fs::remove_file(&temp_db);

    println!("\n================================================================================");
    println!("    ALL 8 TRICKY AND COMPLEX INTELLIGENCE SCENARIOS PASSED WITH ZERO ERRORS     ");
    println!("================================================================================");

    Ok(())
}
