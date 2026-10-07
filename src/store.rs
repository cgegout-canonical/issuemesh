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
