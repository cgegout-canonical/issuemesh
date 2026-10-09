pub mod executor;
pub mod tools;

use anyhow::Result;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::AppState;

/// Handles a single JSON-RPC 2.0 MCP request
pub async fn handle_mcp_request(state: Arc<AppState>, req: Value) -> Option<Value> {
    let jsonrpc = req.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("2.0");
    let id = req.get("id").cloned();
    let method = match req.get("method").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => {
            return id.map(|id_val| {
                json!({
                    "jsonrpc": jsonrpc,
                    "id": id_val,
                    "error": {
                        "code": -32600,
                        "message": "Invalid Request: missing method"
                    }
                })
            });
        }
    };

    let params = req.get("params").cloned().unwrap_or(Value::Null);

    // Notifications (no id) return None
    let is_notification = id.is_none();

    let result_or_err: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "issuemesh",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),

        "notifications/initialized" => {
            // No-op client acknowledgment
            return None;
        }

        "ping" => Ok(json!({})),

        "tools/list" => {
            let tools = tools::get_advertised_tools();
            Ok(json!({
                "tools": tools
            }))
        }

        "tools/call" => {
            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            match executor::execute_tool(state, tool_name, args).await {
                Ok(output) => {
                    let formatted_text = serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|_| output.to_string());
                    Ok(json!({
                        "content": [
                            {
                                "type": "text",
                                "text": formatted_text
                            }
                        ],
                        "structuredData": output,
                        "isError": false
                    }))
                }
                Err(err) => Ok(json!({
                    "content": [
                        {
                            "type": "text",
                            "text": format!("Error executing tool '{}': {}", tool_name, err)
                        }
                    ],
                    "isError": true
                })),
            }
        }

        _ => Err((-32601, format!("Method '{}' not found", method))),
    };

    if is_notification {
        return None;
    }

    let id_val = id.unwrap_or(Value::Null);
    let resp = match result_or_err {
        Ok(res) => json!({
            "jsonrpc": jsonrpc,
            "id": id_val,
            "result": res
        }),
        Err((code, msg)) => json!({
            "jsonrpc": jsonrpc,
            "id": id_val,
            "error": {
                "code": code,
                "message": msg
            }
        }),
    };

    Some(resp)
}

/// Runs MCP server over standard input/output (stdio)
pub async fn run_stdio_server(state: Arc<AppState>) -> Result<()> {
    eprintln!(
        "IssueMesh MCP server (stdio) starting, version {}...",
        env!("CARGO_PKG_VERSION")
    );
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(req) = serde_json::from_str::<Value>(trimmed) {
            if let Some(resp) = handle_mcp_request(Arc::clone(&state), req).await {
                let resp_str = serde_json::to_string(&resp)?;
                stdout.write_all(resp_str.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        } else {
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32700,
                    "message": "Parse error: invalid JSON"
                }
            });
            let resp_str = serde_json::to_string(&err_resp)?;
            stdout.write_all(resp_str.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    Ok(())
}

/// Axum HTTP handler for `/api/mcp` (POST)
pub async fn mcp_http_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if let Some(resp) = handle_mcp_request(state, payload).await {
        Json(resp)
    } else {
        Json(json!({ "jsonrpc": "2.0", "result": null }))
    }
}

/// Axum HTTP handler for `/api/mcp` (GET)
pub async fn mcp_info_handler() -> impl IntoResponse {
    let tools = tools::get_advertised_tools();
    Json(json!({
        "server": "issuemesh",
        "version": env!("CARGO_PKG_VERSION"),
        "protocolVersion": "2024-11-05",
        "tools_count": tools.len(),
        "tools": tools,
    }))
}
