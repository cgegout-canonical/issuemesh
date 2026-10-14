# IssueMesh Architecture

This document outlines the core architectural boundaries, security invariants, data models, and processing pipelines in IssueMesh.

---

## 1. Connector Boundary & Read-Only Invariant

A foundational guarantee of IssueMesh is **absolute read-only operation** against external bug tracking platforms (GitHub, Jira, Launchpad).

### Read-Only Client Design
- External HTTP communication is routed through dedicated clients (`GitHubClient`, `JiraClient`, `LaunchpadClient`) built on a hardened internal request wrapper.
- Tracker clients strictly prohibit mutating HTTP verbs:
  - `POST` is restricted exclusively to allowlisted read-only search endpoints (e.g. Jira `/rest/api/3/search` or `/rest/api/2/search`).
  - `PUT`, `PATCH`, and `DELETE` methods are completely absent from tracker clients and barred by construction.
- Verification is enforced both by Rust type safety and static script assertion in `scripts/verify.sh`.

### Secret Boundary & Safety
- Credentials (API tokens, passwords) are loaded exclusively into memory from `secrets.env` (permissions `0600`).
- Tokens are never persisted to the SQLite database and never output in application logs (`tracing`).
- In API responses and UI configuration forms, secret values are masked (e.g. `••••1234`).

---

## 2. Normalized Object Model

External issues, tasks, bugs, and PRs from different platforms are mapped into a unified internal representation:

- **`Issue`**:
  - `id`: Globally unique identifier (`github:owner/repo#123`, `jira:PROJ-456`, `launchpad:bug/789`).
  - `tracker`: Source system identifier (`github`, `jira`, `launchpad`).
  - `key` / `title` / `body`: Textual description and summaries.
  - `status` / `author` / `assignee`: Normalized metadata.
  - `created_at` / `updated_at`: Standardized UTC timestamps.
  - `raw_json`: Original API payload stored for lossless provenance audits.
- **`Comment`**:
  - `id`, `issue_id`, `author`, `body`, `created_at`.
- **`Relationship`**:
  - Directed edge: `source_id -> target_id`.
  - `relation_type`: `Mentions`, `Blocks`, `DependsOn`, `Duplicates`, `RelatedTo`, `Fixes`, `CausedBy`.
  - `provenance`: `ExplicitTrackerLink` (authoritative) or `ExtractedTextPattern` (inferred).
  - `confidence`: 1.0 for tracker links, 0.7 - 0.9 for regex/pattern-extracted references.

---

## 3. Local Datastore & Memory

All persistent state is stored in an embedded SQLite database using `rusqlite` with the bundled SQLite engine.

- **Schema Migration**: Managed automatically at startup via embedded SQL migrations (`src/db/migrations/`).
- **Full-Text Search (FTS5)**:
  - Virtual table `issues_fts` indexes issue keys, titles, bodies, and comment texts.
  - SQLite FTS5 triggers keep the index synchronized upon issue inserts or updates.
- **Vector Embeddings**:
  - Pre-computed embeddings for issue titles and summaries are stored as compact binary BLOBs (`f32` vectors).
  - Cosine similarity ranking executed in-process.
- **Cache Invalidation & Freshness**:
  - Trackers store an `etag` and `last_synced_at` timestamp.
  - Remote issues always supersede stale local cache entries during explicit deep traversals.

---

## 4. Graph Traversal & Expansion Engine

When an investigation query begins, the **Graph Expander** discovers connected context across tracker boundaries.

1. **Seed Discovery**: Initial candidate issues are retrieved via hybrid lexical (FTS5) and semantic (vector similarity) matching.
2. **Text Reference Extraction**:
   - Issue bodies and comments are parsed for cross-tracker references:
     - GitHub: `#123`, `owner/repo#123`, `GH-123`, commit hashes.
     - Jira: `[A-Z][A-Z0-9_]+-[0-9]+` (e.g. `PROJECT-101`).
     - Launchpad: `LP#123456`, `lp:123456`, bug URLs.
3. **Bounded Traversal**:
   - Expansion depth is strictly limited by configurable search budgets (default max depth: 2, max nodes: 50).
   - Prevents cycles and runaway network calls during high-density issue link graphs.

---

## 5. Search & Ranking Pipeline

The search execution pipeline follows a multi-stage process:

```
User Query ──► [Query Intent Parser]
                     │
         ┌───────────┴───────────┐
         ▼                       ▼
   [Local FTS5]           [Federated Remote Search]
         │                       │
         └───────────┬───────────┘
                     ▼
         [Graph Link Expansion]
                     │
                     ▼
         [Evidence Score Fusion]
         (BM25 + Recency + Link Depth + Semantic)
                     │
                     ▼
         [Consistency Check (L1/L2)]
                     │
                     ▼
         [Deterministic Synthesis / LLM]
```

- **Query Intent Parsing**: Identifies explicit issue IDs (e.g., `LP#123456`), tracker filters, author filters, and free-form natural language terms.
- **Evidence Score Fusion**: Ranks evidence items using a composite weighted score combining lexical relevance (BM25), vector similarity (cosine score), relationship distance, and update recency.
- **Deterministic Synthesis First**: Authoritative tracker status and links take precedence over any probabilistic model inference.

---

## 6. Model & LLM Boundary

- **Optional & Pluggable**: Synthesis and embedding modules connect to OpenAI-compatible endpoints (local Ollama, vLLM, or remote cloud providers).
- **Graceful Offline Fallback**: If the model server is unreachable, IssueMesh falls back immediately to deterministic heuristic summaries and BM25 graph evidence ranking without throwing errors.
- **Zero Hallucination Anchor**: Synthesis prompts explicitly cite object identifiers (`github:foo#12`, `jira:BAR-20`). The UI links citations directly to verified tracker URLs.

---

## 7. Native Concept Layer & Derived Knowledge Invariant

The **Native Concept Layer** extracts, links, and indexes domain-specific concepts across disparate tracker silos without altering authoritative state:

### Storage Schema (v4 Migrations)
- **`concepts`**: Canonical concepts with `name`, `kind` (`component`, `feature`, `technology`, `problem`, `release`, `team`, `other`), `object_count`, `first_seen`, `last_seen`.
- **`object_concepts`**: Association table linking objects to concepts with extraction weight `[0.0, 1.0]`, source field (`title`, `body`, `comments`), content hash, and timestamp.
- **`concept_links`**: Co-occurrence graph representing pairings of concepts co-located on the same engineering objects (`a < b` invariant).
- **`concept_index`**: Object indexing ledger (`pending`, `done`, `failed`, `skipped`) with attempt counts, token tracking, and error taxonomy.
- **`concepts_fts`**: SQLite FTS5 index over concept names for sub-millisecond lexical prefix and keyword discovery.

### Invariants & Non-Authoritative Guardrails
1. **Authoritative Primacy**: SQLite tracker objects and native relationships (`jira:native`, `github:native`) remain authoritative. Concepts are strictly **derived knowledge**.
2. **Capped Confidence**: Concept-inferred relationships use `detected_by = "concept"` and are capped at confidence $\le 0.70$ (e.g. `same_problem` relations use `0.60`). Native tracker relationships retain $1.00$ and strictly outrank derived links.
3. **Freshness Citation Barrier**: A concept match is only cited after resolving to an authoritative object that has passed standard freshness verification.
4. **Hybrid Ranking Fusion**: When concepts are enabled, semantic scores for candidates are computed as $\max(\text{embedding\_cosine}, \text{concept\_overlap\_score})$.
5. **Decoupled Asynchronous Worker**: Background token-budgeted queue (`tokio::sync::mpsc`) extracts concepts via versioned prompt `concept_extract_v1`. If the LLM is unreachable or disabled, extraction jobs queue gracefully while search, workbench, MCP tools, and APIs continue serving existing indices.

