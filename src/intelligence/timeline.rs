use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::connectors::ObjectRef;
use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub timestamp: String,
    pub source: String,
    pub object_key: String,
    pub event_type: String, // "created", "state_transition", "revision", "linked_pr", "merged", "relation"
    pub actor: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineReport {
    pub root_key: String,
    pub title: String,
    pub total_events: usize,
    pub events: Vec<TimelineEvent>,
    pub key_decisions: Vec<String>,
    pub unresolved_items: Vec<String>,
    pub story_narrative: String,
}

pub struct StoryReconstructor {
    store: Store,
}

impl StoryReconstructor {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn reconstruct_timeline(&self, root_key: &str) -> Result<TimelineReport> {
        let root_ref = ObjectRef::from_key(root_key)
            .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", root_key));

        let root_ev = self.store.get_object(&root_ref).await?;
        let root_title = root_ev
            .as_ref()
            .map(|e| e.title.clone())
            .unwrap_or_else(|| root_key.to_string());

        // Collect related objects
        let relations = self.store.get_relations_for_key(root_key).await?;
        let mut relevant_keys = vec![root_key.to_string()];
        for rel in &relations {
            let other_k = if rel.from.to_key() == root_key {
                rel.to.to_key()
            } else {
                rel.from.to_key()
            };
            if !relevant_keys.contains(&other_k) {
                relevant_keys.push(other_k);
            }
        }

        let mut events = Vec::new();
        let mut key_decisions = Vec::new();
        let mut unresolved_items = Vec::new();

        for key in &relevant_keys {
            let oref = ObjectRef::from_key(key)
                .unwrap_or_else(|| ObjectRef::new("unknown", "unknown", key));
            let obj_opt = self.store.get_object(&oref).await?;

            if let Some(obj) = obj_opt {
                // Event: Creation
                if let Some(created_at) = &obj.created_at {
                    events.push(TimelineEvent {
                        timestamp: created_at.clone(),
                        source: obj.r#ref.source.clone(),
                        object_key: key.clone(),
                        event_type: "created".to_string(),
                        actor: obj.author.clone(),
                        description: format!(
                            "[{}] Created {} '{}' by {}",
                            obj.r#ref.source.to_uppercase(),
                            obj.r#ref.r#type,
                            obj.title,
                            obj.author
                        ),
                    });
                }

                // Check revisions
                let revisions = self.store.get_revisions_for_key(key).await?;
                for rev in revisions {
                    events.push(TimelineEvent {
                        timestamp: rev.captured_at.clone(),
                        source: obj.r#ref.source.clone(),
                        object_key: key.clone(),
                        event_type: "revision".to_string(),
                        actor: obj.author.clone(),
                        description: format!(
                            "[{}] Recorded revision in state '{}' (title: '{}')",
                            obj.r#ref.source.to_uppercase(),
                            rev.state,
                            rev.title
                        ),
                    });

                    let state_lower = rev.state.to_lowercase();
                    if state_lower.contains("merged")
                        || state_lower.contains("done")
                        || state_lower.contains("approved")
                    {
                        key_decisions.push(format!(
                            "{}: {} reached state '{}'",
                            key, rev.captured_at, rev.state
                        ));
                    }
                }

                // Unresolved check
                let current_state = obj.state.to_lowercase();
                let is_closed = current_state.contains("done")
                    || current_state.contains("closed")
                    || current_state.contains("merged")
                    || current_state.contains("fix released");

                if !is_closed {
                    unresolved_items.push(format!(
                        "{} remains in state '{}' ({})",
                        key, obj.state, obj.title
                    ));
                }
            }
        }

        // Add relation events
        for rel in &relations {
            let desc = format!(
                "Connected {} --[{}]--> {}",
                rel.from.to_key(),
                rel.relation_type,
                rel.to.to_key()
            );
            key_decisions.push(desc.clone());
        }

        // Sort events chronologically
        events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        // Deduplicate key decisions
        key_decisions.dedup();

        // Build concise narrative
        let story_narrative = if events.is_empty() {
            format!("No historical events found for {}.", root_key)
        } else {
            let first_event = events
                .first()
                .map(|e| e.timestamp.clone())
                .unwrap_or_default();
            let last_event = events
                .last()
                .map(|e| e.timestamp.clone())
                .unwrap_or_default();
            format!(
                "Timeline for '{}' ({}) spans {} event(s) across {} tracker item(s) between {} and {}. {} key decision(s) recorded. Currently, {} item(s) remain unresolved.",
                root_key, root_title, events.len(), relevant_keys.len(), first_event, last_event, key_decisions.len(), unresolved_items.len()
            )
        };

        Ok(TimelineReport {
            root_key: root_key.to_string(),
            title: root_title,
            total_events: events.len(),
            events,
            key_decisions,
            unresolved_items,
            story_narrative,
        })
    }
}
