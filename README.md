# IssueMesh

IssueMesh is a local-first, memory-augmented federated search engine and investigation workbench for engineering issues across multiple trackers: **GitHub**, **Jira**, and **Launchpad**.

IssueMesh builds a local, queryable graph of cross-linked bugs, pull requests, tasks, revisions, and discussions with semantic search, full-text search (SQLite FTS5), graph traversal, consistency analysis, and deterministic evidence synthesis.

---

## Key Highlights

- **Local First & Memory-Augmented**: Stores fetched issues, comments, relations, and embeddings in a local SQLite database (`issuemesh.db`) with zero external database dependencies.
- **Strictly Read-Only Guarantee**: Architecturally enforced HTTP client contract. Tracker APIs are accessed exclusively via read-only endpoints (`GET`, HEAD, and strictly allowlisted search `POST` bodies). Tracker writes (`PUT`, `PATCH`, `DELETE`) are impossible by construction.
- **Zero-Secret Leakage**: Tokens and credentials stored in permissions-hardened `secrets.env` (`0600` on POSIX). Secrets are never logged, never embedded into the SQLite database, and returned as masked strings (`••••1234`) in the web UI and API.
- **Self-Contained Single Binary**: Embedded HTML/CSS/JS frontend (`rust-embed`), embedded SQLite migrations, and pure Rust TLS (`rustls`), producing an ultra-compact ~6 MB binary with no Node.js or Python runtime requirements.
- **Deep Graph Traversal**: Automatically discovers and expands issue references across tracker boundaries (`LP#123456`, `PROJ-789`, `#456`, URLs).
- **Graceful Degradation**: Operates seamlessly even when trackers are unreachable or when LLM/embedding inference endpoints are offline (falls back to lexical FTS and BM25 graph scoring).

---

## Architecture Overview

```
                          ┌──────────────────────────┐
                          │   Web UI / REST API      │
                          │   (Axum + Rust-Embed)    │
                          └────────────┬─────────────┘
                                       │
                                       ▼
                          ┌──────────────────────────┐
                          │   Search Orchestration   │
                          │   & Query Intent Parser  │
                          └──────┬────────────┬──────┘
                                 │            │
            ┌────────────────────┘            └────────────────────┐
            ▼                                                      ▼
 ┌─────────────────────┐                                ┌─────────────────────┐
 │  Federated Trackers │                                │   Local Memory      │
 │  (GitHub, Jira, LP) │                                │   (SQLite + FTS5    │
 │  [Strict Read-Only] │                                │    + Embeddings)    │
 └──────────┬──────────┘                                └──────────┬──────────┘
            │                                                      │
            └────────────────────┐            ┌────────────────────┘
                                 ▼            ▼
                          ┌──────────────────────────┐
                          │   Graph Expander Engine  │
                          │ (Links, Relations, Dedupe│
                          └────────────┬─────────────┘
                                       │
                                       ▼
                          ┌──────────────────────────┐
                          │ Evidence Ranker & L1/L2  │
                          │ Consistency / Synthesis  │
                          └──────────────────────────┘
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for deep-dive technical specifications on the connector boundaries, graph expansion, scoring pipelines, and safety models.

---

## Prerequisites

- **Linux** (x86_64 or aarch64), **macOS**, or modern POSIX environment.
- **Rust Toolchain**: Rust 1.80+ (`cargo`, `rustc`).
- Standard C compiler and runtime (`gcc` / `glibc` / `musl`) for SQLite compilation.

---

## Build & Run

### Quick Start (Development)

```bash
# Build binary
cargo build

# Run test suite
cargo test

# Run application
cargo run
```

### Production Release Build

```bash
# Compile optimized release binary
cargo build --release

# Run standalone executable
./target/release/issuemesh
```

The web interface will be accessible at:
```
http://127.0.0.1:8765
```

---

## First-Run Setup & Onboarding

When starting IssueMesh for the first time without configured trackers:
1. IssueMesh detects missing configuration and displays an interactive terminal notice with the setup URL.
2. Opening `http://127.0.0.1:8765` automatically redirects to `/setup`.
3. The onboarding wizard allows configuring:
   - **GitHub**: Personal Access Token (classic `repo` or fine-grained read-only).
   - **Jira**: Base URL, User Email, and API Token.
   - **Launchpad**: Enable/disable public Launchpad API access.
   - **Local / OpenAI LLM**: Endpoint URL, API key, and model for synthesis.
   - **Embedding Engine**: Optional embedding endpoint and model for vector similarity.
4. Each tracker includes a **Test Connection** button validating credentials in real time.
5. All secrets are securely written to `secrets.env` with `0600` permissions.

---

## Data Paths & Storage

IssueMesh respects the standard XDG base directory specification:

| File / Directory | Location | Purpose |
|---|---|---|
| **Data Directory** | `~/.local/share/issuemesh/` | SQLite database (`issuemesh.db`), search run caches, and query logs. |
| **Config Directory** | `~/.config/issuemesh/` | `config.toml` (tracker settings, search budgets, thresholds). |
| **Secrets File** | `~/.config/issuemesh/secrets.env` | Credentials with strict `0600` file permissions. |

Custom paths can be specified via environment variables:
- `ISSUEMESH_DATA_DIR=/custom/data/path`
- `ISSUEMESH_CONFIG_DIR=/custom/config/path`
- `ISSUEMESH_PORT=8765`
- `ISSUEMESH_HOST=127.0.0.1`

---

## Verification & Testing

IssueMesh includes comprehensive test suites across unit, integration, and live end-to-end scenarios:

```bash
# Run all format and lint checks
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings

# Run all unit and integration tests
cargo test

# Run the complete automated verification pipeline (including live E2E)
./scripts/verify.sh
```

### Verification Pipeline (`verify.sh`)
The automated verification script executes:
1. `cargo fmt --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test`
4. Static grep scan for forbidden write HTTP methods (`PUT`, `PATCH`, `DELETE`) on trackers.
5. Static scan ensuring zero `todo!()` or `unimplemented!()` placeholders.
6. `cargo build --release`
7. Live server E2E test suite running on an isolated test port with zero external leaks.

---

## Snap Strict Confinement Packaging

IssueMesh is designed from day one for seamless packaging under Canonical Snap strict confinement (`core24`):
- Relocatable data/config paths via `$SNAP_USER_COMMON` and `$SNAP_USER_DATA`.
- Pure Rust TLS (`rustls` with `webpki-roots`), removing system OpenSSL/crypto shared library dependencies.
- Static SQLite bundling (`rusqlite` bundled), eliminating external database daemon requirements.
- Required snap interfaces: `network`, `network-bind`.
