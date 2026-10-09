use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::jobs::Source;
use crate::model::client::{ChatMessage, ModelClient};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPlan {
    pub query: String,
    pub sources: Vec<Source>,
    pub projects: Vec<String>,
    pub repositories: Vec<String>,
    pub people: Vec<String>,
    pub time_from: Option<DateTime<Utc>>,
    pub time_to: Option<DateTime<Utc>>,
    pub semantic_queries: Vec<String>,
    pub graph_depth: u8,
    pub max_remote_results: usize,
    pub max_deep_reads: usize,
    pub max_graph_nodes: usize,
    pub relation_types: Vec<String>,
}

impl Default for SearchPlan {
    fn default() -> Self {
        Self {
            query: String::new(),
            sources: vec![
                Source::Local,
                Source::GitHub,
                Source::Jira,
                Source::Launchpad,
            ],
            projects: Vec::new(),
            repositories: Vec::new(),
            people: Vec::new(),
            time_from: None,
            time_to: None,
            semantic_queries: Vec::new(),
            graph_depth: 2,
            max_remote_results: 50,
            max_deep_reads: 30,
            max_graph_nodes: 100,
            relation_types: vec![
                "references".to_string(),
                "fixes".to_string(),
                "implements".to_string(),
                "blocks".to_string(),
                "depends_on".to_string(),
                "duplicates".to_string(),
            ],
        }
    }
}

pub fn clean_natural_language_query(raw: &str) -> (String, Option<usize>, Option<String>) {
    let q_trimmed = raw.trim();
    if q_trimmed.is_empty() {
        return (String::new(), None, None);
    }

    let lower = q_trimmed.to_lowercase();
    let mut limit: Option<usize> = None;
    let mut project: Option<String> = None;
    let mut is_question = false;

    // Check common question starters
    let question_prefixes = [
        "what are the",
        "what is the",
        "what are",
        "what is",
        "show me the",
        "show me",
        "tell me about the",
        "tell me about",
        "tell me",
        "list the",
        "list",
        "find the",
        "find",
        "search for",
        "search",
        "how to",
        "how do i",
        "can you show",
        "can you tell",
        "can you list",
        "can you find",
        "are there any",
        "is there any",
        "are there",
        "is there",
        "give me",
    ];

    let mut remaining = q_trimmed.to_string();
    for prefix in &question_prefixes {
        if lower.starts_with(prefix) {
            is_question = true;
            remaining = remaining[prefix.len()..].trim().to_string();
            break;
        }
    }

    // Check for "top N", "latest N", "first N", "recent N"
    let re_top_n = regex::Regex::new(r"(?i)\b(top|first|latest|recent)\s+(\d+)\b").unwrap();
    if let Some(caps) = re_top_n.captures(&remaining) {
        is_question = true;
        if let Some(m) = caps.get(2) {
            limit = m.as_str().parse().ok();
        }
        let start = caps.get(0).unwrap().start();
        let end = caps.get(0).unwrap().end();
        let before = remaining[..start].trim();
        let after = remaining[end..].trim();
        remaining = format!("{before} {after}").trim().to_string();
    } else {
        let re_top = regex::Regex::new(r"(?i)\b(top|latest|recent|newest)\b").unwrap();
        if re_top.is_match(&remaining) {
            is_question = true;
        }
    }

    if is_question {
        let re_noun_prep = regex::Regex::new(r"(?i)\b(issues|issue|bugs|bug|tickets|ticket|prs|pr|tasks|task)\s+(of|in|for|about|with)\b").unwrap();
        let stripped = re_noun_prep.replace_all(&remaining, " ");
        let re_noun =
            regex::Regex::new(r"(?i)\b(issues|issue|bugs|bug|tickets|ticket|prs|pr|tasks|task)\b")
                .unwrap();
        let stripped = re_noun.replace_all(&stripped, " ");

        let stop_words: std::collections::HashSet<&str> = [
            "a", "an", "the", "and", "or", "of", "in", "on", "at", "to", "for", "from", "with",
            "by", "about", "is", "are", "was", "were", "be", "been", "being", "what", "which",
            "who", "whom", "where", "when", "why", "how", "do", "does", "did", "have", "has",
            "had", "can", "could", "should", "would", "show", "tell", "list", "find", "get",
            "give", "me", "please", "any", "all", "some", "top", "latest", "recent", "newest",
        ]
        .into_iter()
        .collect();

        let mut cleaned_words = Vec::new();
        for word in stripped.split_whitespace() {
            let cw: String = word
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '/' || *c == '#')
                .collect();
            if cw.is_empty() {
                continue;
            }
            if !stop_words.contains(cw.to_lowercase().as_str())
                && !(cw.chars().all(|c| c.is_ascii_digit()) && cw.len() < 4)
            {
                if project.is_none()
                    && cw.chars().next().is_some_and(|c| c.is_uppercase())
                    && cw.len() > 2
                {
                    project = Some(cw.clone());
                }
                cleaned_words.push(cw);
            }
        }

        if !cleaned_words.is_empty() {
            return (cleaned_words.join(" "), limit, project);
        }
    }

    let cleaned_no_punct = remaining
        .trim()
        .trim_end_matches(['?', '!'])
        .trim()
        .to_string();
    (cleaned_no_punct, limit, project)
}

pub struct QueryPlanner;

impl QueryPlanner {
    /// Deterministic fallback parser that operates with zero network or model dependencies
    pub fn parse_deterministic(raw_query: &str) -> SearchPlan {
        let mut plan = SearchPlan {
            query: raw_query.trim().to_string(),
            semantic_queries: vec![raw_query.trim().to_string()],
            ..Default::default()
        };

        let tokens: Vec<&str> = raw_query.split_whitespace().collect();
        let mut filtered_query_parts = Vec::new();

        for token in tokens {
            if let Some(org_repo) = token.strip_prefix("repo:") {
                plan.repositories.push(org_repo.to_string());
                plan.sources = vec![Source::GitHub, Source::Local];
            } else if let Some(proj) = token.strip_prefix("project:") {
                plan.projects.push(proj.to_string());
                plan.sources = vec![Source::Jira, Source::Launchpad, Source::Local];
            } else if let Some(user) = token.strip_prefix("author:") {
                plan.people.push(user.to_string());
            } else if let Some(user) = token.strip_prefix("user:") {
                plan.people.push(user.to_string());
            } else {
                filtered_query_parts.push(token);
            }
        }

        if !filtered_query_parts.is_empty() {
            let joined = filtered_query_parts.join(" ");
            let (cleaned, limit_opt, proj_opt) = clean_natural_language_query(&joined);
            plan.query = if !cleaned.is_empty() {
                cleaned
            } else {
                joined.clone()
            };
            if let Some(p) = proj_opt {
                if plan.projects.is_empty() {
                    plan.projects.push(p);
                }
            }
            if let Some(lim) = limit_opt {
                plan.max_remote_results = plan.max_remote_results.min(lim.max(10));
            }
            plan.semantic_queries = vec![joined, plan.query.clone()];
        }

        Self::sanitize_plan(&mut plan);
        plan
    }

    /// Plans search using LLM if available, gracefully falling back to deterministic parser
    pub async fn plan(raw_query: &str, model_client: Option<&ModelClient>) -> SearchPlan {
        let client = match model_client {
            Some(c) => c,
            None => return Self::parse_deterministic(raw_query),
        };

        let system_prompt = r#"You are an issue query planner for IssueMesh.
Analyze the user's issue search query and generate a JSON query plan matching this schema:
{
  "query": "string (core technical search keywords ONLY: extract the target project, component, error message, or identifier; omit conversational framing like 'what are', ranking like 'top 3', and generic words like 'issues'/'bugs')",
  "sources": ["github", "jira", "launchpad", "local"],
  "projects": ["string (target projects mentioned in query like 'Sunbeam')"],
  "repositories": ["string"],
  "people": ["string"],
  "semantic_queries": ["alternative phrasing 1", "alternative phrasing 2"],
  "graph_depth": 2,
  "max_remote_results": 50,
  "max_deep_reads": 30,
  "max_graph_nodes": 100,
  "relation_types": ["references", "fixes", "implements", "blocks", "depends_on", "duplicates"]
}
Output strictly valid JSON with no surrounding explanations or markdown code blocks."#;

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!("Query: {raw_query}"),
            },
        ];

        match client.chat_completion(messages).await {
            Ok(content) => {
                let trimmed = content.trim();
                let json_str = if trimmed.starts_with("```") {
                    let stripped = trimmed
                        .trim_start_matches("```json")
                        .trim_start_matches("```");
                    stripped.trim_end_matches("```").trim()
                } else {
                    trimmed
                };

                match serde_json::from_str::<SearchPlan>(json_str) {
                    Ok(mut plan) => {
                        Self::sanitize_plan(&mut plan);
                        plan
                    }
                    Err(_) => Self::parse_deterministic(raw_query),
                }
            }
            Err(_) => {
                // Graceful degradation per SPEC.md: fallback to deterministic parser without error to user
                Self::parse_deterministic(raw_query)
            }
        }
    }

    /// Clamp and validate plan values to protect budgets
    pub fn sanitize_plan(plan: &mut SearchPlan) {
        // Enforce graph depth limits [0, 3]
        if plan.graph_depth > 3 {
            plan.graph_depth = 3;
        }

        // Enforce max remote results [1, 100]
        if plan.max_remote_results == 0 {
            plan.max_remote_results = 50;
        } else if plan.max_remote_results > 100 {
            plan.max_remote_results = 100;
        }

        // Enforce max deep reads [1, 50]
        if plan.max_deep_reads == 0 {
            plan.max_deep_reads = 30;
        } else if plan.max_deep_reads > 50 {
            plan.max_deep_reads = 50;
        }

        // Enforce max graph nodes [1, 200]
        if plan.max_graph_nodes == 0 {
            plan.max_graph_nodes = 100;
        } else if plan.max_graph_nodes > 200 {
            plan.max_graph_nodes = 200;
        }

        if plan.sources.is_empty() {
            plan.sources = vec![
                Source::Local,
                Source::GitHub,
                Source::Jira,
                Source::Launchpad,
            ];
        }

        let (cleaned, _, proj_opt) = clean_natural_language_query(&plan.query);
        if !cleaned.is_empty() {
            plan.query = cleaned;
        }
        if plan.projects.is_empty() {
            if let Some(p) = proj_opt {
                plan.projects.push(p);
            }
        }

        if plan.semantic_queries.is_empty() && !plan.query.is_empty() {
            plan.semantic_queries.push(plan.query.clone());
        }
    }
}
