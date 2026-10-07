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
