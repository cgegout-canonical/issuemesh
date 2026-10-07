use anyhow::{Context, Result};
use reqwest::{Client, RequestBuilder, Response};
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::sleep;

#[derive(thiserror::Error, Debug)]
pub enum HttpError {
    #[error("ReadOnlyViolation: POST {0} is not an allow-listed search endpoint")]
    ReadOnlyViolation(String),
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("Timeout error: {0}")]
    Timeout(String),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone)]
pub struct ReadOnlyHttpClient {
    client: Client,
    base: String,
    bearer: Arc<RwLock<Option<String>>>,
    basic: Arc<RwLock<Option<(String, String)>>>,
    post_allow: Vec<String>,
    pub last_latency_ms: Arc<AtomicI64>,
    pub token_expiry: Arc<RwLock<Option<String>>>,
    semaphore: Arc<Semaphore>,
    min_interval: Duration,
    last_call: Arc<tokio::sync::Mutex<Instant>>,
}

impl ReadOnlyHttpClient {
    pub fn new(
        base: &str,
        bearer: Option<String>,
        post_allow: &[&str],
        user_agent: &str,
        concurrency_limit: usize,
        min_interval_ms: u64,
    ) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(user_agent)
            .gzip(true)
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            bearer: Arc::new(RwLock::new(bearer)),
            basic: Arc::new(RwLock::new(None)),
            post_allow: post_allow.iter().map(|s| s.to_string()).collect(),
            last_latency_ms: Arc::new(AtomicI64::new(0)),
            token_expiry: Arc::new(RwLock::new(None)),
            semaphore: Arc::new(Semaphore::new(concurrency_limit)),
            min_interval: Duration::from_millis(min_interval_ms),
            last_call: Arc::new(tokio::sync::Mutex::new(
                Instant::now() - Duration::from_secs(10),
            )),
        })
    }

    pub fn new_basic(
        base: &str,
        user: &str,
        secret: &str,
        post_allow: &[&str],
        user_agent: &str,
        concurrency_limit: usize,
    ) -> Result<Self> {
        let me = Self::new(base, None, post_allow, user_agent, concurrency_limit, 0)?;
        if let Ok(mut b) = me.basic.write() {
            *b = Some((user.to_string(), secret.to_string()));
        }
        Ok(me)
    }

    pub fn set_bearer(&self, token: Option<String>) {
        if let Ok(mut b) = self.bearer.write() {
            *b = token;
        }
    }

    pub fn set_basic(&self, user: String, token: String) {
        if let Ok(mut b) = self.basic.write() {
            *b = Some((user, token));
        }
    }

    fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        let bearer = self.bearer.read().ok().and_then(|b| b.clone());
        let basic = self.basic.read().ok().and_then(|b| b.clone());
        match (bearer, basic) {
            (Some(b), _) => req.bearer_auth(b),
            (None, Some((u, p))) => req.basic_auth(u, Some(p)),
            _ => req,
        }
    }

    async fn enforce_rate_limits(&self) {
        if self.min_interval > Duration::ZERO {
            let mut last = self.last_call.lock().await;
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                sleep(self.min_interval - elapsed).await;
            }
            *last = Instant::now();
        }
    }

    async fn finish(&self, t0: Instant, r: Response) -> Result<Value, HttpError> {
        let latency = t0.elapsed().as_millis() as i64;
        self.last_latency_ms.store(latency, Ordering::Relaxed);

        if let Some(v) = r
            .headers()
            .get("github-authentication-token-expiration")
            .and_then(|h| h.to_str().ok())
        {
            if let Ok(mut g) = self.token_expiry.write() {
                *g = Some(v.to_string());
            }
        }

        let status = r.status();
        let text = r.text().await?;
        if !status.is_success() {
            let body_snip = text.chars().take(200).collect::<String>();
            return Err(HttpError::Status {
                status: status.as_u16(),
                body: body_snip,
            });
        }

        if text.trim().is_empty() {
            return Ok(Value::Null);
        }

        let val: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Ok(val)
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, HttpError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| HttpError::Other(e.into()))?;
        self.enforce_rate_limits().await;

        let url = format!("{}{}", self.base, path);
        let t0 = Instant::now();
        let req = self
            .auth(self.client.get(&url).query(query))
            .header("Accept", "application/json");

        let fut = req.send();
        let resp = tokio::time::timeout(Duration::from_secs(30), fut)
            .await
            .map_err(|_| HttpError::Timeout(format!("GET {path} timed out after 30s")))?
            .map_err(HttpError::Reqwest)?;

        self.finish(t0, resp).await
    }

    pub async fn post_search(&self, path: &str, body: &Value) -> Result<Value, HttpError> {
        if !self.post_allow.iter().any(|p| path.starts_with(p)) {
            return Err(HttpError::ReadOnlyViolation(path.to_string()));
        }

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| HttpError::Other(e.into()))?;
        self.enforce_rate_limits().await;

        let url = format!("{}{}", self.base, path);
        let t0 = Instant::now();
        let req = self
            .auth(self.client.post(&url).json(body))
            .header("Accept", "application/json");

        let fut = req.send();
        let resp = tokio::time::timeout(Duration::from_secs(30), fut)
            .await
            .map_err(|_| HttpError::Timeout(format!("POST {path} timed out after 30s")))?
            .map_err(HttpError::Reqwest)?;

        self.finish(t0, resp).await
    }
}
