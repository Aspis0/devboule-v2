//! Secret-token redaction applied at the retrieval serialization boundary.
//!
//! Moved out of the deleted `answer` module: query still redacts chunk text
//! before it leaves the engine (file/line citations must not leak secrets).

use std::sync::OnceLock;

use regex::Regex;

const SECRET_REDACTION: &str = "[redacted-secret]";

/// Redact secret-looking tokens in chunk text.
///
/// Port of `answerer.py::redact_secret_tokens` — every regex byte-exact.
pub fn redact_secret_tokens(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let mut redacted = text.to_string();

    for pattern in secret_patterns() {
        redacted = pattern.replace_all(&redacted, SECRET_REDACTION).to_string();
    }

    // High-entropy base64 runs (40+ chars) with mixed character classes.
    let re = secret_high_entropy_re();
    let mut entropy_positions: Vec<(usize, usize)> = re
        .find_iter(&redacted)
        .filter_map(|m| {
            let token = m.as_str();
            let has_lower = token.chars().any(|c| c.is_lowercase());
            let has_upper = token.chars().any(|c| c.is_uppercase());
            let has_digit = token.chars().any(|c| c.is_ascii_digit());
            let classes = has_lower as i32 + has_upper as i32 + has_digit as i32;
            if classes >= 2 {
                Some((m.start(), m.end()))
            } else {
                None
            }
        })
        .collect();
    entropy_positions.reverse();
    for (start, end) in entropy_positions {
        redacted.replace_range(start..end, SECRET_REDACTION);
    }

    let re = secret_hex_re();
    let mut hex_positions: Vec<(usize, usize)> = re
        .find_iter(&redacted)
        .map(|m| (m.start(), m.end()))
        .collect();
    hex_positions.reverse();
    for (start, end) in hex_positions {
        redacted.replace_range(start..end, SECRET_REDACTION);
    }

    redacted
}

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"\bgh[opusr]_[A-Za-z0-9]{20,}\b").unwrap(),
            Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").unwrap(),
            Regex::new(r"\bSCW[A-Za-z0-9]{12,}\b").unwrap(),
            Regex::new(r"\bAKIA[0-9A-Z]{12,}\b").unwrap(),
            Regex::new(r"\bxox[baprs]-[A-Za-z0-9\-]{10,}\b").unwrap(),
            Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}").unwrap(),
            Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b").unwrap(),
            Regex::new(r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|access[_-]?key)\b\s*[:=]\s*['"]?[A-Za-z0-9/_+\-]{8,}['"]?"#).unwrap(),
            Regex::new(r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|access[_-]?key)\b\s*[:=]\s*(?:['"][^'"]{4,}['"]|\S{4,})"#).unwrap(),
        ]
    })
}

fn secret_high_entropy_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[A-Za-z0-9+/]{40,}={0,2}\b").unwrap())
}

fn secret_hex_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{40,}\b").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_short_password_assignment() {
        let text = r#"password = "hunter2""#;
        let out = redact_secret_tokens(text);
        assert!(!out.contains("hunter2"), "short password survived: {out}");
        assert!(out.contains(SECRET_REDACTION));
    }

    #[test]
    fn redacts_assignment_any_charset() {
        let text = r#"api_key = "p@ss!word""#;
        let out = redact_secret_tokens(text);
        assert!(
            !out.contains("p@ss!word"),
            "special-char secret survived: {out}"
        );
        assert!(out.contains(SECRET_REDACTION));
    }
}
