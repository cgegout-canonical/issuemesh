# IssueMesh Build Decisions

This document records deviations, decisions, and choices made during the build run, in accordance with `ISSUEMESH_BUILD_PROMPT.md §1` and `SPEC.md §78`.

| Date | Stage | Decision | Rationale |
|---|---|---|---|
| 2026-10-07 | 1 | Single `ISSUEMESH_HOME` replacing XDG split | Required by `ISSUEMESH_BUILD_PROMPT.md §3.1`. |
| 2026-10-07 | 1 | Rusqlite with `bundled` feature | Avoid system SQLite dependency for strict portability (`ISSUEMESH_BUILD_PROMPT.md §1`). |
| 2026-10-07 | 1 | Reqwest with `default-features = false`, `rustls-tls` | Avoid system OpenSSL dependency for strict portability (`ISSUEMESH_BUILD_PROMPT.md §1`). |
| 2026-10-07 | 1 | Embedded migrations in binary | Zero external runtime dependencies (`SPEC.md §4`). |
| 2026-10-07 | 2 | `async-trait` dependency | Clean async trait definitions for `Connector` trait across dynamic dispatch. |
| 2026-10-07 | 6 | First-run onboarding redirection | Redirect `/` to `/setup` when `onboarding_completed == false` to guarantee explicit initial setup sequence before search is used (`SPEC.md §8-9`). |
| 2026-10-07 | 7 | SSE with broadcast channel and history playback | In-memory broadcast stream with history replay so clients connecting after job start immediately receive prior events before live streaming (`SPEC.md §14`). |
| 2026-10-07 | 8 | Deterministic query planner fallback and bounds clamping | When model is unconfigured, unreachable, or returns invalid/out-of-bounds plan, the query planner deterministically extracts query tokens and clamps budgets to hard limits without failing the search request (`SPEC.md §16`, `§61`). |
| 2026-10-07 | 10 | Cross-tracker consistency checks and hybrid search pipeline | Implement 5 deterministic consistency rules with structured findings alongside an 8-step orchestrated search pipeline obeying graph and time budgets. |
| 2026-10-07 | 11 | Search run persistence, in-memory caching, and memory maintenance API | Automatically save completed search runs into `search_runs` table while caching in `AppState` for instant structured UI retrieval; provide full diagnostics and maintenance endpoints for local SQLite datastore. |
| 2026-10-08 | 12 | Live server E2E isolation & single release binary verification | Implement live test harness on isolated port with temporary `0700` home dir and `0600` secrets; verify zero dynamic libssl/libsqlite links and enforce strict static read-only invariants in `scripts/verify.sh`. |
