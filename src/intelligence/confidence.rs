use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::connectors::{Evidence, Relation};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceReport {
    pub score: f64,
    pub level: String, // "High", "Medium", "Low", "Uncertain"
    pub freshness_score: f64,
    pub relation_fidelity_score: f64,
    pub corroboration_bonus: f64,
    pub contradiction_penalty: f64,
    pub unresolved_penalty: f64,
    pub source_health_penalty: f64,
    pub factors: Vec<String>,
}

/// Calculate objective confidence from evidence signals
pub fn calculate_confidence(
    evidence_items: &[Evidence],
    relations: &[Relation],
    unresolved_refs_count: usize,
    unavailable_sources_count: usize,
) -> ConfidenceReport {
    if evidence_items.is_empty() {
        return ConfidenceReport {
            score: 0.0,
            level: "Uncertain".to_string(),
            freshness_score: 0.0,
            relation_fidelity_score: 0.0,
            corroboration_bonus: 0.0,
            contradiction_penalty: 0.0,
            unresolved_penalty: 0.0,
            source_health_penalty: 0.0,
            factors: vec!["No evidence retrieved to support conclusions.".to_string()],
        };
    }

    let mut factors = Vec::new();
    let now = Utc::now();

    // 1. Evidence Freshness
    let mut total_age_days = 0i64;
    let mut dated_items = 0usize;

    for ev in evidence_items {
        if let Some(s) = &ev.updated_at {
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                let days = now
                    .signed_duration_since(dt.with_timezone(&Utc))
                    .num_days()
                    .max(0);
                total_age_days += days;
                dated_items += 1;
            }
        }
    }

    let avg_age_days = if dated_items > 0 {
        total_age_days / dated_items as i64
    } else {
        30
    };

    let freshness_score = if avg_age_days <= 7 {
        factors.push(format!(
            "Evidence is highly fresh (avg {} days old).",
            avg_age_days
        ));
        1.0
    } else if avg_age_days <= 30 {
        factors.push(format!(
            "Evidence is moderately fresh (avg {} days old).",
            avg_age_days
        ));
        0.85
    } else if avg_age_days <= 90 {
        factors.push(format!(
            "Evidence age is somewhat dated (avg {} days old).",
            avg_age_days
        ));
        0.70
    } else {
        factors.push(format!(
            "Evidence is stale (>90 days, avg {} days).",
            avg_age_days
        ));
        0.50
    };

    // 2. Native vs Inferred Relationships
    let relation_fidelity_score = if relations.is_empty() {
        if evidence_items.len() == 1 {
            factors.push(
                "Single primary canonical object retrieved without external relations.".to_string(),
            );
            0.85
        } else {
            factors
                .push("Multiple objects retrieved without explicit graph relations.".to_string());
            0.70
        }
    } else {
        let mut sum_conf = 0.0;
        let mut native_count = 0;
        for rel in relations {
            let conf = rel.confidence as f64;
            let detected = rel.detected_by.to_lowercase();
            if detected.contains("link") || detected.contains("native") || conf >= 0.90 {
                native_count += 1;
                sum_conf += 1.0;
            } else if detected.contains("regex") || conf >= 0.75 {
                sum_conf += 0.85;
            } else {
                sum_conf += 0.60;
            }
        }
        let avg_fid = sum_conf / relations.len() as f64;
        factors.push(format!(
            "Graph traversal includes {} relation(s) ({} native tracker links, avg fidelity {:.2}).",
            relations.len(), native_count, avg_fid
        ));
        avg_fid
    };

    // 3. Independent Corroborating Sources
    let mut sources = HashSet::new();
    for ev in evidence_items {
        sources.insert(ev.r#ref.source.clone());
    }

    let corroboration_bonus = if sources.len() >= 3 {
        factors.push(format!(
            "Corroborated across 3 independent trackers: {:?}.",
            sources
        ));
        0.15
    } else if sources.len() == 2 {
        factors.push(format!(
            "Corroborated across 2 independent trackers: {:?}.",
            sources
        ));
        0.10
    } else {
        factors.push("Evidence sourced from a single tracker system.".to_string());
        0.0
    };

    // 4. Contradiction Penalty
    let mut contradictions = 0;
    for i in 0..evidence_items.len() {
        let a = &evidence_items[i];
        let a_state = a.state.to_lowercase();
        let a_closed = a_state.contains("done")
            || a_state.contains("merged")
            || a_state.contains("fix released");

        for b in evidence_items.iter().skip(i + 1) {
            let b_state = b.state.to_lowercase();
            let b_open = b_state.contains("in progress")
                || b_state.contains("open")
                || b_state.contains("to do");

            // Check if they are connected
            let is_connected = relations.iter().any(|r| {
                (r.from.to_key() == a.r#ref.to_key() && r.to.to_key() == b.r#ref.to_key())
                    || (r.from.to_key() == b.r#ref.to_key() && r.to.to_key() == a.r#ref.to_key())
            });

            if is_connected && a_closed && b_open {
                contradictions += 1;
            }
        }
    }

    let contradiction_penalty = if contradictions > 0 {
        let pen = (contradictions as f64 * 0.25).min(0.50);
        factors.push(format!(
            "Penalty -{:.2}: {} contradiction(s) detected between linked tracker states.",
            pen, contradictions
        ));
        pen
    } else {
        0.0
    };

    // 5. Unresolved References Penalty
    let unresolved_penalty = if unresolved_refs_count > 0 {
        let pen = (unresolved_refs_count as f64 * 0.10).min(0.30);
        factors.push(format!(
            "Penalty -{:.2}: {} unresolved or private reference(s) in evidence chain.",
            pen, unresolved_refs_count
        ));
        pen
    } else {
        0.0
    };

    // 6. Source Health / Availability Penalty
    let source_health_penalty = if unavailable_sources_count > 0 {
        let pen = (unavailable_sources_count as f64 * 0.15).min(0.40);
        factors.push(format!(
            "Penalty -{:.2}: {} tracker source(s) were unavailable or timed out.",
            pen, unavailable_sources_count
        ));
        pen
    } else {
        0.0
    };

    // Weighted synthesis
    let raw_score = (0.40 * freshness_score)
        + (0.35 * relation_fidelity_score)
        + (0.15 * 0.90) // baseline consistency
        + corroboration_bonus
        - contradiction_penalty
        - unresolved_penalty
        - source_health_penalty;

    let clamped_score = raw_score.clamp(0.05, 0.98);
    let final_score = (clamped_score * 100.0).round() / 100.0;

    let level = if final_score >= 0.80 {
        "High".to_string()
    } else if final_score >= 0.60 {
        "Medium".to_string()
    } else if final_score >= 0.40 {
        "Low".to_string()
    } else {
        "Uncertain".to_string()
    };

    ConfidenceReport {
        score: final_score,
        level,
        freshness_score,
        relation_fidelity_score,
        corroboration_bonus,
        contradiction_penalty,
        unresolved_penalty,
        source_health_penalty,
        factors,
    }
}
