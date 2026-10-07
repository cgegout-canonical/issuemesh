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

        Ok(Self {
            client,
            base_url,
            api_key,
            model: model.into(),
            embedding_model: embedding_model.into(),
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
}
