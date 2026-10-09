# IssueMesh

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-linux%20%7C%20macos-lightgrey.svg)]()
[![Snap](https://img.shields.io/badge/snap-strict%20confinement-green.svg)](SNAP_PACKAGING.md)

**IssueMesh** is a local-first, memory-augmented federated search engine and engineering intelligence workbench. It bridges information silos across **GitHub**, **Jira**, and **Launchpad**, constructing an interconnected cross-system graph of issues, pull requests, bugs, revisions, and discussions with deterministic, evidence-based synthesis.

---

## Table of Contents

1. [Motivation & Purpose](#1-motivation--purpose)
2. [Key Capabilities](#2-key-capabilities)
3. [Architecture Overview](#3-architecture-overview)
4. [How Search & Deep Search Works](#4-how-search--deep-search-works)
5. [Build & Installation Instructions](#5-build--installation-instructions)
6. [First-Run Setup & Onboarding](#6-first-run-setup--onboarding)
7. [OpenAI-Compatible Model Discovery](#7-openai-compatible-model-discovery)
8. [Configuration & Datastore Paths](#8-configuration--datastore-paths)
9. [Security & Read-Only Guarantees](#9-security--read-only-guarantees)
10. [Memory, Sources, Health & Introspection Pages](#10-memory-sources-health--introspection-pages)
11. [Continuous Introspection Engine](#11-continuous-introspection-engine)
12. [Testing & 30-Test Validation Framework](#12-testing--30-test-validation-framework)
13. [Portability & Canonical Snap Package](#13-portability--canonical-snap-package)
14. [Known Limitations](#14-known-limitations)
15. [Developer Instructions](#15-developer-instructions)
16. [License](#16-license)

---

## 1. Motivation & Purpose

Engineering organizations frequently distribute work across fragmented tracking systems:
- **Jira** for feature planning, customer commitments, and organizational epics.
- **GitHub** for active pull requests, code reviews, and developer discussions.
- **Launchpad** for Ubuntu and upstream bug tracking, package releases, and security patches.

Navigating complex technical initiatives often requires correlating tickets across these platforms manually. Key questions—such as *"Has the feature planned in Jira landed in code?"* or *"Why is this bug marked Fix Released if the pull request is still open?"*—fall into tracking gaps.

**IssueMesh solves this by:**
- Unifying searches across GitHub, Jira, and Launchpad simultaneously.
- Traversing explicit and inferred relationships (`implements`, `fixes`, `blocks`, `relates_to`).
- Flagging cross-tracker status contradictions (e.g., Jira `Done` while GitHub PR is still `Open`).
- Providing concise, strictly evidence-backed answers (≤ 250 words) with zero hallucination.
- Retaining all query memory locally in SQLite with full-text search (FTS5) and vector embeddings.

---

## 2. Key Capabilities

- **Federated Multi-Tracker Retrieval**: Concurrently queries GitHub, Jira, and Launchpad APIs alongside local cached memory.
- **Strict Read-Only Enforcement**: Guaranteed by architecture and compile-time barriers; IssueMesh cannot modify or delete remote tracker records.
- **Deterministic Evidence Synthesis**: Constrained LLM analysis engine strictly forbidden from inventing objects, statuses, or relationships. If evidence is insufficient, it explicitly reports that limitation.
- **Automated Relationship Extraction**: Identifies cross-references from commit messages, issue descriptions, and comments (`LP#123456`, `PROJ-789`, `org/repo#123`, URLs).
- **Cross-System Consistency Engine**: Detects workflow anomalies, such as tickets closed ahead of their code implementations or divergent milestone assignments.
- **Dynamic Model Discovery**: Queries OpenAI-compatible endpoints (`/v1/models`) on credentials entry, dynamically populating normalized model dropdowns without hardcoding.
- **Local-First & Privacy Preserving**: Zero external telemetry; tokens stored in `0600`-hardened files; all data stored in local SQLite databases.
- **Zero-Dependency Single Binary**: Bundles embedded web UI, embedded SQLite migrations, and pure-Rust TLS (`rustls`).

---

## 3. Architecture Overview

IssueMesh follows a layered, modular architecture written in asynchronous Rust (`tokio`):

```text
                           ┌──────────────────────────┐
                           │   Web UI / REST API      │
                           │   (Axum + Rust-Embed)    │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │   Search Orchestrator    │
                           │   & Query Intent Planner │
                           └──────┬────────────┬──────┘
                                  │            │
             ┌────────────────────┘            └────────────────────┐
             ▼                                                      ▼
  ┌─────────────────────┐                                ┌─────────────────────┐
  │  Federated Trackers │                                │   Local Datastore   │
  │  (GitHub, Jira, LP) │                                │   (SQLite + FTS5    │
  │  [Strict Read-Only] │                                │    + Embeddings)    │
  └──────────┬──────────┘                                └──────────┬──────────┘
             │                                                      │
             └────────────────────┐            ┌────────────────────┘
                                  ▼            ▼
                           ┌──────────────────────────┐
                           │  Graph Traversal Engine  │
                           │ (Relation Deduplication) │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │  Hybrid Evidence Ranker  │
                           │  & Consistency Analyzer  │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │  Deterministic Synthesis │
                           │   Engine (< 250 words)   │
                           └──────────────────────────┘
```

### Core Modules

- **`src/web.rs`**: Axum HTTP router, Server-Sent Events (SSE) streaming, static asset serving, and REST endpoints.
- **`src/search/orchestrator.rs`**: Coordinates federated query execution, graph expansion, consistency checking, and model synthesis.
- **`src/search/ranking.rs`**: Hybrid ranking engine combining lexical (FTS5 BM25), semantic vector similarity, graph proximity, and freshness decay.
- **`src/search/consistency.rs`**: Rules-based cross-tracker conflict detection (`jira_done_pr_open`, `jira_done_lp_incomplete`, etc.).
- **`src/connectors/`**: Protocol-specific read-only connectors for `github.rs`, `jira.rs`, and `launchpad.rs`, backed by `http.rs`.
- **`src/store.rs`**: Asynchronous SQLite abstraction managing objects, relations, revisions, unresolved references, FTS5 virtual tables, and audit logs.
- **`src/model/`**: OpenAI-compatible client, dynamic model discovery, search planner, and synthesis engine.

---

## 4. How Search & Deep Search Works

Every search executed in IssueMesh goes through a rigorous seven-stage pipeline:

```text
1. Intent Planning ──► 2. Federated Fetch ──► 3. Graph Expansion ──► 4. Consistency Analysis
                                                                             │
5. Synthesis Answer ◄── 6. Context Truncation ◄── 5. Hybrid Ranking ◄────────┘
```

1. **Intent Planning**: A fast planning model analyzes the user query to determine target sources, project codes, repository hints, and semantic search variations.
2. **Federated Retrieval**: Concurrently queries enabled remote trackers (GitHub REST API, Jira REST API, Launchpad API) and the local SQLite database.
3. **Graph Expansion**: Discovered references (`LP#...`, `PROJ-...`, `#PR`) are parsed and traversed up to the configured `graph_depth` (default: 2 hops). Cyclic loops are automatically broken, and traversed nodes are deduplicated.
4. **Cross-System Consistency Checking**: Analyzes pairs of linked objects across trackers to identify workflow disconnects (e.g., Jira issue marked `Done` while linked GitHub PR is still `Open`).
5. **Hybrid Ranking**: Ranks all candidate objects using the hybrid scoring formula:
   $$\text{Score} = 0.35 \times \text{Lexical} + 0.35 \times \text{Semantic} + 0.20 \times \text{Graph} + 0.10 \times \text{Freshness}$$
   - **Lexical Score**: SQLite FTS5 BM25 match quality.
   - **Semantic Score**: Cosine similarity between query and object embeddings.
   - **Graph Score**: Proximity decay based on traversal hop distance ($1.0$ for direct hits, $0.8$ for 1-hop, $0.5$ for 2-hop).
   - **Freshness Score**: Exponential decay with a 30-day half-life: $2^{-\Delta \text{days} / 30}$.
6. **Context Management & Truncation**: Truncates excessive discussion threads and limits deep reads to prevent LLM context exhaustion.
7. **Deterministic Synthesis**: Generates a strictly evidence-based response (under 250 words). If evidence is ambiguous, contradictory, or insufficient, it states the exact gap rather than inventing facts.

---

## 5. Build & Installation Instructions

### Prerequisites

- **Linux** (x86_64 or aarch64) or **macOS**.
- **Rust Toolchain**: Rust 1.80 or newer (`cargo`, `rustc`).
- Standard C compiler and runtime (`gcc` or `clang`) for compiling bundled SQLite.

### Quick Start (Build from Source)

```bash
# Clone repository
git clone https://github.com/gegout/issuemesh.git
cd issuemesh

# Build release binary
CFLAGS="-fno-sanitize=undefined -fwrapv" cargo build --release

# Run standalone executable
./target/release/issuemesh
```

The web interface will be available at:
```text
http://127.0.0.1:8765
```

### Installation

To install `issuemesh` into your user binary path:

```bash
cargo install --path .
```

---

## 6. First-Run Setup & Onboarding

When starting IssueMesh for the first time without configured trackers:
1. IssueMesh detects missing configuration and prints the setup URL to the terminal.
2. Navigating to `http://127.0.0.1:8765` automatically redirects to `/setup`.
3. The wizard allows configuring:
   - **GitHub**: Personal Access Token (classic `repo` or fine-grained read-only).
   - **Jira**: Base URL, User Email, and API Token.
   - **Launchpad**: Enable/disable public Launchpad API access.
   - **OpenAI-Compatible LLM**: Endpoint URL, API key, and model for query planning and synthesis.
   - **Embedding Engine**: Optional embedding endpoint and model for semantic vector search.
4. Each tracker includes a **Test Connection** button validating credentials in real time.
5. All secrets are securely written to `secrets.env` with `0600` permissions.

---

## 7. OpenAI-Compatible Model Discovery

IssueMesh dynamically queries the configured AI provider's `/v1/models` endpoint as soon as an API URL and key are provided:

- **Universal Provider Support**: Supports OpenAI, OpenRouter, Ollama, LM Studio, vLLM, and LocalAI.
- **Resilient Normalization**: Automatically extracts model IDs across differing response structures (`data[].id`, `models[].name`, flat string lists).
- **Dynamic Dropdown**: Populates the Model field with available models, eliminating manual typing.
- **Real-Time Refresh**: Refreshes model options whenever the endpoint URL or API key is updated.
- **Fail-Safe Operation**: If the endpoint is unreachable or authentication fails, an error banner is displayed, the dropdown remains empty, and the application never silently falls back to hardcoded models.
- **Embedding Model Discovery**: Filters and detects embedding models when supported by the provider.

---

## 8. Configuration & Datastore Paths

IssueMesh strictly adheres to the XDG Base Directory specification:

| File / Directory | Default Location | Description |
|---|---|---|
| **Data Directory** | `~/.local/share/issuemesh/` (or `~/.issuemesh/`) | SQLite database (`issuemesh.db`), search logs, and FTS tables. |
| **Config File** | `~/.config/issuemesh/config.toml` | Tracker settings, search limits, weights, and models. |
| **Secrets File** | `~/.config/issuemesh/secrets.env` | API keys and tokens with strict `0600` POSIX permissions. |

### Environment Overrides

- `ISSUEMESH_HOME`: Base directory for both config and data (defaults to `~/.issuemesh`).
- `ISSUEMESH_DATA_DIR`: Override path for `issuemesh.db`.
- `ISSUEMESH_CONFIG_DIR`: Override path for `config.toml` and `secrets.env`.
- `ISSUEMESH_PORT`: HTTP listening port (default: `8765`).
- `ISSUEMESH_HOST`: HTTP listening address (default: `127.0.0.1`).

---

## 9. Security & Read-Only Guarantees

IssueMesh is designed for secure deployment in sensitive corporate engineering environments:

- **Architectural Read-Only Contract**: The HTTP connector (`src/connectors/http.rs`) strictly rejects unsafe HTTP methods (`PUT`, `PATCH`, `DELETE`). Only `GET` and allowlisted search `POST` endpoints are permitted.
- **Static Verification**: Continuous integration checks enforce that no write methods exist in connector code:
  ```bash
  ! grep -rn "\.put(\|\.patch(\|\.delete(\|Method::PUT\|Method::PATCH\|Method::DELETE" src/
  ```
- **Zero Secret Leakage**:
  - API keys and tokens are stored in `secrets.env` with `0600` permissions.
  - Secrets are never written to the SQLite database.
  - Web UI and REST API return all tokens masked (e.g., `••••abcd`).
  - Connector error messages redact credentials, preventing token leakage in logs.
- **Auditing**: Every external HTTP request is logged in the local `api_audit` table with timestamp, endpoint, status code, and latency.

---

## 10. Memory, Sources, Health, Introspection & Tutorial Pages

IssueMesh provides dedicated views in its embedded web frontend:

1. **Workbench (`/`)**: Main investigation interface. Live query input, SSE streaming search progression, ranked evidence cards, interactive relationship graph, consistency alerts, and synthesized answer.
2. **Memory (`/memory`)**: Local memory management. Displays cached object counts by tracker, FTS5 index size, stored revisions, and unresolved reference queues. Includes cache clear controls.
3. **Sources (`/sources`)**: Tracker status overview. Shows connection health, authentication state, base URLs, and active project/organization filters.
4. **Health (`/health`)**: Diagnostic dashboard. Real-time API latency measurements, HTTP error rates, recent API audit logs, system resource metrics, and background Introspection probe status.
5. **Introspection (`/introspection`)**: Continuous self-evaluation dashboard. Displays overall pass rates, average weighted scores across 5 dimensions, score trends, active problem areas, and recent self-challenge run histories with manual trigger capability. Complete architecture guide available at `/about-introspection`.
6. **Tutorial (`/tutorial`)**: Beginner-friendly visual guide explaining how IssueMesh works in simple terms, step-by-step setup, example queries, how to interpret evidence and contradictions, and an accessible FAQ for users unfamiliar with AI or cross-tracker engineering workflows.

---

## 11. Continuous Introspection Engine

IssueMesh continuously evaluates its own search and synthesis quality through an autonomous, evidence-driven **Introspection Engine**:

```text
       Recent Tracker Signals
 (changes, conflicts, unresolved refs)
                 │
                 ▼
     [ Question Generator ]  ──(SHA-256 Deduplication)──> Skip recent questions
                 │
                 ▼
  [ Search Orchestrator Pipeline ]
     (Live federated search)
                 │
                 ▼
   [ 5-Dimension Evaluator ]
  (Accuracy, Relevance, Coverage,
    Traceability, Conciseness)
                 │
                 ▼
     [ Introspection Store ] ──> UI Dashboard (`/introspection`)
   (Trends & Problem Area Triage)
```

### Key Properties

- **Signal-Grounded Question Generation**: Questions are synthesized directly from actual tracker signals (recently updated issues, cross-tracker status discrepancies, unresolved relationships, stale cache items, and previous weak runs).
- **Zero Hallucination Tolerance**: Evaluator inspects the generated answer, retrieved objects, and search plan. Any hallucinated tracker key or fabricated relation forces `Accuracy = 0` and immediate `FAIL`.
- **Configurable Cadence**: Configured via an **Introspection level** slider in Settings (`/settings`):
  - **Level 0 (None)**: 0 questions/day *(recommended default)*.
  - **Level 1 (Light)**: 2 questions/day.
  - **Level 2 (Reasonable)**: 4 questions/day.
  - **Level 3 (Curious)**: 7 questions/day.
  - **Level 4 (Introvert!!)**: 10 questions/day.
- **Spaced Scheduling & Crash Safety**: Questions are spread evenly across the day. The scheduler tracks today's run count in SQLite, tolerates application restarts without replaying missed questions in bursts, and strictly enforces single-flight execution (`AtomicBool`).
- **Strict Read-Only Guarantee**: Introspection only queries issue trackers and local databases; it never modifies external trackers or alters prompts/code autonomously.

---

## 12. Testing & 30-Test Validation Framework

IssueMesh is verified using a rigorous **30-Test Validation Framework** evaluated across five dimensions:

### Evaluation Dimensions

| Dimension | Weight | Scoring Description |
|---|---|---|
| **Accuracy** | 35% | Factual correctness supported by tracker evidence. Zero hallucinations allowed. |
| **Relevance** | 25% | Directly answers the user's exact query without irrelevant search dumps. |
| **Coverage** | 15% | Retrieves all necessary objects across relevant trackers. |
| **Traceability** | 15% | All claims reference real object keys (`OPEN-4679`, `LP#...`, `org/repo#...`). |
| **Conciseness** | 10% | Clear, readable, and strictly **less than 250 words**. |

### Scoring & Release Gates

$$\text{Weighted Score} = 0.35 \times \text{Acc} + 0.25 \times \text{Rel} + 0.15 \times \text{Cov} + 0.15 \times \text{Tra} + 0.10 \times \text{Con}$$

- **PASS**: Weighted score $\ge 4.0/5$ with no dimension $< 3$.
- **WEAK**: Weighted score $\ge 3.0/5$ and $< 4.0/5$.
- **FAIL**: Weighted score $< 3.0/5$ or any hard failure.
- **Hard Failure**: Any hallucinated ticket ID, invented status, or wrong relationship triggers `Accuracy = 0` and immediate `FAIL`.

```text
Release Gates:
- PASS >= 28 / 30 tests
- FAIL = 0
- Average Weighted Score >= 4.30 / 5
- Average Accuracy >= 4.50 / 5
- Average Relevance >= 4.50 / 5
- Average Traceability >= 4.00 / 5
- Hallucinations = 0
```

### The 10 Corner-Case Tests (21–30)

21. **Cyclic Relations**: Graph loop detection, node deduplication, and bounded traversal.
22. **Stale Cache**: Newer remote tracker state overrides local cache and preserves revisions history.
23. **Tracker Unavailable**: Search continues on available sources, reports limitation, and does not invent evidence.
24. **Invalid Credentials**: Clear 401 error, zero secret leakage, healthy sources continue unaffected.
25. **Ambiguous Query**: Exposes ambiguity and lists candidate issues rather than guessing.
26. **Misleading Lexical Matches**: Rejects superficial keyword overlap; reports insufficient evidence for absent concepts.
27. **Very Large Thread**: Bounded context management, prioritization of latest decisions, and answer under 250 words.
28. **Conflicting Source States**: Preserves cross-tracker contradictions without fabricating a unified status.
29. **Missing/Private Referenced Object**: Records unresolved references gracefully without panics or fabricated content.
30. **Duplicate Evidence**: Multiple links to the same PR resolve to a canonical key without score inflation.

---

## 13. Portability & Canonical Snap Package

The application has been bumped to v0.2.8 and packaged into a self-contained, strictly confined Canonical Snap package:

• **Package Artifact**: `issuemesh_0.2.8_amd64.snap` (2.6MB)  
• **Base**: `core24` (Ubuntu 24.04 LTS)  
• **Confinement**: `strict`  
• **Plugs**: `network`, `network-bind`, `home`  

#### Packaged Components:

```text
issuemesh_0.2.8_amd64.snap
├── bin/
│   ├── issuemesh           # Release binary (Rustls TLS + bundled SQLite FTS5, zero external dynamic .so dependencies)
│   └── issuemesh-launch    # Environment launcher initializing $SNAP_USER_COMMON and relocatable state
├── meta/
│   ├── snap.yaml           # Snap runtime specification (core24, strict, plugs: network, network-bind, home)
│   └── gui/
│       ├── issuemesh.desktop # Desktop entry for Ubuntu/GNOME application menus
│       └── icon.svg        # Canonical aubergine & orange mesh application icon
└── usr/share/issuemesh/
    ├── static/             # Full Web Frontend assets (Search, Tutorial, Settings, Memory, Sources, Health, etc.)
    │   ├── index.html, setup.html, tutorial.html, settings.html, memory.html, ...
    │   ├── css/style.css
    │   └── js/app.js
    └── schema/
        └── schema.sql      # SQLite Datastore DDL schema (FTS5 table, revisions, embeddings, relations, etc.)
```

#### Datastore & Runtime Storage in Snap:

When run inside the snap sandbox, IssueMesh automatically detects `SNAP_USER_COMMON` (`~/snap/issuemesh/common/`):

• **SQLite database**: `~/snap/issuemesh/common/issuemesh.db` (auto-created and migrated on first launch, preserved across snap upgrades)  
• **Secrets**: `~/snap/issuemesh/common/secrets.env` (enforced 0600 permissions)  
• **Configuration**: `~/snap/issuemesh/common/config.toml`  
• **Logs**: `~/snap/issuemesh/common/logs/`  
• **Schema Reference**: `~/snap/issuemesh/common/schema.sql` (copied on first run from `$SNAP/usr/share/issuemesh/schema/schema.sql`)  

#### Local Installation & Testing

To install and run the generated snap on your workstation:

```bash
# 1. Install under strict confinement
sudo snap install --dangerous ./issuemesh_0.2.8_amd64.snap

# 2. Connect standard interfaces (if not auto-connected)
sudo snap connect issuemesh:network
sudo snap connect issuemesh:network-bind
sudo snap connect issuemesh:home

# 3. Launch IssueMesh
issuemesh
```

See [SNAP_PACKAGING.md](SNAP_PACKAGING.md) for complete snapcraft configuration details, direct packaging scripts, and store distribution guidelines.

---

## 14. Known Limitations

- **Rate Limits**: Anonymous Launchpad access is subject to public IP rate limits. Configuring authentication removes these limits.
- **Private Tracker Access**: Requires network connectivity to internal Jira instances (e.g., via VPN).
- **LLM Context Window**: Extremely large comment threads (> 100 comments) are truncated to the most recent comments and decisions to fit context limits.

---

## 15. Developer Instructions

### Development Workflow

```bash
# Check formatting
cargo fmt --check

# Run Clippy lints
cargo clippy --all-targets --all-features -- -D warnings

# Run all unit and integration tests
CFLAGS="-fno-sanitize=undefined -fwrapv" cargo test

# Run corner-case suite (Tests 21-30)
CFLAGS="-fno-sanitize=undefined -fwrapv" cargo test --test corner_cases

# Run introspection suite
CFLAGS="-fno-sanitize=undefined -fwrapv" cargo test --test introspection

# Run verification pipeline
./scripts/verify.sh

# Run full 30-test benchmark suite
CFLAGS="-fno-sanitize=undefined -fwrapv" cargo run --example run_benchmark -- all
```

---

## 16. License

This project is licensed under the **MIT License**. See the [LICENSE](LICENSE) file for details.

Copyright (c) 2026 Cedric Gegout <cedric.gegout@canonical.com>.
