use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

use crate::connectors::ObjectRef;
use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactedItem {
    pub object_key: String,
    pub title: String,
    pub source: String,
    pub object_type: String,
    pub state: String,
    pub depth: usize,
    pub impact_type: String, // "direct_blocker", "transitive_blocker", "dependency", "related"
    pub evidence_path: Vec<String>,
    pub explanation: String,
    pub severity: String, // "Critical", "High", "Medium", "Low"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysisReport {
    pub target_key: String,
    pub target_title: String,
    pub target_state: String,
    pub overall_severity: String, // "Critical", "High", "Medium", "Low"
    pub total_impacted: usize,
    pub direct_impacts: usize,
    pub transitive_impacts: usize,
    pub items: Vec<ImpactedItem>,
    pub critical_paths: Vec<Vec<String>>,
    pub summary: String,
}

pub struct ImpactAnalyzer {
    store: Store,
}

impl ImpactAnalyzer {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn analyze_impact(&self, target_key: &str) -> Result<ImpactAnalysisReport> {
        let root_ref = ObjectRef::from_key(target_key)
            .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", target_key));

        let root_evidence = self.store.get_object(&root_ref).await?;
        let target_title = root_evidence
            .as_ref()
            .map(|e| e.title.clone())
            .unwrap_or_else(|| target_key.to_string());
        let target_state = root_evidence
            .as_ref()
            .map(|e| e.state.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        // Breadth-first traversal down impact relations
        let mut visited = HashSet::new();
        visited.insert(target_key.to_string());

        // Queue: (current_key, depth, path)
        let mut queue: VecDeque<(String, usize, Vec<String>)> = VecDeque::new();
        queue.push_back((target_key.to_string(), 0, vec![target_key.to_string()]));

        let mut impacted_items = Vec::new();
        let mut critical_paths = Vec::new();
        let max_depth = 4;

        while let Some((curr_key, depth, path)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            let relations = self.store.get_relations_for_key(&curr_key).await?;
            for rel in relations {
                let from_k = rel.from.to_key();
                let to_k = rel.to.to_key();
                let rel_type = rel.relation_type.to_lowercase();

                // Determine if this relation propagates downstream impact from curr_key
                let (next_key, next_ref, _impact_kind, rel_desc) = if from_k == curr_key {
                    // Outgoing relation from curr_key
                    if rel_type.contains("block") {
                        (to_k.clone(), rel.to.clone(), "blocked_by_target", "blocks")
                    } else if rel_type.contains("cause") {
                        (to_k.clone(), rel.to.clone(), "caused_by_target", "causes")
                    } else if rel_type.contains("fix") || rel_type.contains("implement") {
                        (to_k.clone(), rel.to.clone(), "implements_target", "fixes")
                    } else if rel_type.contains("depend") {
                        (
                            to_k.clone(),
                            rel.to.clone(),
                            "dependency_of_target",
                            "depends_on",
                        )
                    } else {
                        (
                            to_k.clone(),
                            rel.to.clone(),
                            "related_to_target",
                            "relates_to",
                        )
                    }
                } else {
                    // Incoming relation to curr_key
                    if rel_type.contains("depend") {
                        // from_k depends on curr_key -> from_k is impacted!
                        (
                            from_k.clone(),
                            rel.from.clone(),
                            "depends_on_target",
                            "depended_on_by",
                        )
                    } else if rel_type.contains("fix") || rel_type.contains("implement") {
                        // from_k fixes curr_key (e.g. PR fixes issue)
                        (from_k.clone(), rel.from.clone(), "fixes_target", "fixed_by")
                    } else {
                        (
                            from_k.clone(),
                            rel.from.clone(),
                            "related_to_target",
                            "related_from",
                        )
                    }
                };

                if visited.contains(&next_key) {
                    continue;
                }
                visited.insert(next_key.clone());

                let mut new_path = path.clone();
                new_path.push(format!("--[{}]-->", rel_desc));
                new_path.push(next_key.clone());

                let next_evidence = self.store.get_object(&next_ref).await?;
                let next_title = next_evidence
                    .as_ref()
                    .map(|e| e.title.clone())
                    .unwrap_or_else(|| next_key.clone());
                let next_state = next_evidence
                    .as_ref()
                    .map(|e| e.state.clone())
                    .unwrap_or_else(|| "Unknown".to_string());

                let is_direct = depth == 0;
                let severity = if rel_desc == "blocks" || rel_desc == "depended_on_by" {
                    if is_direct {
                        "Critical".to_string()
                    } else {
                        "High".to_string()
                    }
                } else if rel_desc == "fixes" || rel_desc == "causes" {
                    "High".to_string()
                } else if depth <= 1 {
                    "Medium".to_string()
                } else {
                    "Low".to_string()
                };

                let explanation = format!(
                    "{} via {} at depth {} from {}",
                    if is_direct {
                        "Directly impacted"
                    } else {
                        "Transitively impacted"
                    },
                    rel_desc,
                    depth + 1,
                    curr_key
                );

                if severity == "Critical" || severity == "High" {
                    critical_paths.push(new_path.clone());
                }

                impacted_items.push(ImpactedItem {
                    object_key: next_key.clone(),
                    title: next_title,
                    source: next_ref.source,
                    object_type: next_ref.r#type,
                    state: next_state,
                    depth: depth + 1,
                    impact_type: if is_direct {
                        "direct".to_string()
                    } else {
                        "transitive".to_string()
                    },
                    evidence_path: new_path.clone(),
                    explanation,
                    severity,
                });

                queue.push_back((next_key, depth + 1, new_path));
            }
        }

        let direct_impacts = impacted_items.iter().filter(|i| i.depth == 1).count();
        let transitive_impacts = impacted_items.iter().filter(|i| i.depth > 1).count();

        let overall_severity = if impacted_items.iter().any(|i| i.severity == "Critical") {
            "Critical".to_string()
        } else if impacted_items.iter().any(|i| i.severity == "High") || direct_impacts >= 3 {
            "High".to_string()
        } else if !impacted_items.is_empty() {
            "Medium".to_string()
        } else {
            "Low".to_string()
        };

        let summary = format!(
            "Impact analysis for '{}' ({}) identified {} potentially affected object(s) ({} direct, {} transitive). Overall blast radius risk: {}.",
            target_key, target_title, impacted_items.len(), direct_impacts, transitive_impacts, overall_severity
        );

        Ok(ImpactAnalysisReport {
            target_key: target_key.to_string(),
            target_title,
            target_state,
            overall_severity,
            total_impacted: impacted_items.len(),
            direct_impacts,
            transitive_impacts,
            items: impacted_items,
            critical_paths,
            summary,
        })
    }
}
