use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::task;

const CURRENT_SCHEMA_VERSION: i64 = 1;

pub const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS objects (
    object_key TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    object_type TEXT NOT NULL,
    external_id TEXT NOT NULL,
    title TEXT NOT NULL,
    snippet TEXT NOT NULL,
    body TEXT,
    url TEXT NOT NULL,
    container TEXT NOT NULL,
    author TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT,
    updated_at TEXT,
    native_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    last_fetched_at TEXT NOT NULL,
    first_seen_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_objects_source ON objects(source);
CREATE INDEX IF NOT EXISTS idx_objects_container ON objects(container);
CREATE INDEX IF NOT EXISTS idx_objects_updated_at ON objects(updated_at);
CREATE INDEX IF NOT EXISTS idx_objects_content_hash ON objects(content_hash);

CREATE VIRTUAL TABLE IF NOT EXISTS object_fts USING fts5(
    object_key UNINDEXED,
    title,
    snippet,
    body,
    container,
    author,
    tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS relations (
    from_key TEXT NOT NULL,
    to_key TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    detected_by TEXT NOT NULL,
    confidence REAL NOT NULL,
    first_seen_at TEXT NOT NULL,
    PRIMARY KEY (from_key, to_key, relation_type)
);

CREATE INDEX IF NOT EXISTS idx_relations_from ON relations(from_key);
CREATE INDEX IF NOT EXISTS idx_relations_to ON relations(to_key);
CREATE INDEX IF NOT EXISTS idx_relations_type ON relations(relation_type);

CREATE TABLE IF NOT EXISTS unresolved_refs (
    ref_key TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    target_id TEXT NOT NULL,
    discovered_from_key TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS revisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    object_key TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    title TEXT NOT NULL,
    snippet TEXT NOT NULL,
    body TEXT,
    state TEXT NOT NULL,
    native_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_revisions_object_key ON revisions(object_key);

CREATE TABLE IF NOT EXISTS embeddings (
    object_key TEXT PRIMARY KEY,
    embedding_blob BLOB NOT NULL,
    dimension INTEGER NOT NULL,
    model_name TEXT NOT NULL,
    stale INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS search_runs (
    id TEXT PRIMARY KEY,
    query TEXT NOT NULL,
    plan_json TEXT NOT NULL,
    stats_json TEXT NOT NULL,
    answer_markdown TEXT,
    prompt_version TEXT,
    model_name TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS search_run_objects (
    search_run_id TEXT NOT NULL,
    object_key TEXT NOT NULL,
    relevance_score REAL,
    rank INTEGER,
    PRIMARY KEY (search_run_id, object_key)
);

CREATE TABLE IF NOT EXISTS api_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    status_code INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL,
    called_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_audit_source ON api_audit(source);
"#;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open SQLite database")?;
        Self::apply_pragmas(&conn)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.run_migrations()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn =
            Connection::open_in_memory().context("Failed to open in-memory SQLite database")?;
        Self::apply_pragmas(&conn)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.run_migrations()?;
        Ok(store)
    }

    fn apply_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA foreign_keys = ON;
            PRAGMA synchronous = NORMAL;
            "#,
        )
        .context("Failed to apply SQLite pragmas")?;
        Ok(())
    }

    pub fn run_migrations(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let current_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_version < 1 {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute_batch(MIGRATION_V1)
                .context("Failed to apply migration v1")?;
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![CURRENT_SCHEMA_VERSION, now],
            )
            .context("Failed to record schema version")?;
        }

        Ok(())
    }

    pub async fn get_schema_version(&self) -> Result<i64> {
        self.call(|conn| {
            let ver: i64 = conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )?;
            Ok(ver)
        })
        .await
    }

    pub async fn call<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn_arc = Arc::clone(&self.conn);
        task::spawn_blocking(move || {
            let mut conn = conn_arc.lock().unwrap();
            f(&mut conn)
        })
        .await
        .context("Task join error during DB call")?
    }

    /// Computes deterministic SHA-256 content hash of normalized evidence
    pub fn compute_content_hash(evidence: &crate::connectors::Evidence) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(evidence.title.as_bytes());
        hasher.update(b"\0");
        hasher.update(evidence.snippet.as_bytes());
        hasher.update(b"\0");
        if let Some(ref b) = evidence.body {
            hasher.update(b.as_bytes());
        }
        hasher.update(b"\0");
        hasher.update(evidence.state.as_bytes());
        hasher.update(b"\0");
        if let Ok(native_str) = serde_json::to_string(&evidence.native) {
            hasher.update(native_str.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    /// Upserts an Evidence record into `objects`.
    /// If content changed, saves a snapshot to `revisions`.
    /// Also marks any existing unresolved_ref for this object as resolved (1).
    pub async fn upsert_object(&self, evidence: crate::connectors::Evidence) -> Result<bool> {
        let content_hash = Self::compute_content_hash(&evidence);
        let now = chrono::Utc::now().to_rfc3339();

        self.call(move |conn| {
            let tx = conn.transaction()?;
            let obj_key = evidence.r#ref.to_key();

            // Check if existing object exists and get previous content hash
            let prev_hash: Option<String> = tx
                .query_row(
                    "SELECT content_hash FROM objects WHERE object_key = ?1",
                    rusqlite::params![&obj_key],
                    |row| row.get(0),
                )
                .ok();

            let native_json = serde_json::to_string(&evidence.native).unwrap_or_else(|_| "{}".to_string());
            let has_changed = match prev_hash {
                Some(ref h) => h != &content_hash,
                None => true,
            };

            if has_changed {
                // Save to revisions if updating an existing record
                if prev_hash.is_some() {
                    tx.execute(
                        "INSERT INTO revisions (object_key, content_hash, captured_at, title, snippet, body, state, native_json)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            &obj_key,
                            &content_hash,
                            &now,
                            &evidence.title,
                            &evidence.snippet,
                            &evidence.body,
                            &evidence.state,
                            &native_json,
                        ],
                    )?;
                }

                // Insert or update objects
                tx.execute(
                    "INSERT INTO objects (
                        object_key, source, object_type, external_id,
                        title, snippet, body, url, container, author,
                        state, created_at, updated_at, native_json,
                        content_hash, last_fetched_at, first_seen_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                    ON CONFLICT(object_key) DO UPDATE SET
                        title = excluded.title,
                        snippet = excluded.snippet,
                        body = excluded.body,
                        url = excluded.url,
                        container = excluded.container,
                        author = excluded.author,
                        state = excluded.state,
                        updated_at = excluded.updated_at,
                        native_json = excluded.native_json,
                        content_hash = excluded.content_hash,
                        last_fetched_at = excluded.last_fetched_at",
                    rusqlite::params![
                        &obj_key,
                        &evidence.r#ref.source,
                        &evidence.r#ref.r#type,
                        &evidence.r#ref.id,
                        &evidence.title,
                        &evidence.snippet,
                        &evidence.body,
                        &evidence.url,
                        &evidence.container,
                        &evidence.author,
                        &evidence.state,
                        &evidence.created_at,
                        &evidence.updated_at,
                        &native_json,
                        &content_hash,
                        &now,
                        &now,
                    ],
                )?;

                // Update or insert into object_fts
                tx.execute(
                    "DELETE FROM object_fts WHERE object_key = ?1",
                    rusqlite::params![&obj_key],
                )?;
                tx.execute(
                    "INSERT INTO object_fts (object_key, title, snippet, body, container, author)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        &obj_key,
                        &evidence.title,
                        &evidence.snippet,
                        &evidence.body.as_deref().unwrap_or(""),
                        &evidence.container,
                        &evidence.author,
                    ],
                )?;
            } else {
                // Just update last_fetched_at
                tx.execute(
                    "UPDATE objects SET last_fetched_at = ?1 WHERE object_key = ?2",
                    rusqlite::params![&now, &obj_key],
                )?;
            }

            // Mark any pending unresolved_refs matching this object_key as resolved
            tx.execute(
                "UPDATE unresolved_refs SET resolved = 1 WHERE ref_key = ?1",
                rusqlite::params![&obj_key],
            )?;

            tx.commit()?;
            Ok(has_changed)
        })
        .await
    }

    /// Retrieve an object by its canonical ObjectRef
    pub async fn get_object(
        &self,
        r: &crate::connectors::ObjectRef,
    ) -> Result<Option<crate::connectors::Evidence>> {
        let key = r.to_key();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT object_key, source, object_type, external_id,
                        title, snippet, body, url, container, author,
                        state, created_at, updated_at, native_json
                 FROM objects WHERE object_key = ?1",
            )?;

            let res = stmt.query_row(rusqlite::params![&key], |row| {
                let obj_key: String = row.get(0)?;
                let source: String = row.get(1)?;
                let object_type: String = row.get(2)?;
                let external_id: String = row.get(3)?;
                let title: String = row.get(4)?;
                let snippet: String = row.get(5)?;
                let body: Option<String> = row.get(6)?;
                let url: String = row.get(7)?;
                let container: String = row.get(8)?;
                let author: String = row.get(9)?;
                let state: String = row.get(10)?;
                let created_at: Option<String> = row.get(11)?;
                let updated_at: Option<String> = row.get(12)?;
                let native_str: String = row.get(13)?;
                let native = serde_json::from_str(&native_str).unwrap_or_default();

                let obj_ref =
                    crate::connectors::ObjectRef::from_key(&obj_key).unwrap_or_else(|| {
                        crate::connectors::ObjectRef::new(source, object_type, external_id)
                    });

                Ok(crate::connectors::Evidence {
                    r#ref: obj_ref,
                    title,
                    snippet,
                    body,
                    url,
                    container,
                    author,
                    created_at,
                    updated_at,
                    state,
                    native,
                })
            });

            match res {
                Ok(ev) => Ok(Some(ev)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(anyhow::anyhow!(e)),
            }
        })
        .await
    }

    /// Upserts a relation between two objects.
    pub async fn insert_relation(&self, rel: crate::connectors::Relation) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.call(move |conn| {
            conn.execute(
                "INSERT INTO relations (from_key, to_key, relation_type, detected_by, confidence, first_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(from_key, to_key, relation_type) DO UPDATE SET
                    confidence = MAX(relations.confidence, excluded.confidence),
                    detected_by = excluded.detected_by",
                rusqlite::params![
                    &rel.from.to_key(),
                    &rel.to.to_key(),
                    &rel.relation_type,
                    &rel.detected_by,
                    rel.confidence,
                    &now,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Batch upsert relations
    pub async fn insert_relations(&self, rels: Vec<crate::connectors::Relation>) -> Result<()> {
        if rels.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now().to_rfc3339();
        self.call(move |conn| {
            let tx = conn.transaction()?;
            for rel in rels {
                tx.execute(
                    "INSERT INTO relations (from_key, to_key, relation_type, detected_by, confidence, first_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(from_key, to_key, relation_type) DO UPDATE SET
                        confidence = MAX(relations.confidence, excluded.confidence),
                        detected_by = excluded.detected_by",
                    rusqlite::params![
                        &rel.from.to_key(),
                        &rel.to.to_key(),
                        &rel.relation_type,
                        &rel.detected_by,
                        rel.confidence,
                        &now,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Get outgoing and incoming relations for an object key
    pub async fn get_relations_for_key(
        &self,
        key: &str,
    ) -> Result<Vec<crate::connectors::Relation>> {
        let k = key.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT from_key, to_key, relation_type, detected_by, confidence
                 FROM relations
                 WHERE from_key = ?1 OR to_key = ?1",
            )?;

            let rows = stmt.query_map(rusqlite::params![&k], |row| {
                let from_k: String = row.get(0)?;
                let to_k: String = row.get(1)?;
                let relation_type: String = row.get(2)?;
                let detected_by: String = row.get(3)?;
                let confidence: f64 = row.get(4)?;

                let from = crate::connectors::ObjectRef::from_key(&from_k).unwrap_or_else(|| {
                    crate::connectors::ObjectRef::new("unknown", "unknown", &from_k)
                });
                let to = crate::connectors::ObjectRef::from_key(&to_k).unwrap_or_else(|| {
                    crate::connectors::ObjectRef::new("unknown", "unknown", &to_k)
                });

                Ok(crate::connectors::Relation {
                    from,
                    to,
                    relation_type,
                    detected_by,
                    confidence: confidence as f32,
                })
            })?;

            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    /// Record an unresolved reference discovered during parsing
    pub async fn record_unresolved_ref(
        &self,
        target: &crate::connectors::ObjectRef,
        discovered_from_key: &str,
    ) -> Result<()> {
        let ref_key = target.to_key();
        let source = target.source.clone();
        let target_id = target.id.clone();
        let from_key = discovered_from_key.to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.call(move |conn| {
            // Only insert if object does not already exist in objects table
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM objects WHERE object_key = ?1",
                    rusqlite::params![&ref_key],
                    |_| Ok(true),
                )
                .unwrap_or(false);

            if !exists {
                conn.execute(
                    "INSERT OR IGNORE INTO unresolved_refs (ref_key, source, target_id, discovered_from_key, first_seen_at, resolved)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                    rusqlite::params![&ref_key, &source, &target_id, &from_key, &now],
                )?;
            }
            Ok(())
        })
        .await
    }

    /// Retrieve pending unresolved references up to limit
    pub async fn get_unresolved_refs(
        &self,
        limit: usize,
    ) -> Result<Vec<(crate::connectors::ObjectRef, String)>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT ref_key, source, target_id, discovered_from_key
                 FROM unresolved_refs
                 WHERE resolved = 0
                 LIMIT ?1",
            )?;

            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                let ref_key: String = row.get(0)?;
                let source: String = row.get(1)?;
                let target_id: String = row.get(2)?;
                let discovered_from: String = row.get(3)?;

                let obj_ref =
                    crate::connectors::ObjectRef::from_key(&ref_key).unwrap_or_else(|| {
                        crate::connectors::ObjectRef::new(source, "unknown", target_id)
                    });

                Ok((obj_ref, discovered_from))
            })?;

            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    /// Record API audit event
    pub async fn record_api_audit(
        &self,
        source: &str,
        method: &str,
        path: &str,
        status_code: u16,
        latency_ms: i64,
    ) -> Result<()> {
        let source = source.to_string();
        let method = method.to_string();
        let path = path.to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.call(move |conn| {
            conn.execute(
                "INSERT INTO api_audit (source, method, path, status_code, latency_ms, called_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    &source,
                    &method,
                    &path,
                    status_code as i64,
                    latency_ms,
                    &now
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Datastore summary statistics
    pub async fn get_stats(&self) -> Result<DatastoreStats> {
        self.call(|conn| {
            let object_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))
                .unwrap_or(0);
            let relation_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))
                .unwrap_or(0);
            let revision_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
                .unwrap_or(0);
            let unresolved_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM unresolved_refs WHERE resolved = 0",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let audit_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM api_audit", [], |r| r.get(0))
                .unwrap_or(0);

            Ok(DatastoreStats {
                object_count,
                relation_count,
                revision_count,
                unresolved_count,
                audit_count,
            })
        })
        .await
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatastoreStats {
    pub object_count: i64,
    pub relation_count: i64,
    pub revision_count: i64,
    pub unresolved_count: i64,
    pub audit_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_migrations_apply_and_idempotent() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        let store = Store::open(&db_path).unwrap();
        let ver1 = store.get_schema_version().await.unwrap();
        assert_eq!(ver1, CURRENT_SCHEMA_VERSION);

        // Apply migrations again: must be an idempotent no-op
        store.run_migrations().unwrap();
        let ver2 = store.get_schema_version().await.unwrap();
        assert_eq!(ver2, CURRENT_SCHEMA_VERSION);
    }
}
