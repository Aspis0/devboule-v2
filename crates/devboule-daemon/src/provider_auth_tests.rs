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
    assert_eq!(classify_result("codex", Some(1), b"").0, "unknown");
    assert_eq!(classify_result("codex", Some(2), b"").0, "unknown");
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
fn run_check_redacts_stdout_and_bounds_captured_output() {
    let mut output = Vec::new();
    append_bounded_output(&mut output, &vec![b'x'; MAX_STATUS_OUTPUT + 100]);
    assert_eq!(output.len(), MAX_STATUS_OUTPUT);

    #[cfg(windows)]
    {
        let agent = fake_agent(vec![
            "/C".into(),
            "for /L %i in (1,1,10000) do @echo private-token".into(),
        ]);
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result.0, "logged_in");
        assert!(!format!("{result:?}").contains("private-token"));
    }
}

#[test]
fn run_check_timeout_returns_static_unknown_reason() {
    #[cfg(windows)]
    {
        let directory = crate::test_dirs::test_temp_dir("devboule-auth-child");
        let marker = directory.join("child-pid.txt");
        let script = format!(
            "$PID | Set-Content -LiteralPath '{}'; Start-Sleep -Seconds 30",
            marker.display()
        );
        let executable = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("C:\\Windows"))
            .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let agent = fake_agent_at(
            executable,
            vec!["-NoProfile".into(), "-Command".into(), script],
        );
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result, ("unknown", "The provider status check timed out."));
        let child_pid = std::fs::read_to_string(&marker).expect("fake child wrote its PID");
        let listing = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", child_pid.trim())])
            .output()
            .expect("tasklist is available on Windows");
        assert!(!String::from_utf8_lossy(&listing.stdout).contains(child_pid.trim()));
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(windows)]
fn fake_agent(prefix_args: Vec<String>) -> InstalledAgent {
    fake_agent_at(
        std::env::var_os("COMSPEC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe")),
        prefix_args,
    )
}

#[cfg(windows)]
fn fake_agent_at(executable: std::path::PathBuf, prefix_args: Vec<String>) -> InstalledAgent {
    InstalledAgent {
        id: "codex".into(),
        aliases: &[],
        installed: true,
        executable,
        prefix_args,
        acp_command: None,
        stream_json_command: None,
        rpc_command: None,
        app_server_command: None,
        authentication: crate::provider_catalog::AuthenticationStatus::Unknown,
        origin: ProviderOrigin::UserBinary,
        launch_args: None,
        pickable: None,
        installed_version: None,
        latest_version: None,
        install_channel: crate::provider_catalog::InstallChannel::Native,
        npm_package: None,
        tools: Vec::new(),
        spawn_path_env: None,
        launch_directory: None,
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
