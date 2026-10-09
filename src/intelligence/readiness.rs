use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::connectors::Evidence;
use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessAssessment {
    pub target: String,
    pub readiness_score: f64, // 0.0 to 100.0
    pub verdict: String,      // "READY", "AT_RISK", "NOT_READY"
    pub total_items: usize,
    pub blockers: Vec<String>,
    pub high_priority_issues: Vec<String>,
    pub implementation_percentage: f64,
    pub stale_work: Vec<String>,
    pub ownership_gaps: Vec<String>,
    pub inconsistencies: Vec<String>,
    pub summary: String,
}

pub struct ReadinessEvaluator {
    store: Store,
}

impl ReadinessEvaluator {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn assess_readiness(&self, target: &str) -> Result<ReadinessAssessment> {
        // Collect target objects
        let mut objects = self.store.get_objects_by_container(target).await?;
        if objects.is_empty() {
            // Try FTS search for topic/milestone
            let fts = self.store.search_fts(target, 50).await?;
            objects = fts.into_iter().map(|f| f.evidence).collect();
        }

        if objects.is_empty() {
            // Target not found or no items
            return Ok(ReadinessAssessment {
                target: target.to_string(),
                readiness_score: 0.0,
                verdict: "NOT_READY".to_string(),
                total_items: 0,
                blockers: Vec::new(),
                high_priority_issues: Vec::new(),
                implementation_percentage: 0.0,
                stale_work: Vec::new(),
                ownership_gaps: Vec::new(),
                inconsistencies: Vec::new(),
                summary: format!("No evidence objects found in mesh for target '{}'.", target),
            });
        }

        let mut object_map: HashMap<String, &Evidence> = HashMap::new();
        for obj in &objects {
            object_map.insert(obj.r#ref.to_key(), obj);
        }

        let now = Utc::now();
        let mut blockers = Vec::new();
        let mut high_priority_issues = Vec::new();
        let mut stale_work = Vec::new();
        let mut ownership_gaps = Vec::new();
        let mut inconsistencies = Vec::new();

        let mut total_requirements = 0;
        let mut implemented_requirements = 0;

        for obj in &objects {
            let key = obj.r#ref.to_key();
            let state_lower = obj.state.to_lowercase();
            let is_closed = state_lower.contains("done")
                || state_lower.contains("closed")
                || state_lower.contains("merged")
                || state_lower.contains("fix released");

            if obj.r#ref.r#type == "issue" || obj.r#ref.r#type == "bug" {
                total_requirements += 1;
                if is_closed {
                    implemented_requirements += 1;
                }
            }

            // Check high priority
            let title_lower = obj.title.to_lowercase();
            let is_critical = title_lower.contains("critical")
                || title_lower.contains("cve")
                || title_lower.contains("p0")
                || title_lower.contains("blocker");

            if is_critical && !is_closed {
                high_priority_issues.push(format!("{} ({}): {}", key, obj.state, obj.title));
            }

            // Check stale work
            if !is_closed {
                let days_idle = days_since(&obj.updated_at, now);
                if days_idle >= 14 {
                    stale_work.push(format!(
                        "{} idle for {} days in state '{}'",
                        key, days_idle, obj.state
                    ));
                }
            }

            // Check ownership gaps
            if !is_closed {
                let author_missing = obj.author.trim().is_empty()
                    || obj.author.eq_ignore_ascii_case("unknown")
                    || obj.author.eq_ignore_ascii_case("unassigned");
                if author_missing {
                    ownership_gaps.push(format!("{} ({}) has no assigned owner", key, obj.title));
                }
            }

            // Check blockers and inconsistencies via relations
            let relations = self.store.get_relations_for_key(&key).await?;
            for rel in relations {
                if rel.relation_type.to_lowercase().contains("block")
                    && rel.from.to_key() == key
                    && !is_closed
                {
                    blockers.push(format!(
                        "{} ({}) blocks {}",
                        key,
                        obj.state,
                        rel.to.to_key()
                    ));
                }

                // Inconsistency check
                let other_ref = if rel.from.to_key() == key {
                    &rel.to
                } else {
                    &rel.from
                };
                if let Some(other_ev) = self.store.get_object(other_ref).await? {
                    let s1 = obj.state.to_lowercase();
                    let s2 = other_ev.state.to_lowercase();
                    let c1 =
                        s1.contains("done") || s1.contains("merged") || s1.contains("fix released");
                    let o2 =
                        s2.contains("in progress") || s2.contains("open") || s2.contains("to do");
                    if c1 && o2 {
                        inconsistencies.push(format!(
                            "Contradiction: {} is '{}' but linked {} is '{}'",
                            key,
                            obj.state,
                            other_ref.to_key(),
                            other_ev.state
                        ));
                    }
                }
            }
        }

        blockers.dedup();
        inconsistencies.dedup();

        let impl_pct = if total_requirements > 0 {
            (implemented_requirements as f64 / total_requirements as f64) * 100.0
        } else {
            100.0
        };

        // Score formula
        let mut score: f64 = 100.0;
        score -= (blockers.len() as f64) * 20.0;
        score -= (high_priority_issues.len() as f64) * 10.0;
        score -= (inconsistencies.len() as f64) * 15.0;
        score -= (stale_work.len() as f64) * 5.0;
        score -= (ownership_gaps.len() as f64) * 5.0;

        // Factor in implementation percentage
        let weighted_score = (score * 0.7) + (impl_pct * 0.3);
        let final_score = weighted_score.clamp(0.0, 100.0);

        let verdict =
            if final_score >= 85.0 && blockers.is_empty() && high_priority_issues.is_empty() {
                "READY".to_string()
            } else if final_score >= 50.0 && blockers.len() <= 1 {
                "AT_RISK".to_string()
            } else {
                "NOT_READY".to_string()
            };

        let summary = format!(
            "Readiness for '{}': Score {:.1}/100 -> {}. Analyzed {} item(s). {} blocker(s), {} high-priority issue(s), {:.1}% implementation, {} stale item(s), {} ownership gap(s), {} inconsistency(s).",
            target, final_score, verdict, objects.len(), blockers.len(), high_priority_issues.len(), impl_pct, stale_work.len(), ownership_gaps.len(), inconsistencies.len()
        );

        Ok(ReadinessAssessment {
            target: target.to_string(),
            readiness_score: (final_score * 10.0).round() / 10.0,
            verdict,
            total_items: objects.len(),
            blockers,
            high_priority_issues,
            implementation_percentage: (impl_pct * 10.0).round() / 10.0,
            stale_work,
            ownership_gaps,
            inconsistencies,
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
    0
}
