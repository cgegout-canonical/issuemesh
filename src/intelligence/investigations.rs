use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::connectors::ObjectRef;
use crate::intelligence::diff::EvidenceSnapshotItem;
use crate::store::{SavedInvestigationRecord, Store};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveInvestigationRequest {
    pub title: String,
    pub query: String,
    pub conclusions_markdown: String,
    pub evidence_keys: Vec<String>,
    pub tags: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvestigationDeltas {
    pub new_evidence: Vec<EvidenceSnapshotItem>,
    pub changed_evidence: Vec<EvidenceSnapshotItem>,
    pub resolved_evidence: Vec<EvidenceSnapshotItem>,
    pub contradictory_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReopenedInvestigationReport {
    pub investigation: SavedInvestigationRecord,
    pub reopened_at: String,
    pub deltas: InvestigationDeltas,
    pub updated_conclusions_guidance: String,
}

pub struct InvestigationManager {
    store: Store,
}

impl InvestigationManager {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn save(&self, req: SaveInvestigationRequest) -> Result<SavedInvestigationRecord> {
        let id = format!("inv-{}", uuid::Uuid::new_v4().simple());
        let now = Utc::now().to_rfc3339();

        let mut snapshot_items = Vec::new();
        for key in &req.evidence_keys {
            let oref = ObjectRef::from_key(key)
                .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", key));
            if let Some(ev) = self.store.get_object(&oref).await? {
                snapshot_items.push(EvidenceSnapshotItem {
                    object_key: ev.r#ref.to_key(),
                    source: ev.r#ref.source.clone(),
                    object_type: ev.r#ref.r#type.clone(),
                    external_id: ev.r#ref.id.clone(),
                    title: ev.title.clone(),
                    state: ev.state.clone(),
                    content_hash: None,
                    updated_at: ev.updated_at.clone(),
                });
            }
        }

        let record = SavedInvestigationRecord {
            id: id.clone(),
            title: req.title,
            query: req.query,
            created_at: now.clone(),
            updated_at: now.clone(),
            conclusions_markdown: req.conclusions_markdown,
            evidence_keys_json: serde_json::to_string(&req.evidence_keys)?,
            snapshot_json: serde_json::to_string(&snapshot_items)?,
            last_reopened_at: None,
            tags: req.tags,
        };

        self.store.save_investigation(record.clone()).await?;
        Ok(record)
    }

    pub async fn list(&self) -> Result<Vec<SavedInvestigationRecord>> {
        self.store.list_investigations().await
    }

    pub async fn get(&self, id: &str) -> Result<Option<SavedInvestigationRecord>> {
        self.store.get_investigation(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        self.store.delete_investigation(id).await
    }

    pub async fn reopen(&self, id: &str) -> Result<Option<ReopenedInvestigationReport>> {
        let inv = match self.store.get_investigation(id).await? {
            Some(i) => i,
            None => return Ok(None),
        };

        let now = Utc::now().to_rfc3339();
        self.store
            .update_investigation_reopened(&inv.id, &now)
            .await?;

        let saved_snapshot: Vec<EvidenceSnapshotItem> =
            serde_json::from_str(&inv.snapshot_json).unwrap_or_default();
        let evidence_keys: Vec<String> =
            serde_json::from_str(&inv.evidence_keys_json).unwrap_or_default();

        let mut saved_map: HashMap<String, &EvidenceSnapshotItem> = HashMap::new();
        for item in &saved_snapshot {
            saved_map.insert(item.object_key.clone(), item);
        }

        let mut current_items = Vec::new();
        for key in &evidence_keys {
            let oref = ObjectRef::from_key(key)
                .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", key));
            if let Some(ev) = self.store.get_object(&oref).await? {
                current_items.push(ev);
            }
        }

        // Also check if new objects match the query
        let fts = self.store.search_fts(&inv.query, 10).await?;
        for f in fts {
            if !current_items
                .iter()
                .any(|c| c.r#ref.to_key() == f.evidence.r#ref.to_key())
            {
                current_items.push(f.evidence);
            }
        }

        let mut new_evidence = Vec::new();
        let mut changed_evidence = Vec::new();
        let mut resolved_evidence = Vec::new();

        for curr in &current_items {
            let k = curr.r#ref.to_key();
            let curr_snap = EvidenceSnapshotItem {
                object_key: k.clone(),
                source: curr.r#ref.source.clone(),
                object_type: curr.r#ref.r#type.clone(),
                external_id: curr.r#ref.id.clone(),
                title: curr.title.clone(),
                state: curr.state.clone(),
                content_hash: None,
                updated_at: curr.updated_at.clone(),
            };

            if let Some(prev) = saved_map.get(&k) {
                if prev.state != curr.state || prev.title != curr.title {
                    changed_evidence.push(curr_snap.clone());

                    let prev_lower = prev.state.to_lowercase();
                    let curr_lower = curr.state.to_lowercase();
                    let was_open = !prev_lower.contains("done")
                        && !prev_lower.contains("closed")
                        && !prev_lower.contains("merged");
                    let now_closed = curr_lower.contains("done")
                        || curr_lower.contains("closed")
                        || curr_lower.contains("merged")
                        || curr_lower.contains("fix released");

                    if was_open && now_closed {
                        resolved_evidence.push(curr_snap);
                    }
                }
            } else {
                new_evidence.push(curr_snap);
            }
        }

        // Check contradictory evidence among current items
        let mut contradictory_evidence = Vec::new();
        for curr in &current_items {
            let relations = self
                .store
                .get_relations_for_key(&curr.r#ref.to_key())
                .await?;
            for rel in relations {
                let other_ref = if rel.from.to_key() == curr.r#ref.to_key() {
                    &rel.to
                } else {
                    &rel.from
                };
                if let Some(other_ev) = self.store.get_object(other_ref).await? {
                    let s1 = curr.state.to_lowercase();
                    let s2 = other_ev.state.to_lowercase();

                    let c1_closed =
                        s1.contains("done") || s1.contains("merged") || s1.contains("fix released");
                    let c2_open =
                        s2.contains("in progress") || s2.contains("open") || s2.contains("to do");

                    if c1_closed && c2_open {
                        contradictory_evidence.push(format!(
                            "Contradiction: {} is '{}' but linked {} is '{}'",
                            curr.r#ref.to_key(),
                            curr.state,
                            other_ev.r#ref.to_key(),
                            other_ev.state
                        ));
                    }
                }
            }
        }
        contradictory_evidence.dedup();

        let updated_conclusions_guidance = format!(
            "Investigation '{}' reopened with {} new evidence item(s), {} changed item(s), and {} newly resolved item(s). {} contradiction(s) found. Review original conclusions against these updates.",
            inv.title, new_evidence.len(), changed_evidence.len(), resolved_evidence.len(), contradictory_evidence.len()
        );

        Ok(Some(ReopenedInvestigationReport {
            investigation: inv,
            reopened_at: now,
            deltas: InvestigationDeltas {
                new_evidence,
                changed_evidence,
                resolved_evidence,
                contradictory_evidence,
            },
            updated_conclusions_guidance,
        }))
    }
}
