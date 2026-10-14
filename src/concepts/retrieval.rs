use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::store::{ConceptRecord, Store};

pub struct ConceptSearchResult {
    pub concept: ConceptRecord,
    pub score: f32,
    pub matching_objects: Vec<(String, f64)>,
}

pub struct RelatedConceptResult {
    pub concept: ConceptRecord,
    pub co_occurrence: i64,
}

/// Search concepts using FTS5 (and semantic embeddings if present)
pub async fn concept_search(
    store: &Store,
    query: &str,
    limit: usize,
) -> Result<Vec<ConceptSearchResult>> {
    let fts_hits = store.search_concepts_fts(query, limit * 2).await?;
    let mut results = Vec::new();

    for (idx, concept) in fts_hits.into_iter().enumerate() {
        let matching_objects = store.get_objects_for_concept(concept.concept_id, 20).await?;
        let rank_score = (1.0 - (idx as f32 * 0.05)).max(0.2);
        results.push(ConceptSearchResult {
            concept,
            score: rank_score,
            matching_objects,
        });
        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Retrieve related concepts through the co-occurrence graph
pub async fn concept_related(
    store: &Store,
    concept_id: i64,
    limit: usize,
) -> Result<Vec<RelatedConceptResult>> {
    let related = store.get_related_concepts(concept_id, limit).await?;
    let mut out = Vec::new();
    for (concept, co_occurrence) in related {
        out.push(RelatedConceptResult {
            concept,
            co_occurrence,
        });
    }
    Ok(out)
}

/// Compute concept overlap score between query concept hits and an object
pub fn compute_concept_overlap_score(
    object_concepts: &[crate::store::ObjectConceptRecord],
    query_concept_ids: &HashSet<i64>,
) -> f32 {
    if object_concepts.is_empty() || query_concept_ids.is_empty() {
        return 0.0;
    }

    let mut overlap_weight = 0.0;
    let mut total_weight = 0.0;

    for oc in object_concepts {
        total_weight += oc.weight;
        if query_concept_ids.contains(&oc.concept_id) {
            overlap_weight += oc.weight;
        }
    }

    if total_weight <= 0.0 {
        return 0.0;
    }

    (overlap_weight / total_weight) as f32
}

/// Compute Jaccard/weighted overlap between two objects' concepts
pub fn compute_pairwise_concept_overlap(
    concepts_a: &[crate::store::ObjectConceptRecord],
    concepts_b: &[crate::store::ObjectConceptRecord],
) -> f32 {
    if concepts_a.is_empty() || concepts_b.is_empty() {
        return 0.0;
    }

    let map_a: HashMap<i64, f64> = concepts_a.iter().map(|c| (c.concept_id, c.weight)).collect();
    let map_b: HashMap<i64, f64> = concepts_b.iter().map(|c| (c.concept_id, c.weight)).collect();

    let mut intersection = 0.0;
    let mut union = 0.0;

    let mut all_ids: HashSet<i64> = map_a.keys().copied().collect();
    all_ids.extend(map_b.keys().copied());

    for id in all_ids {
        let wa = map_a.get(&id).copied().unwrap_or(0.0);
        let wb = map_b.get(&id).copied().unwrap_or(0.0);
        intersection += wa.min(wb);
        union += wa.max(wb);
    }

    if union <= 0.0 {
        return 0.0;
    }

    (intersection / union) as f32
}
