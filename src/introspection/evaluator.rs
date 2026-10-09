use regex::Regex;
use std::collections::HashSet;

use super::types::{EvaluationScores, GeneratedQuestion, IntrospectionResult};
use crate::search::orchestrator::DeepSearchResult;

pub struct AnswerEvaluator;

impl AnswerEvaluator {
    pub fn evaluate(
        question: &GeneratedQuestion,
        search_result: &DeepSearchResult,
    ) -> EvaluationScores {
        let answer_text = search_result.answer.as_deref().unwrap_or("").trim();
        let word_count = answer_text.split_whitespace().count();

        let mut problem_areas: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();

        // 1. Collect all known valid keys & IDs from retrieved evidence and followed relations
        let mut known_ids: HashSet<String> = HashSet::new();
        let mut known_keys: HashSet<String> = HashSet::new();

        for ranked in &search_result.ranked_results {
            let ev = &ranked.evidence;
            known_keys.insert(ev.r#ref.to_key().to_lowercase());
            known_ids.insert(ev.r#ref.id.to_lowercase());
            if let Some(pos) = ev.r#ref.id.find('#') {
                known_ids.insert(ev.r#ref.id[pos + 1..].to_string());
            }
        }

        for rel in &search_result.relations_followed {
            known_keys.insert(rel.from.to_key().to_lowercase());
            known_ids.insert(rel.from.id.to_lowercase());
            known_keys.insert(rel.to.to_key().to_lowercase());
            known_ids.insert(rel.to.id.to_lowercase());
        }

        let is_empty_evidence = search_result.ranked_results.is_empty();
        let states_insufficient_evidence = answer_text
            .to_lowercase()
            .contains("could not find sufficient evidence")
            || answer_text.to_lowercase().contains("insufficient evidence")
            || answer_text.to_lowercase().contains("no matching evidence");

        // -------------------------------------------------------------------
        // Hard Failure Checks:
        // 1. Hallucination check for tracker objects
        // -------------------------------------------------------------------
        let mut detected_hallucination = false;
        let mut hallucinated_items = Vec::new();

        // Regex for Jira keys: e.g. PROJ-1234
        if let Ok(jira_re) = Regex::new(r"\b([A-Z][A-Z0-9_]+-\d+)\b") {
            for cap in jira_re.captures_iter(answer_text) {
                let id = cap[1].to_lowercase();
                if !known_ids.contains(&id) && !known_keys.iter().any(|k| k.contains(&id)) {
                    // Check if it was in the question itself (prompt mentions are valid to cite)
                    if !question.question.to_lowercase().contains(&id) {
                        detected_hallucination = true;
                        hallucinated_items.push(cap[1].to_string());
                    }
                }
            }
        }

        // Regex for Launchpad bug keys: e.g. LP#12345 or LP12345
        if let Ok(lp_re) = Regex::new(r"\b(?:LP#?|bug\s+#?)(\d{6,8})\b") {
            for cap in lp_re.captures_iter(answer_text) {
                let num = cap[1].to_string();
                if !known_ids.contains(&num)
                    && !known_keys.iter().any(|k| k.contains(&num))
                    && !question.question.contains(&num)
                {
                    detected_hallucination = true;
                    hallucinated_items.push(format!("LP#{}", num));
                }
            }
        }

        // Regex for GitHub PR/issues: e.g. owner/repo#123 or repo#123
        if let Ok(gh_re) = Regex::new(r"\b([a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+#\d+)\b") {
            for cap in gh_re.captures_iter(answer_text) {
                let full = cap[1].to_lowercase();
                if !known_ids.contains(&full)
                    && !known_keys.iter().any(|k| k.contains(&full))
                    && !question.question.to_lowercase().contains(&full)
                {
                    detected_hallucination = true;
                    hallucinated_items.push(cap[1].to_string());
                }
            }
        }

        if detected_hallucination {
            notes.push(format!(
                "CRITICAL: Hallucinated tracker object(s) detected: {}",
                hallucinated_items.join(", ")
            ));
            problem_areas.push("hallucination".to_string());
        }

        // 2. Check for ignored critical contradictions
        let mut ignored_critical_contradiction = false;
        if !search_result.findings.is_empty() {
            let ans_lower = answer_text.to_lowercase();
            let mentions_discrepancy = ans_lower.contains("contradict")
                || ans_lower.contains("conflict")
                || ans_lower.contains("discrepanc")
                || ans_lower.contains("inconsisten")
                || ans_lower.contains("disagree")
                || ans_lower.contains("diverg")
                || ans_lower.contains("mismatch");

            if !mentions_discrepancy {
                ignored_critical_contradiction = true;
                notes.push("CRITICAL: Search discovered tracker contradictions, but the answer failed to highlight the discrepancy.".to_string());
                problem_areas.push("consistency analysis".to_string());
            }
        }

        // 3. Check for claiming definitive answer when evidence is insufficient
        let mut claimed_unsupported_definitive = false;
        if is_empty_evidence && !states_insufficient_evidence && !answer_text.is_empty() {
            claimed_unsupported_definitive = true;
            notes.push("CRITICAL: Stated definitive conclusion despite zero evidence retrieved from configured trackers.".to_string());
            problem_areas.push("retrieval".to_string());
            problem_areas.push("LLM synthesis".to_string());
        }

        // -------------------------------------------------------------------
        // Dimension Scoring (0 to 5)
        // -------------------------------------------------------------------

        // 1. Accuracy (35%)
        let accuracy: f64 = if detected_hallucination {
            0.0
        } else if ignored_critical_contradiction {
            1.0
        } else if claimed_unsupported_definitive {
            2.0
        } else if (states_insufficient_evidence && is_empty_evidence)
            || (!search_result.ranked_results.is_empty() && !answer_text.is_empty())
        {
            5.0 // Truthful acknowledgement of absent evidence or grounded answer is optimal behavior
        } else if answer_text.is_empty() {
            1.0
        } else {
            3.0
        };

        // 2. Relevance (25%)
        let relevance: f64 = if answer_text.is_empty() {
            0.0
        } else if states_insufficient_evidence {
            5.0
        } else {
            let mut rel = 4.0;
            // Check candidate target keys
            if !question.candidate_keys.is_empty() {
                let ans_lower = answer_text.to_lowercase();
                let matches_target = question.candidate_keys.iter().any(|k| {
                    let k_lower = k.to_lowercase();
                    if ans_lower.contains(&k_lower) {
                        return true;
                    }
                    if let Some(pos) = k.rfind(':') {
                        let id_part = k[pos + 1..].to_lowercase();
                        if !id_part.is_empty() && ans_lower.contains(&id_part) {
                            return true;
                        }
                    }
                    false
                });
                if matches_target {
                    rel = 5.0;
                } else if !is_empty_evidence {
                    rel = 3.0;
                    problem_areas.push("query planning".to_string());
                }
            } else {
                rel = 4.5;
            }
            rel
        };

        // 3. Coverage (15%)
        let mut coverage: f64 = if states_insufficient_evidence && is_empty_evidence {
            4.5
        } else if search_result.ranked_results.is_empty() {
            if !search_result.warnings.is_empty() {
                problem_areas.push("tracker connector".to_string());
            } else {
                problem_areas.push("retrieval".to_string());
            }
            2.0
        } else {
            let count = search_result.ranked_results.len();
            if count >= 3 {
                5.0
            } else if count >= 1 {
                4.0
            } else {
                3.0
            }
        };

        // Check if any connector threw warnings
        if !search_result.warnings.is_empty() {
            notes.push(format!(
                "Connector warnings encountered: {}",
                search_result.warnings.join("; ")
            ));
            if !problem_areas.contains(&"tracker connector".to_string()) {
                problem_areas.push("tracker connector".to_string());
            }
            coverage = (coverage - 0.5).max(1.0);
        }

        // 4. Traceability (15%)
        let traceability: f64 = if answer_text.is_empty() {
            0.0
        } else if states_insufficient_evidence {
            4.0
        } else {
            let has_identifiers = !known_ids.is_empty()
                && known_ids
                    .iter()
                    .any(|id| answer_text.to_lowercase().contains(id));
            let mentions_source = answer_text.contains("Jira")
                || answer_text.contains("GitHub")
                || answer_text.contains("Launchpad");

            if has_identifiers && mentions_source {
                5.0
            } else if has_identifiers || mentions_source {
                4.0
            } else {
                problem_areas.push("LLM synthesis".to_string());
                2.5
            }
        };

        // 5. Conciseness (10%)
        let conciseness: f64 = if answer_text.is_empty() {
            1.0
        } else if word_count <= 250 {
            5.0
        } else if word_count <= 350 {
            4.0
        } else if word_count <= 500 {
            problem_areas.push("answer formatting".to_string());
            3.0
        } else {
            problem_areas.push("answer formatting".to_string());
            2.0
        };

        // -------------------------------------------------------------------
        // Score Calculation & Hard Failure Overrides
        // -------------------------------------------------------------------
        let weighted_score = (accuracy * 0.35)
            + (relevance * 0.25)
            + (coverage * 0.15)
            + (traceability * 0.15)
            + (conciseness * 0.10);

        let percentage = (weighted_score / 5.0) * 100.0;

        // Interpretation
        let interpretation = if detected_hallucination || accuracy == 0.0 {
            "Unreliable"
        } else if weighted_score >= 4.50 {
            "Excellent"
        } else if weighted_score >= 4.00 {
            "Good"
        } else if weighted_score >= 3.00 {
            "Needs improvement"
        } else if weighted_score >= 2.00 {
            "Poor"
        } else {
            "Unreliable"
        };

        // Classification
        let result = if detected_hallucination
            || accuracy == 0.0
            || ignored_critical_contradiction
            || claimed_unsupported_definitive
            || weighted_score < 3.0
        {
            IntrospectionResult::FAIL
        } else if weighted_score >= 4.0
            && accuracy >= 4.0
            && relevance >= 4.0
            && traceability >= 3.0
        {
            IntrospectionResult::PASS
        } else {
            IntrospectionResult::WEAK
        };

        // De-duplicate problem areas
        problem_areas.sort();
        problem_areas.dedup();

        // Concrete diagnostic recommendation
        let recommendation = match result {
            IntrospectionResult::PASS => {
                "Evidence retrieval, relation discovery, and synthesized answer satisfy all accuracy and traceability criteria.".to_string()
            }
            IntrospectionResult::WEAK => {
                if problem_areas.contains(&"answer formatting".to_string()) {
                    "Streamline synthesis to prioritize latest decisions and maintain concise output under 250 words.".to_string()
                } else if problem_areas.contains(&"traceability".to_string()) {
                    "Strengthen citation linking so all assertions directly map to canonical tracker identifiers.".to_string()
                } else if problem_areas.contains(&"retrieval".to_string()) {
                    "Refine search query expansion to ensure candidate issues are discovered across all active trackers.".to_string()
                } else {
                    "Review retrieved candidate rank ordering and multi-source correlation depth.".to_string()
                }
            }
            IntrospectionResult::FAIL => {
                if problem_areas.contains(&"hallucination".to_string()) {
                    "Enforce strict grounded-extraction validation on LLM responses to eliminate ungrounded tracker keys.".to_string()
                } else if problem_areas.contains(&"consistency analysis".to_string()) {
                    "Ensure detected cross-tracker status discrepancies (e.g. Jira vs GitHub) are explicitly surfaced in synthesis.".to_string()
                } else if problem_areas.contains(&"tracker connector".to_string()) {
                    "Verify connector credentials, network timeouts, and API rate limits for failing tracker endpoints.".to_string()
                } else {
                    "Search pipeline failed to retrieve or ground adequate evidence for the research question.".to_string()
                }
            }
        };

        EvaluationScores {
            accuracy,
            relevance,
            coverage,
            traceability,
            conciseness,
            weighted_score,
            percentage,
            result: result.to_string(),
            interpretation: interpretation.to_string(),
            problem_areas,
            recommendation,
            details: serde_json::json!({
                "word_count": word_count,
                "notes": notes,
                "detected_hallucination": detected_hallucination,
                "ignored_critical_contradiction": ignored_critical_contradiction,
                "claimed_unsupported_definitive": claimed_unsupported_definitive,
                "states_insufficient_evidence": states_insufficient_evidence,
                "objects_consulted": search_result.stats.objects_analysed,
                "sources_queried": search_result.stats.trackers_searched,
                "duration_ms": search_result.stats.duration_ms,
            }),
        }
    }
}
