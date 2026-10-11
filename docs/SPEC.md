# Build IssueMesh

You are implementing a production-quality Rust application named **IssueMesh**.

IssueMesh is a compact, portable, self-contained engineering intelligence application.

Its purpose is to search, retrieve, connect, cache, analyse, and explain engineering issues and related objects across:

- GitHub
- Jira Cloud
- Launchpad

IssueMesh must expose a polished local web interface at:

`http://127.0.0.1:<port>`

The default port should be `8765`.

The application must eventually be easy to package as a strict-confined Snap.

The architecture must therefore favour:

- one Rust executable
- minimal runtime dependencies
- no Python
- no Node.js runtime
- no external database
- no external web server
- embedded web assets
- embedded database migrations
- a lightweight local datastore
- portable configuration
- simple installation

The application name, executable name, configuration namespace, UI branding, and internal naming must consistently use:

**IssueMesh**

Binary:

`issuemesh`

---

# 1. Source of truth for tracker integrations

A reference document named:

`CONNECTING_TRACKERS.md`

is provided with this task.

Read it completely before implementing the connectors.

It documents the existing Cognition integration with GitHub, Launchpad and Jira.

Reuse its behaviour and guarantees unless this specification explicitly overrides something.

In particular preserve:

- read-only behaviour
- credential handling principles
- GitHub API peculiarities
- Launchpad API peculiarities
- Jira Cloud API peculiarities
- pagination
- error handling
- identifier extraction
- native relation extraction
- cross-system references
- normalized Evidence objects
- normalized Relation objects
- health information
- API rate-limit awareness

Do not replace working tracker behaviour with assumptions.

Do not invent undocumented endpoints.

---

# 2. Core product concept

IssueMesh is not just a federated search interface.

It creates a local **engineering issue knowledge graph**.

Tracker objects remain authoritative.

The local database is a cache, search index, relation graph, and memory.

The LLM is used for planning, semantic interpretation, classification, and synthesis.

The LLM must never become the source of truth.

Conceptually:

```text
GitHub ───────┐
              │
Jira ─────────┼──> Normalisation ──> Local object graph
              │                         │
Launchpad ────┘                         ├── Full-text search
                                        ├── Semantic search
                                        ├── Relations
                                        ├── Consistency analysis
                                        └── Deep graph traversal
                                                 │
                                                 ▼
                                             LLM synthesis
                                                 │
                                                 ▼
                                      Evidence-backed answer
```

Every factual answer must ultimately be traceable to tracker objects stored or fetched by IssueMesh.

---

# 3. High-level application architecture

Implement the following logical layers.

```text
Browser
   │
   ▼
Axum Web Server
   │
   ├── Web UI
   ├── REST/JSON API
   └── SSE search progress
   │
   ▼
Search Orchestrator
   │
   ├── Query Planner
   ├── Local Retrieval
   ├── Remote Retrieval
   ├── Deep Reads
   ├── Graph Traversal
   ├── Consistency Analysis
   ├── Ranking
   └── Answer Synthesis
   │
   ├───────────────┐
   ▼               ▼
SQLite          Connectors
                  ├── GitHub
                  ├── Jira
                  └── Launchpad
   │
   ▼
OpenAI-compatible model endpoint
```

Keep clear boundaries between these layers.

---

# 4. Rust technology choices

Prefer a small, well-maintained Rust stack.

Recommended technologies:

- `tokio`
- `axum`
- `reqwest`
- `serde`
- `serde_json`
- `rusqlite`
- SQLite FTS5
- `askama` or an equivalent compile-time Rust HTML templating library
- `rust-embed` or equivalent for static assets
- `tracing`
- `tracing-subscriber`
- `anyhow`
- `thiserror`
- `regex`
- `chrono` or `time`
- `clap` for minimal CLI parameters if useful

Avoid excessive dependencies.

Before adding a dependency, ask whether standard Rust, SQLite, Axum, or a dependency already in the project can solve the problem.

Optimise for:

1. maintainability
2. compactness
3. portability
4. security
5. simple Snap packaging

Do not introduce Node, npm, webpack, Vite, React or another frontend build environment unless absolutely unavoidable.

It should not be necessary.

---

# 5. Async architecture

The existing Cognition examples may use blocking HTTP.

IssueMesh should use asynchronous Rust internally.

Use:

- Tokio
- async Reqwest
- bounded concurrency
- source-specific semaphores
- source-specific pacing
- request timeouts

Deep searches will often perform independent reads.

These should run concurrently when safe.

Do not create unbounded tasks.

---

# 6. Read-only security invariant

IssueMesh is strictly read-only toward all trackers.

This is a fundamental architecture invariant.

Create a shared `ReadOnlyHttpClient`.

Connectors must not receive a raw unrestricted Reqwest client.

Allowed methods:

- GET
- explicitly allow-listed POST requests that perform searches

Examples include:

- GitHub GraphQL query endpoint
- Jira JQL search endpoint
- Jira JQL parse if required

The HTTP abstraction must expose no generic:

- PUT
- PATCH
- DELETE

and no unrestricted POST method.

If a connector attempts an unapproved POST endpoint, reject it locally before sending the request.

The goal is:

> Even if the user has credentials with write permissions, IssueMesh itself cannot mutate tracker data.

---

# 7. Credentials and secrets

Secrets must never be stored:

- in source code
- in Git
- in prompts
- in the SQLite object datastore
- in browser local storage
- in logs
- in audit records
- in generated answers

IssueMesh should maintain a separate secrets file.

For normal non-Snap installation use a platform-appropriate application data/config directory.

For Linux, prefer something equivalent to:

```text
~/.config/issuemesh/
~/.local/share/issuemesh/
```

or use XDG locations correctly.

For future Snap packaging, support mapping state to:

```text
$SNAP_USER_COMMON
```

Secrets file permissions on Unix must be `0600`.

Credentials include:

```text
GITHUB_TOKEN
JIRA_URL
JIRA_EMAIL
JIRA_TOKEN

OPENAI_BASE_URL
OPENAI_API_KEY
OPENAI_MODEL
OPENAI_EMBEDDING_MODEL
```

Launchpad public access requires no credentials.

The application must work with any subset of trackers.

A missing credential disables that connector rather than preventing IssueMesh from starting.

---

# 8. First-run onboarding

IssueMesh must include a polished first-run onboarding process.

This is a major product feature.

Do not require the user to manually edit configuration files for normal setup.

On first launch:

```bash
issuemesh
```

start the local web server.

Display in the terminal:

```text
IssueMesh

Open:
http://127.0.0.1:8765/setup
```

The root page should redirect to `/setup` until onboarding is complete.

The wizard must feel simple and non-technical even though the underlying product is sophisticated.

---

# 9. Onboarding wizard

Use clear steps.

Suggested sequence:

## Step 1 — Welcome

Explain in one sentence:

> IssueMesh searches and connects engineering issues across GitHub, Jira and Launchpad.

Show:

- local-only application
- read-only tracker access
- local embedded datastore

Button:

`Get started`

---

## Step 2 — GitHub

Ask:

- Enable GitHub? yes/no
- GitHub token

Optionally allow:

- default organisation
- default repositories

Default organisation may be empty.

Provide concise guidance about required read permissions.

Button:

`Test connection`

Show immediately:

```text
✓ Connected as <login>
API quota: ...
Token expiry: ...
```

or a clear failure message.

Do not proceed silently when connection testing fails.

Allow the user to skip the connector.

---

## Step 3 — Jira

Ask:

- Enable Jira? yes/no
- Jira URL
- account email
- API token

Button:

`Test connection`

On success show:

```text
✓ Connected
Site: ...
User: ...
```

Optionally retrieve available projects after connection.

Allow project defaults to be selected.

Do not make project selection mandatory.

---

## Step 4 — Launchpad

Explain:

> Public Launchpad bugs can be searched without credentials.

Allow:

- enable/disable Launchpad
- optional default projects/packages

Test connectivity automatically.

---

## Step 5 — Model endpoint

IssueMesh must support an **OpenAI-compatible API**, not only one hardcoded provider.

Ask:

- Base URL
- API key
- model name
- embedding model name

Provide sensible defaults where possible.

Button:

`Test model`

The test should execute a very small API request.

Also test embeddings when an embedding model is configured.

Clearly distinguish:

```text
✓ Chat model available
✓ Embedding model available
```

Do not send tracker credentials to the model.

---

## Step 6 — Local memory

Show default datastore location.

Allow advanced users to change it.

Explain:

> IssueMesh stores previously retrieved tracker objects and their relationships locally to make future searches faster and deeper.

Show:

- datastore path
- estimated initial disk use: minimal
- SQLite
- local only

Do not overwhelm the normal user with database configuration.

---

## Step 7 — Search defaults

Provide simple defaults:

```text
Graph depth: 2
Maximum graph nodes: 100
Maximum deep reads: 30
```

Hide less common values behind:

`Advanced settings`

Normal users should be able to simply accept defaults.

---

## Step 8 — Review

Display:

```text
GitHub       Connected
Jira         Connected
Launchpad    Enabled
Model        Connected
Datastore    /...
```

Button:

`Start IssueMesh`

On completion redirect to `/`.

---

# 10. Configuration changes after onboarding

Provide a `Settings` page.

Users must be able to:

- enable/disable sources
- change tokens
- change Jira URL
- change OpenAI-compatible endpoint
- change models
- change default graph depth
- change search budgets
- retest services
- see configuration paths

Secrets must be masked after saving.

Example:

```text
GitHub token: github_pat_••••••••••••42fa
```

Never return the complete stored secret to the browser.

---

# 11. Main web interface

The UI should resemble a clean modern engineering terminal/search console.

Do not literally emulate a Unix terminal.

The primary action is a large search field.

Suggested page:

```text
──────────────────────────────────────────────────────
IssueMesh                                      ● Healthy
──────────────────────────────────────────────────────

Search GitHub · Jira · Launchpad

┌────────────────────────────────────────────────────┐
│ > Why is BGP blocking multi-site OpenStack?       │
└────────────────────────────────────────────────────┘

                         SEARCH


Recent searches

BGP support in Sunbeam                       3m ago
OVN multi-site limitations                 28m ago
OPEN-4679                                   2h ago
```

Allow direct tracker identifiers such as:

```text
OPEN-4679
LP#2167315
canonical/project#123
https://github.com/...
https://bugs.launchpad.net/...
https://...atlassian.net/browse/...
```

---

# 12. Canonical branding

Follow Canonical-inspired design rules.

Primary palette:

```text
Ubuntu Orange     #E95420
Canonical Slate   #111111
Dark Slate        #2C2C2C

White             #FFFFFF
Warm Grey         #F7F7F7
Dark text         #111111
```

Use Ubuntu fonts when available.

Do not package or redistribute proprietary font files if licensing does not allow it.

Use a good system fallback stack when Ubuntu fonts are unavailable.

The UI should be:

- predominantly white
- spacious
- clean
- engineering-oriented
- restrained
- readable
- responsive

Orange should be an accent.

Use it for:

- primary action
- selected navigation
- search progress
- important highlighting

Do not make the entire UI orange.

Avoid excessive gradients, shadows, glass effects, rounded cards, or generic SaaS aesthetics.

It should feel like a serious Canonical engineering tool.

---

# 13. Navigation

Main navigation:

```text
Search
Memory
Sources
Health
Settings
```

Optionally add:

```text
History
```

if search history becomes useful enough.

Keep navigation compact.

---

# 14. Search progress UX

When a search is submitted, do not show only a generic spinner.

Show real execution progress.

Use Server-Sent Events.

Example:

```text
◉ Understanding query
✓ Local memory searched
✓ Jira             18 objects
✓ GitHub           12 objects
◉ Launchpad        searching...
○ Reading important objects
○ Following relationships
○ Checking consistency
○ Building answer
```

At completion:

```text
✓ 47 objects analysed
✓ 16 deep reads
✓ 29 relations followed
✓ 3 trackers searched
```

The animation must correspond to actual application states.

Do not fake source completion.

Define structured search events similar to:

```rust
enum SearchEvent {
    PlanningStarted,
    PlanningCompleted,
    LocalSearchStarted,
    LocalSearchCompleted { objects: usize },

    SourceSearchStarted { source: Source },
    SourceSearchCompleted {
        source: Source,
        objects: usize,
    },

    DeepReadStarted { object: ObjectRef },
    DeepReadCompleted { object: ObjectRef },

    GraphExpansion {
        depth: u8,
        nodes: usize,
    },

    ConsistencyCheckStarted,
    ConsistencyCheckCompleted {
        findings: usize,
    },

    SynthesisStarted,

    Complete(SearchStats),

    Warning(String),
    Error(String),
}
```

Expose them through an endpoint such as:

```text
GET /api/search/:id/events
```

Use SSE.

---

# 15. Search orchestration

A search should approximately execute this pipeline:

```text
User query
   ↓
Deterministic parsing
   ↓
LLM query planning if required
   ↓
Local FTS search
   +
Local semantic search
   ↓
Remote tracker searches concurrently
   ↓
Normalize and persist results
   ↓
Rank candidate objects
   ↓
Deep-read best candidates
   ↓
Extract relations
   ↓
Expand graph frontier
   ↓
Fetch selected referenced objects
   ↓
Run local retrieval again over expanded graph
   ↓
Consistency checks
   ↓
Final ranking
   ↓
LLM synthesis using evidence only
   ↓
Formatted result
```

---

# 16. Query planning

Do not allow the model to make arbitrary network calls.

The model may produce a constrained `SearchPlan`.

Create a strongly typed Rust structure similar to:

```rust
struct SearchPlan {
    query: String,

    sources: Vec<Source>,

    projects: Vec<String>,
    repositories: Vec<String>,
    people: Vec<String>,

    time_from: Option<DateTime<Utc>>,
    time_to: Option<DateTime<Utc>>,

    semantic_queries: Vec<String>,

    graph_depth: u8,
    max_remote_results: usize,
    max_deep_reads: usize,
    max_graph_nodes: usize,

    relation_types: Vec<RelationType>,
}
```

The application validates and bounds all model-generated values.

The model must never generate direct HTTP requests.

---

# 17. Search budgets

Deep graph traversal must always be bounded.

Provide configurable defaults such as:

```text
graph_depth        = 2
max_graph_nodes    = 100
max_deep_reads     = 30
max_results/source = 50
```

Also enforce:

- overall timeout
- source-specific timeouts
- model timeout
- concurrency limit

If a budget is reached, return a useful warning.

Example:

> Search stopped at the configured graph limit. 18 additional unresolved references remain.

Do not silently truncate deep searches.

---

# 18. Normalized object model

Use the existing Cognition object concepts as the starting point.

A canonical object reference must uniquely identify every object.

Use:

```rust
struct ObjectRef {
    source: String,
    r#type: String,
    id: String,
}
```

Canonical key:

```text
source:type:id
```

Examples:

```text
github:issue:canonical/project#42
github:pr:canonical/project#43
github:commit:canonical/project@abc1234
launchpad:bug:2167315
jira:issue:OPEN-4679
```

The normalized object should contain at minimum:

```rust
struct Evidence {
    r#ref: ObjectRef,

    title: String,
    snippet: String,
    body: Option<String>,

    url: String,
    container: String,

    author: String,

    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,

    state: String,

    native: serde_json::Value,
}
```

It is acceptable to improve the structure while preserving semantic compatibility.

---

# 19. Do not merge cross-system objects

This is a strict rule.

Never merge Jira, Launchpad, and GitHub objects into one record merely because they describe the same work.

Example:

```text
OPEN-4679
    │
    ├── implemented_by
    ▼
canonical/project#912
    │
    └── fixes
        ▼
LP#2167315
```

These remain three independent objects.

Their provenance must always remain visible.

Connect them with relations.

---

# 20. Relations

Persist typed graph edges.

Suggested structure:

```rust
struct Relation {
    from: ObjectRef,
    to: ObjectRef,

    relation_type: String,
    detected_by: String,

    confidence: f32,
}
```

Possible relations include:

```text
references
closes
fixes
implements
implemented_by
blocks
blocked_by
depends_on
required_by
duplicates
duplicated_by
parent
child
commit
linked_branch
also_affects
supersedes
related_to
same_problem
```

Preserve native tracker relation wording when valuable.

---

# 21. Relation provenance and confidence

Every relation should retain how it was discovered.

Examples:

```text
native
id_pattern
url_pattern
llm_inferred
```

Indicative confidence:

```text
native tracker link              1.00
GitHub closingIssuesReference    1.00
Jira native issue link           1.00
explicit remote URL              0.95
explicit textual object ID       0.90
LLM inferred relationship        <= 0.80
```

Do not silently upgrade an LLM inference into a native fact.

Native relations always take precedence.

---

# 22. Cross-system identifier extraction

Preserve the existing Cognition behaviour.

Parse identifiers from:

- descriptions
- comments
- changelogs where relevant
- remote links
- GitHub timeline objects

Recognize:

- Jira issue keys
- Launchpad bug IDs
- GitHub issue URLs
- GitHub PR URLs
- GitHub repository references

Avoid common false-positive Jira prefixes such as CVE, RFC, HTTP, TLS, etc., according to the supplied reference document.

Skip self references.

---

# 23. SQLite datastore

Use SQLite as the only datastore.

Enable:

- WAL where appropriate
- FTS5
- transactions
- indexes
- foreign keys where useful

Do not require an external database.

Suggested logical tables:

```text
objects
object_text
relations
object_refs
embeddings
fetch_state
revisions
search_runs
search_run_objects
api_audit
settings_metadata
```

Exact schema design is your responsibility.

Use migrations.

Embed migrations inside the executable.

---

# 24. `objects`

Store the latest normalized object.

Suggested columns:

```text
object_key
source
object_type
external_id

title
snippet
body

url
container
author
state

created_at
updated_at

native_json

content_hash
last_fetched_at
first_seen_at
```

Provide useful indexes.

---

# 25. Object text

It may be useful to maintain separately searchable text containing:

- description
- comments
- reviews
- PR review comments
- commits
- history
- changelog
- activity

Preserve enough structure to explain where evidence came from.

Do not create one enormous untraceable text field if structured source attribution can be preserved cheaply.

---

# 26. Relations table

Suggested data:

```text
from_key
to_key
relation_type
detected_by
confidence
first_seen
last_seen
```

Avoid duplicate identical edges.

---

# 27. Unresolved references

When text points to an object that has not yet been fetched, store it.

For example:

```text
OPEN-4679 references OPEN-4012
```

If `OPEN-4012` is not cached:

```text
object_refs:
    target = jira:issue:OPEN-4012
    status = unresolved
```

It becomes a candidate for graph expansion.

Do not automatically fetch every unresolved reference immediately.

Use query relevance and graph budgets.

---

# 28. Full-text search

Use SQLite FTS5.

Search at least:

- title
- description/body
- comments
- relevant history text

Full-text retrieval should work without an LLM.

IssueMesh must remain useful if the model endpoint is temporarily unavailable.

---

# 29. Semantic search

Support embeddings.

Do not require a separate vector database.

For initial implementation, storing embeddings inside SQLite is preferred.

For moderate datasets, cosine similarity may be computed in Rust.

Design the abstraction so a future SQLite vector extension or ANN implementation could be added later.

Do not prematurely introduce a separate vector service.

---

# 30. Hybrid ranking

Combine:

- lexical relevance
- semantic similarity
- graph proximity
- freshness

A starting conceptual formula may be:

```text
0.35 lexical
0.35 semantic
0.20 graph relevance
0.10 freshness
```

Do not hard-code this without making ranking understandable and testable.

Implement a clean ranking abstraction so the formula can evolve.

---

# 31. Cache policy

The cache is not simply:

> cached means never fetch again.

Use object freshness.

Suggested policy:

```text
open/recent issue       ~1 hour
recently closed         ~6 hours
old closed issue        several days
repository metadata     ~24 hours
```

These values should be configurable.

Prefer checking source `updated_at` before performing expensive deep reads when practical.

Use ETag / conditional requests if supported cleanly by a source.

---

# 32. Deep search

Deep search is a central feature.

Given a query such as:

> Find everything connected with BGP support for multi-site OpenStack.

IssueMesh should be able to:

1. search all relevant trackers
2. retrieve strong initial candidates
3. deep-read important objects
4. discover related Jira tickets
5. discover GitHub PRs and commits
6. discover Launchpad bugs
7. inspect comments for additional references
8. follow selected graph relations
9. repeat up to the configured depth
10. search the enlarged local graph
11. rank evidence
12. produce a coherent answer

This is not equivalent to merely returning three independent search result lists.

---

# 33. Consistency engine

Implement consistency analysis across related objects.

First version should support deterministic checks where possible.

Examples:

```text
Jira issue = Done
GitHub implementation PR = Open

Launchpad bug = Fix Released
linked GitHub PR = not merged

Jira ticket says blocked by X
X is closed/resolved

GitHub PR says "Fixes OPEN-123"
OPEN-123 has no reverse/native relation

Jira fix version claims release complete
related implementation objects remain open
```

A consistency result should resemble:

```rust
struct ConsistencyFinding {
    finding_type: String,
    severity: Severity,
    objects: Vec<ObjectRef>,
    evidence: Vec<EvidenceRef>,
    explanation: String,
}
```

Do not modify tracker data.

Only report inconsistencies.

---

# 34. LLM role

Use the model only for bounded tasks such as:

1. natural-language query planning
2. generation of semantic search variants
3. ambiguous relation classification
4. evidence synthesis
5. explanation of consistency findings

The model must not:

- invent tracker objects
- invent relation IDs
- claim evidence not retrieved
- make network calls
- decide that cached data overrides newer tracker data
- modify tracker state

---

# 35. OpenAI-compatible endpoint

Do not hardcode OpenAI-specific hostnames.

Implement a provider-neutral OpenAI-compatible client.

Configuration:

```text
base_url
api_key
chat_model
embedding_model
```

The endpoint may eventually point to:

- OpenAI
- enterprise API gateway
- Azure-compatible gateway
- vLLM
- another OpenAI-compatible server

Keep this abstraction isolated from the rest of IssueMesh.

Suggested module:

```text
model/
  client.rs
  planner.rs
  embeddings.rs
  synthesis.rs
  relation_classifier.rs
```

---

# 36. LLM evidence constraints

When generating the final answer, pass structured evidence identifiers.

The model should be instructed:

- use only provided evidence
- distinguish fact from inference
- never invent IDs
- never invent status
- explicitly state uncertainty
- cite evidence objects

Validate model-produced citations against the evidence set before rendering.

If the model refers to an object not in the evidence set, drop or flag that reference.

---

# 37. Search result page

Do not simply print Markdown returned by the model.

Render a structured result.

Suggested layout:

```text
BGP support for multi-site OpenStack
────────────────────────────────────────

SUMMARY

Canonical currently has several independent BGP
capabilities, but the complete multi-site architecture
is not yet covered.

KEY FINDINGS

● OPEN-4679
  Native OVN BGP support
  Jira · In Progress · updated 2 days ago

● PF-6946
  Tenant subnet advertisement
  Jira · Open

● canonical/project#892
  Initial BGP agent integration
  GitHub · Merged

RELATIONSHIPS

OPEN-4679
 ├── implemented by → canonical/project#892
 ├── related to     → PF-6946
 └── references     → LP#2167315

CONSISTENCY

⚠ OPEN-4679 is still In Progress while its implementation
  PR is already merged.

SEARCH COVERAGE

47 objects
16 deep reads
29 relations followed
3 trackers
graph depth 2
```

Use good typography.

---

# 38. Evidence cards

Each evidence object should be expandable.

Show:

- title
- tracker
- object ID
- state/status
- URL
- creation/update date
- author
- relevant excerpt
- matching reason
- relation path
- important native fields

Allow expansion into:

- complete description
- comments
- reviews
- history/changelog
- discovered relations

Do not load extremely large bodies into initial page HTML unnecessarily.

---

# 39. Original tracker links

Always provide clear links to authoritative tracker objects.

Open them in a new browser tab.

The local cache must never hide the original source.

---

# 40. Search history

Persist useful search metadata.

Each search should have a stable ID.

Store:

```text
query
timestamp
search plan
sources queried
objects consulted
objects deep-read
relations followed
warnings
model used
prompt version
final structured answer
duration
```

Never store secrets.

---

# 41. Reproducibility

A user must be able to inspect:

> Why did IssueMesh give me this answer?

Provide an expandable section or dedicated search-run page containing:

```text
Search plan
Sources contacted
Objects retrieved
Objects reused from cache
Deep-read objects
Relation paths
Consistency findings
Model name
Prompt version
Search budgets
```

This is important.

IssueMesh should favour explainability over opaque agent behaviour.

---

# 42. Memory page

Create a first-class `Memory` page.

It must display the local datastore state.

At minimum show:

```text
Database file
absolute database path
file size on disk

number of objects
number by tracker
number by object type

number of relations

FTS size if measurable
embedding storage size if measurable

number of unresolved references

database health
WAL size
free pages
schema version

oldest object
newest object
last write
```

Use SQLite pragmas and filesystem metadata.

Useful SQLite values include:

```sql
PRAGMA page_count;
PRAGMA page_size;
PRAGMA freelist_count;
PRAGMA integrity_check;
```

Do not run expensive integrity checks on every page load.

Offer a manual action.

---

# 43. Memory management operations

Provide safe UI actions such as:

```text
Optimise database
Rebuild full-text index
Purge stale objects
Clear search history
Run integrity check
```

Destructive actions must require confirmation.

Do not provide a prominent "delete everything" action without an explicit confirmation flow.

Tracker data is read-only, but local memory management may alter the SQLite database.

---

# 44. Sources page

Provide a `Sources` page.

Show tracker connectivity and local coverage.

Example:

```text
GitHub
  canonical/project-a       1,124 objects
  canonical/project-b         842

Jira
  OPEN                      4,821
  PF                        1,792

Launchpad
  snap-openstack              451
  sunbeam-charms              291
```

Allow selection of a source/container to inspect:

- object count
- object types
- states
- last update
- relation count
- cache freshness

---

# 45. Health page

Provide an operational health page.

Example:

```text
GitHub       ● Healthy      182 ms
             quota remaining: 4,812

Jira         ● Healthy      342 ms

Launchpad    ● Healthy      871 ms

Model        ● Healthy      216 ms

Embeddings   ● Healthy      228 ms

Database     ● Healthy        4 ms
```

Use actual probes.

Health should indicate:

- enabled/disabled
- connectivity
- auth state
- latency
- rate limit where available
- token expiry where available
- last successful call
- last error

Never display secrets.

---

# 46. API audit

Audit external calls.

Store only safe metadata such as:

```text
timestamp
source
endpoint class
HTTP status
latency
request type
error class
```

Do not store:

- token
- Authorization header
- entire HTTP body
- sensitive query content unnecessarily

---

# 47. Error classification

Handle common remote failures explicitly.

Examples:

```text
401 -> authentication invalid
403 + rate-limit response -> throttled
404 -> object not found
422 GitHub -> malformed query
429 -> rate limited
5xx -> remote source error
timeout -> source timeout
```

The UI should give understandable warnings.

One failing tracker must not necessarily fail the complete search.

Example:

> Jira and GitHub were searched successfully. Launchpad timed out, so this answer may be incomplete.

---

# 48. GitHub

Follow `CONNECTING_TRACKERS.md`.

Support at minimum:

- issue search
- PR search
- repository filters
- organisation filters
- commit search when relevant
- code search only when explicitly useful and rate budgets allow
- issue deep read
- PR deep read
- comments
- reviews
- review comments
- commits
- relation discovery
- closing issue references
- timeline references

Preserve the documented GitHub search quirks.

Use GraphQL where the reference implementation does so efficiently.

Fall back gracefully when GraphQL fails.

---

# 49. Launchpad

Follow `CONNECTING_TRACKERS.md`.

Support:

- global bug search
- project-specific search
- source package search
- status filtering
- importance filtering
- tags
- modified-since
- assignee
- bug details
- bug tasks
- comments/messages
- activity/history
- duplicates
- linked branches
- relation extraction

Respect Launchpad pacing.

Do not hammer the service.

---

# 50. Jira

Follow `CONNECTING_TRACKERS.md`.

Target Jira Cloud.

Support:

- JQL search
- free-text query conversion
- direct issue-key queries
- project filters
- person filters
- date filters
- statuses
- deep issue retrieval
- description
- comments
- changelog
- issue links
- remote links
- parent
- subtasks
- fix versions
- components
- labels

Correctly flatten Atlassian Document Format into searchable text while preserving useful structure.

---

# 51. Connector abstraction

Keep source-specific behaviour isolated.

A conceptual interface:

```rust
#[async_trait]
trait Connector: Send + Sync {
    fn source(&self) -> Source;

    async fn search(
        &self,
        query: &SearchQuery,
    ) -> Result<Vec<Evidence>>;

    async fn get_object(
        &self,
        object: &ObjectRef,
    ) -> Result<Option<Evidence>>;

    async fn get_thread(
        &self,
        object: &ObjectRef,
        limit: usize,
    ) -> Result<Option<Thread>>;

    async fn get_related(
        &self,
        object: &ObjectRef,
    ) -> Result<Vec<Relation>>;

    async fn health(&self) -> Health;
}
```

Adapt as needed.

Do not leak source-specific JSON into search orchestration unnecessarily.

---

# 52. Proposed Rust source layout

Use a clear structure similar to:

```text
src/
 ├── main.rs
 │
 ├── config/
 │   ├── mod.rs
 │   ├── config.rs
 │   ├── secrets.rs
 │   └── paths.rs
 │
 ├── web/
 │   ├── mod.rs
 │   ├── server.rs
 │   ├── routes.rs
 │   ├── setup.rs
 │   ├── search.rs
 │   ├── memory.rs
 │   ├── sources.rs
 │   ├── health.rs
 │   ├── settings.rs
 │   ├── history.rs
 │   └── sse.rs
 │
 ├── http/
 │   ├── mod.rs
 │   ├── readonly.rs
 │   ├── rate_limit.rs
 │   └── audit.rs
 │
 ├── connectors/
 │   ├── mod.rs
 │   ├── github.rs
 │   ├── launchpad.rs
 │   └── jira.rs
 │
 ├── domain/
 │   ├── mod.rs
 │   ├── object.rs
 │   ├── evidence.rs
 │   ├── relation.rs
 │   ├── query.rs
 │   ├── health.rs
 │   └── search_run.rs
 │
 ├── store/
 │   ├── mod.rs
 │   ├── sqlite.rs
 │   ├── objects.rs
 │   ├── text.rs
 │   ├── relations.rs
 │   ├── fts.rs
 │   ├── embeddings.rs
 │   ├── search_runs.rs
 │   └── stats.rs
 │
 ├── graph/
 │   ├── mod.rs
 │   ├── resolver.rs
 │   ├── extraction.rs
 │   ├── traversal.rs
 │   └── consistency.rs
 │
 ├── search/
 │   ├── mod.rs
 │   ├── planner.rs
 │   ├── orchestrator.rs
 │   ├── deep_search.rs
 │   ├── ranking.rs
 │   ├── jobs.rs
 │   └── events.rs
 │
 ├── model/
 │   ├── mod.rs
 │   ├── client.rs
 │   ├── planner.rs
 │   ├── embeddings.rs
 │   ├── relation_classifier.rs
 │   └── synthesis.rs
 │
 └── util/
     ├── redact.rs
     └── time.rs

templates/
static/
migrations/
tests/
```

This is guidance, not a requirement to create needless files.

Keep modules coherent and compact.

---

# 53. Embedded frontend

Templates and static assets must be included in the final executable.

The release binary should not depend on external template directories.

During development hot reload is optional.

For release:

```text
issuemesh
```

must be enough to launch the complete product.

---

# 54. JavaScript

Use minimal vanilla JavaScript.

JavaScript is acceptable for:

- SSE handling
- dynamic progress indicators
- expandable evidence
- settings validation
- graph display if implemented
- clipboard actions

Avoid a large SPA framework.

Server-rendered HTML should remain the default.

The application should remain understandable without a complicated frontend toolchain.

---

# 55. Accessibility and responsiveness

The interface must work on:

- desktop
- laptop
- tablet

Mobile support should remain usable but is not the primary design target.

Follow normal accessibility practices:

- semantic HTML
- visible focus states
- keyboard-accessible controls
- appropriate contrast
- text alternatives
- not relying only on colour for status

---

# 56. Local binding and network exposure

Default:

```text
127.0.0.1:8765
```

Do not bind to all interfaces by default.

Support:

```bash
issuemesh --port 9000
```

Optionally:

```bash
issuemesh --bind 0.0.0.0
```

If binding beyond localhost, display a prominent security warning.

Future authentication may be added, but local-only operation is the initial security model.

---

# 57. Startup terminal output

Keep it concise.

Example:

```text
IssueMesh 0.1.0

Database     ~/.local/share/issuemesh/issuemesh.db
GitHub       connected
Jira         connected
Launchpad    enabled
Model        connected

Web UI:
http://127.0.0.1:8765
```

If onboarding is required:

```text
IssueMesh 0.1.0

First-time setup required.

Open:
http://127.0.0.1:8765/setup
```

No terminal UI library is required.

The "terminal" aspect is the web design, not a curses interface.

---

# 58. Snap readiness

Do not necessarily build the Snap first unless convenient.

But architecture must be Snap-friendly from the beginning.

Assume future strict confinement.

Persistent state should be relocatable to:

```text
$SNAP_USER_COMMON
```

The application must not hardcode access to arbitrary home-directory locations.

Use a central path abstraction.

The Snap should eventually need primarily:

- network access
- its own persistent user storage

Avoid runtime assumptions incompatible with strict confinement.

---

# 59. Compact release binary

Configure release builds sensibly.

Consider:

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

Measure rather than blindly optimise.

Do not sacrifice correctness or TLS security to reduce a few megabytes.

Avoid OpenSSL system dependency if Rustls gives a cleaner portable build.

---

# 60. Database portability

The SQLite database should be movable along with its application data directory.

Do not store absolute filesystem paths inside object records unless necessary.

Database schema must have an explicit version.

Provide automatic forward migration.

Never silently destroy an incompatible database.

---

# 61. Health without model

If the model endpoint is unavailable:

- tracker searching must still work
- direct ID lookup must still work
- local FTS search must still work
- object graph traversal must still work
- deterministic relations must still work
- memory and health pages must work

The UI should clearly state:

> Model unavailable. Showing retrieval results without AI synthesis.

This graceful degradation is required.

---

# 62. Health without one tracker

Likewise, if one source is unavailable:

```text
GitHub       Healthy
Jira         Authentication failed
Launchpad    Healthy
```

search the remaining sources.

Do not crash.

---

# 63. Logging

Use structured logging through `tracing`.

Support a configurable log level.

Default logs should be useful but restrained.

Never log:

- API keys
- Authorization headers
- complete credentials
- secrets file contents

Implement explicit redaction utilities and tests.

---

# 64. Testing strategy

Implement meaningful tests.

## Unit tests

At minimum test:

- identifier parsing
- Jira key false-positive exclusions
- Launchpad reference parsing
- GitHub URL parsing
- ADF flattening
- query plan validation
- graph budget enforcement
- relation deduplication
- ranking
- secret redaction
- read-only HTTP method restrictions
- cache freshness decisions

## Connector tests

Use mocked HTTP responses.

Do not rely on live GitHub/Jira/Launchpad for normal CI tests.

Test:

- pagination
- rate-limit handling
- malformed responses
- 401
- 403
- 404
- 422
- timeout
- tracker-specific quirks

## Database tests

Use temporary SQLite databases.

Test:

- migrations
- CRUD
- FTS
- relation graph
- search runs
- integrity-related utilities

## Web tests

Test important routes.

At minimum:

```text
/
 /setup
 /search
 /memory
 /sources
 /health
 /settings
```

Test onboarding state transitions.

---

# 65. Search integration tests

Create fixture-based tests representing linked tracker objects.

Example fixture graph:

```text
Jira OPEN-100
   ↓ implements
GitHub org/repo#200
   ↓ fixes
Launchpad #3000000
```

Test that a deep search starting with `OPEN-100` discovers the complete graph.

Also test:

- graph depth 0
- graph depth 1
- graph node budget
- unresolved references
- duplicate loops

No infinite graph traversal must be possible.

---

# 66. Performance goals

IssueMesh should feel fast.

Target:

- startup in well under a few seconds on normal Linux hardware
- local search perceived as immediate
- web page load lightweight
- remote source searches concurrent
- no unnecessary re-embedding
- no unnecessary deep reads
- database writes batched where useful

Do not block Axum worker tasks with expensive synchronous work.

Use `spawn_blocking` where appropriate for blocking SQLite or CPU-heavy operations if necessary.

---

# 67. Data refresh

Do not implement a permanent background crawler in the first version.

The primary model is query-driven acquisition.

Objects are learned when:

- searched
- referenced during deep search
- explicitly opened
- refreshed

This keeps IssueMesh small and respectful of API limits.

Future scheduled crawling should remain possible.

Architect accordingly, but do not over-engineer it now.

---

# 68. No silent model hallucination

The final synthesis layer must explicitly constrain the model.

Create a versioned system prompt roughly equivalent to:

> You are analysing engineering tracker evidence. Use only the supplied evidence. Never invent object identifiers, statuses, relationships, dates or claims. Distinguish evidence from inference. If evidence is insufficient, state that clearly. Every material finding must reference one or more supplied object IDs.

Validate output where practical.

Store the prompt version with each search run.

---

# 69. Search result confidence

Where useful distinguish:

```text
Confirmed
Strongly supported
Inferred
Uncertain
```

Do not attach fake numerical probabilities unless they are meaningful.

Native tracker evidence should visually outrank inferred semantic links.

---

# 70. Optional graph visualisation

A simple graph view is useful, but it is secondary.

If implemented, keep it lightweight.

Example:

```text
OPEN-4679
    │
    ├── implements ──> github#892
    │                    │
    │                    └── fixes ──> LP#2167315
    │
    └── blocks ─────> PF-6946
```

A textual graph is acceptable for the first version.

Do not add a large JavaScript graph framework solely for decoration.

---

# 71. Product wording

Product name:

**IssueMesh**

Suggested description:

> Search, connect and understand engineering issues across your trackers.

Alternative short description for metadata:

> Local engineering issue intelligence across GitHub, Jira and Launchpad.

Do not describe IssueMesh as an autonomous agent.

It is an evidence-driven engineering intelligence/search application.

---

# 72. Expected invocation

Normal use:

```bash
issuemesh
```

Optional parameters may include:

```bash
issuemesh --port 9000
issuemesh --bind 127.0.0.1
issuemesh --data-dir /path
issuemesh --config /path/config.toml
```

Keep the CLI small.

Configuration should normally be performed through the web UI.

---

# 73. Development documentation

Create a concise `README.md`.

Include:

- what IssueMesh is
- architecture
- prerequisites
- build command
- run command
- first-run setup
- data paths
- configuration
- testing
- release build
- future Snap packaging

Also include a short `ARCHITECTURE.md`.

Explain:

- connector boundary
- read-only HTTP invariant
- normalized object model
- datastore
- graph expansion
- search pipeline
- model boundary

Do not create excessive documentation.

---

# 74. Build and run

The repository should ultimately support:

```bash
cargo build
cargo test
cargo run
```

and:

```bash
cargo build --release
./target/release/issuemesh
```

with no frontend build prerequisite.

---

# 75. Implementation approach

Do not attempt to build everything as one monolithic file.

Implement in coherent stages.

Recommended order:

### Stage 1
Project skeleton, configuration, paths, logging, SQLite migrations.

### Stage 2
Read-only HTTP layer and tracker connectors based on `CONNECTING_TRACKERS.md`.

### Stage 3
Normalized object storage and relation extraction.

### Stage 4
Local FTS search and graph traversal.

### Stage 5
Axum server and basic Canonical-branded web UI.

### Stage 6
Onboarding wizard and health/settings.

### Stage 7
Search jobs and SSE progress.

### Stage 8
OpenAI-compatible model client and query planning.

### Stage 9
Embeddings and hybrid retrieval.

### Stage 10
Deep search and consistency engine.

### Stage 11
Polished result rendering, memory page and source inspection.

### Stage 12
Hardening, tests, compact release build and Snap preparation.

After each stage keep:

```bash
cargo fmt --check
cargo clippy
cargo test
```

working.

---

# 76. Engineering quality requirements

Avoid:

- giant functions
- global mutable state
- unnecessary `unwrap()`
- leaking implementation-specific JSON through the application
- duplicated tracker logic
- uncontrolled task spawning
- hidden network side effects
- hardcoded secrets
- silent failures
- excessive abstraction before it is needed

Prefer:

- typed domain structures
- explicit errors
- small interfaces
- deterministic behaviour
- good tracing
- testable modules
- source provenance
- bounded resources

---

# 77. Acceptance criteria

The first complete version is acceptable when all of the following work.

## Installation and startup

```text
✓ cargo build --release
✓ one executable starts the entire application
✓ no Node/Python/external database required
✓ browser UI available at localhost:8765
```

## Onboarding

```text
✓ first-run wizard
✓ GitHub setup and connection test
✓ Jira setup and connection test
✓ Launchpad enable/test
✓ OpenAI-compatible endpoint setup/test
✓ embedding model test
✓ datastore location shown
✓ settings persisted safely
```

## Tracker behaviour

```text
✓ GitHub search
✓ Jira search
✓ Launchpad search
✓ deep read
✓ comments/history
✓ relations
✓ pagination
✓ source errors handled
✓ strictly read-only
```

## Local memory

```text
✓ SQLite datastore
✓ cached objects
✓ cached relations
✓ FTS
✓ embeddings
✓ datastore stats
✓ disk size visible
✓ object counts visible
✓ database path visible
✓ database health visible
```

## Search

```text
✓ natural-language search
✓ direct object-ID lookup
✓ local + remote search
✓ deep graph traversal
✓ bounded search budgets
✓ consistency checks
✓ evidence ranking
✓ answer synthesis
```

## UI

```text
✓ Canonical-inspired branding
✓ clean terminal/search feel
✓ responsive
✓ actual animated search progress
✓ source cards
✓ expandable evidence
✓ links to authoritative tracker objects
✓ Memory page
✓ Sources page
✓ Health page
✓ Settings page
```

## Resilience

```text
✓ works if one tracker is unavailable
✓ basic search works if model is unavailable
✓ no secrets leaked to browser/log/database
✓ no unbounded traversal
✓ no write calls to trackers
```

## Portability

```text
✓ embedded frontend assets
✓ embedded migrations
✓ relocatable data directory
✓ Rustls or equally portable TLS approach
✓ architecture ready for Snap strict confinement
```

---

# 78. Final instruction

Build IssueMesh as a real product, not a demonstration.

Prioritise, in this order:

1. correctness
2. evidence provenance
3. read-only security
4. reliability
5. simple user experience
6. useful deep search
7. compactness
8. visual polish

Whenever there is tension between clever agent behaviour and deterministic evidence handling, choose deterministic evidence handling.

Whenever an LLM result conflicts with authoritative tracker data, tracker data wins.

Whenever local cache data conflicts with a newer remote object, the remote object wins.

Whenever a relationship is uncertain, preserve its provenance and uncertainty.

Do not ask for architectural clarification unless implementation is genuinely blocked.

Make sensible engineering choices consistent with the principles above and document significant deviations.

The final product should feel like:

> **a small local engineering intelligence engine that gradually builds a trustworthy mesh of the issues, bugs, PRs, commits and relationships the user has explored.**

That is IssueMesh.
