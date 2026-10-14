use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub inputSchema: Value,
}

pub fn get_advertised_tools() -> Vec<McpToolDefinition> {
    vec![
        McpToolDefinition {
            name: "search".to_string(),
            description: "Fast lexical and semantic search across indexed Jira, GitHub, and Launchpad items.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query terms or issue identifier (e.g. 'OPEN-4679', 'BGP')" },
                    "limit": { "type": "integer", "description": "Maximum number of results (default: 20)" }
                },
                "required": ["query"]
            }),
        },
        McpToolDefinition {
            name: "deep_search".to_string(),
            description: "Full IssueMesh multi-hop graph search with cross-tracker synthesis, consistency checks, and objective confidence.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Engineering question or research topic" }
                },
                "required": ["query"]
            }),
        },
        McpToolDefinition {
            name: "get_object".to_string(),
            description: "Retrieve a single tracked object by canonical key (e.g. 'jira:issue:OPEN-4679', 'github:pull:123').".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "object_key": { "type": "string", "description": "Canonical key of the object" }
                },
                "required": ["object_key"]
            }),
        },
        McpToolDefinition {
            name: "get_evidence".to_string(),
            description: "Fetch comprehensive evidence record for an object including native tracker payloads, relations, and revision snapshots.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "object_key": { "type": "string", "description": "Canonical key of the object" }
                },
                "required": ["object_key"]
            }),
        },
        McpToolDefinition {
            name: "related".to_string(),
            description: "Retrieve linked and neighboring objects in the mesh graph (blockers, dependencies, fixed-by PRs, bug links).".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "object_key": { "type": "string", "description": "Canonical key to traverse outward from" }
                },
                "required": ["object_key"]
            }),
        },
        McpToolDefinition {
            name: "impact_analysis".to_string(),
            description: "Analyse what could be affected downstream or upstream by a delay, change, or failure of an object with full evidence path.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "object_key": { "type": "string", "description": "Canonical key of the target issue, PR, or component" }
                },
                "required": ["object_key"]
            }),
        },
        McpToolDefinition {
            name: "what_changed".to_string(),
            description: "Inspect changes over time for a specific object or evaluate watchlist diffs since previous check.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "object_key": { "type": "string", "description": "Specific object key to inspect revision history for" },
                    "watchlist_id": { "type": "string", "description": "Optional watchlist ID to check for newly added, modified, or resolved items" }
                }
            }),
        },
        McpToolDefinition {
            name: "timeline".to_string(),
            description: "Reconstruct a chronological story across Jira, GitHub, and Launchpad showing how an issue evolved and decisions made.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "object_key": { "type": "string", "description": "Root object key (e.g. 'jira:issue:OPEN-4679')" }
                },
                "required": ["object_key"]
            }),
        },
        McpToolDefinition {
            name: "risk_radar".to_string(),
            description: "Scan the engineering mesh for stale blockers, critical issues without activity, merged PRs with unresolved Jira work, and contradictions.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "Optional category filter: 'stale_blocker', 'critical_inactivity', 'merged_pr_unresolved_jira', 'contradictory_state', 'missing_owner', 'deep_dependency_chain'" }
                }
            }),
        },
        McpToolDefinition {
            name: "readiness".to_string(),
            description: "Generate evidence-based production / release readiness score (0-100%) and assessment for a product, container, or topic.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Project container (e.g. 'canonical/sunbeam'), milestone, or topic" }
                },
                "required": ["target"]
            }),
        },
        McpToolDefinition {
            name: "find_gaps".to_string(),
            description: "Detect governance gaps: requirements without PRs, active Jira items without PRs, merged code without release evidence, unresolved references, and duplicate work.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "Optional filter: 'requirement_without_pr', 'jira_without_pr', 'merged_without_release', 'unresolved_ref', 'duplicated_work'" }
                }
            }),
        },
        McpToolDefinition {
            name: "consistency_check".to_string(),
            description: "Identify cross-tracker contradictions and state divergences (e.g. Jira Done vs GitHub PR Open, Launchpad Fix Released vs Jira In Progress).".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional query to scope consistency checks" }
                }
            }),
        },
        McpToolDefinition {
            name: "saved_investigation".to_string(),
            description: "Manage saved investigations: save, list, or reopen previous investigations with automatic delta detection (new, changed, resolved, contradictory evidence).".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "description": "Action: 'list', 'get', 'reopen', 'save', 'delete'", "enum": ["list", "get", "reopen", "save", "delete"] },
                    "id": { "type": "string", "description": "Investigation ID (required for 'get', 'reopen', 'delete')" },
                    "title": { "type": "string", "description": "Title (required for 'save')" },
                    "query": { "type": "string", "description": "Search query (required for 'save')" },
                    "conclusions_markdown": { "type": "string", "description": "Conclusions in markdown (required for 'save')" },
                    "evidence_keys": { "type": "array", "items": { "type": "string" }, "description": "Array of evidence keys (required for 'save')" }
                },
                "required": ["action"]
            }),
        },
        McpToolDefinition {
            name: "memory_stats".to_string(),
            description: "Datastore memory diagnostics, SQLite table record counts, cache allocations, and WAL statistics.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpToolDefinition {
            name: "health".to_string(),
            description: "Check connector health (Jira, GitHub, Launchpad), auth states, rate limits, and datastore status.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpToolDefinition {
            name: "introspection_status".to_string(),
            description: "Inspect IssueMesh continuous introspection engine: quality scores, pass/weak/fail ratios, recent self-tests, and quality trends.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpToolDefinition {
            name: "concepts".to_string(),
            description: "Search or inspect the native domain concept graph, linked objects, and top concepts across tracked repositories.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional search term for domain concepts" },
                    "source": { "type": "string", "description": "Optional source container filter (e.g. 'jira', 'github', 'launchpad')" },
                    "limit": { "type": "integer", "description": "Maximum number of concepts to return (default: 20)" }
                }
            }),
        },
        McpToolDefinition {
            name: "semantic_related".to_string(),
            description: "Traverse semantic concept co-occurrence and find objects sharing deep domain concepts.".to_string(),
            inputSchema: json!({
                "type": "object",
                "properties": {
                    "concept_id": { "type": "integer", "description": "Concept ID to traverse related concepts and objects for" },
                    "object_key": { "type": "string", "description": "Object key to retrieve domain concepts and semantic neighbors for" },
                    "limit": { "type": "integer", "description": "Maximum number of results (default: 15)" }
                }
            }),
        },
    ]
}
