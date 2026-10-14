use crate::concepts::types::{ExtractedConcept, ExtractionPayload};
use crate::connectors::Evidence;
use crate::model::{ChatMessage, ModelClient, CONCEPT_EXTRACT_V1};
use anyhow::{bail, Context, Result};

pub struct ConceptExtractor;

impl ConceptExtractor {
    /// Format input text for concept extraction
    pub fn format_input(evidence: &Evidence, max_chars: usize) -> String {
        let mut buf = String::new();
        buf.push_str(&format!("Key: {}\n", evidence.r#ref.to_key()));
        buf.push_str(&format!("Title: {}\n", evidence.title));
        if !evidence.snippet.is_empty() {
            buf.push_str(&format!("Snippet: {}\n", evidence.snippet));
        }
        if let Some(body) = &evidence.body {
            buf.push_str(&format!("Body: {}\n", body));
        }

        if buf.chars().count() > max_chars {
            let truncated: String = buf.chars().take(max_chars).collect();
            truncated
        } else {
            buf
        }
    }

    /// Normalise concept name
    pub fn normalise_name(raw: &str, object_key: &str) -> Option<String> {
        let trimmed = raw.trim().to_lowercase();
        // collapse whitespace
        let collapsed: String = trimmed.split_whitespace().collect::<Vec<&str>>().join(" ");

        // strip trailing / leading punctuation
        let stripped = collapsed
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string();

        if stripped.len() < 3 {
            return None;
        }

        // Drop names matching the object's own key
        let lower_key = object_key.to_lowercase();
        if stripped == lower_key
            || lower_key.contains(&stripped) && stripped.len() >= lower_key.len()
        {
            return None;
        }

        Some(stripped)
    }

    /// Normalise concept kind to canonical set
    pub fn normalise_kind(raw: &str) -> &'static str {
        match raw.trim().to_lowercase().as_str() {
            "component" => "component",
            "feature" => "feature",
            "technology" => "technology",
            "problem" => "problem",
            "release" => "release",
            "team" => "team",
            _ => "other",
        }
    }

    /// Extract concepts using ModelClient with 1 retry on malformed JSON
    pub async fn extract(
        client: &ModelClient,
        evidence: &Evidence,
        max_input_tokens: usize,
    ) -> Result<Vec<ExtractedConcept>> {
        let max_chars = max_input_tokens * 4;
        let formatted = Self::format_input(evidence, max_chars);
        let object_key = evidence.r#ref.to_key();

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: CONCEPT_EXTRACT_V1.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!("Issue details:\n\n{}", formatted),
            },
        ];

        let mut resp = client.chat_completion(messages.clone()).await?;
        let parsed = Self::parse_response(&resp, &object_key);

        let concepts = match parsed {
            Ok(c) => c,
            Err(_) => {
                // Retry once with a strict reminder
                let retry_messages = vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: format!("{}\nCRITICAL: Return ONLY valid JSON matching the schema. No markdown formatting, no backticks.", CONCEPT_EXTRACT_V1),
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: format!("Issue details:\n\n{}", formatted),
                    },
                ];
                resp = client.chat_completion(retry_messages).await?;
                Self::parse_response(&resp, &object_key)
                    .context("Failed to parse concept extraction JSON after retry")?
            }
        };

        Ok(concepts)
    }

    fn parse_response(raw: &str, object_key: &str) -> Result<Vec<ExtractedConcept>> {
        let cleaned = clean_json_markdown(raw);
        let payload: ExtractionPayload = serde_json::from_str(&cleaned)?;

        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();

        let count_before = payload.concepts.len();
        for item in payload.concepts {
            if let Some(name) = Self::normalise_name(&item.name, object_key) {
                if seen.insert(name.clone()) {
                    let kind = Self::normalise_kind(&item.kind).to_string();
                    let weight = item.weight.clamp(0.1, 1.0);
                    out.push(ExtractedConcept { name, kind, weight });
                }
            }
            if out.len() >= 12 {
                break;
            }
        }

        if out.is_empty() && count_before > 0 {
            bail!("All extracted concepts were filtered out during normalisation");
        }

        Ok(out)
    }
}

fn clean_json_markdown(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```json") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    } else if let Some(rest) = s.strip_prefix("```") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    s.to_string()
}
