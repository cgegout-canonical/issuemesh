use crate::connectors::Evidence;
use crate::store::RevisionRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub old_val: String,
    pub new_val: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectChange {
    pub object_key: String,
    pub title: String,
    pub change_type: String, // "added", "removed", "modified", "state_change"
    pub field_diffs: Vec<FieldDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionDiffItem {
    pub revision_id: i64,
    pub captured_at: String,
    pub state_from: Option<String>,
    pub state_to: String,
    pub title_changed: bool,
    pub content_changed: bool,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceSnapshotItem {
    pub object_key: String,
    pub source: String,
    pub object_type: String,
    pub external_id: String,
    pub title: String,
    pub state: String,
    pub content_hash: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceDiffReport {
    pub total_before: usize,
    pub total_after: usize,
    pub added: Vec<EvidenceSnapshotItem>,
    pub removed: Vec<EvidenceSnapshotItem>,
    pub modified: Vec<ObjectChange>,
    pub state_changes: Vec<ObjectChange>,
    pub summary: String,
}

/// Diff historical revisions of a single object
pub fn diff_revisions(revisions: &[RevisionRecord]) -> Vec<RevisionDiffItem> {
    let mut diffs = Vec::new();
    for i in 0..revisions.len() {
        let current = &revisions[i];
        if i == 0 {
            diffs.push(RevisionDiffItem {
                revision_id: current.id,
                captured_at: current.captured_at.clone(),
                state_from: None,
                state_to: current.state.clone(),
                title_changed: false,
                content_changed: false,
                details: format!("Initial snapshot recorded with state '{}'", current.state),
            });
        } else {
            let prev = &revisions[i - 1];
            let mut changes = Vec::new();
            let state_changed = prev.state != current.state;
            let title_changed = prev.title != current.title;
            let content_changed = prev.content_hash != current.content_hash;

            if state_changed {
                changes.push(format!(
                    "State changed from '{}' to '{}'",
                    prev.state, current.state
                ));
            }
            if title_changed {
                changes.push(format!("Title updated to '{}'", current.title));
            }
            if content_changed && !title_changed && !state_changed {
                changes.push("Content or description updated".to_string());
            }

            diffs.push(RevisionDiffItem {
                revision_id: current.id,
                captured_at: current.captured_at.clone(),
                state_from: Some(prev.state.clone()),
                state_to: current.state.clone(),
                title_changed,
                content_changed,
                details: if changes.is_empty() {
                    "Metadata snapshot refreshed".to_string()
                } else {
                    changes.join("; ")
                },
            });
        }
    }
    diffs
}

/// Diff a previous evidence snapshot against current live evidence items
pub fn diff_evidence_snapshots(
    previous: &[EvidenceSnapshotItem],
    current: &[Evidence],
) -> EvidenceDiffReport {
    let mut prev_map: HashMap<String, &EvidenceSnapshotItem> = HashMap::new();
    for p in previous {
        prev_map.insert(p.object_key.clone(), p);
    }

    let mut curr_map: HashMap<String, &Evidence> = HashMap::new();
    for c in current {
        curr_map.insert(c.r#ref.to_key(), c);
    }

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    let mut state_changes = Vec::new();

    // Check additions and modifications
    for (k, c) in &curr_map {
        let snap_item = EvidenceSnapshotItem {
            object_key: k.clone(),
            source: c.r#ref.source.clone(),
            object_type: c.r#ref.r#type.clone(),
            external_id: c.r#ref.id.clone(),
            title: c.title.clone(),
            state: c.state.clone(),
            content_hash: None,
            updated_at: c.updated_at.clone(),
        };

        if let Some(prev) = prev_map.get(k) {
            let mut field_diffs = Vec::new();
            if prev.state != c.state {
                field_diffs.push(FieldDiff {
                    field: "state".to_string(),
                    old_val: prev.state.clone(),
                    new_val: c.state.clone(),
                });
            }
            if prev.title != c.title {
                field_diffs.push(FieldDiff {
                    field: "title".to_string(),
                    old_val: prev.title.clone(),
                    new_val: c.title.clone(),
                });
            }

            if !field_diffs.is_empty() {
                let change = ObjectChange {
                    object_key: k.clone(),
                    title: c.title.clone(),
                    change_type: if prev.state != c.state {
                        "state_change".to_string()
                    } else {
                        "modified".to_string()
                    },
                    field_diffs,
                };
                if prev.state != c.state {
                    state_changes.push(change.clone());
                }
                modified.push(change);
            }
        } else {
            added.push(snap_item);
        }
    }

    // Check removals
    for (k, prev) in &prev_map {
        if !curr_map.contains_key(k) {
            removed.push((*prev).clone());
        }
    }

    let summary = format!(
        "Diff complete: {} added, {} removed, {} modified ({} state transitions) across {} objects.",
        added.len(),
        removed.len(),
        modified.len(),
        state_changes.len(),
        current.len()
    );

    EvidenceDiffReport {
        total_before: previous.len(),
        total_after: current.len(),
        added,
        removed,
        modified,
        state_changes,
        summary,
    }
}
