//! Argv redaction for the process index: what a tool result may show of
//! a process's command line, and what is masked or capped before it can.

/// An argv list is evidence, not a transcript: cap it before it reaches a
/// tool result.
const MAX_ARGS: usize = 32;
/// The per-argument cap, in characters; longer values are truncated with a
/// marker rather than dropped.
const MAX_ARG_CHARS: usize = 512;
/// Flags whose next value (or whose inline `=` value) is a credential by
/// name — masked regardless of what it looks like.
const SECRET_FLAGS: &[&str] = &[
    "--token",
    "--password",
    "--passwd",
    "--secret",
    "--api-key",
    "--apikey",
    "--auth-token",
    "--access-key",
];

/// Mask secret-looking argv: the value after a secret flag, the value half
/// of a secret-looking `name=value`, and any bare token-shaped value — long,
/// high-charset and not a path. Everything is capped; nothing secret is ever
/// stored or echoed.
pub(crate) fn redact_argv(argv: &[String]) -> Vec<String> {
    let mut redacted: Vec<String> = Vec::new();
    let mut mask_next = false;
    for arg in argv.iter().take(MAX_ARGS) {
        if mask_next {
            redacted.push("[redacted]".to_string());
            mask_next = false;
            continue;
        }
        let name = arg.split('=').next().unwrap_or(arg);
        let secret_flag = SECRET_FLAGS
            .iter()
            .any(|flag| *flag == name || *flag == arg);
        if secret_flag && arg.contains('=') {
            redacted.push(format!("{name}=[redacted]"));
            continue;
        }
        if secret_flag {
            mask_next = true;
            redacted.push(arg.clone());
            continue;
        }
        if let Some((key, _value)) = arg.split_once('=') {
            if is_secret_name(key) {
                redacted.push(format!("{key}=[redacted]"));
                continue;
            }
            redacted.push(capped(arg, MAX_ARG_CHARS));
            continue;
        }
        if is_token_shaped(arg) {
            redacted.push("[redacted]".to_string());
            continue;
        }
        redacted.push(capped(arg, MAX_ARG_CHARS));
    }
    redacted
}

fn is_secret_name(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SECRET_FLAGS
        .iter()
        .any(|flag| flag.trim_start_matches('-').eq_ignore_ascii_case(&lowered))
}

/// A token-shaped bare value: long, high-charset, not a path — the kind of
/// argument that sits beside a CLI with no flag to key off. The heuristic is
/// deliberately one-sided: it may mask a random-looking id, and it never
/// lets a credential through.
fn is_token_shaped(value: &str) -> bool {
    if value.len() < 24 || value.starts_with('/') || value.starts_with('.') || value.contains(':') {
        return false;
    }
    value
        .chars()
        .all(|cell| cell.is_ascii_alphanumeric() || "+/=_-".contains(cell))
        && value.chars().any(|cell| cell.is_ascii_digit())
        && value.chars().any(|cell| cell.is_ascii_alphabetic())
}

fn capped(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_masks_secret_flags_and_token_shaped_values() {
        let redacted = redact_argv(&[
            "/usr/local/bin/tool".to_string(),
            "--token".to_string(),
            "s3cr3t-value-here".to_string(),
            "--password=hunter2".to_string(),
            "--url=https://example.test/x".to_string(),
            "0123456789abcdef01234567".to_string(),
            "/var/run/devboule.sock".to_string(),
            "session-abc123".to_string(),
        ]);
        assert_eq!(redacted[1], "--token", "the flag itself is not secret");
        assert_eq!(redacted[2], "[redacted]", "the value after a secret flag");
        assert_eq!(
            redacted[3], "--password=[redacted]",
            "an inline secret value"
        );
        assert_eq!(
            redacted[4], "--url=https://example.test/x",
            "a non-secret inline value stays"
        );
        assert_eq!(redacted[5], "[redacted]", "a bare token-shaped value");
        assert_eq!(
            redacted[6], "/var/run/devboule.sock",
            "a path is not a token"
        );
        assert_eq!(redacted[7], "session-abc123", "a short id is not a token");
    }

    #[test]
    fn argv_caps_count_and_length() {
        let mut argv = vec!["tool".to_string()];
        argv.push(format!("/opt/{}", "x".repeat(600)));
        for index in 0..40 {
            argv.push(format!("arg{index}"));
        }
        let redacted = redact_argv(&argv);
        assert_eq!(redacted.len(), MAX_ARGS, "the count is capped");
        assert!(
            redacted[1].chars().count() <= MAX_ARG_CHARS + 1,
            "a long value is truncated with a marker: {}",
            redacted[1].chars().count()
        );
        assert!(redacted[1].ends_with('…'), "truncation is visible");
        assert_eq!(redacted[2], "arg0", "the tail keeps the early arguments");
    }
}
