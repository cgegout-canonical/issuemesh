-- IssueMesh SQLite Datastore Schema (Version 2)
-- Local-first engineering intelligence store for GitHub, Jira, and Launchpad

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

-- Core engineering evidence objects (issues, PRs, bugs, discussions)
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

-- Full-Text Search (FTS5) index with Porter stemmer
CREATE VIRTUAL TABLE IF NOT EXISTS object_fts USING fts5(
    object_key UNINDEXED,
    title,
    snippet,
    body,
    container,
    author,
    tokenize='porter unicode61'
);

-- Inter-object relationship graph (blocks, fixes, mentions, references, parent-child)
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

-- Cross-tracker references pending resolution or inaccessible (403/404)
CREATE TABLE IF NOT EXISTS unresolved_refs (
    ref_key TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    target_id TEXT NOT NULL,
    discovered_from_key TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0
);

-- Historical snapshots of objects over time
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

-- Dense vector embeddings for semantic similarity search
CREATE TABLE IF NOT EXISTS embeddings (
    object_key TEXT PRIMARY KEY,
    embedding_blob BLOB NOT NULL,
    dimension INTEGER NOT NULL,
    model_name TEXT NOT NULL,
    stale INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

-- Search execution history and audit logs
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

-- API call auditing and latency metrics
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

-- Introspection autonomous evaluation runs
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

-- Watchlists for tracking object, project, topic, and query changes
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

-- Saved investigations preserving research questions, evidence graphs, and conclusions
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

