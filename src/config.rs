use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_onboarding_completed")]
    pub onboarding_completed: bool,

    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub search: SearchConfig,

    #[serde(default)]
    pub github: GitHubConfig,

    #[serde(default)]
    pub jira: JiraConfig,

    #[serde(default)]
    pub launchpad: LaunchpadConfig,

    #[serde(default)]
    pub model: ModelConfig,
}

fn default_onboarding_completed() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            port: default_port(),
        }
    }
}

fn default_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8765
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_graph_depth")]
    pub graph_depth: u8,
    #[serde(default = "default_max_graph_nodes")]
    pub max_graph_nodes: usize,
    #[serde(default = "default_max_deep_reads")]
    pub max_deep_reads: usize,
    #[serde(default = "default_max_results_per_source")]
    pub max_results_per_source: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            graph_depth: default_graph_depth(),
            max_graph_nodes: default_max_graph_nodes(),
            max_deep_reads: default_max_deep_reads(),
            max_results_per_source: default_max_results_per_source(),
        }
    }
}

fn default_graph_depth() -> u8 {
    2
}
fn default_max_graph_nodes() -> usize {
    100
}
fn default_max_deep_reads() -> usize {
    30
}
fn default_max_results_per_source() -> usize {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub default_org: Option<String>,
    #[serde(default)]
    pub default_repos: Vec<String>,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_org: None,
            default_repos: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub default_project: Option<String>,
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_project: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchpadConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub default_project: Option<String>,
}

impl Default for LaunchpadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_project: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub enabled: bool,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub embedding_model: Option<String>,
    pub system_prompt: Option<String>,
}

impl Config {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = fs::read_to_string(path).context("Failed to read config.toml")?;
            let cfg: Self = toml::from_str(&content).context("Failed to parse config.toml")?;
            Ok(cfg)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        atomic_write_file(path, content.as_bytes(), 0o600)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub github_token: Option<String>,
    pub jira_url: Option<String>,
    pub jira_email: Option<String>,
    pub jira_token: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_api_key: Option<String>,
    pub openai_model: Option<String>,
    pub openai_embedding_model: Option<String>,
    pub launchpad_token: Option<String>,
    pub extra: BTreeMap<String, String>,
}

impl Secrets {
    pub fn load_from_file(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).context("Failed to read secrets file")?;
        Ok(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v
                    .split("   #")
                    .next()
                    .unwrap_or(v)
                    .trim()
                    .trim_matches('"')
                    .to_string();
                map.insert(k.trim().to_string(), v);
            }
        }

        Self {
            github_token: map.remove("GITHUB_TOKEN").filter(|s| !s.is_empty()),
            jira_url: map.remove("JIRA_URL").filter(|s| !s.is_empty()),
            jira_email: map.remove("JIRA_EMAIL").filter(|s| !s.is_empty()),
            jira_token: map.remove("JIRA_TOKEN").filter(|s| !s.is_empty()),
            openai_base_url: map.remove("OPENAI_BASE_URL").filter(|s| !s.is_empty()),
            openai_api_key: map.remove("OPENAI_API_KEY").filter(|s| !s.is_empty()),
            openai_model: map.remove("OPENAI_MODEL").filter(|s| !s.is_empty()),
            openai_embedding_model: map
                .remove("OPENAI_EMBEDDING_MODEL")
                .filter(|s| !s.is_empty()),
            launchpad_token: map.remove("LAUNCHPAD_TOKEN").filter(|s| !s.is_empty()),
            extra: map,
        }
    }

    pub fn to_env_string(&self) -> String {
        let mut lines = Vec::new();
        if let Some(ref v) = self.github_token {
            lines.push(format!("GITHUB_TOKEN=\"{}\"", v));
        }
        if let Some(ref v) = self.jira_url {
            lines.push(format!("JIRA_URL=\"{}\"", v));
        }
        if let Some(ref v) = self.jira_email {
            lines.push(format!("JIRA_EMAIL=\"{}\"", v));
        }
        if let Some(ref v) = self.jira_token {
            lines.push(format!("JIRA_TOKEN=\"{}\"", v));
        }
        if let Some(ref v) = self.openai_base_url {
            lines.push(format!("OPENAI_BASE_URL=\"{}\"", v));
        }
        if let Some(ref v) = self.openai_api_key {
            lines.push(format!("OPENAI_API_KEY=\"{}\"", v));
        }
        if let Some(ref v) = self.openai_model {
            lines.push(format!("OPENAI_MODEL=\"{}\"", v));
        }
        if let Some(ref v) = self.openai_embedding_model {
            lines.push(format!("OPENAI_EMBEDDING_MODEL=\"{}\"", v));
        }
        if let Some(ref v) = self.launchpad_token {
            lines.push(format!("LAUNCHPAD_TOKEN=\"{}\"", v));
        }
        for (k, v) in &self.extra {
            lines.push(format!("{k}=\"{v}\""));
        }
        lines.join("\n") + "\n"
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = self.to_env_string();
        atomic_write_file(path, content.as_bytes(), 0o600)?;
        Ok(())
    }
}

pub fn atomic_write_file(
    target: &Path,
    content: &[u8],
    #[allow(unused_variables)] mode: u32,
) -> Result<()> {
    let parent = target
        .parent()
        .context("Target file must have a parent directory")?;
    let tmp_path = parent.join(format!(".tmp_{}", uuid_simple()));

    fs::write(&tmp_path, content).context("Failed to write temporary file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(mode);
        fs::set_permissions(&tmp_path, perms).context("Failed to set file permissions")?;
    }

    fs::rename(&tmp_path, target).context("Failed to rename temporary file to target")?;
    Ok(())
}

fn uuid_simple() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}_{}", now.as_nanos(), std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_save_load() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let mut cfg = Config {
            onboarding_completed: true,
            ..Default::default()
        };
        cfg.search.graph_depth = 3;
        cfg.save(&path).unwrap();

        let loaded = Config::load_or_default(&path).unwrap();
        assert!(loaded.onboarding_completed);
        assert_eq!(loaded.search.graph_depth, 3);
    }

    #[test]
    fn test_secrets_parse_and_save() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("secrets.env");

        let raw = r#"
# comment line
GITHUB_TOKEN=ghp_secret123
JIRA_URL="https://test.atlassian.net"
JIRA_EMAIL=dev@example.com
JIRA_TOKEN=jira_tok_abc
CUSTOM_KEY=custom_val
"#;
        let secrets = Secrets::parse(raw);
        assert_eq!(secrets.github_token.as_deref(), Some("ghp_secret123"));
        assert_eq!(
            secrets.jira_url.as_deref(),
            Some("https://test.atlassian.net")
        );
        assert_eq!(secrets.jira_email.as_deref(), Some("dev@example.com"));
        assert_eq!(secrets.jira_token.as_deref(), Some("jira_tok_abc"));
        assert_eq!(
            secrets.extra.get("CUSTOM_KEY").map(|s| s.as_str()),
            Some("custom_val")
        );

        secrets.save_to_file(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }

        let reloaded = Secrets::load_from_file(&path).unwrap();
        assert_eq!(reloaded.github_token.as_deref(), Some("ghp_secret123"));
    }
}
