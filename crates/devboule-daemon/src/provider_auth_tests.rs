use super::*;

#[test]
fn provider_status_commands_are_fixed_by_our_table() {
    assert_eq!(
        status_args("claude"),
        Some(vec!["auth".to_string(), "status".to_string()])
    );
    assert_eq!(
        status_args("codex"),
        Some(vec!["login".to_string(), "status".to_string()])
    );
    assert_eq!(
        status_args("pi"),
        Some(vec!["auth".to_string(), "check".to_string()])
    );
    for provider in ["grok", "qwen", "gemini"] {
        assert_eq!(status_args(provider), None);
    }
}

#[test]
fn pi_uses_only_a_configured_default_provider_and_disables_refresh() {
    assert_eq!(pi_status_args(None, Some("model-x")), None);
    assert_eq!(pi_status_args(Some(" "), Some("model-x")), None);
    assert_eq!(
        pi_status_args(Some("openai"), Some("gpt-x")),
        Some(
            vec![
                "auth",
                "check",
                "--provider",
                "openai",
                "--model",
                "gpt-x",
                "--no-refresh",
                "--json"
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        )
    );
    assert_eq!(
        pi_status_args(Some("openai"), None),
        Some(
            vec![
                "auth",
                "check",
                "--provider",
                "openai",
                "--no-refresh",
                "--json"
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        )
    );
}

#[test]
fn pi_status_parser_uses_only_the_status_field() {
    let output = br#"{"status":"ready","email":"secret@example.test","token":"secret"}"#;
    let result = classify_result("pi", Some(0), output);
    assert_eq!(result.0, "logged_in");
    assert!(!format!("{result:?}").contains("secret"));
}

#[test]
fn claude_and_codex_use_the_documented_success_exit() {
    assert_eq!(classify_result("claude", Some(0), b"").0, "logged_in");
    assert_eq!(classify_result("claude", Some(1), b"").0, "logged_out");
    assert_eq!(classify_result("claude", Some(2), b"").0, "unknown");
    assert_eq!(classify_result("codex", Some(0), b"").0, "logged_in");
    assert_eq!(classify_result("codex", Some(1), b"").0, "logged_out");
    assert_eq!(classify_result("codex", None, b"").0, "unknown");
}

#[test]
fn timeout_and_spawn_failure_are_unknown_without_echoing_cli_text() {
    for outcome in [ProbeFailure::TimedOut, ProbeFailure::SpawnFailed] {
        let result = classify_failure(outcome);
        assert_eq!(result.0, "unknown");
        assert!(!format!("{result:?}").contains("secret@example.test"));
    }
}

#[test]
fn disabled_provider_skips_the_probe_and_each_enabled_request_reruns_it() {
    let mut probes = 0;
    assert_eq!(
        check_if_enabled(false, || {
            probes += 1;
            Some(())
        }),
        None
    );
    assert_eq!(probes, 0);
    for _ in 0..2 {
        assert_eq!(
            check_if_enabled(true, || {
                probes += 1;
                Some(probes)
            }),
            Some(probes)
        );
    }
    assert_eq!(probes, 2);
}
