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
fn only_the_pi_classifier_reads_the_captured_stdout() {
    // One predicate for "this classification reads stdout": the drain
    // wait and the capture seam both branch on it, so a new stdout-parsing
    // provider cannot silently get an empty buffer.
    assert!(reads_stdout("pi"));
    assert!(!reads_stdout("claude"));
    assert!(!reads_stdout("codex"));
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
        // The kill is checked against the PIDs the job actually held, never
        // a machine-wide image name a co-tenant process could fail.
        let job_pids = last_job_pids();
        assert!(
            !job_pids.is_empty(),
            "the runner recorded the job's member PIDs"
        );
        for pid in job_pids {
            assert!(!tasklist_has_pid(pid), "the killed process {pid} is gone");
        }
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
        let job_pids = last_job_pids();
        assert!(
            !job_pids.is_empty(),
            "the runner recorded the job's member PIDs"
        );
        for pid in job_pids {
            assert!(
                !tasklist_has_pid(pid),
                "the grandchild {pid} died with the job"
            );
        }
    }
}

#[test]
fn run_check_does_not_wait_for_the_pipe_when_the_exit_code_decides() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        // cmd.exe exits 0 at once; ping keeps the inherited pipe open as a
        // grandchild. claude and codex classify by exit code alone, so a
        // descendant holding the pipe must not turn a successful exit into
        // a false timeout.
        let executable = std::env::var_os("COMSPEC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
        let agent = fake_agent_at(
            executable,
            vec!["/C".into(), "start /b ping -n 31 127.0.0.1 & exit".into()],
        );
        let result = run_check_with_timeout(&agent, &[], Duration::from_secs(5));
        assert_eq!(result, ("logged_in", "CLI confirmed an active login."));
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
        // The bound itself, on the buffer the real reader captured.
        let captured = last_captured_output().expect("the reader captured the output");
        assert_eq!(
            captured.len(),
            MAX_STATUS_OUTPUT,
            "the real reader keeps the first 64 KiB"
        );
    }
}

#[test]
fn run_check_classifies_a_pi_status_document_through_the_real_runner() {
    let _lock = runner_test_lock();
    #[cfg(windows)]
    {
        // cmd's echo backslash-escapes quotes, so the fixture is a file the
        // child types verbatim. `cd /d` takes the rest of the line as the
        // path, so a temp dir with spaces in it works; a quoted path does
        // not survive the spawn quoting.
        let directory = crate::test_dirs::test_temp_dir("devboule-auth-pi-json");
        assert_cmd_safe_path(&directory);
        let status_file = directory.join("status.json");
        std::fs::write(
            &status_file,
            r#"{"status":"ready","email":"secret@example.test"}"#,
        )
        .expect("write the status fixture");
        let mut agent = fake_agent(vec![
            "/C".into(),
            format!("cd /d {} & type status.json", directory.display()),
        ]);
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

/// The fixture's `cd /d <path>` line breaks on cmd metacharacters, so a
/// hostile %TEMP% must fail loudly, not as an opaque assertion later. Spaces
/// are fine — `cd /d` takes the rest of the line as the path.
#[cfg(windows)]
fn assert_cmd_safe_path(path: &std::path::Path) {
    let text = path.display().to_string();
    if text.chars().any(|c| "&|^<>()%\"".contains(c)) {
        panic!(
            "the fixture path '{text}' contains a cmd metacharacter; set TEMP to a cmd-safe directory"
        );
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
