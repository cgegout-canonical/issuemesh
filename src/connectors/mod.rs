pub mod github;
pub mod http;
pub mod jira;
pub mod launchpad;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub use http::{HttpError, ReadOnlyHttpClient};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectRef {
    pub source: String,
    pub r#type: String,
    pub id: String,
}

impl ObjectRef {
    pub fn new(
        source: impl Into<String>,
        r#type: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            r#type: r#type.into(),
            id: id.into(),
        }
    }

    pub fn to_key(&self) -> String {
        format!("{}:{}:{}", self.source, self.r#type, self.id)
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.splitn(3, ':').collect();
        if parts.len() == 3 {
            Some(Self {
                source: parts[0].to_string(),
                r#type: parts[1].to_string(),
                id: parts[2].to_string(),
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub r#ref: ObjectRef,
    pub title: String,
    pub snippet: String,
    pub body: Option<String>,
    pub url: String,
    pub container: String,
    pub author: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub state: String,
    pub native: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub from: ObjectRef,
    pub to: ObjectRef,
    pub relation_type: String,
    pub detected_by: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadComment {
    pub author: String,
    pub created_at: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub r#ref: ObjectRef,
    pub comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub detail: String,
    pub auth: String, // "ok" | "failed" | "none"
    pub rate_limit_remaining: Option<i64>,
    pub latency_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub query: String,
    pub people: Vec<String>,
    pub projects: Vec<String>,
    pub repositories: Vec<String>,
    pub time_from: Option<DateTime<Utc>>,
    pub time_to: Option<DateTime<Utc>>,
    pub max_results: usize,
    pub native: BTreeMap<String, Value>,
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn source(&self) -> &str;
    async fn search(&self, q: &SearchQuery) -> Result<Vec<Evidence>>;
    async fn get_object(&self, r: &ObjectRef) -> Result<Option<Evidence>>;
    async fn get_thread(&self, r: &ObjectRef, window: usize) -> Result<Option<Thread>>;
    async fn get_related(&self, r: &ObjectRef, kinds: Option<&[String]>) -> Result<Vec<Relation>>;
    async fn health(&self) -> Health;
    fn credential_expiry(&self) -> Option<String> {
        None
    }
}

pub fn helper_s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
