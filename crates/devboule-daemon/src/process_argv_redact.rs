//! Argv redaction for the process index: what a tool result may show of
//! a process's command line, and what is masked or capped before it can.

/// An argv list is evidence, not a transcript: cap it before it reaches a
/// tool result.
const MAX_ARGS: usize = 32;
/// The per-argument cap, in characters; longer values are truncated with a
/// marker rather than dropped.
const MAX_ARG_CHARS: usize = 512;
/// Flag words, stored without dashes and matched case-insensitively against
/// whatever dash prefix the argument carries (`--token`, `-token`, `--TOKEN`).
const SECRET_FLAGS: &[&str] = &[
    "token",
    "password",
    "passwd",
    "secret",
    "api-key",
    "apikey",
    "auth-token",
    "access-key",
];
/// Name words that mark `KEY=value` as a credential: the name is split on
/// `-` and `_`, so `API_KEY`, `AWS_SECRET_ACCESS_KEY` and a bare `password`
/// match while `keyboard` and `MONKEY` do not.
const SECRET_NAME_WORDS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "auth",
];

/// Mask secret-looking argv: the value after a secret flag (any dash count,
/// any case), the value half of a credential-shaped `name=value`, credentials
/// embedded in a `scheme://user:pass@` URL, and any bare token-shaped value.
/// Everything is capped; nothing secret is ever stored or echoed. The
/// heuristics are deliberately one-sided: a false mask costs a bit of
/// evidence, a missed one costs a credential.
pub(crate) fn redact_argv(argv: &[String]) -> Vec<String> {
    let mut redacted: Vec<String> = Vec::new();
    let mut mask_next = false;
    for arg in argv.iter().take(MAX_ARGS) {
        if mask_next {
            redacted.push("[redacted]".to_string());
            mask_next = false;
            continue;
        }
        let arg = mask_url_credentials(arg);
        let name = arg.split('=').next().unwrap_or(&arg).to_string();
        let secret_flag = name.starts_with('-')
            && SECRET_FLAGS
                .iter()
                .any(|&flag| name.trim_start_matches('-').eq_ignore_ascii_case(flag));
        if secret_flag && arg.contains('=') {
            redacted.push(format!("{name}=[redacted]"));
            continue;
        }
        if secret_flag {
            mask_next = true;
            redacted.push(arg);
            continue;
        }
        if arg.contains('=') && is_secret_name(&name) {
            redacted.push(format!("{name}=[redacted]"));
            continue;
        }
        if is_token_shaped(&arg) {
            redacted.push("[redacted]".to_string());
            continue;
        }
        redacted.push(capped(&arg, MAX_ARG_CHARS));
    }
    redacted
}

/// `name=value` is a credential when the name splits into secret words —
/// one-sided on purpose (`file-key` masks, `keyboard` does not).
fn is_secret_name(name: &str) -> bool {
    name.split(&['-', '_'][..]).any(|word| {
        SECRET_NAME_WORDS
            .iter()
            .any(|&secret| word.eq_ignore_ascii_case(secret))
    })
}

/// `scheme://user:pass@host/...` keeps its scheme and host, loses its
/// userinfo: the part between `://` and the first `@` of the authority
/// becomes one marker.
fn mask_url_credentials(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else {
        return value.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority = &value[authority_start..];
    let authority_len = authority.find('/').unwrap_or(authority.len());
    let authority = &authority[..authority_len];
    let Some(colon) = authority.find(':') else {
        return value.to_string();
    };
    let Some(at) = authority[colon + 1..].find('@') else {
        return value.to_string();
    };
    let at = colon + 1 + at;
    format!(
        "{}[redacted]@{}",
        &value[..authority_start],
        &value[authority_start + at + 1..]
    )
}

/// A token-shaped bare value: long, high-charset, not a path — the kind of
/// argument that sits beside a CLI with no flag to key off. One-sided: it may
/// mask a random-looking id, and it never lets a credential through.
fn is_token_shaped(value: &str) -> bool {
    value.len() >= 24
        && !value.starts_with('/')
        && !value.starts_with('.')
        && value
            .chars()
            .all(|cell| cell.is_ascii_alphanumeric() || "+/=_-".contains(cell))
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
    fn argv_masks_case_variant_and_single_dash_flags() {
        let redacted = redact_argv(&[
            "--PASSWORD=hunter2".to_string(),
            "-token".to_string(),
            "abc".to_string(),
            "--Api-Key=xyz".to_string(),
            "--url=https://example.test/ok".to_string(),
        ]);
        assert_eq!(
            redacted[0], "--PASSWORD=[redacted]",
            "the flag's case never matters"
        );
        assert_eq!(redacted[1], "-token", "a single dash is still the flag");
        assert_eq!(redacted[2], "[redacted]", "and it masks its next value");
        assert_eq!(redacted[3], "--Api-Key=[redacted]", "mixed case inline");
        assert_eq!(
            redacted[4], "--url=https://example.test/ok",
            "a non-secret flag stays"
        );
    }

    #[test]
    fn argv_masks_credential_shaped_names_and_embedded_urls() {
        let redacted = redact_argv(&[
            "API_KEY=abc123".to_string(),
            "AWS_SECRET_ACCESS_KEY=AKIAwhatever".to_string(),
            "KEY_NAME=prod".to_string(),
            "--db=postgres://user:hunter2@db.example/x".to_string(),
            "keyboard=left".to_string(),
            "MONKEY=banana".to_string(),
            "abcdefghijklmnopqrstuvwx".to_string(),
        ]);
        assert_eq!(redacted[0], "API_KEY=[redacted]", "env-style name");
        assert_eq!(
            redacted[1], "AWS_SECRET_ACCESS_KEY=[redacted]",
            "a name of secret words"
        );
        assert_eq!(redacted[2], "KEY_NAME=[redacted]", "the KEY_NAME spelling");
        assert_eq!(
            redacted[3], "--db=postgres://[redacted]@db.example/x",
            "userinfo in a URL"
        );
        assert_eq!(
            redacted[4], "keyboard=left",
            "one-sided: not every -key word"
        );
        assert_eq!(redacted[5], "MONKEY=banana", "not a key at all");
        assert_eq!(
            redacted[6], "[redacted]",
            "an all-letter token of length is still a token"
        );
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
