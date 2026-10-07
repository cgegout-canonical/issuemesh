# IssueMesh — Autonomous Build Run

You are building **IssueMesh** end to end, in one continuous run, until the
Definition of Done in section 9 is met.

Three documents are in this workspace:

- `docs/SPEC.md` — the full product specification (78 sections). It is
  authoritative, except where section 3 of this file overrides it.
- `docs/CONNECTING_TRACKERS.md` — the tracker reference. Read it completely
  before writing any connector code.
- This file — the execution contract: how you work, what you test, and when
  you are done.

Read all three before writing a single line of code.

---

## 1. Operating rules

### Autonomy

- Build all 12 stages from `SPEC.md §75`, in order. Do not stop between
  stages. Do not ask "shall I continue?" or "do you want me to proceed?".
- Do not ask for clarification unless you are genuinely blocked. Choose the
  option most consistent with the `SPEC.md §78` priority order and record it
  in `docs/DECISIONS.md` with one line of rationale.
- Genuinely blocked means one of:
  - a required toolchain component (cargo, a C compiler for bundled SQLite)
    is missing and cannot be installed;
  - `~/.issuemesh/secrets.env` exists but every tracker fails authentication.
  Everything else is a decision, not a question.
- A stage is not done while any of these fail:
  `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`.
- A stage is not done if non-test code contains `todo!()`, `unimplemented!()`,
  `TODO`, or a feature "left for later" that the stage gate requires.
- When a test fails, fix the code. Change the test only if it contradicts
  `SPEC.md`, and say so in `DECISIONS.md`.

### Progress tracking (resume safety)

- Maintain `PROGRESS.md` at the repo root. One line per stage:
  `stage N | todo / in progress / done | gate summary | commit hash`.
- Update it at the start and end of every stage.
- If this run is resumed in a fresh context: read `PROGRESS.md`,
  `docs/DECISIONS.md`, and `git log --oneline` first. Continue from the
  first stage not marked `done`. Do not redo finished stages.
- Commit after every stage: `git commit -m "stage N: <title>"`.
  Small, frequent commits. Never leave a stage uncommitted.

### Secrets handling during the build

- Real credentials live in `~/.issuemesh/secrets.env`. They are used only by
  the live e2e suite (section 6) and by the running app.
- Never `cat`, `echo`, print, log, or paste any value from that file. Never
  copy it into the repository. `.gitignore` must exclude `secrets.env`,
  `*.db`, `*.db-wal`, `*.db-shm`.
- To learn which keys exist, run exactly this and nothing else:
  `grep -o '^[A-Z_]*=' ~/.issuemesh/secrets.env`
- Never send a mutating request to GitHub, Jira, or Launchpad — not from app
  code, not from tests, not from an ad-hoc `curl`. GET and the allow-listed
  search POSTs only.

### Dependencies

- Follow `SPEC.md §4`. Before any `cargo add`, write one line in
  `DECISIONS.md` explaining why an existing dependency cannot do the job.
- No `wiremock`, no `httpmock`. Build the mock tracker servers for tests with
  `axum`, which is already a dependency.
- `reqwest` with `default-features = false` and `rustls-tls`.
  `rusqlite` with the `bundled` feature, so no system SQLite is needed.
- `#![forbid(unsafe_code)]` in `main.rs`. `unwrap()` only in tests.

---

## 2. Environment check (first thing)

Run and record the output in `PROGRESS.md`:

```bash
cargo --version && rustc --version && git --version
cc --version || gcc --version      # needed by rusqlite bundled
curl --version | head -1
grep -o '^[A-Z_]*=' ~/.issuemesh/secrets.env   # keys only, never values
```

If `~/.issuemesh/secrets.env` is missing, note it and continue. Stages 1–11
do not need it. Stage 12's live suite will be skipped with a clear message.

---

## 3. Overrides to SPEC.md

These replace the corresponding parts of `SPEC.md`. Where they conflict with
`CONNECTING_TRACKERS.md`, these win.

### 3.1 One application home directory

- Environment variable `ISSUEMESH_HOME`, default `~/.issuemesh/`, mode `0700`.
  This replaces the XDG config/data split in `SPEC.md §7`, `§23`, `§57`.
- Layout:

  ```text
  ~/.issuemesh/
    secrets.env        0600
    config.toml
    issuemesh.db       (+ -wal, -shm)
    logs/
  ```

- Under Snap, the default becomes `$SNAP_USER_COMMON/` with the same layout.
- One `Paths` struct owns every filesystem location. No other module builds
  a path to the home directory.
- `--data-dir` and `--config` CLI flags still work and override the default.
- `secrets.env` format and loader semantics are exactly
  `CONNECTING_TRACKERS.md §2` (KEY=VALUE, `#` comments, optional quotes).
  Keys are the `SPEC.md §7` list.

### 3.2 Secrets are hot-reloaded

- The wizard and the Settings page write `secrets.env` with an atomic
  write (temp file + rename), mode `0600`, directory `0700`.
- After any save, rebuild the connector registry and the model client in
  place. No restart. This replaces "read once at process start" in
  `CONNECTING_TRACKERS.md §2` and `§9`.
- If `secrets.env` already holds `GITHUB_TOKEN` or `JIRA_*` at first run, the
  wizard marks that source as enabled and runs its connection test
  automatically. It never pre-fills or displays the values. The user still
  completes the wizard once.

### 3.3 The model endpoint may be absent

- `OPENAI_*` keys may be missing. The whole product must run without them
  (`SPEC.md §61`). The wizard allows skipping the model step.
- Live tests that need the model are **skipped** (with a message), not
  failed, when `OPENAI_API_KEY` is absent. The degraded path is tested
  instead (E2E-34).

### 3.4 Stricter quality gates

- `cargo clippy --all-targets --all-features -- -D warnings`, not plain
  `cargo clippy`.
- Every remote HTTP call runs inside `tokio::time::timeout`.
- Source-specific `Semaphore` limits: GitHub 4, Jira 4, Launchpad 2, model 2.
- Launchpad pacing: at least 300 ms between calls per process.

### 3.5 Embedding dimension guard

- Store `dimension` and `model_name` with every embedding row.
- If the configured embedding model changes, mark old vectors stale and skip
  them in semantic search. Offer a "Re-embed stale objects" action on the
  Memory page. Never mix dimensions silently.

### 3.6 SQLite access pattern

- One `Store` type owning a `Mutex<rusqlite::Connection>`.
- Pragmas: `journal_mode=WAL`, `busy_timeout=5000`, `foreign_keys=ON`,
  `synchronous=NORMAL`.
- All DB work goes through `tokio::task::spawn_blocking`.
- Never hold the DB lock across an `.await`.

---

## 4. Stage gates

Each stage from `SPEC.md §75` is done only when its gate holds **and** the
three commands in section 1 pass.

| Stage | Gate |
|---|---|
| **1** Skeleton, config, paths, logging, migrations | `issuemesh --version` works. `issuemesh --data-dir /tmp/x` creates the 3.1 layout with correct modes. Migrations apply to an empty DB and are idempotent (test: apply twice = no-op). `schema_version` stored. Redaction utility exists with tests. |
| **2** Read-only HTTP + connectors | `ReadOnlyHttpClient` exposes exactly `get` and `post_search`. Mock-server tests cover every item in `SPEC.md §64 Connector tests` for all three sources. Negative test: a non-allow-listed POST returns `ReadOnlyViolation` and the mock server counted **zero** requests. GraphQL `errors` → REST fallback tested. The two "retry with first 3 words" quirks tested. The GitHub `is:issue` / `is:pull-request` double query tested. |
| **3** Object store + relations | Upsert, `content_hash` change detection, revisions, relation dedup, unresolved refs. Identifier extraction tests include the full false-positive list: CVE, USN, RFC, ISO, SHA, AES, UTF, IPV, IEEE, PCI, HTTP, TLS, GPT, LP. Self-references skipped. |
| **4** FTS + traversal | FTS5 returns ranked hits with snippets. Traversal tested at depth 0 / 1 / 2, node budget, and the cycle A→B→A terminates. The fixture graph from `SPEC.md §65` is fully discovered from `OPEN-100`. |
| **5** Axum + basic UI | All routes in `SPEC.md §64 Web tests` return 200, or redirect to `/setup` when not onboarded. Assets are embedded: the binary runs correctly from an empty directory. |
| **6** Wizard + Settings + Health | Each wizard step POSTs, validates, and tests its connection. Onboarding state machine tested. Secrets masked as `••••` + last 4. Test: the full secret value never appears in any response body. |
| **7** Jobs + SSE | `POST /api/search` returns an id. `GET /api/search/:id/events` streams typed events ending with `complete`. Event order asserted. A search with two mocked sources, one returning 500, still completes with a `Warning` event. |
| **8** Model client + planner | Provider-neutral client with timeout. Planner output parsed into `SearchPlan` and clamped (test: depth 99 → max, negative → default). Model down → deterministic parser only, no error to the user. |
| **9** Embeddings + hybrid ranking | Cosine similarity in Rust. Ranking formula in one testable function with a `Weights` struct. Dimension guard (3.5) tested. |
| **10** Deep search + consistency | The `SPEC.md §15` pipeline runs end to end on the fixture graph. Each check in `SPEC.md §33` has one fixture that triggers it and one that does not. |
| **11** Results UI, Memory, Sources, run page | Structured result page (not raw Markdown). Memory stats from pragmas and filesystem. The search-run "why" page shows plan, sources, objects, relation paths, model, prompt version. |
| **12** Hardening, release, e2e | `scripts/verify.sh` exits 0, including the live suite. Release binary runs from an empty directory. `FINAL_REPORT.md` written. |

---

## 5. Test layers

| Layer | Where | What |
|---|---|---|
| L1 Unit | `cargo test --lib` | The `SPEC.md §64` unit list. |
| L2 Connector | `tests/connector_*.rs` | One axum mock server per source. Fixture JSON in `tests/fixtures/<source>/`. Pagination (GitHub `page`, Launchpad `ws.start` + `next_collection_link`, Jira `nextPageToken`), 401 / 403-ratelimit / 404 / 422 / 429 / 5xx / timeout, both retry quirks, the GitHub type-qualifier double query, GraphQL fallback, ADF flattening. |
| L3 Store | `tests/store_*.rs` | Temp DB per test. Migrations, CRUD, FTS, relations, search runs, integrity utilities. |
| L4 Web | `tests/web_*.rs` | `tower::ServiceExt::oneshot` against the router with a temp home. Routes, onboarding transitions, secret masking. |
| L5 Pipeline | `tests/search_pipeline.rs` | All three trackers mocked plus a mock OpenAI endpoint. The `§65` graph. Depth, budget, loops, model-down degradation. |
| L6 Live e2e | `tests/e2e/` | Section 6. Marked `#[ignore]`. Run only with `ISSUEMESH_E2E=1`. |
| L7 Release | `scripts/verify.sh` | Section 7. |

Normal `cargo test` never touches the network.

---

## 6. Live end-to-end suite

Purpose: prove the product works against the **real** trackers with the
user's **real** credentials, strictly read-only, without leaking anything.

### Setup

- Reads `ISSUEMESH_HOME/secrets.env` (default `~/.issuemesh/secrets.env`).
  If the file is absent, the whole suite skips with a clear message.
- Starts the real `issuemesh` binary on a random free port with a **temp**
  `ISSUEMESH_HOME`. The temp `secrets.env` is a copy of the user's, mode
  `0600`, deleted at the end. The user's own home is never modified.
- Drives the app over HTTP, like a browser would.
- Discovers test objects dynamically. Never hardcode IDs that may not exist.
  Overridable with env vars: `ISSUEMESH_E2E_GITHUB_REPO`,
  `ISSUEMESH_E2E_JIRA_PROJECT`, `ISSUEMESH_E2E_LP_PROJECT`.
- Quota budget per full run: ≤ 20 GitHub search calls, ≤ 150 GitHub core
  calls, ≤ 60 Jira calls, ≤ 60 Launchpad calls. The `api_audit` table is the
  meter; E2E-52 asserts the budget was respected.
- Run serially: `--test-threads=1`.

### Tests

Each test is its own `#[tokio::test]`, named with its ID.

**Startup and onboarding**

- **E2E-01** Binary starts, prints the `§57` banner, `/` redirects to `/setup`.
- **E2E-02** Walk the wizard over HTTP: welcome → GitHub test (login shown,
  token not echoed) → Jira test (site + user shown) → Launchpad auto-test →
  model step (test if keys present, else skip) → memory → defaults → review.
  Afterwards `/` serves the search page.
- **E2E-03** Restart the binary with the same temp home. Onboarding is
  remembered. Banner lists connected sources.

**Health**

- **E2E-10** `/health` and `/api/health`: each enabled source is Healthy with
  latency > 0. GitHub shows quota remaining and token expiry when the token is
  fine-grained. Launchpad auth = `none`.

**Direct lookups (per source)**

- **E2E-20** GitHub: search finds one recent merged PR in the configured
  org/repo. Direct-ID search `owner/repo#N` returns an evidence card with
  state `merged`, ≥ 1 relation (`closes`, `commit`, or `references`), and
  comments in the thread view.
- **E2E-21** Jira: list projects, pick one (or the override), JQL
  `updated >= -30d`, take the first key, direct lookup. Description is
  flattened from ADF (non-empty, contains no `{"type":`). Changelog present.
  Issue links or remote links extracted when the issue has them.
- **E2E-22** Launchpad: search `openstack` (or the override project), take the
  first bug id, direct lookup `LP#<id>`. Tasks, messages, and activity
  present. `duplicates` relation present when `duplicate_of_link` is set.
- **E2E-23** URL inputs: the three URL forms from `SPEC.md §11` resolve to the
  same objects as the IDs above.

**Search pipeline**

- **E2E-30** Natural-language search across all enabled sources. The SSE
  stream contains `PlanningStarted … Complete` in valid order. Every enabled
  source has a `SourceSearchCompleted`. Reported stats match
  `search_run_objects` in the DB.
- **E2E-31** Cache: repeat the same search. Objects reused from cache > 0.
  Audit call count is lower than in the first run.
- **E2E-32** Deep search, depth 2, starting from the Jira key found in E2E-21.
  Graph expanded. Nodes ≤ `max_graph_nodes`. Deep reads ≤ `max_deep_reads`.
  A warning is emitted if a budget is hit. Unresolved references stored.
- **E2E-33** Model synthesis (skip if no `OPENAI_API_KEY`): the answer has
  ≥ 1 citation. Every cited ID exists in the evidence set. `prompt_version`
  stored on the run.
- **E2E-34** Degraded mode: set `OPENAI_BASE_URL` to a closed local port via
  Settings. Search still returns results. The UI shows
  "Model unavailable. Showing retrieval results without AI synthesis."
- **E2E-35** One tracker down: set `JIRA_URL` to an unroutable host via
  Settings. Search completes with a Warning about Jira. GitHub and Launchpad
  results are present. Health shows Jira failed. Restore afterwards.

**Security invariants**

- **E2E-40** Leak scan. After all tests above, read every secret value from
  the temp `secrets.env`. Assert that neither the full value nor its last 12
  characters appears in: `issuemesh.db` bytes (including WAL), `logs/`, any
  HTML or JSON response captured during the run, `api_audit` rows,
  `search_runs` rows. Only the masked form (`••••` + last 4) may appear on
  `/settings`.
- **E2E-41** Read-only. `api_audit` contains only GET entries plus POST
  entries whose path is in the allow-list. Zero other methods. In addition,
  this grep returns nothing:
  `grep -rn "\.put(\|\.patch(\|\.delete(\|Method::PUT\|Method::PATCH\|Method::DELETE" src/`
- **E2E-42** After the wizard writes them: `secrets.env` is `0600`, the home
  directory is `0700`.
- **E2E-43** Default bind is `127.0.0.1` only (check `/proc/net/tcp` or `ss`).
  `--bind 0.0.0.0` prints the security warning.

**Memory and operations**

- **E2E-50** `/memory` numbers match direct SQL counts. DB path and size
  shown. The integrity-check action returns ok.
- **E2E-51** Memory actions: optimise, rebuild FTS, clear history. Each works,
  and FTS search still returns the E2E-20 object afterwards.
- **E2E-52** Budget meter: `api_audit` counts are within the section 6 budget.

**Release**

- **E2E-60** `cargo build --release`. Copy the binary to an empty temp
  directory. Run with a fresh `ISSUEMESH_HOME`. Wizard reachable. `ldd` shows
  no `libssl` and no `libsqlite`. Binary size recorded in `FINAL_REPORT.md`.

---

## 7. scripts/verify.sh

The single finish-line command. Exits non-zero on the first failure.

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

# read-only invariant, compile-time flavour
! grep -rn "\.put(\|\.patch(\|\.delete(\|Method::PUT\|Method::PATCH\|Method::DELETE" src/

# no leftovers
! grep -rn "todo!()\|unimplemented!()" src/

cargo build --release
ls -la target/release/issuemesh

SECRETS="${ISSUEMESH_HOME:-$HOME/.issuemesh}/secrets.env"
if [ -f "$SECRETS" ]; then
  ISSUEMESH_E2E=1 cargo test --release --test e2e -- --ignored --test-threads=1
else
  echo "secrets.env not found: live e2e suite skipped"
fi
```

Run it at the end of stages 7, 10, and 12. It must be green at the end of
stage 12 before you stop.

---

## 8. Browser verification (if a browser tool is available)

After stage 11: open the app in the browser, complete the wizard, run one
search, and watch the progress list update in real time. Open an evidence
card. Open the Memory, Sources, Health, and Settings pages. Fix anything
that looks broken. Save screenshots to `docs/screenshots/`.

Do not type credentials by hand. The wizard detects existing keys (3.2).

---

## 9. Definition of Done

Stop only when **all** of the following hold:

- Every line of `SPEC.md §77` is checked in `FINAL_REPORT.md`, each with the
  test ID or manual check that proves it.
- `scripts/verify.sh` exits 0 on a clean clone.
- `PROGRESS.md` shows all 12 stages `done`, each with a commit hash.
- `docs/DECISIONS.md` lists every deviation from `SPEC.md`.
- `README.md` and `ARCHITECTURE.md` exist per `SPEC.md §73`.
- `FINAL_REPORT.md` contains: binary size, test counts per layer, the e2e
  results table, API calls consumed per source, known limitations.

Then stop. That is the only planned stop in this run.
