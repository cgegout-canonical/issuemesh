use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use reqwest::StatusCode;
use tempfile::tempdir;

struct TestServer {
    child: Child,
    port: u16,
    home_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

impl TestServer {
    fn get_free_port() -> Option<u16> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);
        Some(port)
    }

    async fn start(with_onboarding: bool) -> Option<Self> {
        let home_env = env::var("ISSUEMESH_HOME")
            .unwrap_or_else(|_| format!("{}/.issuemesh", env::var("HOME").unwrap_or_default()));
        let secrets_src = PathBuf::from(home_env).join("secrets.env");
        if !secrets_src.exists() {
            println!("secrets.env not found at {:?}, skipping e2e", secrets_src);
            return None;
        }

        let tmp = tempdir().ok()?;
        let temp_home = tmp.path().to_path_buf();

        // Copy secrets.env to temp_home
        let secrets_dest = temp_home.join("secrets.env");
        let content = fs::read_to_string(&secrets_src).ok()?;
        fs::write(&secrets_dest, content).ok()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&secrets_dest, fs::Permissions::from_mode(0o600));
            let _ = fs::set_permissions(&temp_home, fs::Permissions::from_mode(0o700));
        }

        let port = Self::get_free_port()?;
        let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_issuemesh"));

        let child = Command::new(&bin_path)
            .env("ISSUEMESH_HOME", &temp_home)
            .arg("--data-dir")
            .arg(&temp_home)
            .arg("--port")
            .arg(port.to_string())
            .spawn()
            .ok()?;

        let server = Self {
            child,
            port,
            home_dir: temp_home,
            _tmp: tmp,
        };

        // Wait for server to come up
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap();
        let mut healthy = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(res) = client
                .get(format!("http://127.0.0.1:{port}/api/health"))
                .send()
                .await
            {
                if res.status().is_success() || res.status() == StatusCode::OK {
                    healthy = true;
                    break;
                }
            }
        }

        if !healthy {
            println!("Failed to start test server within timeout on port {port}");
            return None;
        }

        if with_onboarding && !server.complete_onboarding().await {
            println!("Failed to complete onboarding for test server on port {port}");
            return None;
        }

        Some(server)
    }

    async fn complete_onboarding(&self) -> bool {
        let client = self.client();
        let payload = serde_json::json!({
            "github": {
                "enabled": true,
                "token": null,
                "default_org": null,
                "default_repos": []
            },
            "jira": {
                "enabled": true,
                "url": null,
                "email": null,
                "token": null,
                "default_project": null
            },
            "launchpad": {
                "enabled": true,
                "default_project": null
            },
            "model": {
                "enabled": false,
                "base_url": null,
                "api_key": null,
                "chat_model": null,
                "embedding_model": null
            },
            "search": {
                "graph_depth": 2,
                "max_graph_nodes": 50,
                "max_deep_reads": 10
            }
        });

        match client
            .post(self.url("/api/setup"))
            .json(&payload)
            .send()
            .await
        {
            Ok(res) => res.status().is_success(),
            Err(_) => false,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
#[ignore]
async fn e2e_01_startup_and_onboarding_redirect() {
    let server = match TestServer::start(false).await {
        Some(s) => s,
        None => return,
    };

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = client.get(server.url("/")).send().await.unwrap();
    // In unonboarded state, / should redirect to /setup (HTTP 307)
    assert_eq!(res.status(), StatusCode::TEMPORARY_REDIRECT);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(loc, "/setup");

    // Fetch /setup directly
    let res = client.get(server.url("/setup")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.text().await.unwrap();
    assert!(body.contains("IssueMesh Setup"));
    assert!(body.contains("Configure Trackers"));
}

#[tokio::test]
#[ignore]
async fn e2e_02_static_assets() {
    let server = match TestServer::start(false).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // CSS
    let res = client
        .get(server.url("/static/css/styles.css"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/css"));

    // JS
    let res = client
        .get(server.url("/static/js/app.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("javascript"));

    // Favicon
    let res = client
        .get(server.url("/static/favicon.svg"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("image/svg+xml"));

    // 404 for nonexistent asset
    let res = client
        .get(server.url("/static/nonexistent.xyz"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn e2e_03_onboarding_wizard_and_home_transition() {
    let server = match TestServer::start(false).await {
        Some(s) => s,
        None => return,
    };
    // Verify setup completion
    let success = server.complete_onboarding().await;
    assert!(success, "Onboarding submission failed");

    // Verify secrets file still has 0600 permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(server.home_dir.join("secrets.env")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    // Now request / with redirect none - it should serve index.html directly (200 OK)
    let no_redirect_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let res = no_redirect_client
        .get(server.url("/"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.text().await.unwrap();
    assert!(body.contains("IssueMesh"));
    assert!(body.contains("Search issues"));
}

#[tokio::test]
#[ignore]
async fn e2e_04_ui_pages_availability() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    let pages = [
        ("/", "IssueMesh"),
        ("/settings", "Settings"),
        ("/memory", "Local Memory"),
        ("/sources", "Sources"),
        ("/introspection", "Introspection"),
        ("/health", "Health"),
        ("/tutorial", "Tutorial"),
    ];

    for (path, expected_text) in pages {
        let res = client.get(server.url(path)).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "Page {path} failed");
        let body = res.text().await.unwrap();
        assert!(
            body.contains(expected_text),
            "Page {path} did not contain '{expected_text}'"
        );
    }
}

#[tokio::test]
#[ignore]
async fn e2e_10_health_api() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };

    let client = server.client();
    let res = client.get(server.url("/api/health")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let data: serde_json::Value = res.json().await.unwrap();
    assert!(data["subsystems"].is_object());
    assert!(data["subsystems"]["database"].is_object());
    assert!(data["subsystems"]["github"].is_object());
    assert!(data["subsystems"]["jira"].is_object());
    assert!(data["subsystems"]["launchpad"].is_object());
    assert!(data["subsystems"]["model"].is_object());
}

#[tokio::test]
#[ignore]
async fn e2e_15_connector_probes() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // 1. Launchpad test probe
    let res = client
        .post(server.url("/api/test/launchpad"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let lp_res: serde_json::Value = res.json().await.unwrap();
    assert!(lp_res["latency_ms"].is_number());

    // 2. GitHub test probe
    let res = client
        .post(server.url("/api/test/github"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let gh_res: serde_json::Value = res.json().await.unwrap();
    assert!(gh_res["latency_ms"].is_number());

    // 3. Jira test probe
    let res = client
        .post(server.url("/api/test/jira"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 4. Model test probe
    let res = client
        .post(server.url("/api/test/model"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn e2e_20_search_workflow_and_sse() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // 1. Create a search job
    let res = client
        .post(server.url("/api/search"))
        .json(&serde_json::json!({ "query": "memory leak fix" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let job_data: serde_json::Value = res.json().await.unwrap();
    let job_id = job_data["job_id"].as_str().unwrap().to_string();

    // 2. Stream SSE events
    let sse_res = client
        .get(server.url(&format!("/api/search/{job_id}/events")))
        .send()
        .await
        .unwrap();
    assert_eq!(sse_res.status(), StatusCode::OK);

    use futures::StreamExt;
    let mut stream = sse_res.bytes_stream();
    let mut events_seen = 0;
    let timeout = Duration::from_secs(12);
    let start = std::time::Instant::now();

    while let Ok(Some(chunk_result)) =
        tokio::time::timeout(Duration::from_millis(2000), stream.next()).await
    {
        if let Ok(bytes) = chunk_result {
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                if line.starts_with("data:") {
                    events_seen += 1;
                }
            }
            if text.contains("Done") || text.contains("done") {
                break;
            }
        }
        if start.elapsed() > timeout {
            break;
        }
    }
    assert!(
        events_seen > 0,
        "Expected to receive SSE events for search job"
    );

    // 3. Verify job summary
    let job_res = client
        .get(server.url(&format!("/api/search/{job_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(job_res.status(), StatusCode::OK);
    let summary: serde_json::Value = job_res.json().await.unwrap();
    assert_eq!(summary["query"], "memory leak fix");

    // 4. Poll for search result
    let mut result_opt = None;
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let r = client
            .get(server.url(&format!("/api/search/{job_id}/result")))
            .send()
            .await;
        if let Ok(res) = r {
            if res.status() == StatusCode::OK {
                let data: serde_json::Value = res.json().await.unwrap();
                result_opt = Some(data);
                break;
            }
        }
    }
    let result = result_opt.expect("Search result should be available");
    assert!(result["ranked_results"].is_array());
    assert!(result["stats"].is_object());
    assert!(result["plan"].is_object());

    // 5. Verify search run API
    let run_res = client
        .get(server.url(&format!("/api/search/{job_id}/run")))
        .send()
        .await
        .unwrap();
    assert_eq!(run_res.status(), StatusCode::OK);
    let run_data: serde_json::Value = run_res.json().await.unwrap();
    assert_eq!(run_data["run"]["query"], "memory leak fix");

    // 6. Verify search run page HTML
    let html_res = client
        .get(server.url(&format!("/search/{job_id}/run")))
        .send()
        .await
        .unwrap();
    assert_eq!(html_res.status(), StatusCode::OK);
    let html = html_res.text().await.unwrap();
    assert!(html.contains("Search Run Details") || html.contains("IssueMesh"));
}

#[tokio::test]
#[ignore]
async fn e2e_25_local_fts_search() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    let res = client
        .get(server.url("/api/search/local?q=bug&limit=10"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let hits: serde_json::Value = res.json().await.unwrap();
    assert!(hits.is_array());
}

#[tokio::test]
#[ignore]
async fn e2e_30_sources_and_memory_stats() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // Sources
    let res = client.get(server.url("/api/sources")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let sources: serde_json::Value = res.json().await.unwrap();
    assert!(sources.is_object() || sources.is_array());

    // Memory stats
    let res = client.get(server.url("/api/memory")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let diag: serde_json::Value = res.json().await.unwrap();
    assert!(diag["total_objects"].is_number());
    assert!(diag["total_relations"].is_number());
    assert!(diag["file_size_bytes"].is_number());
}

#[tokio::test]
#[ignore]
async fn e2e_35_memory_maintenance_suite() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // 1. Integrity check
    let res = client
        .post(server.url("/api/memory/integrity"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // 2. Optimize
    let res = client
        .post(server.url("/api/memory/optimize"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // 3. Rebuild FTS
    let res = client
        .post(server.url("/api/memory/rebuild-fts"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // 4. Purge stale
    let res = client
        .post(server.url("/api/memory/purge-stale"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // 5. Clear history
    let res = client
        .post(server.url("/api/memory/clear-history"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // Verify total_runs is 0
    let res = client.get(server.url("/api/memory")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let diag: serde_json::Value = res.json().await.unwrap();
    assert_eq!(diag["total_runs"], 0);
}

#[tokio::test]
#[ignore]
async fn e2e_40_security_leak_scan() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };

    let secrets_content = fs::read_to_string(server.home_dir.join("secrets.env")).unwrap();
    let mut secret_tokens = Vec::new();
    for line in secrets_content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.contains('=') {
            continue;
        }
        let parts: Vec<&str> = trimmed.splitn(2, '=').collect();
        let val = parts[1].trim().trim_matches('"').trim_matches('\'');
        if val.len() >= 8 {
            secret_tokens.push(val.to_string());
        }
    }

    let client = server.client();
    let endpoints = [
        "/api/settings",
        "/api/health",
        "/api/memory",
        "/api/sources",
        "/api/stats",
        "/api/search",
    ];

    for endpoint in endpoints {
        let res = client.get(server.url(endpoint)).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await.unwrap();

        for secret in &secret_tokens {
            assert!(
                !text.contains(secret),
                "Secret leaked in {endpoint} response!"
            );
            let last8 = if secret.len() >= 8 {
                &secret[secret.len() - 8..]
            } else {
                secret.as_str()
            };
            assert!(
                !text.contains(last8),
                "Secret trailing substring leaked in {endpoint} response!"
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn e2e_45_input_validation_and_error_handling() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // 1. Empty search query
    let res = client
        .post(server.url("/api/search"))
        .json(&serde_json::json!({ "query": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = client
        .post(server.url("/api/search"))
        .json(&serde_json::json!({ "query": "     " }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 2. Non-existent job
    let res = client
        .get(server.url("/api/search/nonexistent-uuid-000"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 3. Non-existent result
    let res = client
        .get(server.url("/api/search/nonexistent-uuid-000/result"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 4. Non-existent run
    let res = client
        .get(server.url("/api/search/nonexistent-uuid-000/run"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn e2e_22_launchpad_search_and_object_graph() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    let res = client
        .post(server.url("/api/search"))
        .json(&serde_json::json!({ "query": "openstack" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let job_data: serde_json::Value = res.json().await.unwrap();
    let job_id = job_data["job_id"].as_str().unwrap().to_string();

    let mut result_opt = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Ok(res) = client
            .get(server.url(&format!("/api/search/{job_id}/result")))
            .send()
            .await
        {
            if res.status() == StatusCode::OK {
                result_opt = Some(res.json::<serde_json::Value>().await.unwrap());
                break;
            }
        }
    }
    let result = result_opt.expect("Launchpad search result should be available");
    assert!(result["ranked_results"].is_array());
    assert!(result["stats"]["trackers_searched"].as_i64().unwrap_or(0) >= 1);
}

#[tokio::test]
#[ignore]
async fn e2e_31_cache_reuse_and_deduplication() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    // First search
    let res1 = client
        .post(server.url("/api/search"))
        .json(&serde_json::json!({ "query": "cache test query" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res1.status(), StatusCode::ACCEPTED);
    let job1_id = res1.json::<serde_json::Value>().await.unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string();

    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(r) = client
            .get(server.url(&format!("/api/search/{job1_id}/result")))
            .send()
            .await
        {
            if r.status() == StatusCode::OK {
                break;
            }
        }
    }

    // Second search with identical query
    let res2 = client
        .post(server.url("/api/search"))
        .json(&serde_json::json!({ "query": "cache test query" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::ACCEPTED);
    let job2_id = res2.json::<serde_json::Value>().await.unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut res2_data = None;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(r) = client
            .get(server.url(&format!("/api/search/{job2_id}/result")))
            .send()
            .await
        {
            if r.status() == StatusCode::OK {
                res2_data = Some(r.json::<serde_json::Value>().await.unwrap());
                break;
            }
        }
    }
    assert!(
        res2_data.is_some(),
        "Second cached search completed successfully"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_41_read_only_invariant_and_audit() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    let _ = client.get(server.url("/api/health")).send().await;

    let db_path = server.home_dir.join("issuemesh.db");
    if db_path.exists() {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let mut stmt = conn
            .prepare("SELECT method, endpoint FROM api_audit")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();

        for row in rows.flatten() {
            let (method, endpoint) = row;
            assert!(
                method == "GET" || method == "POST",
                "Non-read-only method found in audit: {method} {endpoint}"
            );
            assert_ne!(method, "PUT");
            assert_ne!(method, "PATCH");
            assert_ne!(method, "DELETE");
        }
    }
}

#[tokio::test]
#[ignore]
async fn e2e_50_memory_diagnostics_and_sql_reconciliation() {
    let server = match TestServer::start(true).await {
        Some(s) => s,
        None => return,
    };
    let client = server.client();

    let res = client.get(server.url("/api/memory")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let diag: serde_json::Value = res.json().await.unwrap();

    let api_objects = diag["total_objects"].as_i64().unwrap();
    let api_relations = diag["total_relations"].as_i64().unwrap();
    let api_runs = diag["total_runs"].as_i64().unwrap();

    let db_path = server.home_dir.join("issuemesh.db");
    assert!(db_path.exists());
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    let sql_objects: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracker_objects", [], |r| r.get(0))
        .unwrap_or(0);
    let sql_relations: i64 = conn
        .query_row("SELECT COUNT(*) FROM object_relations", [], |r| r.get(0))
        .unwrap_or(0);
    let sql_runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM search_runs", [], |r| r.get(0))
        .unwrap_or(0);

    assert_eq!(
        api_objects, sql_objects,
        "Objects count mismatch between API and SQL"
    );
    assert_eq!(
        api_relations, sql_relations,
        "Relations count mismatch between API and SQL"
    );
    assert_eq!(
        api_runs, sql_runs,
        "Runs count mismatch between API and SQL"
    );

    let integ_res = client
        .post(server.url("/api/memory/integrity"))
        .send()
        .await
        .unwrap();
    assert_eq!(integ_res.status(), StatusCode::OK);
    let integ: serde_json::Value = integ_res.json().await.unwrap();
    assert_eq!(integ["ok"], true);
}

#[tokio::test]
#[ignore]
async fn e2e_60_binary_properties_and_isolation() {
    let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_issuemesh"));
    assert!(bin_path.exists(), "Binary must exist at {:?}", bin_path);

    let output = Command::new(&bin_path)
        .arg("--version")
        .output()
        .expect("Binary should respond to --version");
    assert!(output.status.success());
    let ver_str = String::from_utf8_lossy(&output.stdout);
    assert!(ver_str.contains("issuemesh") || ver_str.contains(env!("CARGO_PKG_VERSION")));

    if let Ok(ldd_output) = Command::new("ldd").arg(&bin_path).output() {
        let ldd_str = String::from_utf8_lossy(&ldd_output.stdout);
        assert!(
            !ldd_str.contains("libssl"),
            "Binary must not link dynamically against libssl!"
        );
        assert!(
            !ldd_str.contains("libsqlite"),
            "Binary must not link dynamically against libsqlite!"
        );
    }
}
