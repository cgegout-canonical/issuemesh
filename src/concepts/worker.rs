use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use crate::concepts::extractor::ConceptExtractor;
use crate::config::{Config, Secrets};
use crate::connectors::Evidence;
use crate::model::ModelClient;
use crate::store::Store;

pub struct ConceptJob {
    pub evidence: Evidence,
    pub content_hash: String,
}

#[derive(Clone)]
pub struct ConceptWorkerHandle {
    tx: mpsc::Sender<ConceptJob>,
}

impl ConceptWorkerHandle {
    pub async fn enqueue(&self, job: ConceptJob) -> bool {
        self.tx.try_send(job).is_ok()
    }
}

pub struct ConceptWorker {
    store: Store,
    config: Arc<RwLock<Config>>,
    secrets: Arc<RwLock<Secrets>>,
    rx: mpsc::Receiver<ConceptJob>,
}

impl ConceptWorker {
    pub fn start(
        store: Store,
        config: Arc<RwLock<Config>>,
        secrets: Arc<RwLock<Secrets>>,
        max_queue_depth: usize,
    ) -> (ConceptWorkerHandle, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(max_queue_depth.max(16));
        let handle = ConceptWorkerHandle { tx };
        let mut worker = ConceptWorker {
            store,
            config,
            secrets,
            rx,
        };

        let join = tokio::spawn(async move {
            worker.run().await;
        });

        (handle, join)
    }

    async fn run(&mut self) {
        info!("ConceptWorker started");
        // Semaphore to limit concurrent model invocations to 2
        let sem = Arc::new(Semaphore::new(2));

        while let Some(job) = self.rx.recv().await {
            let cfg = self.config.read().await.clone();
            if !cfg.concepts.enabled {
                debug!(
                    "Concept extraction is disabled; skipping job {}",
                    job.evidence.r#ref.to_key()
                );
                continue;
            }

            let object_key = job.evidence.r#ref.to_key();

            // 1. Check if existing index entry has same content_hash
            match self.store.get_concept_index_status(&object_key).await {
                Ok(Some(status)) => {
                    if status.0 == job.content_hash && status.1 == "done" {
                        debug!(
                            "Object {} content_hash unchanged and already indexed; skipping",
                            object_key
                        );
                        let _ = self
                            .store
                            .update_concept_index(
                                &object_key,
                                &job.content_hash,
                                "skipped",
                                0,
                                None,
                            )
                            .await;
                        continue;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        "Failed to query concept index status for {}: {e}",
                        object_key
                    );
                }
            }

            // 2. Check token budget
            if let Ok(used_today) = self.store.get_concept_tokens_used_today().await {
                if used_today >= cfg.concepts.daily_token_budget {
                    warn!(
                        "Daily concept token budget reached ({}/{} tokens); skipping object {}",
                        used_today, cfg.concepts.daily_token_budget, object_key
                    );
                    let _ = self
                        .store
                        .update_concept_index(
                            &object_key,
                            &job.content_hash,
                            "skipped",
                            0,
                            Some("budget_exceeded"),
                        )
                        .await;
                    continue;
                }
            }

            // 3. Check if model is configured
            let sec = self.secrets.read().await.clone();
            let base_url = sec
                .openai_base_url
                .as_deref()
                .or(cfg.model.base_url.as_deref());
            let model_name = sec.openai_model.as_deref().or(cfg.model.model.as_deref());

            let (base_url, model_name) = match (base_url, model_name) {
                (Some(b), Some(m)) if cfg.model.enabled || sec.openai_api_key.is_some() => {
                    (b.to_string(), m.to_string())
                }
                _ => {
                    debug!("Model client not configured for concept extraction; marking pending for {}", object_key);
                    let _ = self
                        .store
                        .update_concept_index(
                            &object_key,
                            &job.content_hash,
                            "pending",
                            0,
                            Some("model_unavailable"),
                        )
                        .await;
                    continue;
                }
            };

            let embedding_model = sec
                .openai_embedding_model
                .or(cfg.model.embedding_model)
                .unwrap_or_else(|| "text-embedding-3-small".to_string());

            let client = match ModelClient::new(
                base_url,
                sec.openai_api_key,
                model_name,
                embedding_model,
                Duration::from_secs(30),
            ) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to build ModelClient for concept extraction: {e}");
                    let _ = self
                        .store
                        .update_concept_index(
                            &object_key,
                            &job.content_hash,
                            "failed",
                            0,
                            Some("client_error"),
                        )
                        .await;
                    continue;
                }
            };

            let permit = match sem.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };

            let store = self.store.clone();
            let max_input_tokens = cfg.concepts.max_input_tokens;

            tokio::spawn(async move {
                let _permit = permit;
                let res = ConceptExtractor::extract(&client, &job.evidence, max_input_tokens).await;

                match res {
                    Ok(extracted) => {
                        let mut concept_ids = Vec::new();
                        for item in &extracted {
                            match store.upsert_concept(&item.name, &item.kind).await {
                                Ok(cid) => {
                                    concept_ids.push(cid);
                                    let _ = store
                                        .link_object_concept(
                                            &object_key,
                                            cid,
                                            item.weight,
                                            "title|body|comments",
                                            &job.content_hash,
                                        )
                                        .await;
                                }
                                Err(e) => {
                                    error!("Failed to upsert concept '{}': {e}", item.name);
                                }
                            }
                        }

                        // Link all concept pairs for co-occurrence graph (a < b)
                        for i in 0..concept_ids.len() {
                            for j in (i + 1)..concept_ids.len() {
                                let (a, b) = if concept_ids[i] < concept_ids[j] {
                                    (concept_ids[i], concept_ids[j])
                                } else {
                                    (concept_ids[j], concept_ids[i])
                                };
                                let _ = store.link_concept_pair(a, b).await;
                            }
                        }

                        // Approximate tokens: input text / 4 + output tokens
                        let tokens_used = (job.evidence.title.len()
                            + job.evidence.body.as_deref().unwrap_or("").len())
                            / 4
                            + 100;
                        let _ = store
                            .update_concept_index(
                                &object_key,
                                &job.content_hash,
                                "done",
                                tokens_used,
                                None,
                            )
                            .await;
                        debug!(
                            "Successfully extracted {} concepts for {}",
                            extracted.len(),
                            object_key
                        );
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        let error_class = if err_str.contains("parse") || err_str.contains("JSON") {
                            "parse"
                        } else if err_str.contains("timeout") {
                            "timeout"
                        } else {
                            "network"
                        };
                        warn!("Concept extraction failed for {}: {err_str}", object_key);
                        let _ = store
                            .update_concept_index(
                                &object_key,
                                &job.content_hash,
                                "failed",
                                0,
                                Some(error_class),
                            )
                            .await;
                    }
                }
            });
        }
        info!("ConceptWorker shut down");
    }
}
