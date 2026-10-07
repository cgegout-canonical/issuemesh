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
}
