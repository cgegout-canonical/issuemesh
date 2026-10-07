# Connecting to GitHub, Launchpad and Jira the Cognition way

This document describes, end to end, how Cognition talks to GitHub, Launchpad and Jira, so that
another Rust application can connect to the same three systems with the same guarantees
(read-only by construction, credentials outside the code, the same endpoints, the same quirks
handled). It is self-contained: everything a second application needs is reproduced here, with
the code reduced to what matters. The reference implementation lives in
`src/connectors/{http,github,launchpad,jira,mod}.rs`.

Summary of the three integrations:

| Source | Base URL | Auth | Secrets keys | Search primitive | Deep read | Rate limits |
|---|---|---|---|---|---|---|
| GitHub | `https://api.github.com` | Bearer token (fine-grained PAT, read-only) | `GITHUB_TOKEN` | `GET /search/{issues,commits,code,repositories}` | REST `/repos/...` + one GraphQL query for PRs | 5 000 req/h core, 30/min search, 10/min code search |
| Launchpad | `https://api.launchpad.net/devel` | none (anonymous, public data) | none (`LAUNCHPAD_TOKEN` accepted but unused) | `GET /bugs?ws.op=searchTasks` or `GET /<project>?ws.op=searchTasks` | `/bugs/{id}` + `bug_tasks`, `messages`, `activity` | informal; keep pacing |
| Jira Cloud | `https://<site>.atlassian.net` | HTTP Basic (email + API token) | `JIRA_URL`, `JIRA_EMAIL`, `JIRA_TOKEN` | `POST /rest/api/3/search/jql` | `/rest/api/3/issue/{key}?expand=changelog` + `/remotelink` + `/comment` | Atlassian per-tenant |

## 1. Dependencies

The connectors use blocking `reqwest` with rustls, `serde_json` for untyped JSON, `anyhow` for
errors, and `regex` for cross-system identifier patterns.

```toml
[dependencies]
reqwest    = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "gzip", "blocking"] }
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow     = "1"
regex      = "1"
dirs       = "5"
```

Untyped `serde_json::Value` is deliberate: the three APIs return large, evolving documents and
each connector only lifts a dozen fields into a common record. A small helper reads a string field
with a default:

```rust
fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
```

## 2. Credentials: where they live and how they are loaded

Rules Cognition applies, and which a second application should keep:

- Credentials live only in `~/.cognition/secrets.env` (mode 0600). Never in source, config files,
  prompts, logs, memory stores or git. Create it with `touch` + `chmod 600` before writing to it.
- The file is `KEY=VALUE` lines. Blank lines and `#` comments are allowed. Values are never logged.
- Credentials are read once at process start into a `BTreeMap<String, String>` and handed to each
  connector constructor. A missing key disables that connector with a warning; it never panics.
- The HTTP layer is read-only by construction (section 3), so even a credential with write scopes
  cannot be used to write.
- The connector never prints the token. Health output shows who the token authenticates as, and for
  GitHub the token's expiry date, nothing else.

Keys used:

```
# ~/.cognition/secrets.env
GITHUB_TOKEN=github_pat_...        # fine-grained PAT, read-only (see 4.1)
JIRA_URL=https://your-site.atlassian.net
JIRA_EMAIL=you@example.com
JIRA_TOKEN=...                     # Atlassian API token (see 6.1)
# Launchpad: nothing; public data is anonymous
```

Loader (the whole thing):

```rust
use std::collections::BTreeMap;

pub fn load_secrets(path: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.split("   #").next().unwrap_or(v).trim().trim_matches('"').to_string();
                out.insert(k.trim().to_string(), v);
            }
        }
    }
    out
}
// let secrets = load_secrets(&dirs::home_dir().unwrap().join(".cognition/secrets.env").to_string_lossy());
```

Optionally, `COGNITION_HOME` overrides `~/.cognition`.

## 3. The shared read-only HTTP client

All three connectors go through one small wrapper around `reqwest::blocking::Client`. It is the
single place where read-only is enforced:

- `GET` is always allowed.
- `POST` is allowed only for an explicit allow-list of *search* endpoints (reads that need a body):
  GitHub `/graphql`, Jira `/rest/api/3/search/jql` and `/rest/api/3/jql/parse`. Anything else is
  refused before a request is built. `PUT`, `PATCH` and `DELETE` do not exist on the client.
- Auth is either a bearer token (GitHub) or HTTP Basic (Jira). Launchpad uses neither.
- One 30 s timeout, gzip on, a descriptive `User-Agent` (GitHub requires one).
- Non-2xx responses become an error string `HTTP <status> : <first 200 chars of body>`. Callers match
  on `"404"` in the message to turn "not found" into `Ok(None)`.
- Latency of the last call is kept for health reporting.
- GitHub announces a fine-grained token's expiry in the response header
  `github-authentication-token-expiration`; the client stores that date (only the date) so the
  application can warn before the token dies.

```rust
use anyhow::{bail, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde_json::Value;
use std::sync::{atomic::{AtomicI64, Ordering}, RwLock};
use std::time::{Duration, Instant};

pub struct ReadOnlyHttp {
    client: Client,
    base: String,
    bearer: RwLock<Option<String>>,
    basic: Option<(String, String)>,
    post_allow: Vec<String>,
    pub last_latency_ms: AtomicI64,
    pub token_expiry: RwLock<Option<String>>,
}

impl ReadOnlyHttp {
    pub fn new(base: &str, bearer: Option<String>, post_allow: &[&str], user_agent: &str) -> Result<Self> {
        let client = Client::builder().timeout(Duration::from_secs(30)).user_agent(user_agent).gzip(true).build()?;
        Ok(Self { client, base: base.trim_end_matches('/').to_string(), bearer: RwLock::new(bearer), basic: None,
                  post_allow: post_allow.iter().map(|s| s.to_string()).collect(),
                  last_latency_ms: AtomicI64::new(0), token_expiry: RwLock::new(None) })
    }

    /// HTTP Basic variant (Atlassian API tokens).
    pub fn new_basic(base: &str, user: &str, secret: &str, post_allow: &[&str], user_agent: &str) -> Result<Self> {
        let mut me = Self::new(base, None, post_allow, user_agent)?;
        me.basic = Some((user.to_string(), secret.to_string()));
        Ok(me)
    }

    fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        let bearer = self.bearer.read().ok().and_then(|b| b.clone());
        match (bearer, &self.basic) {
            (Some(b), _) => req.bearer_auth(b),
            (None, Some((u, p))) => req.basic_auth(u, Some(p)),
            _ => req,
        }
    }

    fn finish(&self, t0: Instant, r: Response) -> Result<Value> {
        self.last_latency_ms.store(t0.elapsed().as_millis() as i64, Ordering::Relaxed);
        if let Some(v) = r.headers().get("github-authentication-token-expiration").and_then(|h| h.to_str().ok()) {
            if let Ok(mut g) = self.token_expiry.write() { *g = Some(v.to_string()); }
        }
        let status = r.status();
        let text = r.text()?;
        if !status.is_success() {
            bail!("HTTP {} : {}", status.as_u16(), text.chars().take(200).collect::<String>());
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let t0 = Instant::now();
        let req = self.auth(self.client.get(format!("{}{}", self.base, path)).query(query)).header("Accept", "application/json");
        self.finish(t0, req.send()?)
    }

    /// POST only for allow-listed search endpoints.
    pub fn post_search(&self, path: &str, body: &Value) -> Result<Value> {
        if !self.post_allow.iter().any(|p| path.starts_with(p)) {
            bail!("ReadOnlyViolation: POST {path} is not an allow-listed search endpoint");
        }
        let t0 = Instant::now();
        let req = self.auth(self.client.post(format!("{}{}", self.base, path)).json(body)).header("Accept", "application/json");
        self.finish(t0, req.send()?)
    }
}
```

## 4. GitHub

### 4.1 Credential

A **fine-grained personal access token**, read-only: `Contents: read`, `Issues: read`,
`Pull requests: read`, `Metadata: read`, `Discussions: read`, resource owner set to the organisations
you need (plus public repositories). A classic PAT also works but the fine-grained token is the one
whose expiry GitHub reports in the response header. Revoke or rotate at GitHub → Settings →
Developer settings → Personal access tokens. Put it in `GITHUB_TOKEN`.

### 4.2 Client construction

```rust
const API: &str = "https://api.github.com";
let tok = secrets.get("GITHUB_TOKEN").cloned().ok_or_else(|| anyhow!("GITHUB_TOKEN missing"))?;
let http = ReadOnlyHttp::new(API, Some(tok), &["/graphql"], "myapp/1.0 (read-only)")?;
```

### 4.3 Health

`GET /user` (gives the login the token authenticates as) and `GET /rate_limit` (read
`resources.core.remaining`). Report the expiry date captured by the client if present. Measured:
about 220 ms.

### 4.4 Search

Endpoint `GET /search/{kind}` with `kind` in `issues` (issues and PRs), `commits`, `code`,
`repositories`. Query parameters: `q`, `per_page` (1..100), `sort` (`updated` for issues), `order=desc`.
The `q` string is a space-separated list of qualifiers built as follows:

1. Free text, cleaned: remove `: " ( ) [ ]`, drop `AND`/`OR`/`NOT`, keep at most 8 words and 200
   characters. GitHub returns 422 for text that looks like a qualifier or exceeds 256 characters.
2. `repo:<owner>/<name>` for each repository filter, otherwise `org:<org>` (Cognition defaults to
   `org:canonical`).
3. `involves:<login>` for each person.
4. `updated:>=YYYY-MM-DD` for a time floor (issues kind only).

Two quirks you must reproduce:

- Since 2026-10 `/search/issues` rejects a query without a type qualifier (422 "Query must include
  `is:issue` or `is:pull-request`"). If the caller did not supply `is:issue`, `is:pull-request`,
  `is:pr` or `type:`, run the query twice, once with each, merge, sort by `updated_at` descending and
  truncate to `per_page`.
- Free terms are conjunctive. If a query with more than three words returns nothing, retry once with
  the first three words.

Items are read from `items[]`. For issues/PRs: `number`, `title`, `body`, `html_url`, `state`,
`user.login`, `created_at`, `updated_at`, `labels[].name`, `assignees[].login`, `comments`;
`pull_request` present means it is a PR; `repository_url` gives the repo (last two path segments).
For commits: `sha`, `commit.message`, `repository.full_name`, `commit.author.date`. For code:
`path`, `repository.full_name`. For repositories: `full_name`, `description`, `pushed_at`.

Cognition identifies objects as `<owner>/<repo>#<number>` for issues and PRs,
`<owner>/<repo>@<sha7>` for commits, `<owner>/<repo>:<path>` for files, `<owner>/<repo>` for repos.

### 4.5 Deep read of an issue or PR

1. `GET /repos/{repo}/issues/{n}`. A 404 means not found. `pull_request` present means it is a PR.
2. For a PR, one GraphQL round trip replaces four to six REST calls (measured about 0.8 s):

```graphql
query($owner:String!,$name:String!,$num:Int!){ repository(owner:$owner,name:$name){ pullRequest(number:$num){
  number title state isDraft merged mergedAt createdAt updatedAt url bodyText author{login} baseRefName headRefName mergeable
  labels(first:20){nodes{name}} assignees(first:10){nodes{login}}
  closingIssuesReferences(first:20){nodes{number repository{nameWithOwner}}}
  timelineItems(first:100, itemTypes:[CROSS_REFERENCED_EVENT,CONNECTED_EVENT,REFERENCED_EVENT,CLOSED_EVENT]){nodes{__typename
    ... on CrossReferencedEvent{source{__typename ... on Issue{number repository{nameWithOwner}} ... on PullRequest{number repository{nameWithOwner}}}}
    ... on ConnectedEvent{subject{__typename ... on Issue{number repository{nameWithOwner}} ... on PullRequest{number repository{nameWithOwner}}}}
    ... on ReferencedEvent{commit{abbreviatedOid}} ... on ClosedEvent{closer{__typename ... on Commit{abbreviatedOid}}} }}
  reviews(first:50){nodes{state author{login} submittedAt body}}
  reviewThreads(first:50){nodes{comments(first:20){nodes{author{login} createdAt path body}}}}
  commits(first:100){nodes{commit{abbreviatedOid messageHeadline author{name date}}}}
  comments(first:50){nodes{author{login} createdAt body}} }}}
```

   Sent as `POST /graphql` with `{"query": ..., "variables": {"owner","name","num"}}`. Check the
   `errors` array first. If GraphQL fails, fall back to REST: `/repos/{repo}/pulls/{n}`,
   `/pulls/{n}/reviews`, `/pulls/{n}/comments`, `/pulls/{n}/commits` (each `per_page=50`).
3. For an issue, append `GET /repos/{repo}/issues/{n}/comments?per_page=50`.
4. Commits: `GET /repos/{repo}/commits/{sha}` (read `files[].filename`). Repos: `GET /repos/{repo}`
   and `GET /repos/{repo}/releases?per_page=10`.

The body Cognition stores is the description followed by sections `Reviews:`, `Review comments:`,
`Commits:`, `Comments:`, each line prefixed with the ISO timestamp (19 chars) and author.

### 4.6 Thread

Issue/PR body first, then `GET /repos/{repo}/issues/{n}/comments?per_page=100` in order.

### 4.7 Relations

- PR via GraphQL: `closingIssuesReferences` → `closes`; timeline cross-references → `references`
  (from the other object to this one); timeline commits and `commits` → `commit`.
- Issue via REST: `GET /repos/{repo}/issues/{n}/timeline?per_page=100`; event `cross-referenced`
  (`source.issue`) → `references`; `connected`/`closed` with `commit_id` → `closes`.
- Text patterns in the body (section 7), including same-repo short references `#123`.

## 5. Launchpad

### 5.1 Credential

None. Public bugs, projects and people are readable anonymously on
`https://api.launchpad.net/devel`. Private data would need Launchpad OAuth (the `launchpadlib`
desktop flow); Cognition does not implement it. The connector accepts a `LAUNCHPAD_TOKEN` key in the
secrets file only to report "token present (unused)".

### 5.2 Client construction

```rust
const API: &str = "https://api.launchpad.net/devel";
let http = ReadOnlyHttp::new(API, None, &[], "myapp/1.0 (read-only)")?;
```

### 5.3 Health

`GET /bugs/1`. About 1.3 s cold. Auth reported as `none`.

### 5.4 Collections and pagination

Launchpad returns collections as `{"entries": [...], "next_collection_link": ...}`. Page with
`ws.start` and `ws.size` (max 75) until the wanted number of entries is reached, the page is empty
or there is no `next_collection_link`:

```rust
fn collection(http: &ReadOnlyHttp, path: &str, q: &[(&str, String)], limit: usize) -> Vec<Value> {
    let mut out = vec![]; let mut start = 0usize;
    loop {
        let mut qq = q.to_vec();
        qq.push(("ws.start", start.to_string())); qq.push(("ws.size", limit.min(75).to_string()));
        let Ok(v) = http.get(path, &qq) else { break };
        let entries: Vec<Value> = v["entries"].as_array().cloned().unwrap_or_default();
        let n = entries.len();
        out.extend(entries);
        if out.len() >= limit || n == 0 || v.get("next_collection_link").is_none() { break; }
        start += n;
    }
    out.truncate(limit);
    out
}
```

Links in Launchpad documents are full URLs (`bug_link`, `owner_link`, `assignee_link`,
`milestone_link`); take the last path segment and strip the leading `~` to get the id or person name.

### 5.5 Search

`searchTasks` is the one search operation. Target is the global collection `/bugs` or a project
(`/<project>`) or a distribution source package (`/<distro>/+source/<package>`), with query
parameters:

| parameter | value |
|---|---|
| `ws.op` | `searchTasks` |
| `order_by` | `-date_last_updated` |
| `search_text` | free text |
| `status` | repeated; default is open statuses only. To include history pass all of: New, Incomplete, Opinion, Invalid, Won't Fix, Expired, Confirmed, Triaged, In Progress, Deferred, Fix Committed, Fix Released, Does Not Exist |
| `importance` | repeated (`High`, `Critical`, ...) |
| `tags` | repeated |
| `modified_since` | `YYYY-MM-DDT00:00:00` |
| `assignee` | `https://api.launchpad.net/devel/~<person>` |

Quirk: `search_text` is conjunctive; if more than three words return nothing, retry once with the
first three words. Launchpad has no cheap global directory of projects; names are learned from
results or given by the caller.

Each entry is a bug *task* (one bug can have several, one per affected project or package):
`bug_link`, `title` (`Bug #N in <target>: "<title>"`), `bug_target_name`, `status`, `importance`,
`assignee_link`, `owner_link`, `milestone_link`, `date_created`, `date_last_updated`. The bug id is
the tail of `bug_link`; the web URL is `https://bugs.launchpad.net/<project>/+bug/<id>`.

### 5.6 Deep read of a bug

- `GET /bugs/{id}`: `title`, `description`, `tags`, `owner_link`, `date_created`,
  `date_last_updated`, `duplicate_of_link`.
- `GET /bugs/{id}/bug_tasks` (first 20): the first task is the primary target; the rest is
  "Also affects".
- `GET /bugs/{id}/messages` (first 40 for the body, up to 200 for a thread): `content`,
  `owner_link`, `date_created`, `web_link`. The first message is the description.
- `GET /bugs/{id}/activity` (last 60): `datechanged`, `person_link`, `whatchanged`, `oldvalue`,
  `newvalue`. This is the status history with timestamps.

### 5.7 Relations

`duplicate_of_link` → `duplicates`; `GET /bugs/{id}/duplicates` → `duplicated_by`; each bug task →
`also_affects` a project; `GET /bugs/{id}/linked_branches` → `linked_branch`; text patterns in the
description and in the messages (fixes are usually announced in comments as a GitHub PR URL).

## 6. Jira Cloud

### 6.1 Credential

An **Atlassian API token** created at id.atlassian.com → Security → API tokens, used with HTTP Basic
auth where the user name is the account email and the password is the token. Atlassian tokens
carry the user's full permissions; read-only is enforced client-side by the allow-list in section 3.
Keys: `JIRA_URL` (site base URL, no trailing slash needed), `JIRA_EMAIL`, `JIRA_TOKEN`. Jira
Server/Data Center would use a bearer personal access token and `GET /rest/api/2/search`; Cognition
targets Cloud only.

### 6.2 Client construction

```rust
let url = secrets.get("JIRA_URL").cloned().ok_or_else(|| anyhow!("JIRA_URL missing"))?;
let email = secrets.get("JIRA_EMAIL").cloned().ok_or_else(|| anyhow!("JIRA_EMAIL missing"))?;
let tok = secrets.get("JIRA_TOKEN").cloned().ok_or_else(|| anyhow!("JIRA_TOKEN missing"))?;
let http = ReadOnlyHttp::new_basic(&url, &email, &tok, &["/rest/api/3/search/jql", "/rest/api/3/jql/parse"], "myapp/1.0 (read-only)")?;
```

### 6.3 Health

`GET /rest/api/3/myself` (returns `displayName`). About 0.3 to 0.6 s.

### 6.4 Search with JQL

`POST /rest/api/3/search/jql` with body:

```json
{"jql": "...", "maxResults": 50,
 "fields": ["summary","status","issuetype","priority","assignee","reporter","created","updated",
            "resolutiondate","fixVersions","components","labels","parent","issuelinks","project","subtasks"]}
```

Pagination uses `nextPageToken` from the response. Cognition builds the JQL from the generic query
unless the caller supplies raw JQL, which wins over everything:

| input | JQL clause |
|---|---|
| free text that is only issue keys (`ABC-12 DEF-3`) | `key in (ABC-12,DEF-3)` |
| other free text | `text ~ "<text with double quotes removed>"` |
| projects | `project in (A,B)` |
| person | `(assignee = "p" OR reporter = "p")` |
| time floor / ceiling | `updated >= "YYYY-MM-DD"` / `updated <= "YYYY-MM-DD"` |
| statuses | `status in ("Blocked","In Progress")` |
| nothing at all | `updated >= -30d` |

Clauses are joined with ` AND ` and suffixed with ` ORDER BY updated DESC`. Quirk: `text ~` is
conjunctive on Jira Cloud; if more than three words return nothing, retry once with the first
three words.

Issue key is `key`; web URL is `<JIRA_URL>/browse/<key>`; project is `fields.project.key`. Useful
fields: `fields.summary`, `fields.status.name`, `fields.status.statusCategory.name`,
`fields.issuetype.name`, `fields.priority.name`, `fields.assignee.displayName`,
`fields.reporter.displayName`, `fields.created`, `fields.updated`, `fields.resolutiondate`,
`fields.fixVersions[].name`, `fields.components[].name`, `fields.labels`, `fields.parent.key`,
`fields.issuelinks[]` (each has `type.name`, `type.outward`/`type.inward`, and either `outwardIssue`
or `inwardIssue` with `key` and `fields.status.name`).

### 6.5 Deep read of an issue

- `GET /rest/api/3/issue/{key}?fields=<list above plus description,comment>&expand=changelog`.
  A 404 means not found.
- `GET /rest/api/3/issue/{key}/remotelink`: `object.url` of each remote link (GitHub or Launchpad
  URLs typically).
- `GET /rest/api/3/issue/{key}/comment?maxResults=100&orderBy=created` for a full thread.

`description` and comment `body` are **Atlassian Document Format** (a JSON tree), not text. Flatten
them: concatenate every `text`, add a newline after `paragraph`, `heading`, `listItem`,
`codeBlock`, `blockquote`, `tableRow` and `hardBreak`, render `mention` as `attrs.text` and
`inlineCard` as `attrs.url`.

```rust
pub fn adf_text(v: &Value) -> String {
    fn walk(n: &Value, out: &mut String) {
        match n {
            Value::Object(m) => {
                if let Some(t) = m.get("text").and_then(|x| x.as_str()) { out.push_str(t); }
                let ty = m.get("type").and_then(|x| x.as_str()).unwrap_or("");
                if ty == "hardBreak" { out.push('\n'); }
                if ty == "mention" { out.push_str(&s(&m.get("attrs").cloned().unwrap_or(Value::Null), "text")); }
                if ty == "inlineCard" { out.push_str(&s(&m.get("attrs").cloned().unwrap_or(Value::Null), "url")); }
                if let Some(c) = m.get("content") { for x in c.as_array().cloned().unwrap_or_default() { walk(&x, out); } }
                if matches!(ty, "paragraph" | "heading" | "listItem" | "codeBlock" | "blockquote" | "tableRow") { out.push('\n'); }
            }
            Value::Array(a) => { for x in a { walk(x, out); } }
            Value::String(t) => out.push_str(t),
            _ => {}
        }
    }
    let mut out = String::new(); walk(v, &mut out); out.trim().to_string()
}
```

The changelog (`changelog.histories[]`, each with `created`, `author`, `items[]` with `field`,
`fromString`, `toString`) gives status, assignee, Fix Version, priority, resolution, Sprint and
due-date transitions with timestamps. Cognition keeps only those fields and renders them as a
`History:` section.

### 6.6 Directory

`GET /rest/api/3/project/search?startAt=&maxResults=100&orderBy=key`, paged until `isLast` is true.
Fields `key`, `name`, `projectTypeKey`.

### 6.7 Relations

Typed issue links (relation name is the link's outward or inward phrase with spaces replaced by
`_`, for example `blocks`, `is_blocked_by`, `relates_to`, `duplicates`); `parent`; remote-link URLs
and the description text through the patterns in section 7.

## 7. Cross-system identifiers found in text

The three systems reference each other in free text. Cognition runs the same three regexes over
every body, comment and remote link and emits `references` relations typed by the target system:

```rust
use regex::Regex; use std::sync::LazyLock;
pub static PAT_JIRA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b((?:[A-Z][A-Z0-9]+)-\d+)\b").unwrap());
pub static PAT_LP:   LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:LP:?\s*#|bugs\.launchpad\.net/[\w./+-]+?/\+bug/|launchpad\.net/bugs/)(\d{5,8})").unwrap());
pub static PAT_GH:   LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:github\.com/)?([\w.-]+/[\w.-]+)(?:#|/pull/|/issues/)(\d+)").unwrap());

/// Things that look like Jira keys but are not.
pub fn is_jira_key(k: &str) -> bool {
    let p = k.split('-').next().unwrap_or("");
    !matches!(p, "CVE" | "USN" | "RFC" | "ISO" | "SHA" | "AES" | "UTF" | "IPV" | "IEEE" | "PCI" | "HTTP" | "TLS" | "GPT" | "LP")
}
```

A GitHub match is a PR when the text contains `<repo>/pull/<n>`, otherwise an issue. Skip
self-references (the object's own id).

## 8. The common shape every connector produces

Cognition normalises the three systems into one record so the rest of the application never sees
source-specific JSON. A second application can adopt the same shape or map it to its own:

```rust
pub struct ObjectRef { pub source: String, pub r#type: String, pub id: String }   // key = "source:type:id"
// github: issue|pr "owner/repo#N", commit "owner/repo@sha7", file "owner/repo:path", repo "owner/repo"
// launchpad: bug "2167315", project "microceph", comment "2167315#3"
// jira: issue "OPS-123", comment "OPS-123#c2"

pub struct Evidence {
    pub r#ref: ObjectRef,
    pub title: String,              // "PR #352 …", "LP#2167315 …", "OPS-123 …"
    pub snippet: String,            // ≤ 400 chars of the description
    pub body: Option<String>,       // full text with Reviews/Comments/History sections (deep read only)
    pub url: String,                // human web URL
    pub container: String,          // "github:owner/repo", "launchpad:project", "jira:PROJECT"
    pub author: String,             // login / LP name / display name
    pub created_at: Option<String>, // ISO 8601, 19 chars + "Z"
    pub updated_at: Option<String>,
    pub state: String,              // "open"/"closed"/"merged", LP status, Jira status name
    pub native: BTreeMap<String, Value>, // labels, assignees, importance, priority, fix_versions, links…
}

pub struct Relation { pub from: ObjectRef, pub to: ObjectRef, pub r#type: String, pub detected_by: String } // "native" | "id_pattern" | "url_pattern"

pub trait Connector: Send + Sync {
    fn source(&self) -> &str;
    fn search(&self, q: &SearchQuery) -> Result<Vec<Evidence>>;
    fn get_object(&self, r: &ObjectRef) -> Result<Option<Evidence>>;
    fn get_thread(&self, r: &ObjectRef, window: usize) -> Result<Option<Thread>>;
    fn get_related(&self, r: &ObjectRef, kinds: Option<&[String]>) -> Result<Vec<Relation>>;
    fn health(&self) -> Health;          // ok, detail, auth ("ok"|"failed"|"none"), rate_limit_remaining, latency_ms
    fn credential_expiry(&self) -> Option<String> { None }
}
```

`SearchQuery` carries `query`, `people`, `projects`, `repositories`, `time_from`, `time_to`,
`max_results` and a `native` map with per-source escape hatches: `native.github.{q, kind, org, sort}`,
`native.launchpad.{status, importance, tags, distribution, package}`, `native.jira.{jql, status}`.

Connectors are built from a config map of enabled sources; a constructor error (missing secret) logs
a warning and leaves that source out, so one bad credential never stops the others:

```rust
let mut connectors: BTreeMap<String, Arc<dyn Connector>> = BTreeMap::new();
match GitHubConnector::new(&secrets) { Ok(c) => { connectors.insert("github".into(), Arc::new(c)); } Err(e) => eprintln!("github disabled: {e}") }
```

## 9. Operational behaviour worth copying

- **One search call per query** on GitHub (two when the type qualifier is added), well inside the
  30/min search limit. Keep `per_page` at or below 100.
- **Error classification** from the status in the error message: 401 → credential invalid
  (GitHub tokens expire; the header date tells you when), 403 with a rate-limit body → throttled,
  wait and retry later rather than treating it as an auth failure, 404 → object does not exist,
  422 on GitHub → malformed search query (clean the free text, add the type qualifier).
- **Audit every call** (source, endpoint class, status, latency), never the token or the full body.
- **Health probes** are cheap reads (`/user`, `/bugs/1`, `/myself`) and are what Cognition's
  incident detection watches to open and close source incidents.
- **Refresh**: Cognition re-reads secrets at process start only. Rotating a token means restarting
  the process that holds the client, or calling `set_bearer` on the client if you add that method.
