use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::connectors::ObjectRef;
use crate::intelligence::{
    GapDetector, ImpactAnalyzer, InvestigationManager, ReadinessEvaluator, RiskRadar,
    SaveInvestigationRequest, StoryReconstructor, WatchlistService,
};
use crate::AppState;

pub async fn execute_tool(state: Arc<AppState>, tool_name: &str, args: Value) -> Result<Value> {
    match tool_name {
        "search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'query' parameter"))?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

            let fts_results = state.store.search_fts(query, limit).await?;
            let formatted: Vec<Value> = fts_results
                .into_iter()
                .map(|r| {
                    json!({
                        "object_key": r.evidence.r#ref.to_key(),
                        "source": r.evidence.r#ref.source,
                        "type": r.evidence.r#ref.r#type,
                        "id": r.evidence.r#ref.id,
                        "title": r.evidence.title,
                        "state": r.evidence.state,
                        "author": r.evidence.author,
                        "snippet": r.snippet,
                        "rank": r.rank,
                    })
                })
                .collect();
            Ok(json!({ "results": formatted, "count": formatted.len() }))
        }

        "deep_search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'query' parameter"))?;

            let result =
                crate::search::orchestrator::SearchOrchestrator::execute(state, None, query).await;
            Ok(json!({
                "query": result.query,
                "answer": result.answer,
                "confidence": result.confidence,
                "findings": result.findings,
                "ranked_count": result.ranked_results.len(),
                "top_results": result.ranked_results.into_iter().take(10).map(|r| {
                    json!({
                        "object_key": r.evidence.r#ref.to_key(),
                        "title": r.evidence.title,
                        "state": r.evidence.state,
                        "source": r.evidence.r#ref.source,
                        "score": r.score,
                    })
                }).collect::<Vec<_>>(),
                "warnings": result.warnings,
            }))
        }

        "get_object" => {
            let key = args
                .get("object_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'object_key' parameter"))?;
            let oref = ObjectRef::from_key(key)
                .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", key));

            match state.store.get_object(&oref).await? {
                Some(obj) => Ok(json!(obj)),
                None => Ok(json!({ "error": "Object not found", "object_key": key })),
            }
        }

        "get_evidence" => {
            let key = args
                .get("object_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'object_key' parameter"))?;
            let oref = ObjectRef::from_key(key)
                .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", key));

            let obj = state.store.get_object(&oref).await?;
            let relations = state.store.get_relations_for_key(key).await?;
            let revisions = state.store.get_revisions_for_key(key).await?;

            Ok(json!({
                "object": obj,
                "relations": relations,
                "revisions_count": revisions.len(),
                "revisions": revisions,
            }))
        }

        "related" => {
            let key = args
                .get("object_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'object_key' parameter"))?;
            let relations = state.store.get_relations_for_key(key).await?;

            let mut neighbors = Vec::new();
            for rel in &relations {
                let other_ref = if rel.from.to_key() == key {
                    &rel.to
                } else {
                    &rel.from
                };
                if let Some(ev) = state.store.get_object(other_ref).await? {
                    neighbors.push(json!({
                        "object_key": ev.r#ref.to_key(),
                        "title": ev.title,
                        "state": ev.state,
                        "source": ev.r#ref.source,
                        "relation_type": rel.relation_type,
                        "direction": if rel.from.to_key() == key { "outbound" } else { "inbound" },
                        "confidence": rel.confidence,
                    }));
                }
            }
            Ok(json!({ "relations_count": relations.len(), "neighbors": neighbors }))
        }

        "impact_analysis" => {
            let key = args
                .get("object_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'object_key' parameter"))?;
            let analyzer = ImpactAnalyzer::new(state.store.clone());
            let report = analyzer.analyze_impact(key).await?;
            Ok(json!(report))
        }

        "what_changed" => {
            let svc = WatchlistService::new(state.store.clone());
            if let Some(wlid) = args.get("watchlist_id").and_then(|v| v.as_str()) {
                let res = svc.check_watchlist(wlid).await?;
                Ok(json!(res))
            } else if let Some(key) = args.get("object_key").and_then(|v| v.as_str()) {
                let res = svc.what_changed_for_object(key).await?;
                Ok(json!(res))
            } else {
                Err(anyhow!(
                    "Must provide either 'object_key' or 'watchlist_id'"
                ))
            }
        }

        "timeline" => {
            let key = args
                .get("object_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'object_key' parameter"))?;
            let rec = StoryReconstructor::new(state.store.clone());
            let report = rec.reconstruct_timeline(key).await?;
            Ok(json!(report))
        }

        "risk_radar" => {
            let radar = RiskRadar::new(state.store.clone());
            let mut report = radar.scan().await?;
            if let Some(cat) = args.get("category").and_then(|v| v.as_str()) {
                report
                    .items
                    .retain(|i| i.category.eq_ignore_ascii_case(cat));
                report.total_risks = report.items.len();
            }
            Ok(json!(report))
        }

        "readiness" => {
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'target' parameter"))?;
            let evaluator = ReadinessEvaluator::new(state.store.clone());
            let report = evaluator.assess_readiness(target).await?;
            Ok(json!(report))
        }

        "find_gaps" => {
            let detector = GapDetector::new(state.store.clone());
            let mut report = detector.detect_gaps().await?;
            if let Some(cat) = args.get("category").and_then(|v| v.as_str()) {
                report
                    .items
                    .retain(|i| i.category.eq_ignore_ascii_case(cat));
                report.total_gaps = report.items.len();
            }
            Ok(json!(report))
        }

        "consistency_check" => {
            let radar = RiskRadar::new(state.store.clone());
            let report = radar.scan().await?;
            let inconsistencies: Vec<_> = report
                .items
                .into_iter()
                .filter(|i| {
                    i.category == "contradictory_state" || i.category == "merged_pr_unresolved_jira"
                })
                .collect();
            Ok(json!({
                "contradictions_found": inconsistencies.len(),
                "items": inconsistencies,
                "summary": format!("Consistency audit identified {} tracker contradiction(s).", inconsistencies.len())
            }))
        }

        "saved_investigation" => {
            let mgr = InvestigationManager::new(state.store.clone());
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing 'action' parameter"))?;

            match action {
                "list" => {
                    let list = mgr.list().await?;
                    Ok(json!(list))
                }
                "get" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing 'id' parameter"))?;
                    let inv = mgr.get(id).await?;
                    Ok(json!(inv))
                }
                "reopen" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing 'id' parameter"))?;
                    let res = mgr.reopen(id).await?;
                    Ok(json!(res))
                }
                "save" => {
                    let title = args
                        .get("title")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing 'title'"))?;
                    let query = args
                        .get("query")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing 'query'"))?;
                    let conclusions = args
                        .get("conclusions_markdown")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let keys: Vec<String> = args
                        .get("evidence_keys")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();

                    let rec = mgr
                        .save(SaveInvestigationRequest {
                            title: title.to_string(),
                            query: query.to_string(),
                            conclusions_markdown: conclusions.to_string(),
                            evidence_keys: keys,
                            tags: args
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        })
                        .await?;
                    Ok(json!(rec))
                }
                "delete" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing 'id' parameter"))?;
                    let deleted = mgr.delete(id).await?;
                    Ok(json!({ "deleted": deleted, "id": id }))
                }
                _ => Err(anyhow!("Unsupported investigation action '{}'", action)),
            }
        }

        "memory_stats" => {
            let diag = state
                .store
                .get_memory_diagnostics(&state.paths.db_file)
                .await?;
            let stats = state.store.get_stats().await?;
            Ok(json!({
                "diagnostics": diag,
                "datastore_stats": stats,
            }))
        }

        "health" => {
            let cfg = state.config.read().await;
            let ver = state.store.get_schema_version().await.unwrap_or(0);
            Ok(json!({
                "status": "healthy",
                "version": env!("CARGO_PKG_VERSION"),
                "schema_version": ver,
                "sources_configured": {
                    "jira": cfg.jira.enabled,
                    "github": cfg.github.enabled,
                    "launchpad": cfg.launchpad.enabled,
                }
            }))
        }

        "introspection_status" => {
            let stats = state.store.get_introspection_stats().await?;
            let last_run = state.store.get_last_introspection_run().await?;
            let trends = state.store.get_introspection_trends(7).await?;
            Ok(json!({
                "stats": stats,
                "last_run": last_run,
                "seven_day_trends": trends,
            }))
        }

        _ => Err(anyhow!("Unknown tool '{}'", tool_name)),
    }
}
