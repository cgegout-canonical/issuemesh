use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ModelClient {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    embedding_model: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
    index: usize,
}

impl ModelClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
        embedding_model: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("Failed to build HTTP client for model")?;

        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }

        let mut model: String = model.into();
        let mut embedding_model: String = embedding_model.into();

        if base_url.contains("openrouter.ai") {
            if model == "gpt-sol-latest" || model.is_empty() {
                model = "openai/gpt-4o-mini".to_string();
            } else if !model.contains('/') {
                model = format!("openai/{}", model);
            }

            if embedding_model == "gpt-sol-latest" || embedding_model.is_empty() {
                embedding_model = "openai/text-embedding-3-small".to_string();
            } else if !embedding_model.contains('/') {
                embedding_model = format!("openai/{}", embedding_model);
            }
        }

        Ok(Self {
            client,
            base_url,
            api_key,
            model,
            embedding_model,
        })
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(key) = &self.api_key {
            if !key.trim().is_empty() {
                if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", key.trim())) {
                    headers.insert(AUTHORIZATION, val);
                }
            }
        }
        headers
    }

    pub async fn chat_completion(&self, messages: Vec<ChatMessage>) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let req_body = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.1),
            max_tokens: Some(2048),
        };

        let resp = self
            .client
            .post(&url)
            .headers(self.auth_headers())
            .json(&req_body)
            .send()
            .await
            .context("Failed to send chat completion request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Model API error ({status}): {err_text}");
        }

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .context("Failed to parse model chat completion response")?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .context("Model returned no completion choices")
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/embeddings", self.base_url);
        let req_body = EmbeddingRequest {
            model: self.embedding_model.clone(),
            input: texts.to_vec(),
        };

        let resp = self
            .client
            .post(&url)
            .headers(self.auth_headers())
            .json(&req_body)
            .send()
            .await
            .context("Failed to send embedding request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Embedding API error ({status}): {err_text}");
        }

        let mut parsed: EmbeddingResponse = resp
            .json()
            .await
            .context("Failed to parse model embedding response")?;

        parsed.data.sort_by_key(|item| item.index);
        Ok(parsed.data.into_iter().map(|item| item.embedding).collect())
    }

    pub async fn discover_models(
        base_url: &str,
        api_key: Option<&str>,
        timeout: Duration,
    ) -> ModelDiscoveryResult {
        let base_url = base_url.trim();
        if base_url.is_empty() {
            return ModelDiscoveryResult {
                ok: false,
                error: Some("Endpoint URL cannot be empty".to_string()),
                models: Vec::new(),
                chat_models: Vec::new(),
                embedding_models: Vec::new(),
            };
        }

        let client = match reqwest::Client::builder().timeout(timeout).build() {
            Ok(c) => c,
            Err(e) => {
                return ModelDiscoveryResult {
                    ok: false,
                    error: Some(format!("Failed to build HTTP client: {e}")),
                    models: Vec::new(),
                    chat_models: Vec::new(),
                    embedding_models: Vec::new(),
                };
            }
        };

        let candidate_urls = build_candidate_urls(base_url);
        if candidate_urls.is_empty() {
            return ModelDiscoveryResult {
                ok: false,
                error: Some("Endpoint URL cannot be empty or invalid".to_string()),
                models: Vec::new(),
                chat_models: Vec::new(),
                embedding_models: Vec::new(),
            };
        }

        let make_request = |url: &str| {
            let mut req = client
                .get(url)
                .header("HTTP-Referer", "https://issuemesh.local")
                .header("X-Title", "IssueMesh");
            if let Some(key) = api_key {
                let key = key.trim();
                if !key.is_empty() {
                    req = req.bearer_auth(key);
                }
            }
            req
        };

        let mut last_error = None;

        for url in &candidate_urls {
            let resp = match make_request(url).send().await {
                Ok(r) => r,
                Err(e) => {
                    return ModelDiscoveryResult {
                        ok: false,
                        error: Some(format!("Endpoint unreachable or invalid: {e}")),
                        models: Vec::new(),
                        chat_models: Vec::new(),
                        embedding_models: Vec::new(),
                    };
                }
            };

            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return ModelDiscoveryResult {
                    ok: false,
                    error: Some(format!(
                        "Authentication failed (HTTP {status}): Invalid API key"
                    )),
                    models: Vec::new(),
                    chat_models: Vec::new(),
                    embedding_models: Vec::new(),
                };
            }

            if status.is_success() {
                let json_val: serde_json::Value = match resp.json().await {
                    Ok(val) => val,
                    Err(e) => {
                        last_error = Some(format!(
                            "The response could not be interpreted as valid JSON: {e}"
                        ));
                        continue;
                    }
                };

                let mut models = parse_model_listing(&json_val);
                if models.is_empty() {
                    last_error = Some(
                        "The response could not be interpreted as a valid model list".to_string(),
                    );
                    continue;
                }

                let chat_models: Vec<DiscoveredModel> =
                    models.iter().filter(|m| m.is_chat).cloned().collect();
                let mut embedding_models: Vec<DiscoveredModel> =
                    models.iter().filter(|m| m.is_embedding).cloned().collect();

                // If no embedding models were found in the primary list (e.g. OpenRouter which isolates
                // embedding models to /api/v1/embeddings/models), probe dedicated embedding model endpoints.
                if embedding_models.is_empty() {
                    let mut embed_candidates = Vec::new();
                    if let Some(stripped) = url.strip_suffix("/models") {
                        embed_candidates.push(format!("{stripped}/embeddings/models"));
                        embed_candidates.push(format!("{url}?output_modalities=embeddings"));
                    }
                    for emb_url in embed_candidates {
                        if let Ok(resp) = make_request(&emb_url).send().await {
                            if resp.status().is_success() {
                                if let Ok(emb_json) = resp.json::<serde_json::Value>().await {
                                    let extra_models = parse_model_listing(&emb_json);
                                    for em in extra_models {
                                        let mut m = em;
                                        m.is_embedding = true;
                                        if !models.iter().any(|existing| existing.id == m.id) {
                                            models.push(m.clone());
                                        }
                                        if !embedding_models
                                            .iter()
                                            .any(|existing| existing.id == m.id)
                                        {
                                            embedding_models.push(m);
                                        }
                                    }
                                    if !embedding_models.is_empty() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                return ModelDiscoveryResult {
                    ok: true,
                    error: None,
                    models,
                    chat_models,
                    embedding_models,
                };
            } else if status != reqwest::StatusCode::NOT_FOUND
                && status != reqwest::StatusCode::METHOD_NOT_ALLOWED
            {
                let body = resp.text().await.unwrap_or_default();
                let short_body = if body.len() > 200 {
                    &body[..200]
                } else {
                    &body
                };
                last_error = Some(format!(
                    "Model endpoint returned HTTP {status}: {short_body}"
                ));
            } else if last_error.is_none() {
                last_error = Some(format!(
                    "Model endpoint returned HTTP {status}: Endpoint not found"
                ));
            }
        }

        ModelDiscoveryResult {
            ok: false,
            error: last_error.or_else(|| {
                Some("The response could not be interpreted as a valid model list".to_string())
            }),
            models: Vec::new(),
            chat_models: Vec::new(),
            embedding_models: Vec::new(),
        }
    }
}

pub fn build_candidate_urls(base_url: &str) -> Vec<String> {
    let mut trimmed = base_url.trim();
    while trimmed.ends_with('/') {
        trimmed = &trimmed[..trimmed.len() - 1];
    }
    if trimmed.is_empty() {
        return Vec::new();
    }

    if let Some(stripped) = trimmed.strip_suffix("/chat/completions") {
        trimmed = stripped.trim_end_matches('/');
    } else if let Some(stripped) = trimmed.strip_suffix("/completions") {
        trimmed = stripped.trim_end_matches('/');
    } else if let Some(stripped) = trimmed.strip_suffix("/embeddings") {
        trimmed = stripped.trim_end_matches('/');
    }

    if trimmed.is_empty() {
        return Vec::new();
    }

    if trimmed.ends_with("/models") || trimmed.ends_with("/tags") {
        return vec![trimmed.to_string()];
    }

    let mut candidates = Vec::new();
    if trimmed.ends_with("/v1") {
        candidates.push(format!("{trimmed}/models"));
        let parent = trimmed
            .strip_suffix("/v1")
            .unwrap_or(trimmed)
            .trim_end_matches('/');
        if !parent.is_empty() {
            candidates.push(format!("{parent}/models"));
            candidates.push(format!("{parent}/api/tags"));
        }
    } else {
        candidates.push(format!("{trimmed}/v1/models"));
        candidates.push(format!("{trimmed}/models"));
        candidates.push(format!("{trimmed}/api/tags"));
    }

    candidates
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: String,
    pub is_embedding: bool,
    pub is_chat: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDiscoveryResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub models: Vec<DiscoveredModel>,
    pub chat_models: Vec<DiscoveredModel>,
    pub embedding_models: Vec<DiscoveredModel>,
}

pub fn parse_model_listing(json_val: &serde_json::Value) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();

    let parse_entry =
        |v: &serde_json::Value, fallback_id: Option<&str>| -> Option<DiscoveredModel> {
            if let Some(s) = v.as_str() {
                let id = s.trim().to_string();
                if id.is_empty() {
                    return None;
                }
                return Some(classify_model(id.clone(), id, None, None, false, None));
            }

            let obj = v.as_object()?;

            let id = obj
                .get("id")
                .or_else(|| obj.get("model"))
                .or_else(|| obj.get("model_name"))
                .or_else(|| obj.get("model_id"))
                .or_else(|| obj.get("modelId"))
                .or_else(|| obj.get("name"))
                .or_else(|| obj.get("key"))
                .and_then(|x| x.as_str())
                .or(fallback_id)?
                .trim()
                .to_string();

            if id.is_empty() {
                return None;
            }

            let name = obj
                .get("name")
                .or_else(|| obj.get("model_name"))
                .and_then(|x| x.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| id.clone());

            let ctx_len = obj
                .get("context_length")
                .or_else(|| obj.get("max_model_len"))
                .or_else(|| obj.get("context_window"))
                .or_else(|| obj.get("max_tokens"))
                .or_else(|| obj.get("context_size"))
                .or_else(|| obj.get("max_position_embeddings"))
                .and_then(|x| x.as_u64());

            let modality = obj
                .get("architecture")
                .and_then(|a| a.get("modality"))
                .and_then(|m| m.as_str())
                .or_else(|| obj.get("modality").and_then(|m| m.as_str()))
                .or_else(|| obj.get("type").and_then(|t| t.as_str()))
                .or_else(|| obj.get("pipeline_tag").and_then(|t| t.as_str()));

            let has_embed_capability = obj
                .get("capabilities")
                .map(|caps| {
                    if let Some(caps_obj) = caps.as_object() {
                        caps_obj
                            .get("embeddings")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                            || caps_obj
                                .get("embedding")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                    } else if let Some(caps_arr) = caps.as_array() {
                        caps_arr
                            .iter()
                            .any(|v| v.as_str().is_some_and(|s| s.contains("embed")))
                    } else {
                        false
                    }
                })
                .unwrap_or(false);

            let description = obj.get("description").and_then(|d| d.as_str());

            Some(classify_model(
                id,
                name,
                ctx_len,
                modality,
                has_embed_capability,
                description,
            ))
        };

    if let Some(arr) = json_val.as_array() {
        for item in arr {
            if let Some(m) = parse_entry(item, None) {
                models.push(m);
            }
        }
    } else if let Some(obj) = json_val.as_object() {
        if let Some(data_val) = obj.get("data") {
            if let Some(arr) = data_val.as_array() {
                for item in arr {
                    if let Some(m) = parse_entry(item, None) {
                        models.push(m);
                    }
                }
            } else if let Some(map) = data_val.as_object() {
                for (k, v) in map {
                    if let Some(m) = parse_entry(v, Some(k.as_str())) {
                        models.push(m);
                    }
                }
            }
        }

        if let Some(models_val) = obj.get("models") {
            if let Some(arr) = models_val.as_array() {
                for item in arr {
                    if let Some(m) = parse_entry(item, None) {
                        models.push(m);
                    }
                }
            } else if let Some(map) = models_val.as_object() {
                for (k, v) in map {
                    if let Some(m) = parse_entry(v, Some(k.as_str())) {
                        models.push(m);
                    }
                }
            }
        }

        if let Some(result_val) = obj.get("result") {
            if let Some(arr) = result_val.as_array() {
                for item in arr {
                    if let Some(m) = parse_entry(item, None) {
                        models.push(m);
                    }
                }
            } else if let Some(res_obj) = result_val.as_object() {
                if let Some(arr) = res_obj.get("models").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(m) = parse_entry(item, None) {
                            models.push(m);
                        }
                    }
                } else if let Some(arr) = res_obj.get("data").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(m) = parse_entry(item, None) {
                            models.push(m);
                        }
                    }
                }
            }
        }

        if let Some(arr) = obj.get("items").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(m) = parse_entry(item, None) {
                    models.push(m);
                }
            }
        }

        if let Some(arr) = obj.get("response").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(m) = parse_entry(item, None) {
                    models.push(m);
                }
            }
        }

        // If no models collected, check if this object is itself a single model
        if models.is_empty() {
            if (obj.contains_key("id")
                || obj.contains_key("model")
                || obj.contains_key("model_name"))
                && !obj.contains_key("error")
                && !obj.contains_key("errors")
                && !obj.contains_key("detail")
            {
                if let Some(m) = parse_entry(json_val, None) {
                    models.push(m);
                }
            } else if !obj.contains_key("error")
                && !obj.contains_key("errors")
                && !obj.contains_key("detail")
                && !obj.contains_key("status")
            {
                let all_objects = !obj.is_empty() && obj.values().all(|v| v.is_object());
                if all_objects {
                    for (k, v) in obj {
                        if k != "error"
                            && k != "errors"
                            && k != "detail"
                            && k != "status"
                            && k != "message"
                        {
                            if let Some(m) = parse_entry(v, Some(k.as_str())) {
                                models.push(m);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    models.retain(|m| seen.insert(m.id.clone()));
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

fn classify_model(
    id: String,
    name: String,
    context_length: Option<u64>,
    modality: Option<&str>,
    has_embed_capability: bool,
    description: Option<&str>,
) -> DiscoveredModel {
    let lower_id = id.to_lowercase();
    let lower_name = name.to_lowercase();
    let lower_mod = modality.map(|s| s.to_lowercase()).unwrap_or_default();
    let lower_desc = description.map(|s| s.to_lowercase()).unwrap_or_default();

    let is_embedding = has_embed_capability
        || lower_mod.contains("embedding")
        || lower_mod.contains("feature-extraction")
        || lower_mod.contains("sentence-similarity")
        || lower_id.contains("embed")
        || lower_id.contains("embedding")
        || lower_id.contains("bge-")
        || lower_id.contains("e5-")
        || lower_id.contains("gte-")
        || lower_id.contains("text-similarity")
        || lower_id.contains("sentence-transformers")
        || lower_id.contains("minilm")
        || lower_id.contains("nomic")
        || lower_id.contains("mxbai")
        || lower_id.contains("voyage")
        || lower_id.contains("ada-002")
        || lower_id.contains("text-embedding-3")
        || lower_name.contains("embedding")
        || lower_desc.contains("embedding model")
        || lower_desc.contains("text embedding");

    let is_chat = !is_embedding;

    DiscoveredModel {
        id,
        name,
        is_embedding,
        is_chat,
        context_length,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_candidate_urls() {
        assert_eq!(
            build_candidate_urls("https://api.openai.com/v1"),
            vec![
                "https://api.openai.com/v1/models".to_string(),
                "https://api.openai.com/models".to_string(),
                "https://api.openai.com/api/tags".to_string(),
            ]
        );
        assert_eq!(
            build_candidate_urls("https://api.openai.com/v1/chat/completions"),
            vec![
                "https://api.openai.com/v1/models".to_string(),
                "https://api.openai.com/models".to_string(),
                "https://api.openai.com/api/tags".to_string(),
            ]
        );
        assert_eq!(
            build_candidate_urls("http://localhost:11434"),
            vec![
                "http://localhost:11434/v1/models".to_string(),
                "http://localhost:11434/models".to_string(),
                "http://localhost:11434/api/tags".to_string(),
            ]
        );
        assert_eq!(
            build_candidate_urls("http://localhost:11434/api/tags"),
            vec!["http://localhost:11434/api/tags".to_string()]
        );
    }

    #[test]
    fn test_parse_openai_format() {
        let payload = json!({
            "object": "list",
            "data": [
                {
                    "id": "gpt-4o",
                    "object": "model",
                    "created": 1715368132,
                    "owned_by": "system"
                },
                {
                    "id": "text-embedding-3-small",
                    "object": "model",
                    "created": 1705948997,
                    "owned_by": "system"
                }
            ]
        });

        let models = parse_model_listing(&payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert!(models[0].is_chat);
        assert!(!models[0].is_embedding);
        assert_eq!(models[1].id, "text-embedding-3-small");
        assert!(models[1].is_embedding);
        assert!(!models[1].is_chat);
    }

    #[test]
    fn test_parse_ollama_format() {
        let payload = json!({
            "models": [
                {
                    "name": "llama3:latest",
                    "model": "llama3:latest",
                    "size": 4661224676_i64
                },
                {
                    "name": "nomic-embed-text:latest",
                    "model": "nomic-embed-text:latest",
                    "size": 274302450
                }
            ]
        });

        let models = parse_model_listing(&payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "llama3:latest");
        assert!(models[0].is_chat);
        assert_eq!(models[1].id, "nomic-embed-text:latest");
        assert!(models[1].is_embedding);
    }

    #[test]
    fn test_parse_cloudflare_format() {
        let payload = json!({
            "result": [
                {
                    "id": "@cf/meta/llama-3-8b-instruct",
                    "name": "Llama 3 8B Instruct",
                    "description": "Meta Llama 3 model"
                },
                {
                    "id": "@cf/baai/bge-small-en-v1.5",
                    "name": "BGE Small EN",
                    "description": "Text embedding model"
                }
            ],
            "success": true
        });

        let models = parse_model_listing(&payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "@cf/baai/bge-small-en-v1.5");
        assert!(models[0].is_embedding);
        assert_eq!(models[1].id, "@cf/meta/llama-3-8b-instruct");
        assert!(models[1].is_chat);
    }

    #[test]
    fn test_parse_raw_array_and_items() {
        let raw_arr = json!(["qwen-2.5-72b", "bge-m3"]);
        let models = parse_model_listing(&raw_arr);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "bge-m3");
        assert!(models[0].is_embedding);
        assert_eq!(models[1].id, "qwen-2.5-72b");
        assert!(models[1].is_chat);

        let items_obj = json!({
            "items": [
                {"model_name": "mistral-large", "context_window": 128000}
            ]
        });
        let models2 = parse_model_listing(&items_obj);
        assert_eq!(models2.len(), 1);
        assert_eq!(models2[0].id, "mistral-large");
        assert_eq!(models2[0].context_length, Some(128000));
    }

    #[test]
    fn test_parse_invalid_or_empty_json() {
        let health = json!({"status": "ok", "version": "1.0"});
        let models = parse_model_listing(&health);
        assert!(models.is_empty());

        let err_resp = json!({"error": {"message": "Invalid auth"}});
        let models_err = parse_model_listing(&err_resp);
        assert!(models_err.is_empty());
    }

    #[test]
    fn test_parse_dictionary_maps() {
        let map_payload = json!({
            "models": {
                "llama-3-8b": {"modality": "text"},
                "text-embedding-3-small": {"modality": "embedding"}
            }
        });
        let models = parse_model_listing(&map_payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "llama-3-8b");
        assert!(models[0].is_chat);
        assert_eq!(models[1].id, "text-embedding-3-small");
        assert!(models[1].is_embedding);
    }

    #[test]
    fn test_parse_openrouter_embeddings_format() {
        let payload = json!({
            "data": [
                {
                    "id": "liquid/lfm-2.5-embedding-350m:free",
                    "name": "LiquidAI: LFM2.5-Embedding-350M (free)",
                    "description": "Text embedding model",
                    "architecture": {
                        "modality": "text->embeddings",
                        "output_modalities": ["embeddings"]
                    }
                },
                {
                    "id": "voyageai/voyage-code-4",
                    "name": "VoyageAI by MongoDB: voyage-code-4",
                    "description": "Code embedding model",
                    "architecture": {
                        "modality": "text->embeddings"
                    }
                }
            ]
        });
        let models = parse_model_listing(&payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "liquid/lfm-2.5-embedding-350m:free");
        assert!(models[0].is_embedding);
        assert_eq!(models[1].id, "voyageai/voyage-code-4");
        assert!(models[1].is_embedding);
    }
}
