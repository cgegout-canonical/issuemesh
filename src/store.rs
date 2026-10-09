use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::task;

pub const CURRENT_SCHEMA_VERSION: i64 = 3;

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

pub const MIGRATION_V2: &str = r#"
CREATE TABLE IF NOT EXISTS introspection_runs (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    question TEXT NOT NULL,
    question_fingerprint TEXT NOT NULL,
    trigger_reason TEXT NOT NULL,
    search_run_id TEXT,
    accuracy REAL NOT NULL,
    relevance REAL NOT NULL,
    coverage REAL NOT NULL,
    traceability REAL NOT NULL,
    conciseness REAL NOT NULL,
    weighted_score REAL NOT NULL,
    percentage REAL NOT NULL,
    result TEXT NOT NULL,
    interpretation TEXT NOT NULL,
    answer TEXT NOT NULL,
    problem_areas_json TEXT NOT NULL,
    recommendation TEXT NOT NULL,
    evidence_keys_json TEXT NOT NULL,
    evaluation_details_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_introspection_runs_created_at ON introspection_runs(created_at);
CREATE INDEX IF NOT EXISTS idx_introspection_runs_fingerprint ON introspection_runs(question_fingerprint);
CREATE INDEX IF NOT EXISTS idx_introspection_runs_result ON introspection_runs(result);
"#;

pub const MIGRATION_V3: &str = r#"
CREATE TABLE IF NOT EXISTS watchlists (
    id TEXT PRIMARY KEY,
    target_type TEXT NOT NULL,
    target_value TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_checked_at TEXT,
    last_snapshot_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_watchlists_target_type ON watchlists(target_type);
CREATE INDEX IF NOT EXISTS idx_watchlists_target_value ON watchlists(target_value);

CREATE TABLE IF NOT EXISTS saved_investigations (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    query TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    conclusions_markdown TEXT NOT NULL,
    evidence_keys_json TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    last_reopened_at TEXT,
    tags TEXT
);

CREATE INDEX IF NOT EXISTS idx_saved_investigations_created_at ON saved_investigations(created_at);
CREATE INDEX IF NOT EXISTS idx_saved_investigations_title ON saved_investigations(title);
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
                rusqlite::params![1, now],
            )
            .context("Failed to record schema version 1")?;
        }

        if current_version < 2 {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute_batch(MIGRATION_V2)
                .context("Failed to apply migration v2")?;
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![2, now],
            )
            .context("Failed to record schema version 2")?;
        }

        if current_version < 3 {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute_batch(MIGRATION_V3)
                .context("Failed to apply migration v3")?;
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![CURRENT_SCHEMA_VERSION, now],
            )
            .context("Failed to record schema version 3")?;
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

    /// Save embedding vector with model_name and dimension guard
    pub async fn upsert_embedding(
        &self,
        object_key: &str,
        vector: &[f32],
        model_name: &str,
    ) -> Result<()> {
        let object_key = object_key.to_string();
        let model_name = model_name.to_string();
        let dimension = vector.len() as i64;
        let now = chrono::Utc::now().to_rfc3339();

        // Convert f32 slice into little-endian bytes blob
        let mut blob = Vec::with_capacity(vector.len() * 4);
        for &val in vector {
            blob.extend_from_slice(&val.to_le_bytes());
        }

        self.call(move |conn| {
            conn.execute(
                "INSERT INTO embeddings (object_key, embedding_blob, dimension, model_name, stale, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)
                 ON CONFLICT(object_key) DO UPDATE SET
                     embedding_blob = excluded.embedding_blob,
                     dimension = excluded.dimension,
                     model_name = excluded.model_name,
                     stale = 0,
                     updated_at = excluded.updated_at",
                rusqlite::params![&object_key, blob, dimension, &model_name, &now],
            )?;
            Ok(())
        })
        .await
    }

    /// Mark embeddings stale if dimension or model_name does not match current configured model
    pub async fn check_and_mark_stale_embeddings(
        &self,
        current_model: &str,
        current_dimension: usize,
    ) -> Result<usize> {
        let current_model = current_model.to_string();
        let current_dim = current_dimension as i64;

        self.call(move |conn| {
            let updated = conn.execute(
                "UPDATE embeddings
                 SET stale = 1
                 WHERE stale = 0 AND (model_name != ?1 OR dimension != ?2)",
                rusqlite::params![&current_model, current_dim],
            )?;
            Ok(updated)
        })
        .await
    }

    /// Retrieve all active (non-stale) embeddings matching expected model_name and dimension
    pub async fn get_valid_embeddings(
        &self,
        expected_model: &str,
        expected_dim: usize,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        let expected_model = expected_model.to_string();
        let expected_dim = expected_dim as i64;

        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT object_key, embedding_blob
                 FROM embeddings
                 WHERE stale = 0 AND model_name = ?1 AND dimension = ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![&expected_model, expected_dim], |row| {
                let key: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((key, blob))
            })?;

            let mut out = Vec::new();
            for item in rows {
                let (key, blob) = item?;
                if blob.len() % 4 == 0 {
                    let mut vec = Vec::with_capacity(blob.len() / 4);
                    #[allow(clippy::chunks_exact_to_as_chunks)]
                    for chunk in blob.chunks_exact(4) {
                        let arr: [u8; 4] = chunk.try_into().unwrap();
                        vec.push(f32::from_le_bytes(arr));
                    }
                    out.push((key, vec));
                }
            }

            Ok(out)
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

    /// Full-text search over objects using FTS5 with snippet generation and ranking
    pub async fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<FtsSearchResult>> {
        let q = query.trim().to_string();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        self.call(move |conn| {
            // Sanitize query tokens for FTS5: strip punctuation that breaks fts5 syntax
            let raw_tokens: Vec<String> = q
                .split_whitespace()
                .map(|token| {
                    token.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect::<String>()
                })
                .filter(|s| !s.is_empty())
                .collect();

            if raw_tokens.is_empty() {
                return Ok(Vec::new());
            }

            // Stop words filter: prevent conjunctive FTS5 queries from failing due to common conversational stop words
            const FTS_STOP_WORDS: &[&str] = &[
                "a", "an", "the", "and", "or", "of", "in", "on", "at", "to", "for", "from",
                "with", "by", "about", "is", "are", "was", "were", "be", "been", "being",
                "what", "which", "who", "whom", "where", "when", "why", "how",
                "do", "does", "did", "have", "has", "had", "can", "could", "should", "would",
                "show", "tell", "list", "find", "get", "give", "me", "please", "any", "all", "some",
                "top", "latest", "recent", "newest", "issue", "issues", "bug", "bugs", "task", "tasks", "pr", "prs",
            ];

            let non_stop: Vec<String> = raw_tokens
                .iter()
                .filter(|t| {
                    let lower = t.to_lowercase();
                    !FTS_STOP_WORDS.contains(&lower.as_str())
                        && !(t.chars().all(|c| c.is_ascii_digit()) && t.len() < 4)
                })
                .cloned()
                .collect();

            let tokens_to_use = if !non_stop.is_empty() {
                non_stop
            } else {
                raw_tokens
            };

            let clean_tokens: Vec<String> = tokens_to_use
                .into_iter()
                .map(|cleaned| format!("\"{}\"", cleaned))
                .collect();

            let execute_query = |conn: &rusqlite::Connection, q_str: &str| -> rusqlite::Result<Vec<FtsSearchResult>> {
                let mut stmt = conn.prepare(
                    "SELECT f.object_key,
                            snippet(object_fts, 1, '<b>', '</b>', '...', 16) AS title_snip,
                            snippet(object_fts, 3, '<b>', '</b>', '...', 32) AS body_snip,
                            bm25(object_fts, 0.0, 10.0, 5.0, 1.0, 5.0, 2.0) AS rank,
                            o.title, o.snippet, o.body, o.url, o.container, o.author, o.state,
                            o.source, o.object_type, o.external_id, o.created_at, o.updated_at, o.native_json
                     FROM object_fts f
                     JOIN objects o ON o.object_key = f.object_key
                     WHERE object_fts MATCH ?1
                     ORDER BY rank ASC
                     LIMIT ?2",
                )?;

                let rows = stmt.query_map(rusqlite::params![q_str, limit as i64], |row| {
                    let obj_key: String = row.get(0)?;
                    let title_snip: String = row.get(1)?;
                    let body_snip: String = row.get(2)?;
                    let rank: f64 = row.get(3)?;

                    let title: String = row.get(4)?;
                    let snippet: String = row.get(5)?;
                    let body: Option<String> = row.get(6)?;
                    let url: String = row.get(7)?;
                    let container: String = row.get(8)?;
                    let author: String = row.get(9)?;
                    let state: String = row.get(10)?;
                    let source: String = row.get(11)?;
                    let object_type: String = row.get(12)?;
                    let external_id: String = row.get(13)?;
                    let created_at: Option<String> = row.get(14)?;
                    let updated_at: Option<String> = row.get(15)?;
                    let native_str: String = row.get(16)?;
                    let native = serde_json::from_str(&native_str).unwrap_or_default();

                    let r#ref = crate::connectors::ObjectRef::from_key(&obj_key)
                        .unwrap_or_else(|| crate::connectors::ObjectRef::new(source, object_type, external_id));

                    let evidence = crate::connectors::Evidence {
                        r#ref,
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
                    };

                    let best_snippet = if !body_snip.is_empty() {
                        body_snip
                    } else if !title_snip.is_empty() {
                        title_snip
                    } else {
                        evidence.snippet.clone()
                    };

                    Ok(FtsSearchResult {
                        evidence,
                        snippet: best_snippet,
                        rank,
                    })
                })?;

                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            };

            let fts_query = clean_tokens.join(" ");
            let mut out = execute_query(conn, &fts_query)?;
            if out.is_empty() && clean_tokens.len() > 1 {
                let or_query = clean_tokens.join(" OR ");
                out = execute_query(conn, &or_query)?;
            }
            Ok(out)
        })
        .await
    }

    /// Bounded multi-hop graph traversal with cycle detection and node budget.
    pub async fn traverse_graph(
        &self,
        seeds: Vec<crate::connectors::ObjectRef>,
        depth: usize,
        max_nodes: usize,
    ) -> Result<GraphResult> {
        self.call(move |conn| {
            use std::collections::{HashSet, VecDeque};

            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            let mut budget_exhausted = false;

            for seed in seeds {
                let k = seed.to_key();
                if visited.insert(k.clone()) {
                    queue.push_back((k, 0usize));
                }
            }

            while let Some((curr_key, curr_depth)) = queue.pop_front() {
                // Fetch Evidence if present in objects table
                let obj_opt: Option<crate::connectors::Evidence> = conn
                    .prepare(
                        "SELECT object_key, source, object_type, external_id,
                                title, snippet, body, url, container, author,
                                state, created_at, updated_at, native_json
                         FROM objects WHERE object_key = ?1",
                    )?
                    .query_row(rusqlite::params![&curr_key], |row| {
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

                        let r#ref = crate::connectors::ObjectRef::from_key(&obj_key)
                            .unwrap_or_else(|| {
                                crate::connectors::ObjectRef::new(source, object_type, external_id)
                            });

                        Ok(crate::connectors::Evidence {
                            r#ref,
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
                    })
                    .ok();

                let curr_ref =
                    crate::connectors::ObjectRef::from_key(&curr_key).unwrap_or_else(|| {
                        crate::connectors::ObjectRef::new("unknown", "unknown", &curr_key)
                    });

                nodes.push(GraphNode {
                    r#ref: curr_ref,
                    evidence: obj_opt,
                    depth: curr_depth,
                });

                if nodes.len() >= max_nodes {
                    if !queue.is_empty() {
                        budget_exhausted = true;
                    }
                    break;
                }

                if curr_depth >= depth {
                    continue;
                }

                // Query adjacent edges
                let mut edge_stmt = conn.prepare(
                    "SELECT from_key, to_key, relation_type, detected_by, confidence
                     FROM relations
                     WHERE from_key = ?1 OR to_key = ?1",
                )?;

                let adjacent_edges = edge_stmt.query_map(rusqlite::params![&curr_key], |row| {
                    let from_k: String = row.get(0)?;
                    let to_k: String = row.get(1)?;
                    let relation_type: String = row.get(2)?;
                    let detected_by: String = row.get(3)?;
                    let confidence: f64 = row.get(4)?;

                    let from =
                        crate::connectors::ObjectRef::from_key(&from_k).unwrap_or_else(|| {
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

                for rel_res in adjacent_edges {
                    let rel = rel_res?;
                    let neighbor_key = if rel.from.to_key() == curr_key {
                        rel.to.to_key()
                    } else {
                        rel.from.to_key()
                    };

                    edges.push(rel);

                    if !visited.contains(&neighbor_key) {
                        if nodes.len() + queue.len() >= max_nodes {
                            budget_exhausted = true;
                        } else {
                            visited.insert(neighbor_key.clone());
                            queue.push_back((neighbor_key, curr_depth + 1));
                        }
                    }
                }
            }

            // Deduplicate collected edges
            let mut edge_set = HashSet::new();
            edges.retain(|e| {
                let key = format!("{}:{}:{}", e.from.to_key(), e.to.to_key(), e.relation_type);
                edge_set.insert(key)
            });

            Ok(GraphResult {
                nodes,
                edges,
                budget_exhausted,
            })
        })
        .await
    }

    /// Save completed search run and top objects
    #[allow(clippy::too_many_arguments)]
    pub async fn save_search_run(
        &self,
        id: &str,
        query: &str,
        plan_json: &str,
        stats_json: &str,
        answer: Option<&str>,
        prompt_version: Option<&str>,
        model_name: Option<&str>,
        object_keys_scores: &[(String, f64)],
    ) -> Result<()> {
        let id = id.to_string();
        let query = query.to_string();
        let plan_json = plan_json.to_string();
        let stats_json = stats_json.to_string();
        let answer = answer.map(|s| s.to_string());
        let prompt_version = prompt_version.map(|s| s.to_string());
        let model_name = model_name.map(|s| s.to_string());
        let now = chrono::Utc::now().to_rfc3339();
        let keys_scores = object_keys_scores.to_vec();

        self.call(move |conn| {
            conn.execute(
                "INSERT INTO search_runs (id, query, plan_json, stats_json, answer_markdown, prompt_version, model_name, created_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                     answer_markdown = excluded.answer_markdown,
                     stats_json = excluded.stats_json,
                     completed_at = excluded.completed_at",
                rusqlite::params![
                    &id,
                    &query,
                    &plan_json,
                    &stats_json,
                    answer.as_deref(),
                    prompt_version.as_deref(),
                    model_name.as_deref(),
                    &now
                ],
            )?;

            let mut obj_stmt = conn.prepare(
                "INSERT OR REPLACE INTO search_run_objects (search_run_id, object_key, relevance_score, rank)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;

            for (idx, (k, score)) in keys_scores.iter().enumerate() {
                obj_stmt.execute(rusqlite::params![&id, k, *score, idx as i64])?;
            }

            Ok(())
        })
        .await
    }

    /// Retrieve search run details
    pub async fn get_search_run(&self, id: &str) -> Result<Option<SearchRunDetails>> {
        let id = id.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, query, plan_json, stats_json, answer_markdown, prompt_version, model_name, created_at, completed_at
                 FROM search_runs WHERE id = ?1",
            )?;

            let res = stmt.query_row(rusqlite::params![&id], |row| {
                let id: String = row.get(0)?;
                let query: String = row.get(1)?;
                let plan_json: String = row.get(2)?;
                let stats_json: String = row.get(3)?;
                let answer_markdown: Option<String> = row.get(4)?;
                let prompt_version: Option<String> = row.get(5)?;
                let model_name: Option<String> = row.get(6)?;
                let created_at: String = row.get(7)?;
                let completed_at: Option<String> = row.get(8)?;

                Ok(SearchRunRecord {
                    id,
                    query,
                    plan_json,
                    stats_json,
                    answer_markdown,
                    prompt_version,
                    model_name,
                    created_at,
                    completed_at,
                })
            });

            let record = match res {
                Ok(r) => r,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(e) => return Err(e.into()),
            };

            let mut obj_stmt = conn.prepare(
                "SELECT s.object_key, s.relevance_score, s.rank, o.title, o.state, o.url, o.source
                 FROM search_run_objects s
                 LEFT JOIN objects o ON o.object_key = s.object_key
                 WHERE s.search_run_id = ?1
                 ORDER BY s.rank ASC",
            )?;

            let obj_rows = obj_stmt.query_map(rusqlite::params![&record.id], |row| {
                let object_key: String = row.get(0)?;
                let relevance_score: Option<f64> = row.get(1)?;
                let rank: Option<i64> = row.get(2)?;
                let title: Option<String> = row.get(3)?;
                let state: Option<String> = row.get(4)?;
                let url: Option<String> = row.get(5)?;
                let source: Option<String> = row.get(6)?;

                Ok(SearchRunObject {
                    object_key,
                    relevance_score: relevance_score.unwrap_or(0.0),
                    rank: rank.unwrap_or(0),
                    title: title.unwrap_or_default(),
                    state: state.unwrap_or_default(),
                    url: url.unwrap_or_default(),
                    source: source.unwrap_or_default(),
                })
            })?;

            let mut objects = Vec::new();
            for item in obj_rows {
                objects.push(item?);
            }

            Ok(Some(SearchRunDetails {
                run: record,
                objects,
            }))
        })
        .await
    }

    /// List recent search runs
    pub async fn list_search_runs(&self, limit: usize) -> Result<Vec<SearchRunRecord>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, query, plan_json, stats_json, answer_markdown, prompt_version, model_name, created_at, completed_at
                 FROM search_runs
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )?;

            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(SearchRunRecord {
                    id: row.get(0)?,
                    query: row.get(1)?,
                    plan_json: row.get(2)?,
                    stats_json: row.get(3)?,
                    answer_markdown: row.get(4)?,
                    prompt_version: row.get(5)?,
                    model_name: row.get(6)?,
                    created_at: row.get(7)?,
                    completed_at: row.get(8)?,
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

    /// Retrieve Memory & Database diagnostics (SPEC.md §42)
    pub async fn get_memory_diagnostics(&self, db_path: &Path) -> Result<MemoryDiagnostics> {
        let file_size_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        let path_str = db_path.to_string_lossy().to_string();

        self.call(move |conn| {
            let page_count: i64 = conn
                .query_row("PRAGMA page_count", [], |r| r.get(0))
                .unwrap_or(0);
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |r| r.get(0))
                .unwrap_or(4096);
            let free_pages: i64 = conn
                .query_row("PRAGMA freelist_count", [], |r| r.get(0))
                .unwrap_or(0);
            let schema_version: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap_or(CURRENT_SCHEMA_VERSION);

            let total_objects: i64 = conn
                .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))
                .unwrap_or(0);
            let total_relations: i64 = conn
                .query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))
                .unwrap_or(0);
            let total_embeddings: i64 = conn
                .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
                .unwrap_or(0);
            let stale_embeddings: i64 = conn
                .query_row("SELECT COUNT(*) FROM embeddings WHERE stale = 1", [], |r| {
                    r.get(0)
                })
                .unwrap_or(0);
            let unresolved_refs: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM unresolved_refs WHERE resolved = 0",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let total_search_runs: i64 = conn
                .query_row("SELECT COUNT(*) FROM search_runs", [], |r| r.get(0))
                .unwrap_or(0);

            let oldest_object: Option<String> = conn
                .query_row(
                    "SELECT MIN(created_at) FROM objects WHERE created_at IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .ok();
            let newest_object: Option<String> = conn
                .query_row(
                    "SELECT MAX(created_at) FROM objects WHERE created_at IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .ok();
            let last_write: Option<String> = conn
                .query_row("SELECT MAX(last_fetched_at) FROM objects", [], |r| r.get(0))
                .ok();

            // Count by source
            let mut by_source = std::collections::HashMap::new();
            let mut src_stmt =
                conn.prepare("SELECT source, COUNT(*) FROM objects GROUP BY source")?;
            let src_rows = src_stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            for item in src_rows {
                let (src, cnt): (String, i64) = item?;
                by_source.insert(src, cnt);
            }

            // Count by object_type
            let mut by_type = std::collections::HashMap::new();
            let mut type_stmt =
                conn.prepare("SELECT object_type, COUNT(*) FROM objects GROUP BY object_type")?;
            let type_rows = type_stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            for item in type_rows {
                let (tp, cnt): (String, i64) = item?;
                by_type.insert(tp, cnt);
            }

            Ok(MemoryDiagnostics {
                database_file: path_str,
                file_size_bytes,
                page_count,
                page_size,
                free_pages,
                schema_version,
                total_objects,
                objects_by_source: by_source,
                objects_by_type: by_type,
                total_relations,
                total_embeddings,
                stale_embeddings,
                unresolved_refs,
                total_search_runs,
                oldest_object,
                newest_object,
                last_write,
            })
        })
        .await
    }

    /// Retrieve Sources Summary (SPEC.md §44)
    pub async fn get_sources_summary(&self) -> Result<Vec<SourceContainerSummary>> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source, container, COUNT(*) as obj_count,
                        MAX(updated_at) as last_updated,
                        MIN(first_seen_at) as first_seen
                 FROM objects
                 GROUP BY source, container
                 ORDER BY source ASC, obj_count DESC",
            )?;

            let rows = stmt.query_map([], |row| {
                let source: String = row.get(0)?;
                let container: String = row.get(1)?;
                let object_count: i64 = row.get(2)?;
                let last_updated: Option<String> = row.get(3)?;
                let first_seen: Option<String> = row.get(4)?;

                Ok(SourceContainerSummary {
                    source,
                    container,
                    object_count,
                    last_updated,
                    first_seen,
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

    /// Run SQLite integrity check (SPEC.md §42)
    pub async fn check_integrity(&self) -> Result<String> {
        self.call(|conn| {
            let res: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            Ok(res)
        })
        .await
    }

    /// Optimize SQLite database (VACUUM and ANALYZE) (SPEC.md §43)
    pub async fn optimize_database(&self) -> Result<()> {
        self.call(|conn| {
            conn.execute_batch("VACUUM; ANALYZE;")?;
            Ok(())
        })
        .await
    }

    /// Rebuild full-text search index (SPEC.md §43)
    pub async fn rebuild_fts(&self) -> Result<()> {
        self.call(|conn| {
            conn.execute_batch(
                "DELETE FROM object_fts;
                 INSERT INTO object_fts(object_key, title, snippet, body, container, author)
                 SELECT object_key, title, snippet, COALESCE(body, ''), container, author FROM objects;",
            )?;
            Ok(())
        })
        .await
    }

    /// Clear search history (SPEC.md §43)
    pub async fn clear_search_history(&self) -> Result<usize> {
        self.call(|conn| {
            conn.execute("DELETE FROM search_run_objects", [])?;
            let affected = conn.execute("DELETE FROM search_runs", [])?;
            Ok(affected)
        })
        .await
    }

    /// Purge stale objects older than given cutoff
    pub async fn purge_stale_objects(&self, cutoff_rfc3339: &str) -> Result<usize> {
        let cutoff = cutoff_rfc3339.to_string();
        self.call(move |conn| {
            let affected = conn.execute(
                "DELETE FROM objects WHERE last_fetched_at < ?1",
                rusqlite::params![&cutoff],
            )?;
            Ok(affected)
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Introspection Methods
    // -----------------------------------------------------------------------

    pub async fn save_introspection_run(&self, run: IntrospectionRunRecord) -> Result<()> {
        self.call(move |conn| {
            conn.execute(
                "INSERT INTO introspection_runs (
                    id, created_at, question, question_fingerprint, trigger_reason,
                    search_run_id, accuracy, relevance, coverage, traceability,
                    conciseness, weighted_score, percentage, result, interpretation,
                    answer, problem_areas_json, recommendation, evidence_keys_json, evaluation_details_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                rusqlite::params![
                    &run.id,
                    &run.created_at,
                    &run.question,
                    &run.question_fingerprint,
                    &run.trigger_reason,
                    run.search_run_id.as_deref(),
                    run.accuracy,
                    run.relevance,
                    run.coverage,
                    run.traceability,
                    run.conciseness,
                    run.weighted_score,
                    run.percentage,
                    &run.result,
                    &run.interpretation,
                    &run.answer,
                    &run.problem_areas_json,
                    &run.recommendation,
                    &run.evidence_keys_json,
                    &run.evaluation_details_json,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_introspection_run(&self, id: &str) -> Result<Option<IntrospectionRunRecord>> {
        let id = id.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, created_at, question, question_fingerprint, trigger_reason,
                        search_run_id, accuracy, relevance, coverage, traceability,
                        conciseness, weighted_score, percentage, result, interpretation,
                        answer, problem_areas_json, recommendation, evidence_keys_json, evaluation_details_json
                 FROM introspection_runs WHERE id = ?1",
            )?;

            let res = stmt.query_row(rusqlite::params![&id], |row| {
                Ok(IntrospectionRunRecord {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    question: row.get(2)?,
                    question_fingerprint: row.get(3)?,
                    trigger_reason: row.get(4)?,
                    search_run_id: row.get(5)?,
                    accuracy: row.get(6)?,
                    relevance: row.get(7)?,
                    coverage: row.get(8)?,
                    traceability: row.get(9)?,
                    conciseness: row.get(10)?,
                    weighted_score: row.get(11)?,
                    percentage: row.get(12)?,
                    result: row.get(13)?,
                    interpretation: row.get(14)?,
                    answer: row.get(15)?,
                    problem_areas_json: row.get(16)?,
                    recommendation: row.get(17)?,
                    evidence_keys_json: row.get(18)?,
                    evaluation_details_json: row.get(19)?,
                })
            });

            match res {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    pub async fn list_introspection_runs(
        &self,
        limit: usize,
    ) -> Result<Vec<IntrospectionRunRecord>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, created_at, question, question_fingerprint, trigger_reason,
                        search_run_id, accuracy, relevance, coverage, traceability,
                        conciseness, weighted_score, percentage, result, interpretation,
                        answer, problem_areas_json, recommendation, evidence_keys_json, evaluation_details_json
                 FROM introspection_runs ORDER BY created_at DESC LIMIT ?1",
            )?;

            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(IntrospectionRunRecord {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    question: row.get(2)?,
                    question_fingerprint: row.get(3)?,
                    trigger_reason: row.get(4)?,
                    search_run_id: row.get(5)?,
                    accuracy: row.get(6)?,
                    relevance: row.get(7)?,
                    coverage: row.get(8)?,
                    traceability: row.get(9)?,
                    conciseness: row.get(10)?,
                    weighted_score: row.get(11)?,
                    percentage: row.get(12)?,
                    result: row.get(13)?,
                    interpretation: row.get(14)?,
                    answer: row.get(15)?,
                    problem_areas_json: row.get(16)?,
                    recommendation: row.get(17)?,
                    evidence_keys_json: row.get(18)?,
                    evaluation_details_json: row.get(19)?,
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

    pub async fn count_introspection_runs_today(&self) -> Result<u32> {
        let today_utc = chrono::Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .to_rfc3339();

        self.call(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM introspection_runs WHERE created_at >= ?1",
                rusqlite::params![&today_utc],
                |r| r.get(0),
            )?;
            Ok(count as u32)
        })
        .await
    }

    pub async fn get_last_introspection_run(&self) -> Result<Option<IntrospectionRunRecord>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, created_at, question, question_fingerprint, trigger_reason,
                        search_run_id, accuracy, relevance, coverage, traceability,
                        conciseness, weighted_score, percentage, result, interpretation,
                        answer, problem_areas_json, recommendation, evidence_keys_json, evaluation_details_json
                 FROM introspection_runs ORDER BY created_at DESC LIMIT 1",
            )?;

            let res = stmt.query_row([], |row| {
                Ok(IntrospectionRunRecord {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    question: row.get(2)?,
                    question_fingerprint: row.get(3)?,
                    trigger_reason: row.get(4)?,
                    search_run_id: row.get(5)?,
                    accuracy: row.get(6)?,
                    relevance: row.get(7)?,
                    coverage: row.get(8)?,
                    traceability: row.get(9)?,
                    conciseness: row.get(10)?,
                    weighted_score: row.get(11)?,
                    percentage: row.get(12)?,
                    result: row.get(13)?,
                    interpretation: row.get(14)?,
                    answer: row.get(15)?,
                    problem_areas_json: row.get(16)?,
                    recommendation: row.get(17)?,
                    evidence_keys_json: row.get(18)?,
                    evaluation_details_json: row.get(19)?,
                })
            });

            match res {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    pub async fn has_question_fingerprint_recently(
        &self,
        fingerprint: &str,
        days: i64,
    ) -> Result<bool> {
        let fingerprint = fingerprint.to_string();
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        self.call(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM introspection_runs WHERE question_fingerprint = ?1 AND created_at >= ?2",
                rusqlite::params![&fingerprint, &cutoff],
                |r| r.get(0),
            )?;
            Ok(count > 0)
        })
        .await
    }

    pub async fn get_introspection_stats(&self) -> Result<IntrospectionStatsSummary> {
        let seven_days_ago = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let today_utc = chrono::Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .to_rfc3339();

        self.call(move |conn| {
            let (avg_score, total_count, pass_count, weak_count, fail_count): (
                Option<f64>,
                i64,
                i64,
                i64,
                i64,
            ) = conn.query_row(
                "SELECT
                    AVG(weighted_score),
                    COUNT(*),
                    COALESCE(SUM(CASE WHEN result = 'PASS' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN result = 'WEAK' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN result = 'FAIL' THEN 1 ELSE 0 END), 0)
                 FROM introspection_runs
                 WHERE created_at >= ?1",
                rusqlite::params![&seven_days_ago],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;

            let runs_today: i64 = conn.query_row(
                "SELECT COUNT(*) FROM introspection_runs WHERE created_at >= ?1",
                rusqlite::params![&today_utc],
                |r| r.get(0),
            )?;

            let last_run_at: Option<String> = conn
                .query_row(
                    "SELECT created_at FROM introspection_runs ORDER BY created_at DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .ok();

            Ok(IntrospectionStatsSummary {
                seven_day_average: avg_score.unwrap_or(0.0),
                pass_count: pass_count as u32,
                weak_count: weak_count as u32,
                fail_count: fail_count as u32,
                total_count: total_count as u32,
                runs_today: runs_today as u32,
                last_run_at,
            })
        })
        .await
    }

    pub async fn get_introspection_trends(
        &self,
        days: usize,
    ) -> Result<Vec<IntrospectionDailyBucket>> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT
                    substr(created_at, 1, 10) as day,
                    COUNT(*),
                    AVG(weighted_score),
                    AVG(accuracy),
                    AVG(relevance),
                    AVG(coverage),
                    AVG(traceability),
                    COALESCE(SUM(CASE WHEN result = 'PASS' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN result = 'WEAK' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN result = 'FAIL' THEN 1 ELSE 0 END), 0)
                 FROM introspection_runs
                 WHERE created_at >= ?1
                 GROUP BY day
                 ORDER BY day ASC",
            )?;

            let rows = stmt.query_map(rusqlite::params![&cutoff], |r| {
                Ok(IntrospectionDailyBucket {
                    date: r.get(0)?,
                    count: r.get::<_, i64>(1)? as u32,
                    average_score: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    average_accuracy: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    average_relevance: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                    average_coverage: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    average_traceability: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                    pass_count: r.get::<_, i64>(7)? as u32,
                    weak_count: r.get::<_, i64>(8)? as u32,
                    fail_count: r.get::<_, i64>(9)? as u32,
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

    pub async fn get_introspection_recent_objects(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String, String, String)>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT object_key, title, source, container, state FROM objects ORDER BY updated_at DESC LIMIT ?1"
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            let mut out = Vec::new();
            for r in rows { out.push(r?); }
            Ok(out)
        }).await
    }

    pub async fn get_introspection_low_confidence_relations(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String, f64)>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT from_key, to_key, relation_type, confidence FROM relations WHERE confidence < 0.85 ORDER BY confidence ASC LIMIT ?1"
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            let mut out = Vec::new();
            for r in rows { out.push(r?); }
            Ok(out)
        }).await
    }

    pub async fn get_introspection_unresolved_refs(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT target_id, discovered_from_key, source FROM unresolved_refs WHERE resolved = 0 LIMIT ?1"
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            let mut out = Vec::new();
            for r in rows { out.push(r?); }
            Ok(out)
        }).await
    }

    pub async fn get_introspection_weak_or_fail_runs(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT question, result, problem_areas_json FROM introspection_runs WHERE result IN ('WEAK', 'FAIL') ORDER BY created_at DESC LIMIT ?1"
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            let mut out = Vec::new();
            for r in rows { out.push(r?); }
            Ok(out)
        }).await
    }

    pub async fn get_introspection_recent_searches(&self, limit: usize) -> Result<Vec<String>> {
        self.call(move |conn| {
            let mut stmt =
                conn.prepare("SELECT query FROM search_runs ORDER BY created_at DESC LIMIT ?1")?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |r| r.get(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    pub async fn get_revisions_for_key(&self, key: &str) -> Result<Vec<RevisionRecord>> {
        let k = key.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, object_key, content_hash, captured_at, title, snippet, body, state, native_json
                 FROM revisions WHERE object_key = ?1 ORDER BY captured_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![&k], |row| {
                Ok(RevisionRecord {
                    id: row.get(0)?,
                    object_key: row.get(1)?,
                    content_hash: row.get(2)?,
                    captured_at: row.get(3)?,
                    title: row.get(4)?,
                    snippet: row.get(5)?,
                    body: row.get(6)?,
                    state: row.get(7)?,
                    native_json: row.get(8)?,
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

    pub async fn get_all_revisions(&self, limit: usize) -> Result<Vec<RevisionRecord>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, object_key, content_hash, captured_at, title, snippet, body, state, native_json
                 FROM revisions ORDER BY captured_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(RevisionRecord {
                    id: row.get(0)?,
                    object_key: row.get(1)?,
                    content_hash: row.get(2)?,
                    captured_at: row.get(3)?,
                    title: row.get(4)?,
                    snippet: row.get(5)?,
                    body: row.get(6)?,
                    state: row.get(7)?,
                    native_json: row.get(8)?,
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

    pub async fn get_all_objects(&self) -> Result<Vec<crate::connectors::Evidence>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT object_key, source, object_type, external_id,
                        title, snippet, body, url, container, author,
                        state, created_at, updated_at, native_json
                 FROM objects ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
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
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    pub async fn get_objects_by_container(
        &self,
        container: &str,
    ) -> Result<Vec<crate::connectors::Evidence>> {
        let c = container.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT object_key, source, object_type, external_id,
                        title, snippet, body, url, container, author,
                        state, created_at, updated_at, native_json
                 FROM objects WHERE container = ?1 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map(rusqlite::params![&c], |row| {
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
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    pub async fn get_all_relations(&self) -> Result<Vec<crate::connectors::Relation>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT from_key, to_key, relation_type, detected_by, confidence FROM relations",
            )?;
            let rows = stmt.query_map([], |row| {
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

    // Watchlists
    pub async fn save_watchlist(&self, item: WatchlistRecord) -> Result<()> {
        self.call(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO watchlists (id, target_type, target_value, title, created_at, last_checked_at, last_snapshot_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    item.id,
                    item.target_type,
                    item.target_value,
                    item.title,
                    item.created_at,
                    item.last_checked_at,
                    item.last_snapshot_json,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_watchlist(&self, id: &str) -> Result<Option<WatchlistRecord>> {
        let id_str = id.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, target_type, target_value, title, created_at, last_checked_at, last_snapshot_json
                 FROM watchlists WHERE id = ?1",
            )?;
            let res = stmt.query_row(rusqlite::params![&id_str], |row| {
                Ok(WatchlistRecord {
                    id: row.get(0)?,
                    target_type: row.get(1)?,
                    target_value: row.get(2)?,
                    title: row.get(3)?,
                    created_at: row.get(4)?,
                    last_checked_at: row.get(5)?,
                    last_snapshot_json: row.get(6)?,
                })
            });
            match res {
                Ok(w) => Ok(Some(w)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(anyhow::anyhow!(e)),
            }
        })
        .await
    }

    pub async fn list_watchlists(&self) -> Result<Vec<WatchlistRecord>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, target_type, target_value, title, created_at, last_checked_at, last_snapshot_json
                 FROM watchlists ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(WatchlistRecord {
                    id: row.get(0)?,
                    target_type: row.get(1)?,
                    target_value: row.get(2)?,
                    title: row.get(3)?,
                    created_at: row.get(4)?,
                    last_checked_at: row.get(5)?,
                    last_snapshot_json: row.get(6)?,
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

    pub async fn delete_watchlist(&self, id: &str) -> Result<bool> {
        let id_str = id.to_string();
        self.call(move |conn| {
            let n = conn.execute(
                "DELETE FROM watchlists WHERE id = ?1",
                rusqlite::params![&id_str],
            )?;
            Ok(n > 0)
        })
        .await
    }

    pub async fn update_watchlist_snapshot(
        &self,
        id: &str,
        snapshot_json: &str,
        last_checked_at: &str,
    ) -> Result<()> {
        let id_str = id.to_string();
        let snap = snapshot_json.to_string();
        let checked = last_checked_at.to_string();
        self.call(move |conn| {
            conn.execute(
                "UPDATE watchlists SET last_snapshot_json = ?1, last_checked_at = ?2 WHERE id = ?3",
                rusqlite::params![snap, checked, id_str],
            )?;
            Ok(())
        })
        .await
    }

    // Saved Investigations
    pub async fn save_investigation(&self, item: SavedInvestigationRecord) -> Result<()> {
        self.call(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO saved_investigations (id, title, query, created_at, updated_at, conclusions_markdown, evidence_keys_json, snapshot_json, last_reopened_at, tags)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    item.id,
                    item.title,
                    item.query,
                    item.created_at,
                    item.updated_at,
                    item.conclusions_markdown,
                    item.evidence_keys_json,
                    item.snapshot_json,
                    item.last_reopened_at,
                    item.tags,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_investigation(&self, id: &str) -> Result<Option<SavedInvestigationRecord>> {
        let id_str = id.to_string();
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, query, created_at, updated_at, conclusions_markdown, evidence_keys_json, snapshot_json, last_reopened_at, tags
                 FROM saved_investigations WHERE id = ?1",
            )?;
            let res = stmt.query_row(rusqlite::params![&id_str], |row| {
                Ok(SavedInvestigationRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    query: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    conclusions_markdown: row.get(5)?,
                    evidence_keys_json: row.get(6)?,
                    snapshot_json: row.get(7)?,
                    last_reopened_at: row.get(8)?,
                    tags: row.get(9)?,
                })
            });
            match res {
                Ok(inv) => Ok(Some(inv)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(anyhow::anyhow!(e)),
            }
        })
        .await
    }

    pub async fn list_investigations(&self) -> Result<Vec<SavedInvestigationRecord>> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, query, created_at, updated_at, conclusions_markdown, evidence_keys_json, snapshot_json, last_reopened_at, tags
                 FROM saved_investigations ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(SavedInvestigationRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    query: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    conclusions_markdown: row.get(5)?,
                    evidence_keys_json: row.get(6)?,
                    snapshot_json: row.get(7)?,
                    last_reopened_at: row.get(8)?,
                    tags: row.get(9)?,
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

    pub async fn delete_investigation(&self, id: &str) -> Result<bool> {
        let id_str = id.to_string();
        self.call(move |conn| {
            let n = conn.execute(
                "DELETE FROM saved_investigations WHERE id = ?1",
                rusqlite::params![&id_str],
            )?;
            Ok(n > 0)
        })
        .await
    }

    pub async fn update_investigation_reopened(&self, id: &str, reopened_at: &str) -> Result<()> {
        let id_str = id.to_string();
        let reopened = reopened_at.to_string();
        self.call(move |conn| {
            conn.execute(
                "UPDATE saved_investigations SET last_reopened_at = ?1 WHERE id = ?2",
                rusqlite::params![reopened, id_str],
            )?;
            Ok(())
        })
        .await
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FtsSearchResult {
    pub evidence: crate::connectors::Evidence,
    pub snippet: String,
    pub rank: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphNode {
    pub r#ref: crate::connectors::ObjectRef,
    pub evidence: Option<crate::connectors::Evidence>,
    pub depth: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<crate::connectors::Relation>,
    pub budget_exhausted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatastoreStats {
    pub object_count: i64,
    pub relation_count: i64,
    pub revision_count: i64,
    pub unresolved_count: i64,
    pub audit_count: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchRunRecord {
    pub id: String,
    pub query: String,
    pub plan_json: String,
    pub stats_json: String,
    pub answer_markdown: Option<String>,
    pub prompt_version: Option<String>,
    pub model_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchRunObject {
    pub object_key: String,
    pub relevance_score: f64,
    pub rank: i64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub source: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchRunDetails {
    pub run: SearchRunRecord,
    pub objects: Vec<SearchRunObject>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryDiagnostics {
    pub database_file: String,
    pub file_size_bytes: u64,
    pub page_count: i64,
    pub page_size: i64,
    pub free_pages: i64,
    pub schema_version: i64,
    pub total_objects: i64,
    pub objects_by_source: std::collections::HashMap<String, i64>,
    pub objects_by_type: std::collections::HashMap<String, i64>,
    pub total_relations: i64,
    pub total_embeddings: i64,
    pub stale_embeddings: i64,
    pub unresolved_refs: i64,
    pub total_search_runs: i64,
    pub oldest_object: Option<String>,
    pub newest_object: Option<String>,
    pub last_write: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceContainerSummary {
    pub source: String,
    pub container: String,
    pub object_count: i64,
    pub last_updated: Option<String>,
    pub first_seen: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntrospectionRunRecord {
    pub id: String,
    pub created_at: String,
    pub question: String,
    pub question_fingerprint: String,
    pub trigger_reason: String,
    pub search_run_id: Option<String>,
    pub accuracy: f64,
    pub relevance: f64,
    pub coverage: f64,
    pub traceability: f64,
    pub conciseness: f64,
    pub weighted_score: f64,
    pub percentage: f64,
    pub result: String,
    pub interpretation: String,
    pub answer: String,
    pub problem_areas_json: String,
    pub recommendation: String,
    pub evidence_keys_json: String,
    pub evaluation_details_json: String,
}

impl IntrospectionRunRecord {
    pub fn problem_areas(&self) -> Vec<String> {
        serde_json::from_str(&self.problem_areas_json).unwrap_or_default()
    }

    pub fn evidence_keys(&self) -> Vec<String> {
        serde_json::from_str(&self.evidence_keys_json).unwrap_or_default()
    }

    pub fn evaluation_details(&self) -> serde_json::Value {
        serde_json::from_str(&self.evaluation_details_json).unwrap_or_default()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntrospectionStatsSummary {
    pub seven_day_average: f64,
    pub pass_count: u32,
    pub weak_count: u32,
    pub fail_count: u32,
    pub total_count: u32,
    pub runs_today: u32,
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntrospectionDailyBucket {
    pub date: String,
    pub count: u32,
    pub average_score: f64,
    pub average_accuracy: f64,
    pub average_relevance: f64,
    pub average_coverage: f64,
    pub average_traceability: f64,
    pub pass_count: u32,
    pub weak_count: u32,
    pub fail_count: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RevisionRecord {
    pub id: i64,
    pub object_key: String,
    pub content_hash: String,
    pub captured_at: String,
    pub title: String,
    pub snippet: String,
    pub body: Option<String>,
    pub state: String,
    pub native_json: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WatchlistRecord {
    pub id: String,
    pub target_type: String,
    pub target_value: String,
    pub title: String,
    pub created_at: String,
    pub last_checked_at: Option<String>,
    pub last_snapshot_json: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedInvestigationRecord {
    pub id: String,
    pub title: String,
    pub query: String,
    pub created_at: String,
    pub updated_at: String,
    pub conclusions_markdown: String,
    pub evidence_keys_json: String,
    pub snapshot_json: String,
    pub last_reopened_at: Option<String>,
    pub tags: Option<String>,
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
