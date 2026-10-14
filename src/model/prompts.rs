//! Prompts used across IssueMesh models.

pub const CONCEPT_EXTRACT_V1: &str = r#"You are an expert system engineer extracting canonical domain concepts from engineering issues and pull requests.
Extract the key domain concepts (components, features, technologies, problems, releases, teams) referenced in the issue.
Return ONLY valid JSON matching this exact schema:
{
  "concepts": [
    {
      "name": "canonical concept name (lowercase, concise, 2-4 words max)",
      "kind": "component|feature|technology|problem|release|team|other",
      "weight": 0.95
    }
  ]
}

Rules:
1. Extract at most 12 concepts.
2. concept "name" must be lowercase, alphanumeric/hyphen/space, without punctuation. Do not use generic words like "bug", "issue", "fix", "error", or the tracker key itself.
3. "kind" must be one of: component, feature, technology, problem, release, team, other.
4. "weight" is a float from 0.1 to 1.0 representing importance to the issue's core topic.
5. Return ONLY the JSON object. Do not include markdown ticks, explanation, or commentary.
"#;
