use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntrospectionResult {
    PASS,
    WEAK,
    FAIL,
}

impl IntrospectionResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PASS => "PASS",
            Self::WEAK => "WEAK",
            Self::FAIL => "FAIL",
        }
    }
}

impl std::fmt::Display for IntrospectionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

pub const PROBLEM_AREAS: &[&str] = &[
    "retrieval",
    "query planning",
    "tracker connector",
    "relation discovery",
    "graph traversal",
    "ranking",
    "cache freshness",
    "consistency analysis",
    "semantic search",
    "LLM synthesis",
    "hallucination",
    "answer formatting",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedQuestion {
    pub question: String,
    pub fingerprint: String,
    pub trigger_reason: String,
    pub candidate_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationScores {
    pub accuracy: f64,
    pub relevance: f64,
    pub coverage: f64,
    pub traceability: f64,
    pub conciseness: f64,
    pub weighted_score: f64,
    pub percentage: f64,
    pub result: String,
    pub interpretation: String,
    pub problem_areas: Vec<String>,
    pub recommendation: String,
    pub details: serde_json::Value,
}
