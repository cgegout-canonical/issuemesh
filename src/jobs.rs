use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use crate::connectors::ObjectRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    GitHub,
    Jira,
    Launchpad,
    Local,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::GitHub => write!(f, "github"),
            Source::Jira => write!(f, "jira"),
            Source::Launchpad => write!(f, "launchpad"),
            Source::Local => write!(f, "local"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchStats {
    pub objects_analysed: usize,
    pub deep_reads: usize,
    pub relations_followed: usize,
    pub trackers_searched: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SearchEvent {
    PlanningStarted,
    PlanningCompleted,
    LocalSearchStarted,
    LocalSearchCompleted { objects: usize },
    SourceSearchStarted { source: Source },
    SourceSearchCompleted { source: Source, objects: usize },
    DeepReadStarted { object: ObjectRef },
    DeepReadCompleted { object: ObjectRef },
    GraphExpansion { depth: u8, nodes: usize },
    ConsistencyCheckStarted,
    ConsistencyCheckCompleted { findings: usize },
    SynthesisStarted,
    Complete(SearchStats),
    Warning(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchJobSummary {
    pub id: String,
    pub query: String,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub stats: Option<SearchStats>,
    pub error: Option<String>,
}

pub struct SearchJob {
    pub summary: SearchJobSummary,
    pub event_history: Vec<SearchEvent>,
    pub event_sender: broadcast::Sender<SearchEvent>,
}

#[derive(Clone)]
pub struct JobManager {
    jobs: Arc<RwLock<HashMap<String, Arc<RwLock<SearchJob>>>>>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_job(&self, query: String) -> String {
        let id = format!("job_{}", uuid_simple());
        let (tx, _rx) = broadcast::channel(128);

        let job = SearchJob {
            summary: SearchJobSummary {
                id: id.clone(),
                query,
                status: JobStatus::Queued,
                created_at: Utc::now(),
                completed_at: None,
                stats: None,
                error: None,
            },
            event_history: Vec::new(),
            event_sender: tx,
        };

        let mut lock = self.jobs.write().await;
        lock.insert(id.clone(), Arc::new(RwLock::new(job)));
        id
    }

    pub async fn get_job_summary(&self, id: &str) -> Option<SearchJobSummary> {
        let lock = self.jobs.read().await;
        let job_arc = lock.get(id)?.clone();
        let job = job_arc.read().await;
        Some(job.summary.clone())
    }

    pub async fn list_jobs(&self) -> Vec<SearchJobSummary> {
        let lock = self.jobs.read().await;
        let mut list = Vec::new();
        for job_arc in lock.values() {
            list.push(job_arc.read().await.summary.clone());
        }
        list.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        list
    }

    pub async fn subscribe_events(
        &self,
        id: &str,
    ) -> Option<(Vec<SearchEvent>, broadcast::Receiver<SearchEvent>)> {
        let lock = self.jobs.read().await;
        let job_arc = lock.get(id)?.clone();
        let job = job_arc.read().await;
        let history = job.event_history.clone();
        let rx = job.event_sender.subscribe();
        Some((history, rx))
    }

    pub async fn get_event_history(&self, id: &str) -> Option<Vec<SearchEvent>> {
        let lock = self.jobs.read().await;
        let job_arc = lock.get(id)?.clone();
        let job = job_arc.read().await;
        Some(job.event_history.clone())
    }

    pub async fn emit_event(&self, id: &str, event: SearchEvent) {
        let lock = self.jobs.read().await;
        if let Some(job_arc) = lock.get(id) {
            let mut job = job_arc.write().await;
            job.event_history.push(event.clone());

            // Update summary status on transition events
            match &event {
                SearchEvent::PlanningStarted | SearchEvent::LocalSearchStarted => {
                    job.summary.status = JobStatus::Running;
                }
                SearchEvent::Complete(stats) => {
                    job.summary.status = JobStatus::Completed;
                    job.summary.completed_at = Some(Utc::now());
                    job.summary.stats = Some(stats.clone());
                }
                SearchEvent::Error(err) => {
                    job.summary.status = JobStatus::Failed;
                    job.summary.completed_at = Some(Utc::now());
                    job.summary.error = Some(err.clone());
                }
                _ => {}
            }

            let _ = job.event_sender.send(event);
        }
    }
}

fn uuid_simple() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}{:x}", now.as_secs(), now.subsec_nanos())
}
