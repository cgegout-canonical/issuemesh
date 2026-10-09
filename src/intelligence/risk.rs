use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::connectors::Evidence;
use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskItem {
    pub id: String,
    pub category: String, // "stale_blocker", "critical_inactivity", "merged_pr_unresolved_jira", "contradictory_state", "missing_owner", "deep_dependency_chain"
    pub severity: String, // "Critical", "High", "Medium", "Low"
    pub title: String,
    pub description: String,
    pub objects: Vec<String>,
    pub remediation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskRadarReport {
    pub total_risks: usize,
    pub critical_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub items: Vec<RiskItem>,
    pub summary: String,
}

pub struct RiskRadar {
    store: Store,
}

impl RiskRadar {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn scan(&self) -> Result<RiskRadarReport> {
        let all_objects = self.store.get_all_objects().await?;
        let all_relations = self.store.get_all_relations().await?;

        let mut object_map: HashMap<String, &Evidence> = HashMap::new();
        for obj in &all_objects {
            object_map.insert(obj.r#ref.to_key(), obj);
        }

        let now = Utc::now();
        let mut items = Vec::new();

        // 1. Stale Blockers: non-closed blocker issues with no update > 14 days
        for rel in &all_relations {
            if rel.relation_type.to_lowercase().contains("block") {
                let from_k = rel.from.to_key();
                let to_k = rel.to.to_key();
                if let Some(blocker) = object_map.get(&from_k) {
                    let state_lower = blocker.state.to_lowercase();
                    let is_closed = state_lower.contains("done")
                        || state_lower.contains("closed")
                        || state_lower.contains("fix released")
                        || state_lower.contains("resolved");

                    if !is_closed {
                        let days_idle = days_since(&blocker.updated_at, now);
                        if days_idle >= 14 {
                            items.push(RiskItem {
                                id: format!("risk-stale-blocker-{}", items.len() + 1),
                                category: "stale_blocker".to_string(),
                                severity: "Critical".to_string(),
                                title: format!("Stale blocker '{}' idle for {} days", blocker.title, days_idle),
                                description: format!(
                                    "Issue '{}' ({}) is blocking '{}' but has not been updated in {} days while in state '{}'.",
                                    blocker.r#ref.to_key(), blocker.title, to_k, days_idle, blocker.state
                                ),
                                objects: vec![from_k.clone(), to_k.clone()],
                                remediation: format!(
                                    "Check if '{}' is abandoned or if its blocker status on '{}' should be removed.",
                                    from_k, to_k
                                ),
                            });
                        }
                    }
                }
            }
        }

        // 2. Critical Issues Without Activity: critical/cve/p0 with no update > 7 days
        for obj in &all_objects {
            let state_lower = obj.state.to_lowercase();
            let is_closed = state_lower.contains("done")
                || state_lower.contains("closed")
                || state_lower.contains("fix released")
                || state_lower.contains("resolved");

            if !is_closed {
                let title_lower = obj.title.to_lowercase();
                let is_critical = title_lower.contains("critical")
                    || title_lower.contains("cve")
                    || title_lower.contains("p0")
                    || title_lower.contains("p1")
                    || title_lower.contains("blocker")
                    || title_lower.contains("security");

                if is_critical {
                    let days_idle = days_since(&obj.updated_at, now);
                    if days_idle >= 7 {
                        items.push(RiskItem {
                            id: format!("risk-critical-idle-{}", items.len() + 1),
                            category: "critical_inactivity".to_string(),
                            severity: "High".to_string(),
                            title: format!("Critical issue idle: {}", obj.title),
                            description: format!(
                                "High-severity item '{}' ({}) in state '{}' has had no activity for {} days.",
                                obj.r#ref.to_key(), obj.title, obj.state, days_idle
                            ),
                            objects: vec![obj.r#ref.to_key()],
                            remediation: format!(
                                "Triage '{}' immediately to verify assignment and active progress.",
                                obj.r#ref.to_key()
                            ),
                        });
                    }
                }
            }
        }

        // 3. Merged PRs with Unresolved Jira Work
        // 4. Contradictory Tracker States
        for rel in &all_relations {
            let from_k = rel.from.to_key();
            let to_k = rel.to.to_key();
            if let (Some(a), Some(b)) = (object_map.get(&from_k), object_map.get(&to_k)) {
                let a_source = a.r#ref.source.as_str();
                let b_source = b.r#ref.source.as_str();

                let (pr_opt, jira_opt) = if a_source == "github" && b_source == "jira" {
                    (Some(*a), Some(*b))
                } else if a_source == "jira" && b_source == "github" {
                    (Some(*b), Some(*a))
                } else {
                    (None, None)
                };

                if let (Some(pr), Some(jira)) = (pr_opt, jira_opt) {
                    let pr_state = pr.state.to_lowercase();
                    let jira_state = jira.state.to_lowercase();

                    let pr_merged = pr_state.contains("merged") || pr_state.contains("closed");
                    let jira_open = jira_state.contains("open")
                        || jira_state.contains("in progress")
                        || jira_state.contains("to do")
                        || jira_state.contains("reopened");

                    if pr_merged && jira_open {
                        items.push(RiskItem {
                            id: format!("risk-merged-pr-open-jira-{}", items.len() + 1),
                            category: "merged_pr_unresolved_jira".to_string(),
                            severity: "High".to_string(),
                            title: format!("Merged PR '{}' with unresolved Jira '{}'", pr.r#ref.id, jira.r#ref.id),
                            description: format!(
                                "GitHub PR {} is '{}', but linked Jira ticket {} is still '{}'. Code is integrated but ticket is not updated.",
                                pr.r#ref.to_key(), pr.state, jira.r#ref.to_key(), jira.state
                            ),
                            objects: vec![pr.r#ref.to_key(), jira.r#ref.to_key()],
                            remediation: format!(
                                "Verify if Jira {} should be moved to Done or QA review.",
                                jira.r#ref.to_key()
                            ),
                        });
                    }

                    // Contradiction: Jira is Done, but PR is Open
                    let jira_done = jira_state.contains("done")
                        || jira_state.contains("closed")
                        || jira_state.contains("resolved");
                    let pr_open = pr_state == "open";
                    if jira_done && pr_open {
                        items.push(RiskItem {
                            id: format!("risk-contradiction-{}", items.len() + 1),
                            category: "contradictory_state".to_string(),
                            severity: "Critical".to_string(),
                            title: format!("Contradictory state: Jira Done but PR Open ({})", jira.r#ref.id),
                            description: format!(
                                "Jira ticket {} is marked '{}', yet linked PR {} is still '{}'. Tracker statuses directly contradict.",
                                jira.r#ref.to_key(), jira.state, pr.r#ref.to_key(), pr.state
                            ),
                            objects: vec![jira.r#ref.to_key(), pr.r#ref.to_key()],
                            remediation: "Reconcile trackers: check whether PR was abandoned or merged under another ticket.".to_string(),
                        });
                    }
                }

                // Check Launchpad vs Jira contradictions
                let (lp_opt, jira2_opt) = if a_source == "launchpad" && b_source == "jira" {
                    (Some(*a), Some(*b))
                } else if a_source == "jira" && b_source == "launchpad" {
                    (Some(*b), Some(*a))
                } else {
                    (None, None)
                };

                if let (Some(lp), Some(jira)) = (lp_opt, jira2_opt) {
                    let lp_state = lp.state.to_lowercase();
                    let jira_state = jira.state.to_lowercase();
                    let lp_released =
                        lp_state.contains("fix released") || lp_state.contains("fix committed");
                    let jira_in_prog = jira_state.contains("in progress")
                        || jira_state.contains("open")
                        || jira_state.contains("to do");

                    if lp_released && jira_in_prog {
                        items.push(RiskItem {
                            id: format!("risk-lp-jira-contradiction-{}", items.len() + 1),
                            category: "contradictory_state".to_string(),
                            severity: "High".to_string(),
                            title: format!(
                                "Contradictory state: Launchpad {} vs Jira {}",
                                lp.state, jira.state
                            ),
                            description: format!(
                                "Launchpad bug {} says '{}', but Jira {} is still '{}'.",
                                lp.r#ref.to_key(),
                                lp.state,
                                jira.r#ref.to_key(),
                                jira.state
                            ),
                            objects: vec![lp.r#ref.to_key(), jira.r#ref.to_key()],
                            remediation: "Synchronize Launchpad bug resolution with Jira backlog."
                                .to_string(),
                        });
                    }
                }
            }
        }

        // 5. Missing Owners: unassigned open issues
        for obj in &all_objects {
            let state_lower = obj.state.to_lowercase();
            let is_open = state_lower.contains("in progress")
                || state_lower.contains("open")
                || state_lower.contains("triaged")
                || state_lower.contains("confirmed");

            if is_open {
                let author_missing = obj.author.trim().is_empty()
                    || obj.author.eq_ignore_ascii_case("unknown")
                    || obj.author.eq_ignore_ascii_case("unassigned");

                let assignee_missing = if let Some(assignee) = obj.native.get("assignee") {
                    assignee.is_null()
                        || assignee
                            .as_str()
                            .map(|s| s.trim().is_empty() || s == "unassigned")
                            .unwrap_or(false)
                } else {
                    true
                };

                if author_missing && assignee_missing {
                    items.push(RiskItem {
                        id: format!("risk-unowned-{}", items.len() + 1),
                        category: "missing_owner".to_string(),
                        severity: "Medium".to_string(),
                        title: format!("Unassigned active issue: {}", obj.title),
                        description: format!(
                            "Issue '{}' ({}) is actively '{}' but has no assigned engineer or identifiable owner.",
                            obj.r#ref.to_key(), obj.title, obj.state
                        ),
                        objects: vec![obj.r#ref.to_key()],
                        remediation: "Assign an engineering owner to drive this issue to resolution.".to_string(),
                    });
                }
            }
        }

        // 6. Growing Dependency Chains (chains of depth >= 3)
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for rel in &all_relations {
            if rel.relation_type.to_lowercase().contains("block")
                || rel.relation_type.to_lowercase().contains("depend")
            {
                adj.entry(rel.from.to_key())
                    .or_default()
                    .push(rel.to.to_key());
            }
        }

        for (start_node, neighbors) in &adj {
            if neighbors.len() >= 4 {
                items.push(RiskItem {
                    id: format!("risk-fanout-{}", items.len() + 1),
                    category: "deep_dependency_chain".to_string(),
                    severity: "High".to_string(),
                    title: format!("High dependency fan-out on '{}'", start_node),
                    description: format!(
                        "Object '{}' has {} outbound blocker/dependency links. High risk of bottlenecks.",
                        start_node, neighbors.len()
                    ),
                    objects: vec![start_node.clone()],
                    remediation: "Decompose this epic/issue into independent, parallelizable deliverables.".to_string(),
                });
            }

            // Check depth 3 chain
            for n2 in neighbors {
                if let Some(n3_list) = adj.get(n2) {
                    for n3 in n3_list {
                        if n3 != start_node {
                            items.push(RiskItem {
                                id: format!("risk-deep-chain-{}", items.len() + 1),
                                category: "deep_dependency_chain".to_string(),
                                severity: "Medium".to_string(),
                                title: format!("Deep blocker chain: {} -> {} -> {}", start_node, n2, n3),
                                description: format!(
                                    "Sequential blocker chain identified of depth 3: '{}' blocks '{}' which blocks '{}'.",
                                    start_node, n2, n3
                                ),
                                objects: vec![start_node.clone(), n2.clone(), n3.clone()],
                                remediation: "Review whether intermediate dependencies can be unblocked or decoupled.".to_string(),
                            });
                            break;
                        }
                    }
                }
            }
        }

        // Deduplicate items with exact same category and objects
        let mut seen = HashSet::new();
        let mut deduped_items = Vec::new();
        for it in items {
            let key = format!("{}-{:?}", it.category, it.objects);
            if seen.insert(key) {
                deduped_items.push(it);
            }
        }

        let critical_count = deduped_items
            .iter()
            .filter(|i| i.severity == "Critical")
            .count();
        let high_count = deduped_items
            .iter()
            .filter(|i| i.severity == "High")
            .count();
        let medium_count = deduped_items
            .iter()
            .filter(|i| i.severity == "Medium")
            .count();
        let low_count = deduped_items.iter().filter(|i| i.severity == "Low").count();

        let summary = format!(
            "Engineering Risk Radar scanned {} objects and {} relations. Detected {} risk item(s): {} Critical, {} High, {} Medium, {} Low.",
            all_objects.len(), all_relations.len(), deduped_items.len(), critical_count, high_count, medium_count, low_count
        );

        Ok(RiskRadarReport {
            total_risks: deduped_items.len(),
            critical_count,
            high_count,
            medium_count,
            low_count,
            items: deduped_items,
            summary,
        })
    }
}

fn days_since(date_str_opt: &Option<String>, now: DateTime<Utc>) -> i64 {
    if let Some(s) = date_str_opt {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            let duration = now.signed_duration_since(dt.with_timezone(&Utc));
            return duration.num_days().max(0);
        }
    }
    // If no date available, treat as 0
    0
}
