use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::connectors::{Evidence, ObjectRef};
use crate::intelligence::diff::{
    diff_evidence_snapshots, diff_revisions, EvidenceDiffReport, EvidenceSnapshotItem,
    RevisionDiffItem,
};
use crate::store::{Store, WatchlistRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWatchlistRequest {
    pub target_type: String, // "issue", "project", "topic", "query"
    pub target_value: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistCheckResult {
    pub watchlist_id: String,
    pub title: String,
    pub target_type: String,
    pub target_value: String,
    pub checked_at: String,
    pub diff: EvidenceDiffReport,
    pub current_items_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectWhatChangedResult {
    pub object_key: String,
    pub current: Option<Evidence>,
    pub revision_history: Vec<RevisionDiffItem>,
    pub total_revisions: usize,
    pub summary: String,
}

pub struct WatchlistService {
    store: Store,
}

impl WatchlistService {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn create_watchlist(&self, req: CreateWatchlistRequest) -> Result<WatchlistRecord> {
        let id = format!("wl-{}", uuid::Uuid::new_v4().simple());
        let now = Utc::now().to_rfc3339();

        // Collect initial snapshot
        let items = self
            .fetch_targets(&req.target_type, &req.target_value)
            .await?;
        let snapshot_items: Vec<EvidenceSnapshotItem> = items
            .iter()
            .map(|e| EvidenceSnapshotItem {
                object_key: e.r#ref.to_key(),
                source: e.r#ref.source.clone(),
                object_type: e.r#ref.r#type.clone(),
                external_id: e.r#ref.id.clone(),
                title: e.title.clone(),
                state: e.state.clone(),
                content_hash: None,
                updated_at: e.updated_at.clone(),
            })
            .collect();

        let snapshot_json = serde_json::to_string(&snapshot_items)?;

        let record = WatchlistRecord {
            id: id.clone(),
            target_type: req.target_type,
            target_value: req.target_value,
            title: req.title,
            created_at: now.clone(),
            last_checked_at: Some(now),
            last_snapshot_json: snapshot_json,
        };

        self.store.save_watchlist(record.clone()).await?;
        Ok(record)
    }

    pub async fn list_watchlists(&self) -> Result<Vec<WatchlistRecord>> {
        self.store.list_watchlists().await
    }

    pub async fn get_watchlist(&self, id: &str) -> Result<Option<WatchlistRecord>> {
        self.store.get_watchlist(id).await
    }

    pub async fn delete_watchlist(&self, id: &str) -> Result<bool> {
        self.store.delete_watchlist(id).await
    }

    /// Check watchlist for changes since last snapshot
    pub async fn check_watchlist(&self, id: &str) -> Result<Option<WatchlistCheckResult>> {
        let wl = match self.store.get_watchlist(id).await? {
            Some(w) => w,
            None => return Ok(None),
        };

        let prev_snapshot: Vec<EvidenceSnapshotItem> =
            serde_json::from_str(&wl.last_snapshot_json).unwrap_or_default();

        let current_items = self
            .fetch_targets(&wl.target_type, &wl.target_value)
            .await?;
        let diff = diff_evidence_snapshots(&prev_snapshot, &current_items);

        let now = Utc::now().to_rfc3339();
        let current_snapshot: Vec<EvidenceSnapshotItem> = current_items
            .iter()
            .map(|e| EvidenceSnapshotItem {
                object_key: e.r#ref.to_key(),
                source: e.r#ref.source.clone(),
                object_type: e.r#ref.r#type.clone(),
                external_id: e.r#ref.id.clone(),
                title: e.title.clone(),
                state: e.state.clone(),
                content_hash: None,
                updated_at: e.updated_at.clone(),
            })
            .collect();

        let new_snapshot_json = serde_json::to_string(&current_snapshot)?;
        self.store
            .update_watchlist_snapshot(&wl.id, &new_snapshot_json, &now)
            .await?;

        Ok(Some(WatchlistCheckResult {
            watchlist_id: wl.id,
            title: wl.title,
            target_type: wl.target_type,
            target_value: wl.target_value,
            checked_at: now,
            diff,
            current_items_count: current_items.len(),
        }))
    }

    /// What changed for a specific object across its history
    pub async fn what_changed_for_object(
        &self,
        object_key: &str,
    ) -> Result<ObjectWhatChangedResult> {
        let obj_ref = ObjectRef::from_key(object_key)
            .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", object_key));

        let current = self.store.get_object(&obj_ref).await?;
        let revisions = self.store.get_revisions_for_key(object_key).await?;
        let total_revisions = revisions.len();
        let history = diff_revisions(&revisions);

        let summary = if total_revisions == 0 {
            format!(
                "No historical revisions recorded yet for '{}'. Current state: {}",
                object_key,
                current
                    .as_ref()
                    .map(|c| c.state.as_str())
                    .unwrap_or("unknown")
            )
        } else {
            let last_change = history
                .last()
                .map(|h| h.details.clone())
                .unwrap_or_default();
            format!(
                "Object '{}' has {} recorded revision(s). Latest transition: {}",
                object_key, total_revisions, last_change
            )
        };

        Ok(ObjectWhatChangedResult {
            object_key: object_key.to_string(),
            current,
            revision_history: history,
            total_revisions,
            summary,
        })
    }

    async fn fetch_targets(&self, target_type: &str, target_val: &str) -> Result<Vec<Evidence>> {
        match target_type {
            "issue" => {
                let mut out = Vec::new();
                let obj_ref = ObjectRef::from_key(target_val)
                    .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", target_val));
                if let Some(ev) = self.store.get_object(&obj_ref).await? {
                    out.push(ev);
                }
                // Also fetch 1-hop related objects
                let relations = self.store.get_relations_for_key(target_val).await?;
                for rel in relations {
                    let other_ref = if rel.from.to_key() == target_val {
                        &rel.to
                    } else {
                        &rel.from
                    };
                    if let Some(other_ev) = self.store.get_object(other_ref).await? {
                        if !out
                            .iter()
                            .any(|e| e.r#ref.to_key() == other_ev.r#ref.to_key())
                        {
                            out.push(other_ev);
                        }
                    }
                }
                Ok(out)
            }
            "project" => {
                // Fetch objects by container
                let items = self.store.get_objects_by_container(target_val).await?;
                if items.is_empty() {
                    // Try partial match across all objects
                    let all = self.store.get_all_objects().await?;
                    let filtered: Vec<Evidence> = all
                        .into_iter()
                        .filter(|e| {
                            e.container.eq_ignore_ascii_case(target_val)
                                || e.r#ref.id.starts_with(target_val)
                        })
                        .collect();
                    Ok(filtered)
                } else {
                    Ok(items)
                }
            }
            "topic" | "query" => {
                let fts = self.store.search_fts(target_val, 50).await?;
                let items = fts.into_iter().map(|f| f.evidence).collect();
                Ok(items)
            }
            _ => {
                // Fallback to FTS
                let fts = self.store.search_fts(target_val, 50).await?;
                let items = fts.into_iter().map(|f| f.evidence).collect();
                Ok(items)
            }
        }
    }
}
