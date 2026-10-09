use anyhow::Result;
use sha2::{Digest, Sha256};

use super::types::GeneratedQuestion;
use crate::config::Config;
use crate::store::Store;

pub fn compute_fingerprint(text: &str) -> String {
    let normalized: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let words: Vec<&str> = normalized.split_whitespace().collect();
    let clean = words.join(" ");
    let mut hasher = Sha256::new();
    hasher.update(clean.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub struct QuestionGenerator;

impl QuestionGenerator {
    pub async fn generate(store: &Store, config: &Config) -> Result<GeneratedQuestion> {
        let mut candidates = Vec::new();

        // 1. Check unresolved references
        if let Ok(unresolved) = store.get_introspection_unresolved_refs(10).await {
            for (target_id, discovered_from, source) in unresolved {
                let q = format!(
                    "Is {} referenced in {} still unresolved, and what blocks its resolution?",
                    target_id, discovered_from
                );
                let reason = format!(
                    "Unresolved {} reference {} discovered from {}",
                    source, target_id, discovered_from
                );
                candidates.push(GeneratedQuestion {
                    fingerprint: compute_fingerprint(&q),
                    question: q,
                    trigger_reason: reason,
                    candidate_keys: vec![target_id, discovered_from],
                });
            }
        }

        // 2. Check low-confidence relations (< 0.85)
        if let Ok(low_conf) = store.get_introspection_low_confidence_relations(10).await {
            for (from_k, to_k, rel_type, conf) in low_conf {
                let q = format!(
                    "Does {} actually {} {}, or is this relationship ungrounded?",
                    from_k, rel_type, to_k
                );
                let reason = format!(
                    "Low-confidence relation ({:.2}) '{}' between {} and {}",
                    conf, rel_type, from_k, to_k
                );
                candidates.push(GeneratedQuestion {
                    fingerprint: compute_fingerprint(&q),
                    question: q,
                    trigger_reason: reason,
                    candidate_keys: vec![from_k, to_k],
                });
            }
        }

        // 3. Check recently changed objects
        if let Ok(recent) = store.get_introspection_recent_objects(10).await {
            for (key, title, source, container, state) in recent {
                // Variation A: Blocked dependencies
                let q_blocked = format!("Is {} still blocked by the objects linked to it?", key);
                candidates.push(GeneratedQuestion {
                    fingerprint: compute_fingerprint(&q_blocked),
                    question: q_blocked,
                    trigger_reason: format!(
                        "Recently active {} object {} ({}) in {}",
                        source, key, state, container
                    ),
                    candidate_keys: vec![key.clone()],
                });

                // Variation B: Cross-tracker status consistency
                let q_status = format!(
                    "What recent changes or decisions occurred in {} ({}), and are its dependencies resolved?",
                    key, title
                );
                candidates.push(GeneratedQuestion {
                    fingerprint: compute_fingerprint(&q_status),
                    question: q_status,
                    trigger_reason: format!(
                        "Status check for recently updated {} object {}",
                        source, key
                    ),
                    candidate_keys: vec![key],
                });
            }
        }

        // 4. Follow-up on previous WEAK or FAIL runs
        if let Ok(past_fails) = store.get_introspection_weak_or_fail_runs(5).await {
            for (prev_q, result, problem_areas) in past_fails {
                let q = format!("Re-evaluating previous weakness: {}", prev_q);
                let reason = format!(
                    "Follow-up verification of past {} introspection run ({})",
                    result, problem_areas
                );
                candidates.push(GeneratedQuestion {
                    fingerprint: compute_fingerprint(&q),
                    question: q,
                    trigger_reason: reason,
                    candidate_keys: vec![],
                });
            }
        }

        // 5. Frequently searched engineering topics
        if let Ok(searches) = store.get_introspection_recent_searches(5).await {
            for query in searches {
                if !query.starts_with("Re-evaluating")
                    && !query.starts_with("What is the status of active")
                {
                    let q = format!("What is the current engineering consensus and cross-tracker status of: {}?", query);
                    let reason = "Re-validating frequently queried engineering topic".to_string();
                    candidates.push(GeneratedQuestion {
                        fingerprint: compute_fingerprint(&q),
                        question: q,
                        trigger_reason: reason,
                        candidate_keys: vec![],
                    });
                }
            }
        }

        // 6. Seed questions tailored to configured sources
        let mut seeds = Vec::new();
        if config.github.enabled && config.jira.enabled {
            seeds.push((
                "Which open Jira issues have linked GitHub pull requests, and are their states consistent?",
                "Periodic verification of cross-tracker Jira/GitHub linkages",
            ));
        }
        if config.launchpad.enabled && config.github.enabled {
            seeds.push((
                "This Launchpad bug says Fix Released. Is there implementation evidence in GitHub?",
                "Audit Launchpad 'Fix Released' status against GitHub implementation evidence",
            ));
        }
        if config.jira.enabled {
            seeds.push((
                "Are there unresolved blocker dependencies for open high-priority issues in Jira?",
                "Probing Jira blocker relations for active issues",
            ));
        }
        if config.github.enabled {
            seeds.push((
                "Which pull requests recently changed state, and are all referenced issues updated?",
                "Audit GitHub pull request states against referenced issues",
            ));
        }
        if config.launchpad.enabled {
            seeds.push((
                "What is the status of high-priority bugs recently reported in Launchpad?",
                "Exploratory check for recently active Launchpad bugs",
            ));
        }

        // Default universal seed if none match
        seeds.push((
            "What is the status of active issues across configured engineering trackers?",
            "Baseline exploration of configured tracking sources",
        ));

        for (sq, sreason) in seeds {
            candidates.push(GeneratedQuestion {
                fingerprint: compute_fingerprint(sq),
                question: sq.to_string(),
                trigger_reason: sreason.to_string(),
                candidate_keys: vec![],
            });
        }

        // Filter against recent runs in the last 7 days to avoid repetitive questions
        for cand in candidates {
            if let Ok(recent) = store
                .has_question_fingerprint_recently(&cand.fingerprint, 7)
                .await
            {
                if !recent {
                    return Ok(cand);
                }
            } else {
                return Ok(cand);
            }
        }

        // If all candidates were asked within the last 7 days, generate a timestamped variant of the primary seed
        let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let fallback_q = format!(
            "What is the current status and dependency state across configured trackers as of {}?",
            now
        );
        let fallback_reason = "Daily recurring baseline assessment of tracker health".to_string();
        let fp = compute_fingerprint(&fallback_q);
        Ok(GeneratedQuestion {
            question: fallback_q,
            fingerprint: fp,
            trigger_reason: fallback_reason,
            candidate_keys: vec![],
        })
    }
}
