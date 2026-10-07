use serde::{Deserialize, Serialize};

use crate::connectors::{Evidence, ObjectRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyFinding {
    pub finding_type: String,
    pub severity: Severity,
    pub objects: Vec<ObjectRef>,
    pub explanation: String,
}

/// Run consistency checks across a set of evidence objects and their relationships
/// Implements deterministic checks per SPEC.md §33:
/// 1. Jira issue = Done while linked GitHub implementation PR = Open
/// 2. Launchpad bug = Fix Released while linked GitHub PR = not merged (open/closed without merge)
/// 3. Jira ticket says blocked by X, but X is closed/resolved
/// 4. GitHub PR says "Fixes OPEN-123" but OPEN-123 has no reverse/native relation (missing tracker sync)
/// 5. Jira fix version claims release complete while related implementation objects remain open
pub fn analyze_consistency(
    evidence_list: &[Evidence],
    relations: &[crate::connectors::Relation],
) -> Vec<ConsistencyFinding> {
    let mut findings = Vec::new();

    // Map evidence by object key for quick lookup
    let mut ev_map = std::collections::HashMap::new();
    for ev in evidence_list {
        ev_map.insert(ev.r#ref.to_key(), ev);
    }

    // 1. Jira issue = Done & GitHub PR = Open
    // 2. Launchpad bug = Fix Released & GitHub PR = Open (not merged)
    for rel in relations {
        let from_key = rel.from.to_key();
        let to_key = rel.to.to_key();

        if let (Some(from_ev), Some(to_ev)) = (ev_map.get(&from_key), ev_map.get(&to_key)) {
            // Check Jira <-> GitHub PR
            check_jira_github_status(from_ev, to_ev, &mut findings);
            check_jira_github_status(to_ev, from_ev, &mut findings);

            // Check Launchpad <-> GitHub PR
            check_lp_github_status(from_ev, to_ev, &mut findings);
            check_lp_github_status(to_ev, from_ev, &mut findings);

            // Check Blocked by closed object
            check_blocked_by_status(from_ev, to_ev, rel, &mut findings);
        }
    }

    // 4. GitHub PR says "Fixes X" but tracker X has no reverse/native relation
    for ev in evidence_list {
        if ev.r#ref.source == "github" && ev.r#ref.r#type == "pr" {
            let body = ev.body.as_deref().unwrap_or("");
            let extracted = crate::util::extract_references(body, Some(&ev.r#ref));
            for target_ref in extracted {
                // If PR claims to fix target_ref, check if there's a confirmed relation
                let has_rel = relations.iter().any(|r| {
                    (r.from == ev.r#ref && r.to == target_ref)
                        || (r.to == ev.r#ref && r.from == target_ref)
                });
                if !has_rel {
                    findings.push(ConsistencyFinding {
                        finding_type: "unreciprocated_fix_reference".to_string(),
                        severity: Severity::Warning,
                        objects: vec![ev.r#ref.clone(), target_ref.clone()],
                        explanation: format!(
                            "GitHub PR {} references {}, but no reciprocal relation was confirmed in the tracker.",
                            ev.r#ref.to_key(),
                            target_ref.to_key()
                        ),
                    });
                }
            }
        }
    }

    // 5. Fix version release complete while implementation objects remain open
    for ev in evidence_list {
        if ev.r#ref.source == "jira" {
            let state_lower = ev.state.to_lowercase();
            let is_released = state_lower.contains("released")
                || ev
                    .native
                    .get("fixVersions")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter().any(|ver| {
                            ver.get("released")
                                .and_then(|r| r.as_bool())
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);

            if is_released {
                // Check if any linked implementation objects are still open
                for rel in relations {
                    let other_ref = if rel.from == ev.r#ref {
                        Some(&rel.to)
                    } else if rel.to == ev.r#ref {
                        Some(&rel.from)
                    } else {
                        None
                    };

                    if let Some(target) = other_ref {
                        if let Some(target_ev) = ev_map.get(&target.to_key()) {
                            let target_state = target_ev.state.to_lowercase();
                            if target_state == "open" || target_state == "in progress" {
                                findings.push(ConsistencyFinding {
                                    finding_type: "release_version_with_open_items".to_string(),
                                    severity: Severity::Error,
                                    objects: vec![ev.r#ref.clone(), target_ev.r#ref.clone()],
                                    explanation: format!(
                                        "Jira issue {} indicates release complete, but related object {} remains in '{}' state.",
                                        ev.r#ref.to_key(),
                                        target_ev.r#ref.to_key(),
                                        target_ev.state
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    findings
}

fn check_jira_github_status(
    jira_ev: &Evidence,
    gh_ev: &Evidence,
    findings: &mut Vec<ConsistencyFinding>,
) {
    if jira_ev.r#ref.source == "jira"
        && gh_ev.r#ref.source == "github"
        && gh_ev.r#ref.r#type == "pr"
    {
        let jira_done = is_done_state(&jira_ev.state);
        let gh_open = gh_ev.state.to_lowercase() == "open";

        if jira_done && gh_open {
            findings.push(ConsistencyFinding {
                finding_type: "jira_done_pr_open".to_string(),
                severity: Severity::Error,
                objects: vec![jira_ev.r#ref.clone(), gh_ev.r#ref.clone()],
                explanation: format!(
                    "Jira issue {} is marked as '{}', but linked GitHub PR {} is still '{}'.",
                    jira_ev.r#ref.to_key(),
                    jira_ev.state,
                    gh_ev.r#ref.to_key(),
                    gh_ev.state
                ),
            });
        }
    }
}

fn check_lp_github_status(
    lp_ev: &Evidence,
    gh_ev: &Evidence,
    findings: &mut Vec<ConsistencyFinding>,
) {
    if lp_ev.r#ref.source == "launchpad"
        && gh_ev.r#ref.source == "github"
        && gh_ev.r#ref.r#type == "pr"
    {
        let lp_fix_released = lp_ev.state.to_lowercase().contains("fix released");
        let gh_merged = gh_ev.state.to_lowercase() == "merged"
            || gh_ev
                .native
                .get("merged")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

        if lp_fix_released && !gh_merged {
            findings.push(ConsistencyFinding {
                finding_type: "launchpad_released_pr_unmerged".to_string(),
                severity: Severity::Error,
                objects: vec![lp_ev.r#ref.clone(), gh_ev.r#ref.clone()],
                explanation: format!(
                    "Launchpad bug {} is marked as 'Fix Released', but linked GitHub PR {} is not merged (state: '{}').",
                    lp_ev.r#ref.to_key(),
                    gh_ev.r#ref.to_key(),
                    gh_ev.state
                ),
            });
        }
    }
}

fn check_blocked_by_status(
    from_ev: &Evidence,
    to_ev: &Evidence,
    rel: &crate::connectors::Relation,
    findings: &mut Vec<ConsistencyFinding>,
) {
    if rel.relation_type.to_lowercase().contains("block") {
        // "from" is blocked by "to"
        if is_done_state(&to_ev.state) && !is_done_state(&from_ev.state) {
            findings.push(ConsistencyFinding {
                finding_type: "blocked_by_resolved_item".to_string(),
                severity: Severity::Info,
                objects: vec![from_ev.r#ref.clone(), to_ev.r#ref.clone()],
                explanation: format!(
                    "Issue {} has a blocking dependency on {}, but {} is already resolved (state: '{}').",
                    from_ev.r#ref.to_key(),
                    to_ev.r#ref.to_key(),
                    to_ev.r#ref.to_key(),
                    to_ev.state
                ),
            });
        }
    }
}

fn is_done_state(state: &str) -> bool {
    let s = state.to_lowercase();
    s == "done" || s == "closed" || s == "resolved" || s == "completed" || s == "fix released"
}
