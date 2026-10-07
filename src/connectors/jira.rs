use super::{
    helper_s, Connector, Evidence, Health, HttpError, ObjectRef, ReadOnlyHttpClient, Relation,
    SearchQuery, Thread, ThreadComment,
};
use crate::util::{adf_text, PAT_GH, PAT_JIRA, PAT_LP};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

pub struct JiraConnector {
    pub http: ReadOnlyHttpClient,
    pub base_web_url: String,
    pub default_project: Option<String>,
}

impl JiraConnector {
    pub fn new(
        base_url: &str,
        email: Option<String>,
        token: Option<String>,
        default_project: Option<String>,
    ) -> Result<Self> {
        let trimmed_base = base_url.trim_end_matches('/').to_string();
        let http = if let (Some(em), Some(tk)) = (email, token) {
            ReadOnlyHttpClient::new_basic(
                &trimmed_base,
                &em,
                &tk,
                &["/rest/api/3/search/jql", "/rest/api/3/jql/parse"],
                "issuemesh/0.1.0 (read-only)",
                4,
            )?
        } else {
            ReadOnlyHttpClient::new(
                &trimmed_base,
                None,
                &["/rest/api/3/search/jql", "/rest/api/3/jql/parse"],
                "issuemesh/0.1.0 (read-only)",
                4,
                0,
            )?
        };

        Ok(Self {
            http,
            base_web_url: trimmed_base,
            default_project,
        })
    }

    fn build_jql(&self, q: &SearchQuery, override_text: Option<&str>) -> String {
        // If query native has raw JQL, it wins over everything
        if let Some(jql) = q
            .native
            .get("jira")
            .and_then(|x| x.get("jql"))
            .and_then(|x| x.as_str())
        {
            return jql.to_string();
        }

        let mut clauses = Vec::new();
        let text_input = override_text.unwrap_or(&q.query).trim();

        if !text_input.is_empty() {
            let tokens: Vec<&str> = text_input.split_whitespace().collect();
            let all_keys = !tokens.is_empty()
                && tokens
                    .iter()
                    .all(|t| crate::util::PAT_JIRA.is_match(t) && crate::util::is_jira_key(t));
            if all_keys {
                clauses.push(format!("key in ({})", tokens.join(",")));
            } else {
                let cleaned: String = text_input.chars().filter(|&c| c != '"').collect();
                clauses.push(format!("text ~ \"{}\"", cleaned));
            }
        }

        let mut projs = q.projects.clone();
        if projs.is_empty() {
            if let Some(ref dp) = self.default_project {
                projs.push(dp.clone());
            }
        }
        if !projs.is_empty() {
            clauses.push(format!("project in ({})", projs.join(",")));
        }

        if !q.people.is_empty() {
            let p_clauses: Vec<String> = q
                .people
                .iter()
                .map(|p| format!("(assignee = \"{0}\" OR reporter = \"{0}\")", p))
                .collect();
            clauses.push(format!("({})", p_clauses.join(" OR ")));
        }

        if let Some(from) = q.time_from {
            clauses.push(format!("updated >= \"{}\"", from.format("%Y-%m-%d")));
        }
        if let Some(to) = q.time_to {
            clauses.push(format!("updated <= \"{}\"", to.format("%Y-%m-%d")));
        }

        if clauses.is_empty() {
            clauses.push("updated >= -30d".to_string());
        }

        format!("{} ORDER BY updated DESC", clauses.join(" AND "))
    }

    async fn execute_search(&self, jql: &str, limit: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut next_page_token: Option<String> = None;

        let fields = json!([
            "summary",
            "status",
            "issuetype",
            "priority",
            "assignee",
            "reporter",
            "created",
            "updated",
            "resolutiondate",
            "fixVersions",
            "components",
            "labels",
            "parent",
            "issuelinks",
            "project",
            "subtasks",
            "description"
        ]);

        loop {
            let mut body_map = json!({
                "jql": jql,
                "maxResults": limit.min(50),
                "fields": fields
            });

            if let Some(ref token) = next_page_token {
                body_map["nextPageToken"] = Value::String(token.clone());
            }

            let res = self
                .http
                .post_search("/rest/api/3/search/jql", &body_map)
                .await;
            let val = match res {
                Ok(v) => v,
                Err(HttpError::Status { status, body }) => {
                    return Err(anyhow!("HTTP {} : {}", status, body))
                }
                Err(e) => return Err(anyhow::anyhow!(e)),
            };

            let issues = val
                .get("issues")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            let n = issues.len();
            out.extend(issues);

            if out.len() >= limit || n == 0 {
                break;
            }

            next_page_token = val
                .get("nextPageToken")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            if next_page_token.is_none() {
                break;
            }
        }

        out.truncate(limit);
        Ok(out)
    }
}

#[async_trait]
impl Connector for JiraConnector {
    fn source(&self) -> &str {
        "jira"
    }

    async fn health(&self) -> Health {
        let res = self.http.get("/rest/api/3/myself", &[]).await;
        let latency = self.http.last_latency_ms.load(Ordering::Relaxed);

        match res {
            Ok(myself) => {
                let name = helper_s(&myself, "displayName");
                Health {
                    ok: true,
                    detail: format!("Connected as {}", name),
                    auth: "ok".to_string(),
                    rate_limit_remaining: None,
                    latency_ms: latency,
                }
            }
            Err(HttpError::Status { status, body }) if status == 401 || status == 403 => Health {
                ok: false,
                detail: format!("Authentication failed (HTTP {}): {}", status, body),
                auth: "failed".to_string(),
                rate_limit_remaining: None,
                latency_ms: latency,
            },
            Err(e) => Health {
                ok: false,
                detail: format!("Health check failed: {}", e),
                auth: "failed".to_string(),
                rate_limit_remaining: None,
                latency_ms: latency,
            },
        }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<Evidence>> {
        let limit = if q.max_results == 0 {
            50
        } else {
            q.max_results.min(100)
        };
        let jql = self.build_jql(q, None);
        let mut issues = self.execute_search(&jql, limit).await?;

        // Quirk: text ~ is conjunctive on Jira Cloud; if more than three words return nothing, retry with first three words
        if issues.is_empty() && !q.query.trim().is_empty() {
            let words: Vec<&str> = q.query.split_whitespace().collect();
            if words.len() > 3 {
                let first3 = words.into_iter().take(3).collect::<Vec<_>>().join(" ");
                let retry_jql = self.build_jql(q, Some(&first3));
                issues = self.execute_search(&retry_jql, limit).await?;
            }
        }

        let mut out = Vec::new();
        for issue in issues {
            let key = helper_s(&issue, "key");
            let fields = issue.get("fields").cloned().unwrap_or(Value::Null);

            let title = helper_s(&fields, "summary");
            let desc_val = fields.get("description").unwrap_or(&Value::Null);
            let desc = adf_text(desc_val);
            let snippet = desc.chars().take(400).collect();

            let status = fields
                .pointer("/status/name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let author = fields
                .pointer("/reporter/displayName")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = fields
                .get("created")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let updated_at = fields
                .get("updated")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let proj_key = fields
                .pointer("/project/key")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();

            let mut native = BTreeMap::new();
            native.insert(
                "priority".to_string(),
                fields
                    .pointer("/priority/name")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            native.insert(
                "issuetype".to_string(),
                fields
                    .pointer("/issuetype/name")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            native.insert(
                "assignee".to_string(),
                fields
                    .pointer("/assignee/displayName")
                    .cloned()
                    .unwrap_or(Value::Null),
            );

            let web_url = format!("{}/browse/{}", self.base_web_url, key);

            out.push(Evidence {
                r#ref: ObjectRef::new("jira", "issue", key),
                title,
                snippet,
                body: None,
                url: web_url,
                container: format!("jira:{}", proj_key),
                author,
                created_at,
                updated_at,
                state: status,
                native,
            });
        }

        Ok(out)
    }

    async fn get_object(&self, r: &ObjectRef) -> Result<Option<Evidence>> {
        let path = format!("/rest/api/3/issue/{}?expand=changelog", r.id);
        let issue = match self.http.get(&path, &[]).await {
            Ok(v) => v,
            Err(HttpError::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!(e)),
        };

        let fields = issue.get("fields").cloned().unwrap_or(Value::Null);
        let title = helper_s(&fields, "summary");
        let desc_val = fields.get("description").unwrap_or(&Value::Null);
        let desc = adf_text(desc_val);

        let status = fields
            .pointer("/status/name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let author = fields
            .pointer("/reporter/displayName")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let created_at = fields
            .get("created")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let updated_at = fields
            .get("updated")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let proj_key = fields
            .pointer("/project/key")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        let mut full_body = desc.clone();

        // Comments
        let c_path = format!(
            "/rest/api/3/issue/{}/comment?maxResults=100&orderBy=created",
            r.id
        );
        if let Ok(c_val) = self.http.get(&c_path, &[]).await {
            if let Some(arr) = c_val.get("comments").and_then(|x| x.as_array()) {
                if !arr.is_empty() {
                    full_body.push_str("\n\nComments:\n");
                    for c in arr {
                        let c_author = c
                            .pointer("/author/displayName")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        let c_created = helper_s(c, "created");
                        let c_body = adf_text(c.get("body").unwrap_or(&Value::Null));
                        full_body.push_str(&format!(
                            "{} {}: {}\n",
                            &c_created[..c_created.len().min(19)],
                            c_author,
                            c_body
                        ));
                    }
                }
            }
        }

        // Changelog (History)
        if let Some(histories) = issue
            .pointer("/changelog/histories")
            .and_then(|x| x.as_array())
        {
            if !histories.is_empty() {
                full_body.push_str("\n\nHistory:\n");
                for h in histories {
                    let h_created = helper_s(h, "created");
                    let h_author = h
                        .pointer("/author/displayName")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    if let Some(items) = h.get("items").and_then(|x| x.as_array()) {
                        for item in items {
                            let field = helper_s(item, "field");
                            if matches!(
                                field.as_str(),
                                "status"
                                    | "assignee"
                                    | "Fix Version"
                                    | "priority"
                                    | "resolution"
                                    | "Sprint"
                                    | "duedate"
                            ) {
                                let from_str = helper_s(item, "fromString");
                                let to_str = helper_s(item, "toString");
                                full_body.push_str(&format!(
                                    "{} {} changed {}: {} -> {}\n",
                                    &h_created[..h_created.len().min(19)],
                                    h_author,
                                    field,
                                    from_str,
                                    to_str
                                ));
                            }
                        }
                    }
                }
            }
        }

        let web_url = format!("{}/browse/{}", self.base_web_url, r.id);
        let mut native = BTreeMap::new();
        native.insert("fields".to_string(), fields.clone());

        Ok(Some(Evidence {
            r#ref: r.clone(),
            title,
            snippet: desc.chars().take(400).collect(),
            body: Some(full_body),
            url: web_url,
            container: format!("jira:{}", proj_key),
            author,
            created_at,
            updated_at,
            state: status,
            native,
        }))
    }

    async fn get_thread(&self, r: &ObjectRef, window: usize) -> Result<Option<Thread>> {
        let path = format!(
            "/rest/api/3/issue/{}?fields=summary,description,reporter,created",
            r.id
        );
        let issue = match self.http.get(&path, &[]).await {
            Ok(v) => v,
            Err(HttpError::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!(e)),
        };

        let mut comments = Vec::new();
        let fields = issue.get("fields").cloned().unwrap_or(Value::Null);
        let desc = adf_text(fields.get("description").unwrap_or(&Value::Null));
        let author = fields
            .pointer("/reporter/displayName")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let created_at = fields
            .get("created")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        comments.push(ThreadComment {
            author,
            created_at,
            body: desc,
        });

        let c_path = format!(
            "/rest/api/3/issue/{}/comment?maxResults={}&orderBy=created",
            r.id,
            window.min(100)
        );
        if let Ok(c_val) = self.http.get(&c_path, &[]).await {
            if let Some(arr) = c_val.get("comments").and_then(|x| x.as_array()) {
                for c in arr {
                    let c_author = c
                        .pointer("/author/displayName")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let c_created = c
                        .get("created")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    let c_body = adf_text(c.get("body").unwrap_or(&Value::Null));
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
        let path = format!(
            "/rest/api/3/issue/{}?fields=issuelinks,parent,description",
            r.id
        );
        let issue = match self.http.get(&path, &[]).await {
            Ok(v) => v,
            Err(_) => return Ok(Vec::new()),
        };

        let mut relations = Vec::new();
        let fields = issue.get("fields").cloned().unwrap_or(Value::Null);

        // Parent
        if let Some(parent_key) = fields.pointer("/parent/key").and_then(|x| x.as_str()) {
            relations.push(Relation {
                from: r.clone(),
                to: ObjectRef::new("jira", "issue", parent_key),
                relation_type: "parent".to_string(),
                detected_by: "native".to_string(),
                confidence: 1.0,
            });
        }

        // Issue links
        if let Some(links) = fields.get("issuelinks").and_then(|x| x.as_array()) {
            for link in links {
                if let Some(outward) = link.get("outwardIssue") {
                    let o_key = helper_s(outward, "key");
                    let phrase = link
                        .pointer("/type/outward")
                        .and_then(|x| x.as_str())
                        .unwrap_or("relates_to");
                    let rel_type = phrase.to_lowercase().replace(' ', "_");
                    if !o_key.is_empty() {
                        relations.push(Relation {
                            from: r.clone(),
                            to: ObjectRef::new("jira", "issue", o_key),
                            relation_type: rel_type,
                            detected_by: "native".to_string(),
                            confidence: 1.0,
                        });
                    }
                } else if let Some(inward) = link.get("inwardIssue") {
                    let i_key = helper_s(inward, "key");
                    let phrase = link
                        .pointer("/type/inward")
                        .and_then(|x| x.as_str())
                        .unwrap_or("relates_to");
                    let rel_type = phrase.to_lowercase().replace(' ', "_");
                    if !i_key.is_empty() {
                        relations.push(Relation {
                            from: r.clone(),
                            to: ObjectRef::new("jira", "issue", i_key),
                            relation_type: rel_type,
                            detected_by: "native".to_string(),
                            confidence: 1.0,
                        });
                    }
                }
            }
        }

        // Remote links
        let rl_path = format!("/rest/api/3/issue/{}/remotelink", r.id);
        if let Ok(rl_val) = self.http.get(&rl_path, &[]).await {
            if let Some(links) = rl_val.as_array() {
                for l in links {
                    let url = l
                        .pointer("/object/url")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    if url.contains("github.com") {
                        if let Some(cap) = PAT_GH.captures(url) {
                            let repo = &cap[1];
                            let num = &cap[2];
                            let is_pr = url.contains("/pull/");
                            relations.push(Relation {
                                from: r.clone(),
                                to: ObjectRef::new(
                                    "github",
                                    if is_pr { "pr" } else { "issue" },
                                    format!("{}#{}", repo, num),
                                ),
                                relation_type: "references".to_string(),
                                detected_by: "url_pattern".to_string(),
                                confidence: 0.95,
                            });
                        }
                    } else if url.contains("launchpad.net") {
                        if let Some(cap) = PAT_LP.captures(url) {
                            relations.push(Relation {
                                from: r.clone(),
                                to: ObjectRef::new("launchpad", "bug", &cap[1]),
                                relation_type: "references".to_string(),
                                detected_by: "url_pattern".to_string(),
                                confidence: 0.95,
                            });
                        }
                    }
                }
            }
        }

        // Description patterns
        let desc = adf_text(fields.get("description").unwrap_or(&Value::Null));
        for cap in PAT_LP.captures_iter(&desc) {
            relations.push(Relation {
                from: r.clone(),
                to: ObjectRef::new("launchpad", "bug", &cap[1]),
                relation_type: "references".to_string(),
                detected_by: "id_pattern".to_string(),
                confidence: 0.90,
            });
        }
        for cap in PAT_JIRA.captures_iter(&desc) {
            let key = &cap[1];
            if key != r.id && crate::util::is_jira_key(key) {
                relations.push(Relation {
                    from: r.clone(),
                    to: ObjectRef::new("jira", "issue", key),
                    relation_type: "references".to_string(),
                    detected_by: "id_pattern".to_string(),
                    confidence: 0.90,
                });
            }
        }
        for cap in PAT_GH.captures_iter(&desc) {
            let repo = &cap[1];
            let num = &cap[2];
            let is_pr = desc.contains(&format!("{}/pull/{}", repo, num));
            relations.push(Relation {
                from: r.clone(),
                to: ObjectRef::new(
                    "github",
                    if is_pr { "pr" } else { "issue" },
                    format!("{}#{}", repo, num),
                ),
                relation_type: "references".to_string(),
                detected_by: "url_pattern".to_string(),
                confidence: 0.95,
            });
        }

        Ok(relations)
    }
}
