use super::{
    helper_s, Connector, Evidence, Health, HttpError, ObjectRef, ReadOnlyHttpClient, Relation,
    SearchQuery, Thread, ThreadComment,
};
use crate::util::{PAT_GH, PAT_JIRA, PAT_LP};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

pub struct LaunchpadConnector {
    pub http: ReadOnlyHttpClient,
    pub default_project: Option<String>,
}

impl LaunchpadConnector {
    pub fn new(base_url: &str, default_project: Option<String>) -> Result<Self> {
        let http = ReadOnlyHttpClient::new(
            base_url,
            None,
            &[],
            concat!("issuemesh/", env!("CARGO_PKG_VERSION"), " (read-only)"),
            2,
            300, // 300ms pacing between calls
        )?;
        Ok(Self {
            http,
            default_project,
        })
    }

    async fn collection(
        &self,
        path: &str,
        q: &[(&str, String)],
        limit: usize,
    ) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut start = 0usize;

        loop {
            let mut qq = q.to_vec();
            let start_str = start.to_string();
            let size_str = limit.min(75).to_string();
            qq.push(("ws.start", start_str));
            qq.push(("ws.size", size_str));

            let res = self.http.get(path, &qq).await;
            let val = match res {
                Ok(v) => v,
                Err(HttpError::Status {
                    status: 404,
                    body: _,
                }) => break,
                Err(HttpError::Status { status, body }) => {
                    return Err(anyhow!("HTTP {} : {}", status, body))
                }
                Err(e) => return Err(anyhow::anyhow!(e)),
            };

            let entries = val["entries"].as_array().cloned().unwrap_or_default();
            let n = entries.len();
            out.extend(entries);

            if out.len() >= limit || n == 0 || val.get("next_collection_link").is_none() {
                break;
            }
            start += n;
        }

        out.truncate(limit);
        Ok(out)
    }
}

#[async_trait]
impl Connector for LaunchpadConnector {
    fn source(&self) -> &str {
        "launchpad"
    }

    async fn health(&self) -> Health {
        let res = self.http.get("/bugs/1", &[]).await;
        let latency = self.http.last_latency_ms.load(Ordering::Relaxed);

        match res {
            Ok(_) => Health {
                ok: true,
                detail: "Launchpad API reachable".to_string(),
                auth: "none".to_string(),
                rate_limit_remaining: None,
                latency_ms: latency,
            },
            Err(e) => Health {
                ok: false,
                detail: format!("Launchpad unreachable: {}", e),
                auth: "none".to_string(),
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
        let target_path = if let Some(p) = q.projects.first().or(self.default_project.as_ref()) {
            format!("/{}", p)
        } else {
            "/bugs".to_string()
        };

        let mut params = vec![
            ("ws.op", "searchTasks".to_string()),
            ("order_by", "-date_last_updated".to_string()),
        ];

        let trimmed_query = q.query.trim().to_string();
        if !trimmed_query.is_empty() {
            params.push(("search_text", trimmed_query.clone()));
        }

        let mut entries = self.collection(&target_path, &params, limit).await?;

        // Quirk: search_text is conjunctive; if more than three words return nothing, retry once with first three words
        if entries.is_empty() && !trimmed_query.is_empty() {
            let words: Vec<&str> = trimmed_query.split_whitespace().collect();
            if words.len() > 3 {
                let first3 = words.into_iter().take(3).collect::<Vec<_>>().join(" ");
                let retry_params = vec![
                    ("ws.op", "searchTasks".to_string()),
                    ("order_by", "-date_last_updated".to_string()),
                    ("search_text", first3),
                ];
                entries = self.collection(&target_path, &retry_params, limit).await?;
            }
        }

        let mut out = Vec::new();
        for task in entries {
            let bug_link = helper_s(&task, "bug_link");
            let bug_id = bug_link.split('/').next_back().unwrap_or("").to_string();
            if bug_id.is_empty() {
                continue;
            }

            let title = helper_s(&task, "title");
            let target_name = helper_s(&task, "bug_target_name");
            let status = helper_s(&task, "status");
            let date_created = task
                .get("date_created")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let date_updated = task
                .get("date_last_updated")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());

            let owner_link = helper_s(&task, "owner_link");
            let author = owner_link.split('~').next_back().unwrap_or("").to_string();

            let mut native = BTreeMap::new();
            native.insert(
                "importance".to_string(),
                task.get("importance").cloned().unwrap_or(Value::Null),
            );
            native.insert("target".to_string(), Value::String(target_name.clone()));

            let web_url = format!(
                "https://bugs.launchpad.net/{}/+bug/{}",
                if target_name.is_empty() {
                    "bugs"
                } else {
                    &target_name
                },
                bug_id
            );

            out.push(Evidence {
                r#ref: ObjectRef::new("launchpad", "bug", bug_id),
                title,
                snippet: String::new(),
                body: None,
                url: web_url,
                container: format!("launchpad:{}", target_name),
                author,
                created_at: date_created,
                updated_at: date_updated,
                state: status,
                native,
            });
        }

        Ok(out)
    }

    async fn get_object(&self, r: &ObjectRef) -> Result<Option<Evidence>> {
        let path = format!("/bugs/{}", r.id);
        let bug = match self.http.get(&path, &[]).await {
            Ok(v) => v,
            Err(HttpError::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!(e)),
        };

        let title = helper_s(&bug, "title");
        let desc = helper_s(&bug, "description");
        let owner_link = helper_s(&bug, "owner_link");
        let author = owner_link.split('~').next_back().unwrap_or("").to_string();
        let created_at = bug
            .get("date_created")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let updated_at = bug
            .get("date_last_updated")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());

        let mut full_body = desc.clone();

        // Tasks
        let tasks_path = format!("/bugs/{}/bug_tasks", r.id);
        let mut target_name = String::new();
        let mut primary_status = "open".to_string();
        if let Ok(tasks_val) = self.http.get(&tasks_path, &[]).await {
            if let Some(entries) = tasks_val.get("entries").and_then(|x| x.as_array()) {
                if let Some(first) = entries.first() {
                    target_name = helper_s(first, "bug_target_name");
                    primary_status = helper_s(first, "status");
                }
            }
        }

        // Messages
        let msgs_path = format!("/bugs/{}/messages", r.id);
        if let Ok(msgs_val) = self.http.get(&msgs_path, &[]).await {
            if let Some(entries) = msgs_val.get("entries").and_then(|x| x.as_array()) {
                if entries.len() > 1 {
                    full_body.push_str("\n\nMessages:\n");
                    for msg in entries.iter().skip(1).take(40) {
                        let m_date = helper_s(msg, "date_created");
                        let m_owner = helper_s(msg, "owner_link")
                            .split('~')
                            .next_back()
                            .unwrap_or("")
                            .to_string();
                        let m_content = helper_s(msg, "content");
                        full_body.push_str(&format!(
                            "{} {}: {}\n",
                            &m_date[..m_date.len().min(19)],
                            m_owner,
                            m_content
                        ));
                    }
                }
            }
        }

        // Activity
        let act_path = format!("/bugs/{}/activity", r.id);
        if let Ok(act_val) = self.http.get(&act_path, &[]).await {
            if let Some(entries) = act_val.get("entries").and_then(|x| x.as_array()) {
                if !entries.is_empty() {
                    full_body.push_str("\n\nActivity:\n");
                    for act in entries.iter().take(60) {
                        let a_date = helper_s(act, "datechanged");
                        let a_person = helper_s(act, "person_link")
                            .split('~')
                            .next_back()
                            .unwrap_or("")
                            .to_string();
                        let a_what = helper_s(act, "whatchanged");
                        let a_old = helper_s(act, "oldvalue");
                        let a_new = helper_s(act, "newvalue");
                        full_body.push_str(&format!(
                            "{} {} changed {}: {} -> {}\n",
                            &a_date[..a_date.len().min(19)],
                            a_person,
                            a_what,
                            a_old,
                            a_new
                        ));
                    }
                }
            }
        }

        let web_url = format!(
            "https://bugs.launchpad.net/{}/+bug/{}",
            if target_name.is_empty() {
                "bugs"
            } else {
                &target_name
            },
            r.id
        );

        let mut native = BTreeMap::new();
        native.insert("bug".to_string(), bug.clone());

        Ok(Some(Evidence {
            r#ref: r.clone(),
            title,
            snippet: desc.chars().take(400).collect(),
            body: Some(full_body),
            url: web_url,
            container: format!("launchpad:{}", target_name),
            author,
            created_at,
            updated_at,
            state: primary_status,
            native,
        }))
    }

    async fn get_thread(&self, r: &ObjectRef, window: usize) -> Result<Option<Thread>> {
        let msgs_path = format!("/bugs/{}/messages", r.id);
        let msgs_val = match self.http.get(&msgs_path, &[]).await {
            Ok(v) => v,
            Err(HttpError::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!(e)),
        };

        let mut comments = Vec::new();
        if let Some(entries) = msgs_val.get("entries").and_then(|x| x.as_array()) {
            for msg in entries.iter().take(window.min(200)) {
                let author = helper_s(msg, "owner_link")
                    .split('~')
                    .next_back()
                    .unwrap_or("")
                    .to_string();
                let created_at = msg
                    .get("date_created")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let body = helper_s(msg, "content");
                comments.push(ThreadComment {
                    author,
                    created_at,
                    body,
                });
            }
        }

        Ok(Some(Thread {
            r#ref: r.clone(),
            comments,
        }))
    }

    async fn get_related(&self, r: &ObjectRef, _kinds: Option<&[String]>) -> Result<Vec<Relation>> {
        let path = format!("/bugs/{}", r.id);
        let bug = match self.http.get(&path, &[]).await {
            Ok(v) => v,
            Err(_) => return Ok(Vec::new()),
        };

        let mut relations = Vec::new();

        if let Some(dup_link) = bug.get("duplicate_of_link").and_then(|x| x.as_str()) {
            let dup_id = dup_link.split('/').next_back().unwrap_or("");
            if !dup_id.is_empty() {
                relations.push(Relation {
                    from: r.clone(),
                    to: ObjectRef::new("launchpad", "bug", dup_id),
                    relation_type: "duplicates".to_string(),
                    detected_by: "native".to_string(),
                    confidence: 1.0,
                });
            }
        }

        let tasks_path = format!("/bugs/{}/bug_tasks", r.id);
        if let Ok(tasks_val) = self.http.get(&tasks_path, &[]).await {
            if let Some(entries) = tasks_val.get("entries").and_then(|x| x.as_array()) {
                for task in entries.iter().skip(1) {
                    let target_name = helper_s(task, "bug_target_name");
                    if !target_name.is_empty() {
                        relations.push(Relation {
                            from: r.clone(),
                            to: ObjectRef::new("launchpad", "project", target_name),
                            relation_type: "also_affects".to_string(),
                            detected_by: "native".to_string(),
                            confidence: 1.0,
                        });
                    }
                }
            }
        }

        // Also check text patterns in description & messages
        if let Ok(Some(ev)) = self.get_object(r).await {
            if let Some(ref text) = ev.body {
                for cap in PAT_LP.captures_iter(text) {
                    let dup_id = &cap[1];
                    if dup_id != r.id {
                        relations.push(Relation {
                            from: r.clone(),
                            to: ObjectRef::new("launchpad", "bug", dup_id),
                            relation_type: "references".to_string(),
                            detected_by: "id_pattern".to_string(),
                            confidence: 0.90,
                        });
                    }
                }

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

                for cap in PAT_GH.captures_iter(text) {
                    let matched_repo = &cap[1];
                    let matched_num = &cap[2];
                    let is_pr = text.contains(&format!("{}/pull/{}", matched_repo, matched_num));
                    relations.push(Relation {
                        from: r.clone(),
                        to: ObjectRef::new(
                            "github",
                            if is_pr { "pr" } else { "issue" },
                            format!("{}#{}", matched_repo, matched_num),
                        ),
                        relation_type: "references".to_string(),
                        detected_by: "url_pattern".to_string(),
                        confidence: 0.95,
                    });
                }
            }
        }

        Ok(relations)
    }
}
