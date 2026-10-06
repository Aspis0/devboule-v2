//! Argv and executable redaction against the credential shapes real command
//! lines carry: flags, `name=value`, headers, URLs and user:password pairs.

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

/// The curl shapes: a separate `-H` argument holding a whole header, a
/// header glued to its flag, and a header split at the colon.
#[test]
fn argv_masks_credential_headers_whatever_their_shape() {
    let redacted = redact_argv(&[
        "curl".to_string(),
        "-H".to_string(),
        "Authorization: Bearer short".to_string(),
        "--header=authorization: Basic dTpw".to_string(),
        "Cookie: sid=abc".to_string(),
        "X-Api-Key: k".to_string(),
        "Proxy-Authorization:".to_string(),
        "Basic dTpwYXNz".to_string(),
        "Bearer abc".to_string(),
        "Accept: application/json".to_string(),
        "https://example.test/path".to_string(),
    ]);
    assert_eq!(redacted[1], "-H", "the flag is not secret");
    assert_eq!(redacted[2], "Authorization: [redacted]");
    assert_eq!(redacted[3], "--header=authorization: [redacted]");
    assert_eq!(redacted[4], "Cookie: [redacted]");
    assert_eq!(redacted[5], "X-Api-Key: [redacted]");
    assert_eq!(redacted[6], "Proxy-Authorization: [redacted]");
    assert_eq!(redacted[7], "[redacted]", "the value split from its header");
    assert_eq!(redacted[8], "[redacted]", "a bare Bearer credential");
    assert_eq!(
        redacted[9], "Accept: application/json",
        "a harmless header stays"
    );
    assert_eq!(
        redacted[10], "https://example.test/path",
        "a URL is not a header"
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

fn redacted(args: &[&str]) -> Vec<String> {
    redact_argv(&args.iter().map(ToString::to_string).collect::<Vec<_>>())
}

/// curl's `user:password` in every spelling, and the flags that look like it
/// but carry no credential.
#[test]
fn argv_masks_user_password_pairs_after_user_flags() {
    let out = redacted(&[
        "curl",
        "-u",
        "alice:hunter2",
        "--user",
        "bob:pw",
        "--user=carol:pw",
        "-udave:pw",
        "--proxy-user",
        "erin:pw",
        "-U",
        "frank:pw",
    ]);
    assert_eq!(out[1], "-u", "the flag stays");
    assert_eq!(out[2], "[redacted]");
    assert_eq!(out[4], "[redacted]");
    assert_eq!(out[5], "--user=[redacted]");
    assert_eq!(out[6], "-u[redacted]");
    assert_eq!(out[8], "[redacted]");
    assert_eq!(out[10], "[redacted]");

    let plain = redacted(&[
        "python",
        "-u",
        "server.py",
        "docker",
        "-u",
        "1000",
        "--user=bob",
    ]);
    assert_eq!(
        plain,
        [
            "python",
            "-u",
            "server.py",
            "docker",
            "-u",
            "1000",
            "--user=bob"
        ],
        "no colon, no pair: `-u` is also an ordinary flag"
    );
}

/// Password flags in the spellings other tools use.
#[test]
fn argv_masks_password_flag_forms() {
    let out = redacted(&[
        "mysql",
        "-p",
        "hunter2",
        "-phunter2",
        "--password",
        "x1",
        "--pass=x2",
        "--pwd",
        "x3",
        "--client-secret=x4",
    ]);
    assert_eq!(out[1], "-p");
    assert_eq!(out[2], "[redacted]");
    assert_eq!(out[3], "-p[redacted]", "mysql's glued form");
    assert_eq!(out[5], "[redacted]");
    assert_eq!(out[6], "--pass=[redacted]");
    assert_eq!(out[8], "[redacted]");
    assert_eq!(out[9], "--client-secret=[redacted]");
}

/// Userinfo in any argument, however it is embedded.
#[test]
fn argv_masks_url_userinfo_wherever_it_appears() {
    let out = redacted(&[
        "https://user:pass@example.test/x",
        "https://ghp_tokenonly@example.test/repo",
        "--urls=a://u:p@one.test/x,b://v:q@two.test",
        "sh -c 'curl https://u:p@h.test/x && echo ok'",
        "--url=https://example.test/a@b",
        "git@example.test:repo.git",
    ]);
    assert_eq!(out[0], "https://[redacted]@example.test/x");
    assert_eq!(out[1], "https://[redacted]@example.test/repo");
    assert_eq!(
        out[2],
        "--urls=a://[redacted]@one.test/x,b://[redacted]@two.test"
    );
    assert_eq!(
        out[3],
        "sh -c 'curl https://[redacted]@h.test/x && echo ok'"
    );
    assert_eq!(
        out[4], "--url=https://example.test/a@b",
        "an @ in the path is not userinfo"
    );
    assert_eq!(
        out[5], "git@example.test:repo.git",
        "scp-style is not a URL"
    );
}

/// `env`-prefixed assignments: the name carries the secret word, glued or
/// split, in any case.
#[test]
fn argv_masks_env_style_credential_assignments() {
    let out = redacted(&[
        "env",
        "PGPASSWORD=a",
        "GITHUB_TOKEN=b",
        "APP_SECRET=c",
        "MYSQL_PWD=d",
        "STRIPE_API_KEY=e",
        "apiKey=f",
        "DATABASE_URL=postgres://u:p@db.test/x",
        "MONKEY=banana",
        "keyboard=left",
        "PATH=/usr/bin",
    ]);
    for (index, name) in [
        (1, "PGPASSWORD"),
        (2, "GITHUB_TOKEN"),
        (3, "APP_SECRET"),
        (4, "MYSQL_PWD"),
        (5, "STRIPE_API_KEY"),
        (6, "apiKey"),
    ] {
        assert_eq!(out[index], format!("{name}=[redacted]"));
    }
    assert_eq!(out[7], "DATABASE_URL=postgres://[redacted]@db.test/x");
    assert_eq!(out[8], "MONKEY=banana");
    assert_eq!(out[9], "keyboard=left");
    assert_eq!(out[10], "PATH=/usr/bin");
}

/// The executable string a process chose for itself is redacted like an
/// argument; an honest path is untouched.
#[test]
fn an_executable_spelled_as_a_credential_is_masked() {
    assert_eq!(redact_exe("API_KEY=secret"), "API_KEY=[redacted]");
    assert_eq!(
        redact_exe("https://token@example.test/x"),
        "https://[redacted]@example.test/x"
    );
    assert_eq!(redact_exe("0123456789abcdef01234567"), "[redacted]");
    assert_eq!(redact_exe("/usr/local/bin/node"), "/usr/local/bin/node");
    assert_eq!(
        redact_exe(r"C:\Windows\System32\ping.exe"),
        r"C:\Windows\System32\ping.exe"
    );
}
