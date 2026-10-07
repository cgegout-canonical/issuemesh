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
            let clean_tokens: Vec<String> = q
                .split_whitespace()
                .map(|token| {
                    let cleaned: String = token.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
                    if cleaned.is_empty() {
                        String::new()
                    } else {
                        format!("\"{}\"", cleaned)
                    }
                })
                .filter(|s| !s.is_empty())
                .collect();

            if clean_tokens.is_empty() {
                return Ok(Vec::new());
            }

            let fts_query = clean_tokens.join(" ");

            let mut stmt = conn.prepare(
                "SELECT f.object_key,
                        snippet(object_fts, 1, '<b>', '</b>', '...', 16) AS title_snip,
                        snippet(object_fts, 3, '<b>', '</b>', '...', 32) AS body_snip,
                        bm25(object_fts) AS rank,
                        o.title, o.snippet, o.body, o.url, o.container, o.author, o.state,
                        o.source, o.object_type, o.external_id, o.created_at, o.updated_at, o.native_json
                 FROM object_fts f
                 JOIN objects o ON o.object_key = f.object_key
                 WHERE object_fts MATCH ?1
                 ORDER BY rank ASC
                 LIMIT ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![&fts_query, limit as i64], |row| {
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
