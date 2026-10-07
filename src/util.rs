use regex::Regex;
use std::sync::LazyLock;

static RE_BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)bearer\s+([a-zA-Z0-9_\-\.]{4,})").unwrap());
static RE_BASIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)basic\s+([a-zA-Z0-9+/=]{4,})").unwrap());
static RE_AUTH_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)("?authorization"?\s*[:=]\s*"?[^",\s]+"?)"#).unwrap());
static RE_KEY_PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(api[_-]?key|token|secret|password)\s*[:=]\s*["']?([^"',\s]+)["']?"#).unwrap()
});

/// Redacts sensitive authentication tokens and secrets from strings for safe logging.
pub fn redact_text(input: &str) -> String {
    let mut out = input.to_string();

    out = RE_BEARER.replace_all(&out, "Bearer [REDACTED]").to_string();
    out = RE_BASIC.replace_all(&out, "Basic [REDACTED]").to_string();
    out = RE_AUTH_HEADER
        .replace_all(&out, "authorization: [REDACTED]")
        .to_string();
    out = RE_KEY_PARAM.replace_all(&out, "$1=[REDACTED]").to_string();

    out
}

/// Masks a secret string keeping only the last 4 characters if long enough.
/// Example: "github_pat_1234567890abcdef" -> "••••cdef"
pub fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= 4 {
        return "••••".to_string();
    }
    let last4: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("••••{}", last4)
}

pub static PAT_JIRA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b((?:[A-Z][A-Z0-9]+)-\d+)\b").unwrap());
pub static PAT_LP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:LP:?\s*#|bugs\.launchpad\.net/[\w./+-]+?/\+bug/|launchpad\.net/bugs/)(\d{5,8})")
        .unwrap()
});
pub static PAT_GH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:https?://github\.com/|github\.com/)?([\w.-]+/[\w.-]+)(?:#|/pull/|/issues/)(\d+)",
    )
    .unwrap()
});

/// Returns true if the prefix is NOT one of the false-positive exclusions.
pub fn is_jira_key(k: &str) -> bool {
    let p = k.split('-').next().unwrap_or("");
    !matches!(
        p,
        "CVE"
            | "USN"
            | "RFC"
            | "ISO"
            | "SHA"
            | "AES"
            | "UTF"
            | "IPV"
            | "IEEE"
            | "PCI"
            | "HTTP"
            | "TLS"
            | "GPT"
            | "LP"
    )
}

/// Atlassian Document Format flattening helper
pub fn adf_text(v: &serde_json::Value) -> String {
    fn walk(n: &serde_json::Value, out: &mut String) {
        match n {
            serde_json::Value::Object(m) => {
                if let Some(t) = m.get("text").and_then(|x| x.as_str()) {
                    out.push_str(t);
                }
                let ty = m.get("type").and_then(|x| x.as_str()).unwrap_or("");
                if ty == "hardBreak" {
                    out.push('\n');
                }
                if ty == "mention" {
                    if let Some(attrs) = m.get("attrs") {
                        if let Some(txt) = attrs.get("text").and_then(|x| x.as_str()) {
                            out.push_str(txt);
                        }
                    }
                }
                if ty == "inlineCard" {
                    if let Some(attrs) = m.get("attrs") {
                        if let Some(url) = attrs.get("url").and_then(|x| x.as_str()) {
                            out.push_str(url);
                        }
                    }
                }
                if let Some(c) = m.get("content").and_then(|x| x.as_array()) {
                    for x in c {
                        walk(x, out);
                    }
                }
                if matches!(
                    ty,
                    "paragraph" | "heading" | "listItem" | "codeBlock" | "blockquote" | "tableRow"
                ) {
                    out.push('\n');
                }
            }
            serde_json::Value::Array(a) => {
                for x in a {
                    walk(x, out);
                }
            }
            serde_json::Value::String(t) => out.push_str(t),
            _ => {}
        }
    }
    let mut out = String::new();
    walk(v, &mut out);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_secret() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("abc"), "••••");
        assert_eq!(mask_secret("abcd"), "••••");
        assert_eq!(mask_secret("github_pat_1234abcd"), "••••abcd");
    }

    #[test]
    fn test_redact_text() {
        let text = "Sending request with Bearer ghp_secret123456 to endpoint";
        assert_eq!(
            redact_text(text),
            "Sending request with Bearer [REDACTED] to endpoint"
        );

        let text2 = "Header: Authorization: Bearer abcdef123456";
        assert!(redact_text(text2).contains("[REDACTED]"));

        let text3 = "token=secret_value_12345";
        assert_eq!(redact_text(text3), "token=[REDACTED]");
    }

    #[test]
    fn test_is_jira_key_exclusions() {
        let excluded = [
            "CVE-2023-1234",
            "USN-5890-1",
            "RFC-2616",
            "ISO-8601",
            "SHA-256",
            "AES-128",
            "UTF-8",
            "IPV-6",
            "IEEE-802",
            "PCI-DSS",
            "HTTP-2",
            "TLS-13",
            "GPT-4",
            "LP-12345",
        ];
        for key in excluded {
            assert!(!is_jira_key(key), "Key {} should be excluded", key);
        }

        assert!(is_jira_key("OPEN-4679"));
        assert!(is_jira_key("PROJ-101"));
        assert!(is_jira_key("CANONICAL-42"));
    }

    #[test]
    fn test_pat_lp_and_gh() {
        let text = "Fixes LP#2167315 and references canonical/project#42 and https://github.com/org/repo/pull/123";
        let lp_matches: Vec<_> = PAT_LP
            .captures_iter(text)
            .map(|c| c[1].to_string())
            .collect();
        assert_eq!(lp_matches, vec!["2167315"]);

        let gh_matches: Vec<_> = PAT_GH
            .captures_iter(text)
            .map(|c| (c[1].to_string(), c[2].to_string()))
            .collect();
        assert_eq!(
            gh_matches,
            vec![
                ("canonical/project".to_string(), "42".to_string()),
                ("org/repo".to_string(), "123".to_string())
            ]
        );
    }

    #[test]
    fn test_adf_flattening() {
        let doc = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [
                {
                    "type": "paragraph",
                    "content": [
                        { "type": "text", "text": "Hello " },
                        { "type": "mention", "attrs": { "text": "@cgegout" } },
                        { "type": "text", "text": "! Look at " },
                        { "type": "inlineCard", "attrs": { "url": "https://bugs.launchpad.net/bugs/12345" } }
                    ]
                },
                {
                    "type": "codeBlock",
                    "content": [
                        { "type": "text", "text": "let x = 42;" }
                    ]
                }
            ]
        });

        let flat = adf_text(&doc);
        assert!(flat.contains("Hello @cgegout! Look at https://bugs.launchpad.net/bugs/12345"));
        assert!(flat.contains("let x = 42;"));
    }
}
