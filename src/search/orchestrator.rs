use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::connectors::github::GitHubConnector;
use crate::connectors::jira::JiraConnector;
use crate::connectors::launchpad::LaunchpadConnector;
use crate::connectors::{Connector, Evidence, ObjectRef, SearchQuery};
use crate::jobs::{SearchEvent, SearchStats, Source};
use crate::model::client::ChatMessage;
use crate::model::planner::SearchPlan;
use crate::model::ModelClient;
use crate::search::consistency::{analyze_consistency, ConsistencyFinding};
use crate::search::ranking::{rank_candidates, RankedEvidence, Weights};
use crate::AppState;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeepSearchResult {
    pub query: String,
    pub plan: SearchPlan,
    pub ranked_results: Vec<RankedEvidence>,
    pub findings: Vec<ConsistencyFinding>,
    pub relations_followed: Vec<crate::connectors::Relation>,
    pub stats: SearchStats,
    pub answer: Option<String>,
}

pub struct SearchOrchestrator;

impl SearchOrchestrator {
    pub async fn execute(
        state: Arc<AppState>,
        job_id: Option<&str>,
        query: &str,
    ) -> DeepSearchResult {
        let start = Instant::now();
        let mut stats = SearchStats::default();

        let emit = |ev: SearchEvent| {
            let state = Arc::clone(&state);
            let j_id = job_id.map(|s| s.to_string());
            async move {
                if let Some(id) = j_id {
                    state.jobs.emit_event(&id, ev).await;
                }
            }
        };

        // 1. Planning
        emit(SearchEvent::PlanningStarted).await;

        let (cfg, sec) = {
            let c = state.config.read().await;
            let s = state.secrets.read().await;
            (c.clone(), s.clone())
        };

        let model_client = if cfg.model.enabled {
            let base_url = cfg
                .model
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
            let model_name = cfg
                .model
                .model
                .clone()
                .unwrap_or_else(|| "gpt-4o-mini".to_string());
            let embedding_model = cfg
                .model
                .embedding_model
                .clone()
                .unwrap_or_else(|| "text-embedding-3-small".to_string());
            ModelClient::new(
                &base_url,
                sec.openai_api_key.clone(),
                &model_name,
                &embedding_model,
                std::time::Duration::from_millis(5000),
            )
            .ok()
        } else {
            None
        };

        let plan = crate::model::QueryPlanner::plan(query, model_client.as_ref()).await;
        emit(SearchEvent::PlanningCompleted).await;

        // 2. Local FTS Search + Local Semantic Search
        emit(SearchEvent::LocalSearchStarted).await;

        let mut candidate_evidence: HashMap<String, Evidence> = HashMap::new();
        let mut lexical_scores: HashMap<String, f32> = HashMap::new();
        let mut semantic_scores: HashMap<String, f32> = HashMap::new();
        let mut depths: HashMap<String, u8> = HashMap::new();

        if let Ok(fts_hits) = state.store.search_fts(&plan.query, 50).await {
            for (idx, hit) in fts_hits.into_iter().enumerate() {
                let key = hit.evidence.r#ref.to_key();
                // Normalized lexical score by position
                let lex_score = (1.0 - (idx as f32 / 50.0)).max(0.1);
                lexical_scores.insert(key.clone(), lex_score);
                depths.insert(key.clone(), 0);
                candidate_evidence.insert(key, hit.evidence);
            }
        }

        // Local semantic search if embeddings available
        if let Some(ref mc) = model_client {
            if let Ok(embeddings) = mc.embed(std::slice::from_ref(&plan.query)).await {
                if let Some(query_embedding) = embeddings.into_iter().next() {
                    let emb_model = cfg
                        .model
                        .embedding_model
                        .as_deref()
                        .unwrap_or("text-embedding-3-small");
                    if let Ok(all_embs) = state
                        .store
                        .get_valid_embeddings(emb_model, query_embedding.len())
                        .await
                    {
                        for (key, vec) in all_embs {
                            let sim =
                                crate::search::ranking::cosine_similarity(&query_embedding, &vec);
                            let norm_sim = ((sim + 1.0) / 2.0).clamp(0.0, 1.0);
                            semantic_scores.insert(key.clone(), norm_sim);

                            if norm_sim > 0.4 && !candidate_evidence.contains_key(&key) {
                                if let Some(parsed_ref) = ObjectRef::from_key(&key) {
                                    if let Ok(Some(ev)) = state.store.get_object(&parsed_ref).await
                                    {
                                        candidate_evidence.insert(key.clone(), ev);
                                        depths.insert(key, 0);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        emit(SearchEvent::LocalSearchCompleted {
            objects: candidate_evidence.len(),
        })
        .await;
        stats.objects_analysed += candidate_evidence.len();

        // 3. Remote Tracker Searches Concurrently
        let search_req = SearchQuery {
            query: plan.query.clone(),
            max_results: plan.max_remote_results.min(20),
            ..Default::default()
        };

        let mut trackers_searched = 0;

        // GitHub search
        if cfg.github.enabled {
            trackers_searched += 1;
            emit(SearchEvent::SourceSearchStarted {
                source: Source::GitHub,
            })
            .await;
            if let Ok(gh) = GitHubConnector::new(
                "https://api.github.com",
                sec.github_token.clone(),
                cfg.github.default_org.clone(),
            ) {
                match gh.search(&search_req).await {
                    Ok(items) => {
                        let count = items.len();
                        stats.objects_analysed += count;
                        for item in items {
                            let key = item.r#ref.to_key();
                            depths.entry(key.clone()).or_insert(0);
                            let _ = state.store.upsert_object(item.clone()).await;
                            candidate_evidence.insert(key, item);
                        }
                        emit(SearchEvent::SourceSearchCompleted {
                            source: Source::GitHub,
                            objects: count,
                        })
                        .await;
                    }
                    Err(e) => {
                        emit(SearchEvent::Warning(format!("GitHub search failed: {e}"))).await;
                        emit(SearchEvent::SourceSearchCompleted {
                            source: Source::GitHub,
                            objects: 0,
                        })
                        .await;
                    }
                }
            }
        }

        // Jira search
        if cfg.jira.enabled {
            if let Some(jira_url) = sec.jira_url.clone() {
                trackers_searched += 1;
                emit(SearchEvent::SourceSearchStarted {
                    source: Source::Jira,
                })
                .await;
                if let Ok(jira) = JiraConnector::new(
                    &jira_url,
                    sec.jira_email.clone(),
                    sec.jira_token.clone(),
                    cfg.jira.default_project.clone(),
                ) {
                    match jira.search(&search_req).await {
                        Ok(items) => {
                            let count = items.len();
                            stats.objects_analysed += count;
                            for item in items {
                                let key = item.r#ref.to_key();
                                depths.entry(key.clone()).or_insert(0);
                                let _ = state.store.upsert_object(item.clone()).await;
                                candidate_evidence.insert(key, item);
                            }
                            emit(SearchEvent::SourceSearchCompleted {
                                source: Source::Jira,
                                objects: count,
                            })
                            .await;
                        }
                        Err(e) => {
                            emit(SearchEvent::Warning(format!("Jira search failed: {e}"))).await;
                            emit(SearchEvent::SourceSearchCompleted {
                                source: Source::Jira,
                                objects: 0,
                            })
                            .await;
                        }
                    }
                }
            }
        }

        // Launchpad search
        if cfg.launchpad.enabled {
            trackers_searched += 1;
            emit(SearchEvent::SourceSearchStarted {
                source: Source::Launchpad,
            })
            .await;
            if let Ok(lp) = LaunchpadConnector::new(
                "https://api.launchpad.net/1.0",
                cfg.launchpad.default_project.clone(),
            ) {
                match lp.search(&search_req).await {
                    Ok(items) => {
                        let count = items.len();
                        stats.objects_analysed += count;
                        for item in items {
                            let key = item.r#ref.to_key();
                            depths.entry(key.clone()).or_insert(0);
                            let _ = state.store.upsert_object(item.clone()).await;
                            candidate_evidence.insert(key, item);
                        }
                        emit(SearchEvent::SourceSearchCompleted {
                            source: Source::Launchpad,
                            objects: count,
                        })
                        .await;
                    }
                    Err(e) => {
                        emit(SearchEvent::Warning(format!(
                            "Launchpad search failed: {e}"
                        )))
                        .await;
                        emit(SearchEvent::SourceSearchCompleted {
                            source: Source::Launchpad,
                            objects: 0,
                        })
                        .await;
                    }
                }
            }
        }

        stats.trackers_searched = trackers_searched;

        // 4. Initial Candidate Ranking & Deep-read best candidates
        let weights = Weights::default();
        let initial_ranked = rank_candidates(
            candidate_evidence.values().cloned().collect(),
            &lexical_scores,
            &semantic_scores,
            &depths,
            &weights,
        );

        // Take top candidates for deep reads
        let deep_read_limit = plan.max_deep_reads.clamp(1, 20);
        let candidates_to_deep_read: Vec<ObjectRef> = initial_ranked
            .iter()
            .take(deep_read_limit)
            .map(|r| r.evidence.r#ref.clone())
            .collect();

        for target_ref in candidates_to_deep_read {
            emit(SearchEvent::DeepReadStarted {
                object: target_ref.clone(),
            })
            .await;

            let deep_ev = match target_ref.source.as_str() {
                "github" if cfg.github.enabled => {
                    if let Ok(gh) = GitHubConnector::new(
                        "https://api.github.com",
                        sec.github_token.clone(),
                        cfg.github.default_org.clone(),
                    ) {
                        gh.get_object(&target_ref).await.unwrap_or(None)
                    } else {
                        None
                    }
                }
                "jira" if cfg.jira.enabled => {
                    if let Some(jira_url) = sec.jira_url.clone() {
                        if let Ok(jira) = JiraConnector::new(
                            &jira_url,
                            sec.jira_email.clone(),
                            sec.jira_token.clone(),
                            cfg.jira.default_project.clone(),
                        ) {
                            jira.get_object(&target_ref).await.unwrap_or(None)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                "launchpad" if cfg.launchpad.enabled => {
                    if let Ok(lp) = LaunchpadConnector::new(
                        "https://api.launchpad.net/1.0",
                        cfg.launchpad.default_project.clone(),
                    ) {
                        lp.get_object(&target_ref).await.unwrap_or(None)
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some(detailed) = deep_ev {
                stats.deep_reads += 1;
                let key = detailed.r#ref.to_key();
                let _ = state.store.upsert_object(detailed.clone()).await;
                candidate_evidence.insert(key, detailed);
            }

            emit(SearchEvent::DeepReadCompleted {
                object: target_ref.clone(),
            })
            .await;
        }

        // 5. Expand Graph Frontier (Multi-hop traversal)
        let seed_refs: Vec<ObjectRef> = candidate_evidence
            .values()
            .take(20)
            .map(|e| e.r#ref.clone())
            .collect();

        let traversal_depth = plan.graph_depth.min(3) as usize;
        let max_nodes = plan.max_graph_nodes.min(100);

        let mut collected_relations = Vec::new();
        if let Ok(graph) = state
            .store
            .traverse_graph(seed_refs, traversal_depth, max_nodes)
            .await
        {
            stats.relations_followed += graph.edges.len();
            collected_relations = graph.edges;

            for node in graph.nodes {
                let k = node.r#ref.to_key();
                depths.insert(k.clone(), node.depth as u8);
                if let Some(ev) = node.evidence {
                    candidate_evidence.insert(k, ev);
                }
            }

            emit(SearchEvent::GraphExpansion {
                depth: traversal_depth as u8,
                nodes: candidate_evidence.len(),
            })
            .await;
        }

        // 6. Consistency Checks
        emit(SearchEvent::ConsistencyCheckStarted).await;

        let all_evidence: Vec<Evidence> = candidate_evidence.values().cloned().collect();
        let findings = analyze_consistency(&all_evidence, &collected_relations);

        emit(SearchEvent::ConsistencyCheckCompleted {
            findings: findings.len(),
        })
        .await;

        // 7. Final Ranking
        let final_ranked = rank_candidates(
            all_evidence,
            &lexical_scores,
            &semantic_scores,
            &depths,
            &weights,
        );

        // 8. Evidence Synthesis
        emit(SearchEvent::SynthesisStarted).await;

        let answer = if let Some(ref mc) = model_client {
            // Build bounded context from top ranked results
            let top_context: Vec<String> = final_ranked
                .iter()
                .take(5)
                .map(|r| {
                    format!(
                        "- [{}]: {} ({}) - {}",
                        r.evidence.r#ref.to_key(),
                        r.evidence.title,
                        r.evidence.state,
                        r.evidence.snippet
                    )
                })
                .collect();

            let sys_prompt = "You are an assistant synthesizing software tracker evidence (issues, PRs, bugs). Answer concisely citing keys.";
            let user_prompt = format!(
                "Query: {}\n\nEvidence:\n{}\n\nSynthesize findings and any inconsistencies found:",
                query,
                top_context.join("\n")
            );

            let messages = vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: sys_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt,
                },
            ];

            mc.chat_completion(messages).await.ok()
        } else {
            None
        };

        stats.duration_ms = start.elapsed().as_millis() as u64;
        emit(SearchEvent::Complete(stats.clone())).await;

        // Persist search run into Store if job_id is present
        if let Some(id) = job_id {
            let plan_json = serde_json::to_string(&plan).unwrap_or_default();
            let stats_json = serde_json::to_string(&stats).unwrap_or_default();
            let object_keys_scores: Vec<(String, f64)> = final_ranked
                .iter()
                .map(|r| (r.evidence.r#ref.to_key(), r.score as f64))
                .collect();

            let model_name = if cfg.model.enabled {
                cfg.model.model.as_deref()
            } else {
                None
            };

            let _ = state
                .store
                .save_search_run(
                    id,
                    query,
                    &plan_json,
                    &stats_json,
                    answer.as_deref(),
                    Some("v1.0"),
                    model_name,
                    &object_keys_scores,
                )
                .await;
        }

        DeepSearchResult {
            query: query.to_string(),
            plan,
            ranked_results: final_ranked,
            findings,
            relations_followed: collected_relations,
            stats,
            answer,
        }
    }
}
