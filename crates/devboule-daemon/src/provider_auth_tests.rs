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
        assert_eq!(classify_failure(outcome).0, "unknown");
    }
}

#[test]
fn run_check_redacts_stdout_and_bounds_captured_output() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        let agent = fake_agent(vec![
            "/C".into(),
            "for /L %i in (1,1,10000) do @echo private-token".into(),
        ]);
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result.0, "logged_in");
        let captured = last_captured_output().expect("the reader captured the output");
        assert!(
            String::from_utf8_lossy(&captured).contains("private-token"),
            "the fixture really printed the token into the pipe"
        );
        assert_eq!(
            captured.len(),
            MAX_STATUS_OUTPUT,
            "the real reader keeps the first 64 KiB"
        );
    }
}

#[test]
fn run_check_timeout_returns_static_unknown_reason() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        // cmd.exe starts far faster than PowerShell, so the child is
        // running well inside the five-second budget — no cold-start race.
        // ping then holds the tree as a real grandchild of the direct
        // child, which is the shape the job has to reap.
        let executable = std::env::var_os("COMSPEC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
        let agent = fake_agent_at(executable, vec!["/C".into(), "ping -n 31 127.0.0.1".into()]);
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result, ("unknown", "The provider status check timed out."));
        let child_pid = last_child_pid().expect("the runner recorded the child PID");
        assert!(
            !tasklist_has_pid(child_pid),
            "the killed child {child_pid} is gone"
        );
        assert!(
            !tasklist_has_image("ping.exe"),
            "the grandchild died with the job, not just the direct child"
        );
    }
}

#[test]
fn run_check_reaps_the_whole_tree_after_a_successful_child_exits() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        // cmd.exe exits at once; `start /b` leaves ping holding the pipe as
        // a real grandchild. The post-exit job terminate is what reaps it
        // — `Child::kill` alone would not.
        let executable = std::env::var_os("COMSPEC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
        let agent = fake_agent_at(
            executable,
            vec!["/C".into(), "start /b ping -n 31 127.0.0.1 & exit".into()],
        );
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result.0, "logged_in");
        assert!(
            !tasklist_has_image("ping.exe"),
            "the grandchild holding the pipe died with the job"
        );
    }
}

#[test]
fn run_check_reports_truncation_instead_of_a_bad_cli_result() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        // A pi build that logs more than the capture cap before its status
        // field is truncated by the bound; blaming the CLI for an
        // unrecognized result would be a false reason.
        let mut agent = fake_agent(vec![
            "/C".into(),
            "for /L %i in (1,1,3000) do @echo padding-line-0123456789012345678901234".into(),
        ]);
        agent.id = "pi".to_string();
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(
            result,
            (
                "unknown",
                "The provider status output was too large to read."
            )
        );
    }
}

#[test]
fn run_check_classifies_a_pi_status_document_through_the_real_runner() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        // cmd's echo backslash-escapes quotes, so the fixture is a file the
        // child types verbatim. Unquoted: a quoted path does not survive
        // the spawn quoting, and the temp dir has no spaces to require it.
        let directory = crate::test_dirs::test_temp_dir("devboule-auth-pi-json");
        let status_file = directory.join("status.json");
        std::fs::write(
            &status_file,
            r#"{"status":"ready","email":"secret@example.test"}"#,
        )
        .expect("write the status fixture");
        let path = status_file.display().to_string();
        assert!(
            !path.contains(' '),
            "the fixture path must not contain spaces"
        );
        let mut agent = fake_agent(vec!["/C".into(), format!("type {path}")]);
        agent.id = "pi".to_string();
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result.0, "logged_in");
        let captured = last_captured_output().expect("the reader captured the output");
        assert!(
            String::from_utf8_lossy(&captured).contains("\"status\":\"ready\""),
            "the child typed the status document into the pipe"
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// Whether tasklist still reports a process with exactly this PID: the PID
/// column of a data row, never a substring of the whole listing.
#[cfg(windows)]
fn tasklist_has_pid(pid: u32) -> bool {
    let listing = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .expect("tasklist is available on Windows");
    let stdout = String::from_utf8_lossy(&listing.stdout).into_owned();
    tasklist_data_rows(&stdout).into_iter().any(|row| {
        row.get(1)
            .is_some_and(|column| column.parse::<u32>() == Ok(pid))
    })
}

/// Whether tasklist still reports any process with this image name.
#[cfg(windows)]
fn tasklist_has_image(image: &str) -> bool {
    let listing = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {image}")])
        .output()
        .expect("tasklist is available on Windows");
    let stdout = String::from_utf8_lossy(&listing.stdout).into_owned();
    tasklist_data_rows(&stdout).into_iter().any(|row| {
        row.first()
            .is_some_and(|name| name.eq_ignore_ascii_case(image))
    })
}

/// The data rows of a tasklist listing: the header and the separator line
/// carry no PID column to misparse.
#[cfg(windows)]
fn tasklist_data_rows(listing: &str) -> Vec<Vec<&str>> {
    listing
        .lines()
        .filter(|line| {
            !line.starts_with("Image Name") && !line.starts_with("===") && !line.trim().is_empty()
        })
        .map(|line| line.split_whitespace().collect())
        .collect()
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
