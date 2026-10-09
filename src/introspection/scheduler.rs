use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

use super::evaluator::AnswerEvaluator;
use super::generator::QuestionGenerator;
use crate::search::orchestrator::SearchOrchestrator;
use crate::store::IntrospectionRunRecord;
use crate::AppState;

#[derive(Clone)]
pub struct IntrospectionScheduler {
    pub is_running: Arc<AtomicBool>,
    pub scheduler_status: Arc<RwLock<String>>,
    pub last_run_at: Arc<RwLock<Option<String>>>,
    pub next_expected_at: Arc<RwLock<Option<String>>>,
    pub failures_today: Arc<AtomicU32>,
}

impl Default for IntrospectionScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl IntrospectionScheduler {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            scheduler_status: Arc::new(RwLock::new("Healthy".to_string())),
            last_run_at: Arc::new(RwLock::new(None)),
            next_expected_at: Arc::new(RwLock::new(None)),
            failures_today: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Spawns the background evaluation loop
    pub fn start(&self, state: Arc<AppState>) {
        let sched = self.clone();
        tokio::spawn(async move {
            sched.run_loop(state).await;
        });
    }

    async fn run_loop(&self, state: Arc<AppState>) {
        tracing::info!("Introspection background scheduler started");

        // Populate initial last_run_at from store if present
        if let Ok(Some(last)) = state.store.get_last_introspection_run().await {
            let mut l = self.last_run_at.write().await;
            *l = Some(last.created_at);
        }

        loop {
            // Check loop every 30 seconds
            sleep(Duration::from_secs(30)).await;

            let (cfg, _sec) = {
                let c = state.config.read().await;
                let s = state.secrets.read().await;
                (c.clone(), s.clone())
            };

            let level = cfg.introspection.level;
            if level == 0 {
                let mut status = self.scheduler_status.write().await;
                *status = "Disabled".to_string();
                let mut next = self.next_expected_at.write().await;
                *next = None;
                continue;
            }

            let questions_per_day = cfg.introspection.questions_per_day();
            if questions_per_day == 0 {
                continue;
            }

            // Check how many runs completed today in UTC
            let runs_today = match state.store.count_introspection_runs_today().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Failed to query today's introspection count: {}", e);
                    continue;
                }
            };

            let interval_secs = (24 * 3600) / (questions_per_day as u64);

            if runs_today >= questions_per_day {
                let mut status = self.scheduler_status.write().await;
                *status = "Healthy (daily quota met)".to_string();

                // Next run will be tomorrow morning
                let tomorrow_utc = chrono::Utc::now()
                    .date_naive()
                    .succ_opt()
                    .unwrap_or_else(|| chrono::Utc::now().date_naive())
                    .and_hms_opt(1, 0, 0)
                    .unwrap()
                    .and_utc()
                    .to_rfc3339();
                let mut next = self.next_expected_at.write().await;
                *next = Some(tomorrow_utc);
                continue;
            }

            // Check time elapsed since last run
            let last_dt = {
                let l = self.last_run_at.read().await;
                l.as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
            };

            let now = chrono::Utc::now();
            let should_run = match last_dt {
                None => true, // Never run before: eligible immediately
                Some(prev) => {
                    let elapsed = now.signed_duration_since(prev).num_seconds();
                    if elapsed >= interval_secs as i64 {
                        true
                    } else {
                        let remaining_secs = (interval_secs as i64 - elapsed).max(0);
                        let next_dt = now + chrono::Duration::seconds(remaining_secs);
                        let mut next = self.next_expected_at.write().await;
                        *next = Some(next_dt.to_rfc3339());
                        false
                    }
                }
            };

            if should_run {
                if self
                    .is_running
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    // Already running
                    continue;
                }

                {
                    let mut status = self.scheduler_status.write().await;
                    *status = "Running".to_string();
                }

                tracing::info!("Executing scheduled introspection run...");
                match self.execute_cycle(state.clone()).await {
                    Ok(record) => {
                        tracing::info!(
                            "Introspection run completed successfully: id={} result={} score={:.2}",
                            record.id,
                            record.result,
                            record.weighted_score
                        );
                        let mut last = self.last_run_at.write().await;
                        *last = Some(record.created_at.clone());

                        let next_dt = now + chrono::Duration::seconds(interval_secs as i64);
                        let mut next = self.next_expected_at.write().await;
                        *next = Some(next_dt.to_rfc3339());

                        let mut status = self.scheduler_status.write().await;
                        *status = "Healthy".to_string();
                    }
                    Err(e) => {
                        tracing::error!("Introspection cycle error (scheduler resilient): {}", e);
                        self.failures_today.fetch_add(1, Ordering::SeqCst);
                        let mut status = self.scheduler_status.write().await;
                        *status = "Healthy".to_string();
                    }
                }

                self.is_running.store(false, Ordering::SeqCst);
            }
        }
    }

    /// Triggers an immediate introspection cycle on demand
    pub async fn trigger_now(&self, state: Arc<AppState>) -> Result<IntrospectionRunRecord> {
        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(anyhow!(
                "An introspection search is already actively executing"
            ));
        }

        {
            let mut status = self.scheduler_status.write().await;
            *status = "Running".to_string();
        }

        let res = self.execute_cycle(state).await;

        match &res {
            Ok(rec) => {
                let mut last = self.last_run_at.write().await;
                *last = Some(rec.created_at.clone());
                let mut status = self.scheduler_status.write().await;
                *status = "Healthy".to_string();
            }
            Err(e) => {
                self.failures_today.fetch_add(1, Ordering::SeqCst);
                let mut status = self.scheduler_status.write().await;
                *status = "Healthy".to_string();
                tracing::warn!("Manual introspection cycle error: {}", e);
            }
        }

        self.is_running.store(false, Ordering::SeqCst);
        res
    }

    async fn execute_cycle(&self, state: Arc<AppState>) -> Result<IntrospectionRunRecord> {
        let cfg = state.config.read().await.clone();

        // 1. Generate evidence-driven question
        let question = QuestionGenerator::generate(&state.store, &cfg).await?;

        // 2. Execute standard search pipeline
        let search_run_id = format!("intro-{}", chrono::Utc::now().timestamp_millis());
        let search_result =
            SearchOrchestrator::execute(state.clone(), Some(&search_run_id), &question.question)
                .await;

        // 3. Evaluate results across all 5 dimensions
        let eval = AnswerEvaluator::evaluate(&question, &search_result);

        // 4. Construct record and persist
        let run_id = format!("ir-{}", chrono::Utc::now().timestamp_millis());
        let created_at = chrono::Utc::now().to_rfc3339();
        let evidence_keys: Vec<String> = search_result
            .ranked_results
            .iter()
            .map(|r| r.evidence.r#ref.to_key())
            .collect();

        let record = IntrospectionRunRecord {
            id: run_id,
            created_at,
            question: question.question,
            question_fingerprint: question.fingerprint,
            trigger_reason: question.trigger_reason,
            search_run_id: Some(search_run_id),
            accuracy: eval.accuracy,
            relevance: eval.relevance,
            coverage: eval.coverage,
            traceability: eval.traceability,
            conciseness: eval.conciseness,
            weighted_score: eval.weighted_score,
            percentage: eval.percentage,
            result: eval.result,
            interpretation: eval.interpretation,
            answer: search_result.answer.unwrap_or_default(),
            problem_areas_json: serde_json::to_string(&eval.problem_areas)?,
            recommendation: eval.recommendation,
            evidence_keys_json: serde_json::to_string(&evidence_keys)?,
            evaluation_details_json: eval.details.to_string(),
        };

        state.store.save_introspection_run(record.clone()).await?;

        Ok(record)
    }
}
