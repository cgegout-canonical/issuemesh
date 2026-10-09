use anyhow::Result;
use issuemesh::config::Config;
use issuemesh::connectors::{Evidence, ObjectRef};
use issuemesh::mcp::handle_mcp_request;
use issuemesh::paths::Paths;
use issuemesh::store::Store;
use issuemesh::AppState;
use serde_json::json;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;

async fn create_test_state() -> Result<Arc<AppState>> {
    let tmp = NamedTempFile::new()?;
    let path = tmp.path().to_path_buf();
    let store = Store::open(&path)?;

    // Insert dummy evidence
    let ev = Evidence {
        r#ref: ObjectRef::new("jira", "issue", "MCP-1"),
        title: "Test MCP Capability".to_string(),
        state: "Open".to_string(),
        container: "MCP".to_string(),
        author: "tester".to_string(),
        snippet: "Checking MCP tools integration".to_string(),
        body: Some("Checking MCP tools integration".to_string()),
        url: "https://jira.example.com/issue/MCP-1".to_string(),
        created_at: Some("2026-10-01T10:00:00Z".to_string()),
        updated_at: Some("2026-10-08T12:00:00Z".to_string()),
        native: std::collections::BTreeMap::new(),
    };
    store.upsert_object(ev).await?;

    let state = AppState {
        paths: Paths {
            home: path.clone(),
            secrets_file: path.clone(),
            config_file: path.clone(),
            db_file: path.clone(),
            logs_dir: path.clone(),
        },
        config: Arc::new(RwLock::new(Config::default())),
        secrets: Arc::new(RwLock::new(issuemesh::config::Secrets::default())),
        store,
        jobs: issuemesh::jobs::JobManager::new(),
        introspection: issuemesh::introspection::IntrospectionScheduler::new(),
        search_results: Arc::new(RwLock::new(std::collections::HashMap::new())),
    };

    Ok(Arc::new(state))
}

#[tokio::test]
async fn test_mcp_initialize_and_ping() -> Result<()> {
    let state = create_test_state().await?;

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });

    let resp = handle_mcp_request(state.clone(), init_req).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "issuemesh");
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");

    // 2. Ping
    let ping_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping"
    });
    let ping_resp = handle_mcp_request(state, ping_req).await;
    assert!(ping_resp.is_some());
    assert_eq!(ping_resp.unwrap()["id"], 2);

    Ok(())
}

#[tokio::test]
async fn test_mcp_tools_list_advertisement() -> Result<()> {
    let state = create_test_state().await?;

    let list_req = json!({
        "jsonrpc": "2.0",
        "id": "req-tools",
        "method": "tools/list",
        "params": {}
    });

    let resp = handle_mcp_request(state, list_req).await;
    assert!(resp.is_some());
    let val = resp.unwrap();
    let tools = val["result"]["tools"].as_array().expect("Tools list array");

    // We advertised at least 16 tools
    assert!(tools.len() >= 16);

    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(tool_names.contains(&"search"));
    assert!(tool_names.contains(&"deep_search"));
    assert!(tool_names.contains(&"get_object"));
    assert!(tool_names.contains(&"what_changed"));
    assert!(tool_names.contains(&"impact_analysis"));
    assert!(tool_names.contains(&"risk_radar"));
    assert!(tool_names.contains(&"timeline"));
    assert!(tool_names.contains(&"find_gaps"));
    assert!(tool_names.contains(&"saved_investigation"));
    assert!(tool_names.contains(&"readiness"));
    assert!(tool_names.contains(&"consistency_check"));
    assert!(tool_names.contains(&"get_evidence"));

    Ok(())
}

#[tokio::test]
async fn test_mcp_tool_execution() -> Result<()> {
    let state = create_test_state().await?;

    // 1. Tool Call: search
    let search_req = json!({
        "jsonrpc": "2.0",
        "id": 101,
        "method": "tools/call",
        "params": {
            "name": "search",
            "arguments": {
                "query": "MCP"
            }
        }
    });

    let resp = handle_mcp_request(state.clone(), search_req).await.unwrap();
    assert_eq!(resp["id"], 101);
    assert_eq!(resp["result"]["isError"], false);
    let content = &resp["result"]["content"][0]["text"];
    assert!(content.as_str().unwrap().contains("jira:issue:MCP-1"));

    // 2. Tool Call: impact_analysis
    let impact_req = json!({
        "jsonrpc": "2.0",
        "id": 102,
        "method": "tools/call",
        "params": {
            "name": "impact_analysis",
            "arguments": {
                "object_key": "jira:issue:MCP-1"
            }
        }
    });
    let impact_resp = handle_mcp_request(state.clone(), impact_req).await.unwrap();
    assert_eq!(impact_resp["result"]["isError"], false);

    // 3. Tool Call: risk_radar
    let risk_req = json!({
        "jsonrpc": "2.0",
        "id": 103,
        "method": "tools/call",
        "params": {
            "name": "risk_radar",
            "arguments": {}
        }
    });
    let risk_resp = handle_mcp_request(state.clone(), risk_req).await.unwrap();
    assert_eq!(risk_resp["result"]["isError"], false);

    // 4. Tool Call: readiness
    let ready_req = json!({
        "jsonrpc": "2.0",
        "id": 104,
        "method": "tools/call",
        "params": {
            "name": "readiness",
            "arguments": {
                "target": "MCP"
            }
        }
    });
    let ready_resp = handle_mcp_request(state.clone(), ready_req).await.unwrap();
    assert_eq!(ready_resp["result"]["isError"], false);

    // 5. Unknown tool error
    let bad_req = json!({
        "jsonrpc": "2.0",
        "id": 105,
        "method": "tools/call",
        "params": {
            "name": "non_existent_tool",
            "arguments": {}
        }
    });
    let bad_resp = handle_mcp_request(state, bad_req).await.unwrap();
    assert_eq!(bad_resp["result"]["isError"], true);

    Ok(())
}
