use super::{
    helper_s, Connector, Evidence, Health, HttpError, ObjectRef, ReadOnlyHttpClient, Relation,
    SearchQuery, Thread, ThreadComment,
};
use crate::util::{PAT_GH, PAT_JIRA, PAT_LP};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

pub struct GitHubConnector {
    pub http: ReadOnlyHttpClient,
    pub default_org: Option<String>,
}

impl GitHubConnector {
    pub fn new(base_url: &str, token: Option<String>, default_org: Option<String>) -> Result<Self> {
        let http = ReadOnlyHttpClient::new(
            base_url,
            token,
            &["/graphql"],
            concat!("issuemesh/", env!("CARGO_PKG_VERSION"), " (read-only)"),
            4,
            0,
        )?;
        Ok(Self { http, default_org })
    }

    fn clean_free_text(s: &str) -> String {
        let stripped: String = s
            .chars()
            .filter(|&c| c != ':' && c != '"' && c != '(' && c != ')' && c != '[' && c != ']')
            .collect();
        let words: Vec<&str> = stripped
            .split_whitespace()
            .filter(|&w| w != "AND" && w != "OR" && w != "NOT")
            .take(8)
            .collect();
        let mut res = words.join(" ");
        if res.len() > 200 {
            res.truncate(200);
        }
        res
    }

    fn build_q(&self, q: &SearchQuery, kind_qualifier: Option<&str>) -> String {
        let mut parts = Vec::new();
        let cleaned = Self::clean_free_text(&q.query);
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }

        if let Some(kq) = kind_qualifier {
            parts.push(kq.to_string());
        }

        if !q.repositories.is_empty() {
            for r in &q.repositories {
                parts.push(format!("repo:{}", r));
            }
        } else if let Some(ref org) = self.default_org {
            parts.push(format!("org:{}", org));
        }

        for p in &q.people {
            parts.push(format!("involves:{}", p));
        }

        if let Some(from) = q.time_from {
            parts.push(format!("updated:>={}", from.format("%Y-%m-%d")));
        }

        parts.join(" ")
    }

    async fn search_issues_with_q(&self, q_str: &str, per_page: usize) -> Result<Vec<Value>> {
        let per_page_str = per_page.to_string();
        let params = [
            ("q", q_str.to_string()),
            ("per_page", per_page_str),
            ("sort", "updated".to_string()),
            ("order", "desc".to_string()),
        ];

        let res = self.http.get("/search/issues", &params).await;
        match res {
            Ok(val) => {
                let items = val
                    .get("items")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default();
                Ok(items)
            }
            Err(HttpError::Status { status, body }) => Err(anyhow!("HTTP {} : {}", status, body)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

#[async_trait]
impl Connector for GitHubConnector {
    fn source(&self) -> &str {
        "github"
    }

    async fn health(&self) -> Health {
        let user_res = self.http.get("/user", &[]).await;
        let rate_res = self.http.get("/rate_limit", &[]).await;
        let latency = self.http.last_latency_ms.load(Ordering::Relaxed);

        let mut rate_limit = None;
        if let Ok(ref val) = rate_res {
            if let Some(rem) = val
                .pointer("/resources/core/remaining")
                .and_then(|x| x.as_i64())
            {
                rate_limit = Some(rem);
            }
        }

        match user_res {
            Ok(user) => {
                let login = helper_s(&user, "login");
                Health {
                    ok: true,
                    detail: format!("Connected as {}", login),
                    auth: "ok".to_string(),
                    rate_limit_remaining: rate_limit,
                    latency_ms: latency,
                }
            }
            Err(HttpError::Status { status: 401, body }) => Health {
                ok: false,
                detail: format!("Authentication failed: {}", body),
                auth: "failed".to_string(),
                rate_limit_remaining: rate_limit,
                latency_ms: latency,
            },
            Err(e) => Health {
                ok: false,
                detail: format!("Health check failed: {}", e),
                auth: "failed".to_string(),
                rate_limit_remaining: rate_limit,
                latency_ms: latency,
            },
        }
    }

    fn credential_expiry(&self) -> Option<String> {
        self.http.token_expiry.read().ok().and_then(|g| g.clone())
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<Evidence>> {
        let limit = if q.max_results == 0 {
            50
        } else {
            q.max_results.min(100)
        };
        let has_type = q.query.contains("is:issue")
            || q.query.contains("is:pull-request")
            || q.query.contains("is:pr")
            || q.query.contains("type:");

        let mut items = if has_type {
            let q_str = self.build_q(q, None);
            let mut list = self.search_issues_with_q(&q_str, limit).await?;
            if list.is_empty() {
                // Quirk: if > 3 words returned nothing, retry once with first 3 words
                let words: Vec<&str> = q.query.split_whitespace().collect();
                if words.len() > 3 {
                    let mut fallback_q = q.clone();
                    fallback_q.query = words.into_iter().take(3).collect::<Vec<_>>().join(" ");
                    let fq_str = self.build_q(&fallback_q, None);
                    list = self.search_issues_with_q(&fq_str, limit).await?;
                }
            }
            list
        } else {
            // Quirk: run query twice (is:issue and is:pull-request), merge and sort
            let q_issue = self.build_q(q, Some("is:issue"));
            let q_pr = self.build_q(q, Some("is:pull-request"));

            let (res_i, res_p) = tokio::join!(
                self.search_issues_with_q(&q_issue, limit),
                self.search_issues_with_q(&q_pr, limit)
            );

            let mut merged = res_i.unwrap_or_default();
            merged.extend(res_p.unwrap_or_default());

            if merged.is_empty() {
                let words: Vec<&str> = q.query.split_whitespace().collect();
                if words.len() > 3 {
                    let mut fallback_q = q.clone();
                    fallback_q.query = words.into_iter().take(3).collect::<Vec<_>>().join(" ");
                    let fq_i = self.build_q(&fallback_q, Some("is:issue"));
                    let fq_p = self.build_q(&fallback_q, Some("is:pull-request"));

                    let (rf_i, rf_p) = tokio::join!(
                        self.search_issues_with_q(&fq_i, limit),
                        self.search_issues_with_q(&fq_p, limit)
                    );
                    merged = rf_i.unwrap_or_default();
                    merged.extend(rf_p.unwrap_or_default());
                }
            }

            merged.sort_by(|a, b| {
                let ta = a.get("updated_at").and_then(|x| x.as_str()).unwrap_or("");
                let tb = b.get("updated_at").and_then(|x| x.as_str()).unwrap_or("");
                tb.cmp(ta)
            });
            merged.truncate(limit);
            merged
        };

        items.truncate(limit);

        let mut out = Vec::new();
        for item in items {
            let number = item.get("number").and_then(|x| x.as_i64()).unwrap_or(0);
            let is_pr = item.get("pull_request").is_some();
            let r_type = if is_pr { "pr" } else { "issue" };

            let repo_url = helper_s(&item, "repository_url");
            let repo = repo_url
                .split('/')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/");
            let id = format!("{}#{}", repo, number);

            let title = helper_s(&item, "title");
            let body_snip = helper_s(&item, "body")
                .chars()
                .take(400)
                .collect::<String>();
            let url = helper_s(&item, "html_url");
            let author = item
                .pointer("/user/login")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let state = helper_s(&item, "state");
            let created_at = item
                .get("created_at")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let updated_at = item
                .get("updated_at")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());

            let mut native = BTreeMap::new();
            native.insert(
                "labels".to_string(),
                item.get("labels").cloned().unwrap_or(Value::Null),
            );
            native.insert(
                "assignees".to_string(),
                item.get("assignees").cloned().unwrap_or(Value::Null),
            );
            native.insert(
                "comments_count".to_string(),
                item.get("comments").cloned().unwrap_or(Value::Null),
            );

            out.push(Evidence {
                r#ref: ObjectRef::new("github", r_type, id),
                title,
                snippet: body_snip,
                body: None,
                url,
                container: format!("github:{}", repo),
                author,
                created_at,
                updated_at,
                state,
                native,
            });
        }

        Ok(out)
    }

    async fn get_object(&self, r: &ObjectRef) -> Result<Option<Evidence>> {
        let (repo, num_str) = match r.id.split_once('#') {
            Some(pair) => pair,
            None => return Ok(None),
        };
        let num: i64 = match num_str.parse() {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };

        let is_pr = r.r#type == "pr";
        if is_pr {
            let (owner, name) = match repo.split_once('/') {
                Some(p) => p,
                None => return Ok(None),
            };

            let query = r#"
query($owner:String!,$name:String!,$num:Int!){
  repository(owner:$owner,name:$name){
    pullRequest(number:$num){
      number title state isDraft merged mergedAt createdAt updatedAt url bodyText
      author{login}
      baseRefName headRefName mergeable
      labels(first:20){nodes{name}}
      assignees(first:10){nodes{login}}
      closingIssuesReferences(first:20){nodes{number repository{nameWithOwner}}}
      timelineItems(first:100, itemTypes:[CROSS_REFERENCED_EVENT,CONNECTED_EVENT,REFERENCED_EVENT,CLOSED_EVENT]){
        nodes{
          __typename
          ... on CrossReferencedEvent{source{__typename ... on Issue{number repository{nameWithOwner}} ... on PullRequest{number repository{nameWithOwner}}}}
          ... on ConnectedEvent{subject{__typename ... on Issue{number repository{nameWithOwner}} ... on PullRequest{number repository{nameWithOwner}}}}
          ... on ReferencedEvent{commit{abbreviatedOid}}
          ... on ClosedEvent{closer{__typename ... on Commit{abbreviatedOid}}}
        }
      }
      reviews(first:50){nodes{state author{login} submittedAt body}}
      reviewThreads(first:50){nodes{comments(first:20){nodes{author{login} createdAt path body}}}}
      commits(first:100){nodes{commit{abbreviatedOid messageHeadline author{name date}}}}
      comments(first:50){nodes{author{login} createdAt body}}
    }
  }
}
"#;
            let body = json!({
                "query": query,
                "variables": {
                    "owner": owner,
                    "name": name,
                    "num": num
                }
            });

            let gql_res = self.http.post_search("/graphql", &body).await;
            if let Ok(val) = gql_res {
                if val.get("errors").is_none()
                    || val
                        .get("errors")
                        .and_then(|e| e.as_array())
                        .map(|a| a.is_empty())
                        .unwrap_or(false)
                {
                    if let Some(pr) = val.pointer("/data/repository/pullRequest") {
                        if !pr.is_null() {
                            let title = helper_s(pr, "title");
                            let desc = helper_s(pr, "bodyText");
                            let url = helper_s(pr, "url");
                            let author = pr
                                .pointer("/author/login")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let state =
                                if pr.get("merged").and_then(|x| x.as_bool()).unwrap_or(false) {
                                    "merged".to_string()
                                } else {
                                    helper_s(pr, "state").to_lowercase()
                                };

                            let mut full_body = desc.clone();
                            if let Some(revs) =
                                pr.pointer("/reviews/nodes").and_then(|x| x.as_array())
                            {
                                if !revs.is_empty() {
                                    full_body.push_str("\n\nReviews:\n");
                                    for rev in revs {
                                        let rauth = rev
                                            .pointer("/author/login")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("");
                                        let rdate = rev
                                            .get("submittedAt")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("");
                                        let rstate = helper_s(rev, "state");
                                        let rbody = helper_s(rev, "body");
                                        full_body.push_str(&format!(
                                            "{} {} {}: {}\n",
                                            &rdate[..rdate.len().min(19)],
                                            rauth,
                                            rstate,
                                            rbody
                                        ));
                                    }
                                }
                            }

                            let mut native = BTreeMap::new();
                            native.insert("graphql".to_string(), pr.clone());

                            return Ok(Some(Evidence {
                                r#ref: r.clone(),
                                title,
                                snippet: desc.chars().take(400).collect(),
                                body: Some(full_body),
                                url,
                                container: format!("github:{}", repo),
                                author,
                                created_at: pr
                                    .get("createdAt")
                                    .and_then(|x| x.as_str())
                                    .map(|s| s.to_string()),
                                updated_at: pr
                                    .get("updatedAt")
                                    .and_then(|x| x.as_str())
                                    .map(|s| s.to_string()),
                                state,
                                native,
                            }));
                        }
                    }
                }
            }

            // Fallback to REST for PR if GraphQL failed
            let path = format!("/repos/{}/pulls/{}", repo, num);
            match self.http.get(&path, &[]).await {
                Ok(val) => {
                    let title = helper_s(&val, "title");
                    let desc = helper_s(&val, "body");
                    let url = helper_s(&val, "html_url");
                    let author = val
                        .pointer("/user/login")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let state = if val.get("merged").and_then(|x| x.as_bool()).unwrap_or(false) {
                        "merged".to_string()
                    } else {
                        helper_s(&val, "state")
                    };

                    let mut native = BTreeMap::new();
                    native.insert("rest".to_string(), val.clone());

                    Ok(Some(Evidence {
                        r#ref: r.clone(),
                        title,
                        snippet: desc.chars().take(400).collect(),
                        body: Some(desc),
                        url,
                        container: format!("github:{}", repo),
                        author,
                        created_at: val
                            .get("created_at")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string()),
                        updated_at: val
                            .get("updated_at")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string()),
                        state,
                        native,
                    }))
                }
                Err(HttpError::Status { status: 404, .. }) => Ok(None),
                Err(e) => Err(anyhow::anyhow!(e)),
            }
        } else {
            // Issue REST deep read
            let path = format!("/repos/{}/issues/{}", repo, num);
            let issue_res = match self.http.get(&path, &[]).await {
                Ok(v) => v,
                Err(HttpError::Status { status: 404, .. }) => return Ok(None),
                Err(e) => return Err(anyhow::anyhow!(e)),
            };

            let title = helper_s(&issue_res, "title");
            let desc = helper_s(&issue_res, "body");
            let url = helper_s(&issue_res, "html_url");
            let author = issue_res
                .pointer("/user/login")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let state = helper_s(&issue_res, "state");

            let mut full_body = desc.clone();
            let comments_path = format!("/repos/{}/issues/{}/comments", repo, num);
            if let Ok(c_val) = self
                .http
                .get(&comments_path, &[("per_page", "50".to_string())])
                .await
            {
                if let Some(arr) = c_val.as_array() {
                    if !arr.is_empty() {
                        full_body.push_str("\n\nComments:\n");
                        for c in arr {
                            let cauthor = c
                                .pointer("/user/login")
                                .and_then(|x| x.as_str())
                                .unwrap_or("");
                            let cdate = c.get("created_at").and_then(|x| x.as_str()).unwrap_or("");
                            let cbody = helper_s(c, "body");
                            full_body.push_str(&format!(
                                "{} {}: {}\n",
                                &cdate[..cdate.len().min(19)],
                                cauthor,
                                cbody
                            ));
                        }
                    }
                }
            }

            let mut native = BTreeMap::new();
            native.insert("issue".to_string(), issue_res.clone());

            Ok(Some(Evidence {
                r#ref: r.clone(),
                title,
                snippet: desc.chars().take(400).collect(),
                body: Some(full_body),
                url,
                container: format!("github:{}", repo),
                author,
                created_at: issue_res
                    .get("created_at")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
                updated_at: issue_res
                    .get("updated_at")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
                state,
                native,
            }))
        }
    }

    async fn get_thread(&self, r: &ObjectRef, window: usize) -> Result<Option<Thread>> {
        let (repo, num_str) = match r.id.split_once('#') {
            Some(pair) => pair,
            None => return Ok(None),
        };
        let num: i64 = match num_str.parse() {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };

        let path = format!("/repos/{}/issues/{}", repo, num);
        let issue = match self.http.get(&path, &[]).await {
            Ok(v) => v,
            Err(HttpError::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!(e)),
        };

        let mut comments = Vec::new();
        let body = helper_s(&issue, "body");
        let author = issue
            .pointer("/user/login")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let created_at = issue
            .get("created_at")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        comments.push(ThreadComment {
            author,
            created_at,
            body,
        });

        let c_path = format!("/repos/{}/issues/{}/comments", repo, num);
        let limit_str = window.min(100).to_string();
        if let Ok(c_val) = self.http.get(&c_path, &[("per_page", limit_str)]).await {
            if let Some(arr) = c_val.as_array() {
                for c in arr {
                    let c_author = c
                        .pointer("/user/login")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let c_created = c
                        .get("created_at")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    let c_body = helper_s(c, "body");
                    comments.push(ThreadComment {
                        author: c_author,
                        created_at: c_created,
                        body: c_body,
                    });
                }
            }
        }

        Ok(Some(Thread {
            r#ref: r.clone(),
            comments,
        }))
    }

    async fn get_related(&self, r: &ObjectRef, _kinds: Option<&[String]>) -> Result<Vec<Relation>> {
        let (repo, num_str) = match r.id.split_once('#') {
            Some(pair) => pair,
            None => return Ok(Vec::new()),
        };
        let num: i64 = match num_str.parse() {
            Ok(n) => n,
            Err(_) => return Ok(Vec::new()),
        };

        let mut relations = Vec::new();
        let is_pr = r.r#type == "pr";

        if is_pr {
            let (owner, name) = match repo.split_once('/') {
                Some(p) => p,
                None => return Ok(Vec::new()),
            };

            let query = r#"
query($owner:String!,$name:String!,$num:Int!){
  repository(owner:$owner,name:$name){
    pullRequest(number:$num){
      closingIssuesReferences(first:20){nodes{number repository{nameWithOwner}}}
      timelineItems(first:100, itemTypes:[CROSS_REFERENCED_EVENT,CONNECTED_EVENT,REFERENCED_EVENT,CLOSED_EVENT]){
        nodes{
          __typename
          ... on CrossReferencedEvent{source{__typename ... on Issue{number repository{nameWithOwner}} ... on PullRequest{number repository{nameWithOwner}}}}
          ... on ConnectedEvent{subject{__typename ... on Issue{number repository{nameWithOwner}} ... on PullRequest{number repository{nameWithOwner}}}}
          ... on ReferencedEvent{commit{abbreviatedOid}}
          ... on ClosedEvent{closer{__typename ... on Commit{abbreviatedOid}}}
        }
      }
      commits(first:100){nodes{commit{abbreviatedOid messageHeadline}}}
    }
  }
}
"#;
            let body = json!({
                "query": query,
                "variables": { "owner": owner, "name": name, "num": num }
            });

            if let Ok(val) = self.http.post_search("/graphql", &body).await {
                if let Some(nodes) = val
                    .pointer("/data/repository/pullRequest/closingIssuesReferences/nodes")
                    .and_then(|x| x.as_array())
                {
                    for node in nodes {
                        let cnum = node.get("number").and_then(|x| x.as_i64()).unwrap_or(0);
                        let crepo = node
                            .pointer("/repository/nameWithOwner")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        if cnum > 0 && !crepo.is_empty() {
                            relations.push(Relation {
                                from: r.clone(),
                                to: ObjectRef::new(
                                    "github",
                                    "issue",
                                    format!("{}#{}", crepo, cnum),
                                ),
                                relation_type: "closes".to_string(),
                                detected_by: "native".to_string(),
                                confidence: 1.0,
                            });
                        }
                    }
                }

                if let Some(nodes) = val
                    .pointer("/data/repository/pullRequest/commits/nodes")
                    .and_then(|x| x.as_array())
                {
                    for node in nodes {
                        if let Some(oid) = node
                            .pointer("/commit/abbreviatedOid")
                            .and_then(|x| x.as_str())
                        {
                            relations.push(Relation {
                                from: r.clone(),
                                to: ObjectRef::new("github", "commit", format!("{}@{}", repo, oid)),
                                relation_type: "commit".to_string(),
                                detected_by: "native".to_string(),
                                confidence: 1.0,
                            });
                        }
                    }
                }
            }
        }

        // Also parse text patterns in body / comments
        if let Ok(Some(ev)) = self.get_object(r).await {
            if let Some(ref text) = ev.body {
                // LP pattern
                for cap in PAT_LP.captures_iter(text) {
                    relations.push(Relation {
                        from: r.clone(),
                        to: ObjectRef::new("launchpad", "bug", &cap[1]),
                        relation_type: "references".to_string(),
                        detected_by: "id_pattern".to_string(),
                        confidence: 0.90,
                    });
                }

                // Jira pattern
                for cap in PAT_JIRA.captures_iter(text) {
                    let key = &cap[1];
                    if crate::util::is_jira_key(key) {
                        relations.push(Relation {
                            from: r.clone(),
                            to: ObjectRef::new("jira", "issue", key),
                            relation_type: "references".to_string(),
                            detected_by: "id_pattern".to_string(),
                            confidence: 0.90,
                        });
                    }
                }

                // GH pattern
                for cap in PAT_GH.captures_iter(text) {
                    let matched_repo = &cap[1];
                    let matched_num = &cap[2];
                    let matched_id = format!("{}#{}", matched_repo, matched_num);
                    if matched_id != r.id {
                        let is_pr =
                            text.contains(&format!("{}/pull/{}", matched_repo, matched_num));
                        relations.push(Relation {
                            from: r.clone(),
                            to: ObjectRef::new(
                                "github",
                                if is_pr { "pr" } else { "issue" },
                                matched_id,
                            ),
                            relation_type: "references".to_string(),
                            detected_by: "url_pattern".to_string(),
                            confidence: 0.95,
                        });
                    }
                }
            }
        }

        Ok(relations)
    }
}
