//! Secret-token redaction applied at the retrieval serialization boundary.
//!
//! Moved out of the deleted `answer` module: query still redacts chunk text
//! before it leaves the engine (file/line citations must not leak secrets).

use std::sync::OnceLock;

use regex::Regex;

const SECRET_REDACTION: &str = "[redacted-secret]";

/// The marker, plus one newline for every newline the redacted span contained.
///
/// Several of the patterns below can match across a line break: `\s` and the
/// negated classes both accept `\n`, so `token:` with its value on the next
/// line, or a quoted string spanning two lines, is one match containing a
/// newline. Replacing that with a marker that has none would **delete a line**,
/// and every line after it in the chunk would shift up by one against the line
/// numbers recorded at index time. The chunk text and its citation would then
/// disagree, silently and only on files that contain something secret-looking.
///
/// So the marker carries the newlines back. What was on those lines is gone —
/// that is the point — but the count is preserved, which is what the line
/// numbers depend on.
fn redaction_for(matched: &str) -> String {
    let newlines = matched.bytes().filter(|byte| *byte == b'\n').count();
    let mut replacement = String::with_capacity(SECRET_REDACTION.len() + newlines);
    replacement.push_str(SECRET_REDACTION);
    replacement.extend(std::iter::repeat_n('\n', newlines));
    replacement
}

/// Redact secret-looking tokens in chunk text.
///
/// Port of `answerer.py::redact_secret_tokens` — every regex byte-exact. The
/// replacement is not byte-exact with Python: it preserves the line count, for
/// the reason given on [`redaction_for`].
pub fn redact_secret_tokens(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let mut redacted = text.to_string();

    for pattern in secret_patterns() {
        redacted = pattern
            .replace_all(&redacted, |captures: &regex::Captures| {
                redaction_for(&captures[0])
            })
            .to_string();
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
        let replacement = redaction_for(&redacted[start..end]);
        redacted.replace_range(start..end, &replacement);
    }

    let re = secret_hex_re();
    let mut hex_positions: Vec<(usize, usize)> = re
        .find_iter(&redacted)
        .map(|m| (m.start(), m.end()))
        .collect();
    hex_positions.reverse();
    for (start, end) in hex_positions {
        let replacement = redaction_for(&redacted[start..end]);
        redacted.replace_range(start..end, &replacement);
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

    /// Line numbers travel separately from the text they describe: the index
    /// records them before redaction, the citation uses them after. If a
    /// redaction can swallow a newline, every line below it in the chunk points
    /// one line too high, on exactly the files nobody wants to misread.
    #[test]
    fn redaction_never_changes_the_number_of_lines() {
        let cases = [
            "let header = format!(\n    \"Bearer\n    abcdefghijklmnopqrstuvwx\"\n);\n",
            "const c = {\n  token: 'abcd\nefgh',\n};\n",
            "deploy:\n  api_key:\n    ${{ secrets.DEPLOY_TOKEN }}\n  run: build\n",
            "password\n= hunter2000\nnext_line()\n",
            "let blob = \"QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbg==\";\nafter();\n",
            "hash = 0123456789abcdef0123456789abcdef01234567\nafter();\n",
            "no secrets here\njust two lines\n",
            "",
        ];
        for text in cases {
            let out = redact_secret_tokens(text);
            assert_eq!(
                out.lines().count(),
                text.lines().count(),
                "line count changed for {text:?} -> {out:?}"
            );
            assert_eq!(
                out.bytes().filter(|b| *b == b'\n').count(),
                text.bytes().filter(|b| *b == b'\n').count(),
                "newline count changed for {text:?} -> {out:?}"
            );
        }
    }

    #[test]
    fn a_secret_spanning_lines_is_still_redacted() {
        let text = "const c = {\n  token: 'abcd\nefgh',\n};\n";
        let out = redact_secret_tokens(text);
        assert!(!out.contains("abcd"), "multi-line secret survived: {out}");
        assert!(out.contains(SECRET_REDACTION));
    }
}
