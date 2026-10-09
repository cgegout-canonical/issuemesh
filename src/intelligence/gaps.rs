use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::connectors::Evidence;
use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapItem {
    pub id: String,
    pub category: String, // "requirement_without_pr", "jira_without_pr", "merged_without_release", "unresolved_ref", "duplicated_work", "missing_dependency"
    pub severity: String,
    pub title: String,
    pub description: String,
    pub affected_keys: Vec<String>,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapAnalysisReport {
    pub total_gaps: usize,
    pub items: Vec<GapItem>,
    pub summary: String,
}

pub struct GapDetector {
    store: Store,
}

impl GapDetector {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn detect_gaps(&self) -> Result<GapAnalysisReport> {
        let all_objects = self.store.get_all_objects().await?;
        let all_relations = self.store.get_all_relations().await?;
        let unresolved_refs = self.store.get_unresolved_refs(500).await?;

        let mut object_map: HashMap<String, &Evidence> = HashMap::new();
        for obj in &all_objects {
            object_map.insert(obj.r#ref.to_key(), obj);
        }

        // Map outbound and inbound relations
        let mut prs_by_jira: HashMap<String, Vec<String>> = HashMap::new();
        let mut releases_by_pr: HashMap<String, Vec<String>> = HashMap::new();
        let mut issues_by_pr: HashMap<String, Vec<String>> = HashMap::new();

        for rel in &all_relations {
            let from_k = rel.from.to_key();
            let to_k = rel.to.to_key();

            if rel.from.source == "jira" && rel.to.source == "github" && rel.to.r#type == "pull" {
                prs_by_jira
                    .entry(from_k.clone())
                    .or_default()
                    .push(to_k.clone());
                issues_by_pr
                    .entry(to_k.clone())
                    .or_default()
                    .push(from_k.clone());
            } else if rel.to.source == "jira"
                && rel.from.source == "github"
                && rel.from.r#type == "pull"
            {
                prs_by_jira
                    .entry(to_k.clone())
                    .or_default()
                    .push(from_k.clone());
                issues_by_pr
                    .entry(from_k.clone())
                    .or_default()
                    .push(to_k.clone());
            }

            if rel.from.r#type == "pull"
                && (rel.to.r#type == "release" || rel.to.r#type == "milestone")
            {
                releases_by_pr
                    .entry(from_k.clone())
                    .or_default()
                    .push(to_k.clone());
            }
        }

        let mut items = Vec::new();

        // 1. Requirements & Jira Items Without PRs
        for obj in &all_objects {
            if obj.r#ref.source == "jira" {
                let state_lower = obj.state.to_lowercase();
                let is_active = state_lower.contains("in progress")
                    || state_lower.contains("review")
                    || state_lower.contains("done");

                let key = obj.r#ref.to_key();
                let linked_prs = prs_by_jira.get(&key);

                if linked_prs.is_none() || linked_prs.unwrap().is_empty() {
                    if state_lower.contains("done") {
                        items.push(GapItem {
                            id: format!("gap-done-no-pr-{}", items.len() + 1),
                            category: "requirement_without_pr".to_string(),
                            severity: "High".to_string(),
                            title: format!("Jira ticket '{}' marked Done without PR evidence", obj.r#ref.id),
                            description: format!(
                                "Ticket {} ('{}') is resolved as '{}' but no linked pull requests or commits were found in the mesh.",
                                key, obj.title, obj.state
                            ),
                            affected_keys: vec![key.clone()],
                            recommendation: "Link the implementing PR or document why no code change was required.".to_string(),
                        });
                    } else if is_active {
                        items.push(GapItem {
                            id: format!("gap-active-no-pr-{}", items.len() + 1),
                            category: "jira_without_pr".to_string(),
                            severity: "Medium".to_string(),
                            title: format!("In-progress Jira ticket '{}' lacks linked PR", obj.r#ref.id),
                            description: format!(
                                "Ticket {} ('{}') is '{}' but has no active PR linked for review.",
                                key, obj.title, obj.state
                            ),
                            affected_keys: vec![key.clone()],
                            recommendation: "Create or link GitHub pull request to allow tracking implementation progress.".to_string(),
                        });
                    }
                }
            }
        }

        // 2. Merged Code Without Release Evidence
        for obj in &all_objects {
            if obj.r#ref.source == "github" && obj.r#ref.r#type == "pull" {
                let state_lower = obj.state.to_lowercase();
                if state_lower.contains("merged") {
                    let key = obj.r#ref.to_key();
                    let linked_releases = releases_by_pr.get(&key);
                    if linked_releases.is_none() || linked_releases.unwrap().is_empty() {
                        items.push(GapItem {
                            id: format!("gap-merged-no-release-{}", items.len() + 1),
                            category: "merged_without_release".to_string(),
                            severity: "Low".to_string(),
                            title: format!("Merged PR #{} not mapped to any release milestone", obj.r#ref.id),
                            description: format!(
                                "Pull request {} ('{}') was merged but has no milestone or release tag evidence.",
                                key, obj.title
                            ),
                            affected_keys: vec![key.clone()],
                            recommendation: "Tag PR with target milestone or release version.".to_string(),
                        });
                    }
                }
            }
        }

        // 3. Unresolved References (Missing/Private/404 objects)
        for (target_ref, discovered_from_key) in unresolved_refs {
            let target_key = target_ref.to_key();
            items.push(GapItem {
                id: format!("gap-unresolved-ref-{}", items.len() + 1),
                category: "unresolved_ref".to_string(),
                severity: "High".to_string(),
                title: format!("Unresolved tracker reference: {}", target_key),
                description: format!(
                    "Reference to '{}' discovered from '{}' could not be resolved or fetched (404/403 or missing connector).",
                    target_key, discovered_from_key
                ),
                affected_keys: vec![target_key, discovered_from_key],
                recommendation: "Verify tracker permissions or fix broken issue/PR reference.".to_string(),
            });
        }

        // 4. Duplicated Work (Similar open issues across same project without duplicate relation)
        for i in 0..all_objects.len() {
            let a = &all_objects[i];
            if a.r#ref.r#type != "issue" && a.r#ref.r#type != "bug" {
                continue;
            }
            for b in all_objects.iter().skip(i + 1) {
                if b.r#ref.r#type != "issue" && b.r#ref.r#type != "bug" {
                    continue;
                }
                if a.container == b.container && !a.container.is_empty() {
                    let sim = lexical_similarity(&a.title, &b.title);
                    if sim > 0.85 {
                        let a_k = a.r#ref.to_key();
                        let b_k = b.r#ref.to_key();
                        items.push(GapItem {
                            id: format!("gap-dup-work-{}", items.len() + 1),
                            category: "duplicated_work".to_string(),
                            severity: "Medium".to_string(),
                            title: format!("Potential duplicated work: '{}' and '{}'", a.r#ref.id, b.r#ref.id),
                            description: format!(
                                "Issues {} ('{}') and {} ('{}') share high title similarity ({:.0}%).",
                                a_k, a.title, b_k, b.title, sim * 100.0
                            ),
                            affected_keys: vec![a_k, b_k],
                            recommendation: "Review whether these tickets duplicate effort and link them accordingly.".to_string(),
                        });
                    }
                }
            }
        }

        let summary = format!(
            "Gap analysis detected {} governance & engineering gap(s) across the indexed mesh.",
            items.len()
        );

        Ok(GapAnalysisReport {
            total_gaps: items.len(),
            items,
            summary,
        })
    }
}

fn lexical_similarity(s1: &str, s2: &str) -> f64 {
    let w1: HashSet<&str> = s1
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    let w2: HashSet<&str> = s2
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();

    if w1.is_empty() || w2.is_empty() {
        return 0.0;
    }

    let intersection = w1.intersection(&w2).count();
    let union = w1.union(&w2).count();

    intersection as f64 / union as f64
}
