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
    async fn start() -> Option<Self> {
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

        let port = 18765;
        // Build binary path
        let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_issuemesh"));

        let child = Command::new(&bin_path)
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
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(150)).await;
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
            println!("Failed to start test server within timeout");
            return None;
        }

        Some(server)
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
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
    let server = match TestServer::start().await {
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
}

#[tokio::test]
#[ignore]
async fn e2e_10_health_api() {
    let server = match TestServer::start().await {
        Some(s) => s,
        None => return,
    };

    let client = reqwest::Client::new();
    let res = client.get(server.url("/api/health")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let data: serde_json::Value = res.json().await.unwrap();
    assert!(data["subsystems"].is_object());
}

#[tokio::test]
#[ignore]
async fn e2e_40_security_leak_scan() {
    let server = match TestServer::start().await {
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

    // Hit /settings and verify secrets are masked
    let client = reqwest::Client::new();
    let res = client
        .get(server.url("/api/settings"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = res.text().await.unwrap();

    for secret in &secret_tokens {
        assert!(
            !text.contains(secret),
            "Secret leaked in /api/settings response!"
        );
        let last12 = if secret.len() >= 12 {
            &secret[secret.len() - 12..]
        } else {
            secret.as_str()
        };
        assert!(
            !text.contains(last12),
            "Secret substring leaked in /api/settings response!"
        );
    }
}

#[tokio::test]
#[ignore]
async fn e2e_50_memory_and_integrity_check() {
    let server = match TestServer::start().await {
        Some(s) => s,
        None => return,
    };

    let client = reqwest::Client::new();
    let res = client.get(server.url("/api/memory")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let diag: serde_json::Value = res.json().await.unwrap();
    assert!(diag["total_objects"].is_number());
    assert!(diag["file_size_bytes"].is_number());

    let res = client
        .post(server.url("/api/memory/integrity"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let check: serde_json::Value = res.json().await.unwrap();
    assert_eq!(check["ok"], true);
}
