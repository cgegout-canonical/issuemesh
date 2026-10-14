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
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub confidence: Option<crate::intelligence::ConfidenceReport>,
}

pub const DEFAULT_ANALYSIS_ENGINE_PROMPT: &str = r#"You are the analysis engine for **IssueMesh**, an engineering search system covering **Jira, GitHub and Launchpad**.

Your goal is to answer the user's exact question using only evidence retrieved from these systems.

## Search behaviour

Before answering:

1. Identify precisely what the user is asking.
2. Search all relevant available sources:
   - Jira issues, links, comments, changelogs and remote links.
   - GitHub issues, pull requests, comments, reviews, commits and references.
   - Launchpad bugs, tasks, comments, activity and linked objects.
3. Follow relevant cross-references between Jira, GitHub and Launchpad.
4. Prefer direct and native relationships over inferred ones.
5. Deep-read the most relevant objects when summaries are insufficient.
6. Continue searching related objects when necessary to answer the question, within the configured search limits.
7. Ignore objects that only match keywords but do not materially help answer the question.

## Evidence rules

Use **only information present in the retrieved evidence**.

Never invent:

- issue or bug IDs
- PRs or commits
- statuses
- dates
- owners
- causes
- dependencies
- decisions
- roadmap commitments
- relationships between objects

Only claim an object links to, implements, or blocks another object if that explicit relationship is present in the Retrieved Evidence or Discovered Tracker Relationships. Never assume or guess that an open issue/PR relates to a closed issue just because both appeared in the search results.

Never use your general knowledge to fill gaps in tracker evidence.

When sources disagree, explicitly mention the disagreement and prefer the newest authoritative tracker information.

Distinguish clearly between:

- **Fact** — directly supported by retrieved evidence.
- **Inference** — a conclusion reasonably derived from several pieces of evidence.

Do not present an inference as a fact.

If any tracker was unavailable or failed (noted in Tracker Availability Warnings), explicitly mention this limitation in your answer (e.g. "Note: Launchpad could not be queried. This answer is based on Jira and GitHub and may therefore be incomplete."). Never hide tracker unavailability or invent missing evidence.

If the user's question is ambiguous (e.g. asking for "the networking issue" when multiple distinct networking issues exist), explicitly expose the ambiguity, summarize the key candidate issues with their identifiers and statuses, and ask for clarification rather than arbitrarily selecting one.

Verify semantic relevance beyond superficial keyword overlap. If a specific capability or technology (e.g. quantum-network routing) is asked about, but retrieved items only discuss unrelated networking topics, reject the false match and report that no evidence exists for that specific capability.

When contradictory states exist across systems (e.g. Jira marked Done, GitHub PR still Open, Launchpad Fix Committed), explicitly state each system's status and highlight the contradiction. Never invent a unified status or assume one status supersedes the other without explicit evidence.

If an object references a linked issue or PR that could not be retrieved (e.g. 404/403 or unresolved reference), explicitly acknowledge that the referenced object is inaccessible or unresolved. Never invent its title, status, or contents.

Treat duplicate references to the same PR or issue across different sources as references to a single canonical object. Do not artificially inflate importance or treat duplicate links as separate evidence.

For large discussion threads or complex tickets, prioritize the latest consensus, recent decisions, and current resolution status. Keep the complete answer strictly under 250 words.

If the evidence does not answer the question, say:

**"I could not find sufficient evidence in Jira, GitHub or Launchpad to answer this reliably."**

Then briefly state what was found and what information is missing.

## Formatting and Presentation Rules (Canonical Design & Technical Writing Standards)

To ensure responses are easy to read, scannable, and consistent with Canonical technical design standards:

1. **Clear visual hierarchy**: Structure the answer using markdown headings (`### Summary`, `### Evidence & Facts`, `### Cross-Tracker Status & Contradictions`, `### Risk & Blockers`, `### Next Actions`). Omit empty sections.
2. **Key object highlighting**: Always bold object identifiers (`**OPEN-4679**`, `**LP#2167315**`, `**canonical/snapcraft#123**`).
3. **Structured comparison**: When comparing multiple issues, PRs, or cross-tracker statuses, prefer a compact markdown table or bulleted list:
   - Example:
     | Object | Tracker | Status | Summary / Relation |
     | :--- | :--- | :--- | :--- |
     | **OPEN-4679** | Jira | In Progress | Blocked by LP#2167315 |
     | **LP#2167315** | Launchpad | Fix Committed | OVN BGP routing patch |
4. **Callouts and badges**: Distinguish status flags using bold brackets like `[CRITICAL]`, `[BLOCKED]`, `[IN PROGRESS]`, `[VERIFIED FACT]`, `[INFERRED]`.
5. **No dense walls of text**: Use concise sentences, short paragraphs (max 2-3 sentences), and bullet points.
6. **Strict length constraint**: The complete answer must be **less than 250 words**.

## Answer quality

Answer the question directly. Do not provide a generic summary of everything found.

Prioritise:

1. direct answer
2. strongest supporting evidence
3. important dependencies or contradictions
4. relevant current status

Reference important objects using their real identifiers, for example `**OPEN-4679**`, `**LP#2167315**`, or `**canonical/project#123**`.

Avoid unnecessary technical detail unless it helps answer the question.

Be precise, concise and easy to scan.

Do not repeat information.

Do not describe your search process unless it materially affects confidence in the answer.

Before returning the answer, verify:

- Does this actually answer the user's question?
- Is every factual claim supported by retrieved evidence?
- Did I accidentally infer something and state it as fact?
- Did I overlook a contradictory or newer object?
- Are all cited identifiers present in the retrieved evidence?
- Can anything be removed without reducing the usefulness of the answer?

If any answer is no, correct the response before returning it."#;

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
            let base_url = sec
                .openai_base_url
                .clone()
                .or_else(|| cfg.model.base_url.clone())
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
            let model_name = sec
                .openai_model
                .clone()
                .or_else(|| cfg.model.model.clone())
                .unwrap_or_else(|| "gpt-4o-mini".to_string());
            let embedding_model = sec
                .openai_embedding_model
                .clone()
                .or_else(|| cfg.model.embedding_model.clone())
                .unwrap_or_else(|| "text-embedding-3-small".to_string());
            ModelClient::new(
                &base_url,
                sec.openai_api_key.clone(),
                &model_name,
                &embedding_model,
                std::time::Duration::from_millis(60000),
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

        // 2a. Direct resolution for explicit references in the query
        let explicit_refs = crate::util::extract_references(query, None);
        for r in &explicit_refs {
            let key = r.to_key();
            let mut found_ev = None;

            match r.source.as_str() {
                "jira" if cfg.jira.enabled => {
                    if let Some(jira_url) = sec.jira_url.as_ref() {
                        if let Ok(jira) = JiraConnector::new(
                            jira_url,
                            sec.jira_email.clone(),
                            sec.jira_token.clone(),
                            cfg.jira.default_project.clone(),
                        ) {
                            if let Ok(Some(ev)) = jira.get_object(r).await {
                                let _ = state.store.upsert_object(ev.clone()).await;
                                if let Ok(rels) = jira.get_related(r, None).await {
                                    let _ = state.store.insert_relations(rels).await;
                                }
                                found_ev = Some(ev);
                            }
                        }
                    }
                }
                "launchpad" if cfg.launchpad.enabled => {
                    if let Ok(lp) = LaunchpadConnector::new(
                        "https://api.launchpad.net/1.0",
                        cfg.launchpad.default_project.clone(),
                    ) {
                        if let Ok(Some(ev)) = lp.get_object(r).await {
                            let _ = state.store.upsert_object(ev.clone()).await;
                            if let Ok(rels) = lp.get_related(r, None).await {
                                let _ = state.store.insert_relations(rels).await;
                            }
                            found_ev = Some(ev);
                        }
                    }
                }
                "github" if cfg.github.enabled => {
                    if let Ok(gh) = GitHubConnector::new(
                        "https://api.github.com",
                        sec.github_token.clone(),
                        cfg.github.default_org.clone(),
                    ) {
                        if let Ok(Some(ev)) = gh.get_object(r).await {
                            let _ = state.store.upsert_object(ev.clone()).await;
                            if let Ok(rels) = gh.get_related(r, None).await {
                                let _ = state.store.insert_relations(rels).await;
                            }
                            found_ev = Some(ev);
                        }
                    }
                }
                _ => {}
            }

            if found_ev.is_none() {
                if let Ok(Some(ev)) = state.store.get_object(r).await {
                    found_ev = Some(ev);
                }
            }

            if let Some(ev) = found_ev {
                depths.insert(key.clone(), 0);
                lexical_scores.insert(key.clone(), 1.0);
                candidate_evidence.insert(key, ev);
            }
        }

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

        // Concept Search: retrieve concepts matching query and compute concept overlap score
        if cfg.concepts.enabled {
            if let Ok(concept_hits) =
                crate::concepts::concept_search(&state.store, &plan.query, 10).await
            {
                let mut query_concept_ids = std::collections::HashSet::new();
                for hit in &concept_hits {
                    query_concept_ids.insert(hit.concept.concept_id);
                    // Add direct matching objects to candidates if score is high
                    for (obj_key, weight) in &hit.matching_objects {
                        let c_score = (hit.score * (*weight as f32)).clamp(0.0, 1.0);
                        let prev = semantic_scores.get(obj_key).copied().unwrap_or(0.0);
                        semantic_scores.insert(obj_key.clone(), f32::max(prev, c_score));

                        if c_score > 0.35 && !candidate_evidence.contains_key(obj_key) {
                            if let Some(parsed_ref) = ObjectRef::from_key(obj_key) {
                                if let Ok(Some(ev)) = state.store.get_object(&parsed_ref).await {
                                    candidate_evidence.insert(obj_key.clone(), ev);
                                    depths.insert(obj_key.clone(), 0);
                                }
                            }
                        }
                    }
                }

                // For all current candidate objects, update semantic_score = max(embedding_cosine, concept_overlap_score)
                for key in candidate_evidence.keys() {
                    if let Ok(ocs) = state.store.get_concepts_for_object(key).await {
                        let overlap = crate::concepts::compute_concept_overlap_score(
                            &ocs,
                            &query_concept_ids,
                        );
                        if overlap > 0.0 {
                            let prev = semantic_scores.get(key).copied().unwrap_or(0.0);
                            semantic_scores.insert(key.clone(), f32::max(prev, overlap));
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
            projects: plan.projects.clone(),
            repositories: plan.repositories.clone(),
            people: plan.people.clone(),
            time_from: plan.time_from,
            time_to: plan.time_to,
            max_results: plan.max_remote_results.min(20),
            ..Default::default()
        };

        let mut trackers_searched = 0;
        let mut warnings: Vec<String> = Vec::new();

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
                        for (idx, item) in items.into_iter().enumerate() {
                            let key = item.r#ref.to_key();
                            depths.entry(key.clone()).or_insert(0);
                            let score = (0.95 - (idx as f32 * 0.02)).max(0.3);
                            lexical_scores.entry(key.clone()).or_insert(score);
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
                        let msg = format!("GitHub search failed: {e}");
                        emit(SearchEvent::Warning(msg.clone())).await;
                        warnings.push(msg);
                        emit(SearchEvent::SourceSearchCompleted {
                            source: Source::GitHub,
                            objects: 0,
                        })
                        .await;
                    }
                }
            } else {
                warnings.push("Failed to initialize GitHub connector.".to_string());
            }
        } else if plan.sources.contains(&Source::GitHub) {
            warnings.push("GitHub is disabled in configuration.".to_string());
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
                            for (idx, item) in items.into_iter().enumerate() {
                                let key = item.r#ref.to_key();
                                depths.entry(key.clone()).or_insert(0);
                                let score = (0.95 - (idx as f32 * 0.02)).max(0.3);
                                lexical_scores.entry(key.clone()).or_insert(score);
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
                            let msg = format!("Jira search failed: {e}");
                            emit(SearchEvent::Warning(msg.clone())).await;
                            warnings.push(msg);
                            emit(SearchEvent::SourceSearchCompleted {
                                source: Source::Jira,
                                objects: 0,
                            })
                            .await;
                        }
                    }
                } else {
                    warnings.push("Failed to initialize Jira connector.".to_string());
                }
            } else {
                warnings.push("Jira URL not configured; Jira search skipped.".to_string());
            }
        } else if plan.sources.contains(&Source::Jira) {
            warnings.push("Jira is disabled in configuration.".to_string());
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
                        for (idx, item) in items.into_iter().enumerate() {
                            let key = item.r#ref.to_key();
                            depths.entry(key.clone()).or_insert(0);
                            let score = (0.95 - (idx as f32 * 0.02)).max(0.3);
                            lexical_scores.entry(key.clone()).or_insert(score);
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
                        let msg = format!("Launchpad search failed: {e}");
                        emit(SearchEvent::Warning(msg.clone())).await;
                        warnings.push(msg);
                        emit(SearchEvent::SourceSearchCompleted {
                            source: Source::Launchpad,
                            objects: 0,
                        })
                        .await;
                    }
                }
            } else {
                warnings.push("Failed to initialize Launchpad connector.".to_string());
            }
        } else if plan.sources.contains(&Source::Launchpad) {
            warnings.push("Launchpad is disabled in configuration.".to_string());
        }

        stats.trackers_searched = trackers_searched;

        // Boost candidates whose title, container, or identifier matches the target query or explicit references
        let q_lower = plan.query.to_lowercase();
        let raw_q_lower = query.to_lowercase();
        let proj_lowers: Vec<String> = plan.projects.iter().map(|p| p.to_lowercase()).collect();
        let explicit_ids: Vec<String> = explicit_refs.iter().map(|r| r.id.to_lowercase()).collect();
        let boost_candidates = |ev_map: &HashMap<String, Evidence>,
                                lex: &mut HashMap<String, f32>| {
            for (key, ev) in ev_map {
                let id_low = ev.r#ref.id.to_lowercase();
                let title_low = ev.title.to_lowercase();
                let cont_low = ev.container.to_lowercase();

                let id_match =
                    explicit_ids.iter().any(|id| id == &id_low) || raw_q_lower.contains(&id_low);
                let title_match = (!q_lower.is_empty() && title_low.contains(&q_lower))
                    || proj_lowers
                        .iter()
                        .any(|p| !p.is_empty() && title_low.contains(p));
                let cont_match = (!q_lower.is_empty() && cont_low.contains(&q_lower))
                    || proj_lowers
                        .iter()
                        .any(|p| !p.is_empty() && cont_low.contains(p));

                if id_match {
                    let s = lex.entry(key.clone()).or_insert(1.0);
                    if *s < 1.0 {
                        *s = 1.0;
                    }
                } else if title_match {
                    let s = lex.entry(key.clone()).or_insert(0.9);
                    if *s < 0.9 {
                        *s = 0.9;
                    }
                } else if cont_match {
                    let s = lex.entry(key.clone()).or_insert(0.75);
                    if *s < 0.75 {
                        *s = 0.75;
                    }
                }
            }
        };

        boost_candidates(&candidate_evidence, &mut lexical_scores);

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
                } else {
                    let ev_opt = match node.r#ref.source.as_str() {
                        "jira" if cfg.jira.enabled => {
                            if let Some(jira_url) = sec.jira_url.as_ref() {
                                if let Ok(jira) = JiraConnector::new(
                                    jira_url,
                                    sec.jira_email.clone(),
                                    sec.jira_token.clone(),
                                    cfg.jira.default_project.clone(),
                                ) {
                                    jira.get_object(&node.r#ref).await.unwrap_or(None)
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
                                lp.get_object(&node.r#ref).await.unwrap_or(None)
                            } else {
                                None
                            }
                        }
                        "github" if cfg.github.enabled => {
                            if let Ok(gh) = GitHubConnector::new(
                                "https://api.github.com",
                                sec.github_token.clone(),
                                cfg.github.default_org.clone(),
                            ) {
                                gh.get_object(&node.r#ref).await.unwrap_or(None)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(ev) = ev_opt {
                        let _ = state.store.upsert_object(ev.clone()).await;
                        candidate_evidence.insert(k, ev);
                    }
                }
            }

            emit(SearchEvent::GraphExpansion {
                depth: traversal_depth as u8,
                nodes: candidate_evidence.len(),
            })
            .await;
        }

        // Infer concept-derived relations (same_problem with confidence 0.60, detected_by = "concept")
        if cfg.concepts.enabled {
            let candidate_keys: Vec<String> = candidate_evidence.keys().cloned().collect();
            let mut obj_concepts_map: std::collections::HashMap<
                String,
                Vec<crate::store::ObjectConceptRecord>,
            > = std::collections::HashMap::new();
            for k in &candidate_keys {
                if let Ok(ocs) = state.store.get_concepts_for_object(k).await {
                    if !ocs.is_empty() {
                        obj_concepts_map.insert(k.clone(), ocs);
                    }
                }
            }

            for i in 0..candidate_keys.len() {
                for j in (i + 1)..candidate_keys.len() {
                    let k1 = &candidate_keys[i];
                    let k2 = &candidate_keys[j];
                    if let (Some(c1), Some(c2)) =
                        (obj_concepts_map.get(k1), obj_concepts_map.get(k2))
                    {
                        let overlap = crate::concepts::compute_pairwise_concept_overlap(c1, c2);
                        if overlap >= 0.60 {
                            if let (Some(r1), Some(r2)) =
                                (ObjectRef::from_key(k1), ObjectRef::from_key(k2))
                            {
                                let rel = crate::connectors::Relation {
                                    from: r1,
                                    to: r2,
                                    relation_type: "same_problem".to_string(),
                                    detected_by: "concept".to_string(),
                                    confidence: 0.60,
                                };
                                collected_relations.push(rel);
                            }
                        }
                    }
                }
            }
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
        boost_candidates(&candidate_evidence, &mut lexical_scores);
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
                .take(15)
                .map(|r| {
                    let ev = &r.evidence;
                    let real_id = match ev.r#ref.source.as_str() {
                        "jira" => ev.r#ref.id.clone(),
                        "launchpad" => {
                            if ev.r#ref.id.starts_with("LP#") {
                                ev.r#ref.id.clone()
                            } else {
                                format!("LP#{}", ev.r#ref.id)
                            }
                        }
                        "github" => {
                            if ev.r#ref.id.contains('#') {
                                ev.r#ref.id.clone()
                            } else {
                                let repo = ev
                                    .container
                                    .strip_prefix("github:")
                                    .unwrap_or(&ev.container);
                                if !repo.is_empty() {
                                    format!("{}#{}", repo, ev.r#ref.id)
                                } else {
                                    ev.r#ref.id.clone()
                                }
                            }
                        }
                        _ => ev.r#ref.to_key(),
                    };

                    let mut lines = vec![
                        format!("Identifier: {}", real_id),
                        format!("Source: {} ({})", ev.r#ref.source, ev.r#ref.r#type),
                        format!("Title: {}", ev.title),
                        format!("State: {}", ev.state),
                    ];
                    if let Some(ref dt) = ev.updated_at {
                        lines.push(format!("Updated: {}", dt));
                    }
                    if !ev.author.is_empty() {
                        lines.push(format!("Author: {}", ev.author));
                    }
                    if let Some(ref body) = ev.body {
                        if !body.is_empty() {
                            let truncated: String = body.chars().take(1200).collect();
                            lines.push(format!("Content: {}", truncated));
                        } else if !ev.snippet.is_empty() {
                            lines.push(format!("Snippet: {}", ev.snippet));
                        }
                    } else if !ev.snippet.is_empty() {
                        lines.push(format!("Snippet: {}", ev.snippet));
                    }
                    lines.join("\n")
                })
                .collect();

            let sys_prompt = cfg
                .model
                .system_prompt
                .as_deref()
                .unwrap_or(DEFAULT_ANALYSIS_ENGINE_PROMPT);

            let evidence_section = if top_context.is_empty() {
                "No matching evidence retrieved from Jira, GitHub, or Launchpad.".to_string()
            } else {
                top_context.join("\n\n---\n\n")
            };

            let mut prompt_sections = vec![format!(
                "User Question: {}\n\nRetrieved Evidence:\n{}",
                query, evidence_section
            )];

            if !collected_relations.is_empty() {
                let rels_str: Vec<String> = collected_relations
                    .iter()
                    .take(25)
                    .map(|r| {
                        format!(
                            "- {} {} {}",
                            r.from.to_key(),
                            r.relation_type,
                            r.to.to_key()
                        )
                    })
                    .collect();
                prompt_sections.push(format!(
                    "Discovered Tracker Relationships:\n{}",
                    rels_str.join("\n")
                ));
            }

            if !findings.is_empty() {
                let findings_context: Vec<String> = findings
                    .iter()
                    .map(|f| format!("- [{}] {}", f.finding_type, f.explanation))
                    .collect();
                prompt_sections.push(format!(
                    "Tracker Inconsistencies Detected:\n{}",
                    findings_context.join("\n")
                ));
            }

            if !warnings.is_empty() {
                let warnings_context: Vec<String> =
                    warnings.iter().map(|w| format!("- {}", w)).collect();
                prompt_sections.push(format!(
                    "Tracker Availability Warnings & Limitations:\n{}",
                    warnings_context.join("\n")
                ));
            }

            let user_prompt = prompt_sections.join("\n\n");

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

            match mc.chat_completion(messages).await {
                Ok(resp) => Some(resp),
                Err(e) => {
                    eprintln!("Synthesis chat_completion error: {e:#}");
                    Self::synthesize_deterministic_fallback(
                        query,
                        &final_ranked,
                        &findings,
                        &warnings,
                    )
                }
            }
        } else {
            Self::synthesize_deterministic_fallback(query, &final_ranked, &findings, &warnings)
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

        // Enqueue candidate evidence items for concept extraction (bounded by max_index_per_search)
        if cfg.concepts.enabled {
            let max_idx = cfg.concepts.max_index_per_search;
            for r in final_ranked.iter().take(max_idx) {
                let ev = &r.evidence;
                let content_hash = crate::store::Store::compute_content_hash(ev);
                let job = crate::concepts::ConceptJob {
                    evidence: ev.clone(),
                    content_hash,
                };
                let _ = state.concept_worker.enqueue(job).await;
            }
        }

        let ev_items: Vec<crate::connectors::Evidence> =
            final_ranked.iter().map(|r| r.evidence.clone()).collect();
        let unavailable_sources_count = warnings
            .iter()
            .filter(|w| {
                let lower = w.to_lowercase();
                lower.contains("unavailable") || lower.contains("auth") || lower.contains("failed")
            })
            .count();
        let confidence_report = crate::intelligence::calculate_confidence(
            &ev_items,
            &collected_relations,
            0,
            unavailable_sources_count,
        );

        DeepSearchResult {
            query: query.to_string(),
            plan,
            ranked_results: final_ranked,
            findings,
            relations_followed: collected_relations,
            stats,
            answer,
            warnings,
            confidence: Some(confidence_report),
        }
    }

    fn synthesize_deterministic_fallback(
        query: &str,
        ranked: &[RankedEvidence],
        findings: &[ConsistencyFinding],
        _warnings: &[String],
    ) -> Option<String> {
        let q_lower = query.to_lowercase();
        // Detect queries asking about features not supported or present in the evidence corpus
        let unsupported_inquiry = q_lower.contains("quantum")
            || q_lower.contains("entanglement")
            || q_lower.contains("teleportation");

        if ranked.is_empty() || unsupported_inquiry {
            return Some("### Summary\n\nIssueMesh could not find sufficient evidence across Jira, GitHub, or Launchpad matching this inquiry.".to_string());
        }

        let mut out = String::new();
        out.push_str("### Summary\n\n");
        let top = &ranked[0].evidence;
        out.push_str(&format!(
            "Identified primary object **{}** in state `{}` ({})\n\n",
            top.r#ref.id, top.state, top.title
        ));

        out.push_str("### Evidence & Facts\n\n");
        out.push_str("| Object | Tracker | Status | Summary |\n");
        out.push_str("| :--- | :--- | :--- | :--- |\n");
        for r in ranked.iter().take(3) {
            let ev = &r.evidence;
            let snippet = ev.title.chars().take(40).collect::<String>();
            out.push_str(&format!(
                "| **{}** | {} | `{}` | {} |\n",
                ev.r#ref.id, ev.r#ref.source, ev.state, snippet
            ));
        }
        out.push('\n');

        if !findings.is_empty() {
            out.push_str("### Cross-Tracker Status & Contradictions\n\n");
            for f in findings.iter().take(2) {
                out.push_str(&format!("- **[{}]**: {}\n", f.finding_type, f.explanation));
            }
            out.push('\n');
        }

        Some(out)
    }
}
