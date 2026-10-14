use anyhow::Result;
use reqwest::StatusCode;
use serde_json::Value;
use std::net::SocketAddr;
use tempfile::NamedTempFile;
use tokio::net::TcpListener;

use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::web::build_router;
use issuemesh::AppState;

#[allow(clippy::too_many_arguments)]
fn make_evidence(
    source: &str,
    r#type: &str,
    id: &str,
    title: &str,
    state: &str,
    container: &str,
    author: &str,
    body: &str,
) -> Evidence {
    Evidence {
        r#ref: ObjectRef::new(source, r#type, id),
        title: title.to_string(),
        state: state.to_string(),
        container: container.to_string(),
        author: author.to_string(),
        snippet: body.to_string(),
        body: Some(body.to_string()),
        url: format!("https://{}.example.com/{}/{}", source, r#type, id),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-08T12:00:00Z".to_string()),
        native: std::collections::BTreeMap::new(),
    }
}

async fn populate_test_graph(store: &Store) -> Result<()> {
    // 1. Jira feature: OPEN-4679 (Blocks OPEN-102)
    let ev1 = make_evidence(
        "jira",
        "issue",
        "OPEN-4679",
        "Implement BGP EVPN for MicroCeph",
        "In Progress",
        "OPEN",
        "alice",
        "Need BGP EVPN support across clusters. Blocked on libbgp core.",
    );
    store.upsert_object(ev1.clone()).await?;

    // 2. Downstream dependent Jira: OPEN-102
    let ev2 = make_evidence(
        "jira",
        "issue",
        "OPEN-102",
        "Expose BGP peering routes in Sunbeam CLI",
        "Blocked",
        "OPEN",
        "bob",
        "Depends on OPEN-4679 completion.",
    );
    store.upsert_object(ev2.clone()).await?;

    // 3. GitHub PR: canonical/sunbeam#45 (Merged, links to OPEN-4679)
    let ev3 = make_evidence(
        "github",
        "pull",
        "canonical/sunbeam#45",
        "bgp: add routing controller",
        "merged",
        "canonical/sunbeam",
        "alice",
        "Merged routing controller implementing OPEN-4679.",
    );
    store.upsert_object(ev3.clone()).await?;

    // 4. Launchpad Bug: LP#2054123 (Fix Committed)
    let ev4 = make_evidence(
        "launchpad",
        "bug",
        "2054123",
        "BGP daemon crash under heavy subnet fanout",
        "Fix Committed",
        "sunbeam-charms",
        "charlie",
        "Fixed BGP daemon routing table overflow.",
    );
    store.upsert_object(ev4.clone()).await?;

    // 5. Stale Jira without PR or owner: OPEN-999
    let mut ev5 = make_evidence(
        "jira",
        "issue",
        "OPEN-999",
        "Unassigned Critical CVE patch for BGP parser",
        "Open",
        "OPEN",
        "",
        "Critical memory safety issue in parser. Stale for 3 weeks.",
    );
    ev5.updated_at = Some("2026-09-01T00:00:00Z".to_string());
    store.upsert_object(ev5.clone()).await?;

    // 6. Merged PR without Release or Jira: canonical/sunbeam#99
    let mut ev6 = make_evidence(
        "github",
        "pull",
        "canonical/sunbeam#99",
        "chore: fix internal build harness",
        "merged",
        "canonical/sunbeam",
        "dave",
        "Internal build harness cleanup.",
    );
    ev6.updated_at = Some("2026-09-05T00:00:00Z".to_string());
    store.upsert_object(ev6.clone()).await?;

    // Add relations
    store
        .insert_relations(vec![
            Relation {
                from: ObjectRef::new("jira", "issue", "OPEN-4679"),
                to: ObjectRef::new("jira", "issue", "OPEN-102"),
                relation_type: "blocks".to_string(),
                detected_by: "jira".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: ObjectRef::new("github", "pull", "canonical/sunbeam#45"),
                to: ObjectRef::new("jira", "issue", "OPEN-4679"),
                relation_type: "implements".to_string(),
                detected_by: "github".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: ObjectRef::new("jira", "issue", "OPEN-4679"),
                to: ObjectRef::new("launchpad", "bug", "2054123"),
                relation_type: "relates_to".to_string(),
                detected_by: "launchpad".to_string(),
                confidence: 0.9,
            },
        ])
        .await?;

    Ok(())
}

#[tokio::test]
async fn test_e2e_all_intelligence_tabs_and_endpoints() -> Result<()> {
    // 1. Start real SQLite store and populate with graph
    let tmp = NamedTempFile::new()?;
    let db_path = tmp.path().to_path_buf();
    let store = Store::open(&db_path)?;
    populate_test_graph(&store).await?;

    // 2. Start Axum HTTP server on random ephemeral port
    let paths = Paths::resolve(Some(tmp.path().parent().unwrap()), None)?;
    let config = Config {
        onboarding_completed: true,
        ..Default::default()
    };
    let secrets = Secrets::default();
    let app_state = AppState::new(paths, config, secrets, store.clone());
    let app = build_router(app_state);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    println!("\n=== Starting End-to-End HTTP Tests against {} ===", base);

    // ------------------------------------------------------------------------
    // TEST TAB 0: HTML Page Rendering
    // ------------------------------------------------------------------------
    let res = client.get(format!("{}/intelligence", base)).send().await?;
    assert_eq!(res.status(), StatusCode::OK);
    let html = res.text().await?;
    assert!(html.contains("Engineering Intelligence"));
    assert!(html.contains("Watchlists & Diffs"));
    assert!(html.contains("Impact Analysis"));
    assert!(html.contains("Risk Radar"));
    assert!(html.contains("Story & Timeline"));
    assert!(html.contains("Governance Gaps"));
    assert!(html.contains("Saved Investigations"));
    assert!(html.contains("Release Readiness"));
    assert!(html.contains("Model Context Protocol (MCP) Server"));
    println!("✓ HTML Page: /intelligence served correctly with all tabs");

    // ------------------------------------------------------------------------
    // TEST TAB 1: Watchlists & "What Changed?" (Create -> Check -> Diff -> Delete)
    // ------------------------------------------------------------------------
    // Create
    let res = client
        .post(format!("{}/api/intelligence/watchlists", base))
        .json(&serde_json::json!({
            "title": "Sunbeam BGP Watchlist",
            "target_type": "issue",
            "target_value": "jira:issue:OPEN-4679"
        }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::CREATED);
    let wl: Value = res.json().await?;
    let wl_id = wl["id"].as_str().unwrap().to_string();
    assert_eq!(wl["title"], "Sunbeam BGP Watchlist");

    // List
    let res = client
        .get(format!("{}/api/intelligence/watchlists", base))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let list: Vec<Value> = res.json().await?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], wl_id);

    // Initial check (no changes)
    let res = client
        .post(format!(
            "{}/api/intelligence/watchlists/{}/check",
            base, wl_id
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let check: Value = res.json().await?;
    assert!(check["diff"]["state_changes"]
        .as_array()
        .unwrap()
        .is_empty());

    // Update object to induce change
    let mut ev_updated = store
        .get_object(&ObjectRef::new("jira", "issue", "OPEN-4679"))
        .await?
        .unwrap();
    ev_updated.state = "Done".to_string();
    store.upsert_object(ev_updated).await?;

    // Check again (delta detected)
    let res = client
        .post(format!(
            "{}/api/intelligence/watchlists/{}/check",
            base, wl_id
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let check2: Value = res.json().await?;
    assert!(
        !check2["diff"]["state_changes"]
            .as_array()
            .unwrap()
            .is_empty()
            || !check2["diff"]["modified"].as_array().unwrap().is_empty()
    );

    // What Changed for specific object
    let res = client
        .get(format!(
            "{}/api/intelligence/what-changed?key=jira:issue:OPEN-4679",
            base
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let wc: Value = res.json().await?;
    assert_eq!(wc["object_key"], "jira:issue:OPEN-4679");
    assert!(!wc["revision_history"].as_array().unwrap().is_empty());

    // Delete Watchlist
    let res = client
        .delete(format!("{}/api/intelligence/watchlists/{}", base, wl_id))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let res = client
        .get(format!("{}/api/intelligence/watchlists", base))
        .send()
        .await?;
    let list: Vec<Value> = res.json().await?;
    assert_eq!(list.len(), 0);
    println!("✓ Tab 1: Watchlists & What Changed E2E flow verified");

    // ------------------------------------------------------------------------
    // TEST TAB 2: Impact Analysis
    // ------------------------------------------------------------------------
    let res = client
        .get(format!(
            "{}/api/intelligence/impact?key=jira:issue:OPEN-4679",
            base
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let impact: Value = res.json().await?;
    assert_eq!(impact["target_key"], "jira:issue:OPEN-4679");
    assert!(impact["total_impacted"].as_u64().unwrap() >= 1);
    // OPEN-102 must be in the impact list with evidence path
    let items = impact["items"].as_array().unwrap();
    let has_open102 = items
        .iter()
        .any(|i| i["object_key"] == "jira:issue:OPEN-102");
    assert!(has_open102, "OPEN-102 should be impacted by OPEN-4679");
    println!(
        "✓ Tab 2: Impact Analysis E2E verified (found {} downstream items, severity: {})",
        impact["total_impacted"], impact["overall_severity"]
    );

    // ------------------------------------------------------------------------
    // TEST TAB 3: Engineering Risk Radar
    // ------------------------------------------------------------------------
    let res = client
        .get(format!("{}/api/intelligence/risk", base))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let risk: Value = res.json().await?;
    let total_risks = risk["total_risks"].as_u64().unwrap();
    assert!(
        total_risks >= 1,
        "Should have detected risks (stale/unassigned)"
    );
    let items = risk["items"].as_array().unwrap();
    let categories: Vec<String> = items
        .iter()
        .map(|i| i["category"].as_str().unwrap().to_string())
        .collect();
    println!(
        "✓ Tab 3: Risk Radar E2E verified (total risks: {}, categories: {:?})",
        total_risks, categories
    );

    // ------------------------------------------------------------------------
    // TEST TAB 4: Story & Timeline Reconstruction
    // ------------------------------------------------------------------------
    let res = client
        .get(format!(
            "{}/api/intelligence/timeline?key=jira:issue:OPEN-4679",
            base
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let timeline: Value = res.json().await?;
    assert_eq!(timeline["root_key"], "jira:issue:OPEN-4679");
    assert!(!timeline["story_narrative"].as_str().unwrap().is_empty());
    let events = timeline["events"].as_array().unwrap();
    assert!(
        !events.is_empty(),
        "Timeline should contain events across Jira and GitHub PR"
    );
    println!(
        "✓ Tab 4: Story Reconstruction E2E verified (events: {})",
        events.len()
    );

    // ------------------------------------------------------------------------
    // TEST TAB 5: What Are We Missing? (Governance Gaps)
    // ------------------------------------------------------------------------
    let res = client
        .get(format!("{}/api/intelligence/gaps", base))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let gaps: Value = res.json().await?;
    let total_gaps = gaps["total_gaps"].as_u64().unwrap();
    assert!(
        total_gaps >= 1,
        "Should detect gaps (e.g. merged PR without release)"
    );
    let items = gaps["items"].as_array().unwrap();
    let gap_cats: Vec<String> = items
        .iter()
        .map(|i| i["category"].as_str().unwrap().to_string())
        .collect();
    println!(
        "✓ Tab 5: Governance Gaps E2E verified (total gaps: {}, categories: {:?})",
        total_gaps, gap_cats
    );

    // ------------------------------------------------------------------------
    // TEST TAB 6: Saved Investigations (Save -> Reopen with Delta -> Delete)
    // ------------------------------------------------------------------------
    // Save
    let res = client
        .post(format!("{}/api/intelligence/investigations", base))
        .json(&serde_json::json!({
            "title": "Sunbeam BGP EVPN Readiness",
            "query": "Is Sunbeam BGP ready for production?",
            "conclusions_markdown": "OPEN-4679 is Done, but needs validation under cluster load.",
            "evidence_keys": ["jira:issue:OPEN-4679", "github:pull:canonical/sunbeam#45"],
            "tags": "sunbeam,bgp,production"
        }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::CREATED);
    let inv: Value = res.json().await?;
    let inv_id = inv["id"].as_str().unwrap().to_string();
    assert_eq!(inv["title"], "Sunbeam BGP EVPN Readiness");

    // List
    let res = client
        .get(format!("{}/api/intelligence/investigations", base))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let inv_list: Vec<Value> = res.json().await?;
    assert_eq!(inv_list.len(), 1);

    // Reopen & Delta check
    let res = client
        .post(format!(
            "{}/api/intelligence/investigations/{}/reopen",
            base, inv_id
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let reopened: Value = res.json().await?;
    assert_eq!(reopened["investigation"]["id"], inv_id);
    assert!(!reopened["updated_conclusions_guidance"]
        .as_str()
        .unwrap()
        .is_empty());

    // Delete
    let res = client
        .delete(format!(
            "{}/api/intelligence/investigations/{}",
            base, inv_id
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let res = client
        .get(format!("{}/api/intelligence/investigations", base))
        .send()
        .await?;
    let inv_list: Vec<Value> = res.json().await?;
    assert_eq!(inv_list.len(), 0);
    println!("✓ Tab 6: Saved Investigations E2E verified (save, reopen delta, delete)");

    // ------------------------------------------------------------------------
    // TEST TAB 7: Release Readiness
    // ------------------------------------------------------------------------
    // Test with ?target=OPEN
    let res = client
        .get(format!("{}/api/intelligence/readiness?target=OPEN", base))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let readiness: Value = res.json().await?;
    assert_eq!(readiness["target"], "OPEN");
    let score = readiness["readiness_score"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&score));
    assert!(matches!(
        readiness["verdict"].as_str().unwrap(),
        "READY" | "AT_RISK" | "NOT_READY"
    ));

    // Test with ?container=OPEN
    let res = client
        .get(format!(
            "{}/api/intelligence/readiness?container=OPEN",
            base
        ))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let r2: Value = res.json().await?;
    assert_eq!(r2["target"], "OPEN");
    println!(
        "✓ Tab 7: Release Readiness E2E verified (score: {}%, verdict: {})",
        score, readiness["verdict"]
    );

    // ------------------------------------------------------------------------
    // TEST TAB 8: Native MCP Server (JSON-RPC 2.0 over HTTP)
    // ------------------------------------------------------------------------
    // 1. tools/list
    let res = client
        .post(format!("{}/api/mcp", base))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let mcp_res: Value = res.json().await?;
    let tools = mcp_res["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 18, "Expected exactly 18 advertised MCP tools");

    // 2. tools/call impact_analysis
    let res = client
        .post(format!("{}/api/mcp", base))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "impact_analysis",
                "arguments": {
                    "object_key": "jira:issue:OPEN-4679"
                }
            }
        }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let call_res: Value = res.json().await?;
    assert!(call_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("total_impacted"));

    // 3. tools/call risk_radar
    let res = client
        .post(format!("{}/api/mcp", base))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "risk_radar",
                "arguments": {}
            }
        }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let call_res: Value = res.json().await?;
    assert!(call_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("total_risks"));

    // 4. tools/call concepts
    let res = client
        .post(format!("{}/api/mcp", base))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "concepts",
                "arguments": { "limit": 10 }
            }
        }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    let call_res: Value = res.json().await?;
    assert!(call_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("total_concepts"));

    // 5. REST API: GET /api/concepts
    let res = client.get(format!("{}/api/concepts", base)).send().await?;
    assert_eq!(res.status(), StatusCode::OK);
    let concepts_val: Value = res.json().await?;
    assert!(concepts_val.get("total_concepts").is_some());

    println!(
        "✓ Tab 8: Native MCP Server E2E verified (18 tools listed, live execution functional)"
    );

    println!("\n=== ALL INTELLIGENCE E2E TESTS PASSED SUCCESSFULLY! ===");
    Ok(())
}
