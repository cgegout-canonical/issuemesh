use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::connectors::Evidence;

/// Weights configuration for hybrid scoring
/// Default formula per SPEC.md §30:
/// 0.35 lexical + 0.35 semantic + 0.20 graph + 0.10 freshness
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub lexical: f32,
    pub semantic: f32,
    pub graph: f32,
    pub freshness: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            lexical: 0.35,
            semantic: 0.35,
            graph: 0.20,
            freshness: 0.10,
        }
    }
}

/// Compute cosine similarity between two float vectors in pure Rust
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a <= 0.0 || norm_b <= 0.0 {
        return 0.0;
    }

    (dot / (norm_a.sqrt() * norm_b.sqrt())).clamp(-1.0, 1.0)
}

/// Compute freshness score in [0.0, 1.0] using exponential decay
/// Half-life of 30 days
pub fn compute_freshness_score(updated_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f32 {
    let dt = match updated_at {
        Some(d) => d,
        None => return 0.2, // Default base score for unknown timestamps
    };

    let diff = now.signed_duration_since(dt);
    let days = diff.num_days().max(0) as f32;

    // Exponential decay: score = 2^(-days / 30)
    let half_life_days = 30.0f32;
    let score = (-days * (2.0f32.ln() / half_life_days)).exp();
    score.clamp(0.0, 1.0)
}

/// Compute graph proximity score in [0.0, 1.0] given graph traversal distance/depth
pub fn compute_graph_proximity_score(depth: u8) -> f32 {
    match depth {
        0 => 1.0, // Seed / direct query hit
        1 => 0.8, // 1-hop neighbor
        2 => 0.5, // 2-hop neighbor
        3 => 0.2, // 3-hop neighbor
        _ => 0.05,
    }
}

/// Compute hybrid score combining all components
pub fn compute_hybrid_score(
    weights: &Weights,
    lexical_score: f32,   // Normalized [0.0, 1.0]
    semantic_score: f32,  // Normalized [0.0, 1.0]
    graph_proximity: f32, // Normalized [0.0, 1.0]
    freshness_score: f32, // Normalized [0.0, 1.0]
) -> f32 {
    let raw = weights.lexical * lexical_score.clamp(0.0, 1.0)
        + weights.semantic * semantic_score.clamp(0.0, 1.0)
        + weights.graph * graph_proximity.clamp(0.0, 1.0)
        + weights.freshness * freshness_score.clamp(0.0, 1.0);

    raw.clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedEvidence {
    pub evidence: Evidence,
    pub score: f32,
    pub lexical_score: f32,
    pub semantic_score: f32,
    pub graph_score: f32,
    pub freshness_score: f32,
}
