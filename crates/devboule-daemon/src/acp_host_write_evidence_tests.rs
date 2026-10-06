//! What the write-evidence log accepts: the write the ACP host performed, and
//! nothing an agent said it was about to do.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::PermissionOutcome;

use super::super::permission_broker::PermissionBroker;
use super::super::SessionRuntime;
use crate::write_evidence::{log, repo_key, WriteEvidence, WriteLog, MAX_ROWS};

use super::AcpHost;

/// The window every assertion here reads over.
const LOOKBACK: Duration = Duration::from_secs(3600);

/// A file name per case: the cases below drive the process-wide log through
/// the host, so two of them must never ask about the same path.
fn named(case: &str, leaf: &str) -> String {
    format!("collision-evidence-{case}-{leaf}")
}

/// A workspace folder with a repository in it, which is what a session's cwd
/// is: the writer log is keyed by that repository, and a folder with no `.git`
/// above it drops every row.
fn workspace_checkout(label: &str) -> PathBuf {
    let folder = crate::test_dirs::test_temp_dir(&format!("devboule-acp-evidence-{label}"));
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&folder)
        .output()
        .expect("git could be spawned");
    assert!(
        output.status.success(),
        "git init in the test workspace: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    folder
}

struct Host {
    host: Arc<AcpHost>,
    broker: Arc<PermissionBroker>,
    _runtime: Arc<SessionRuntime>,
    cwd: PathBuf,
    runtime: PathBuf,
}

impl Host {
    fn new(label: &str) -> Self {
        let cwd = workspace_checkout(&format!("{label}-cwd"));
        let runtime =
            crate::test_dirs::test_temp_dir(&format!("devboule-acp-evidence-{label}-runtime"));
        let host = AcpHost::new(cwd.clone(), runtime.clone());
        host.set_session_id("evidence-session".to_string());
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let runtime_holder =
            SessionRuntime::for_acp("evidence-session".to_string(), None, Arc::clone(&broker));
        host.bind_permission_gate(&broker, &runtime_holder);
        Self {
            host,
            broker,
            _runtime: runtime_holder,
            cwd,
            runtime,
        }
    }

    /// Write one file through the host's own `fs/write_text_file`.
    fn write(&self, relative: &str, contents: &str) {
        self.host
            .write_text_file(serde_json::json!({
                "sessionId": "evidence-session",
                "path": self.cwd.join(relative),
                "content": contents,
            }))
            .expect("the host writes a file inside its own workspace");
    }

    /// Run a shell line in the workspace, once the permission card is
    /// answered, and wait until it has written what it was told to write.
    fn run_shell(&self, command: &str, marker: &str, label: &str) {
        #[cfg(windows)]
        let params = serde_json::json!({
            "sessionId": "evidence-session",
            "command": "cmd.exe",
            "args": ["/c", command],
            "cwd": self.cwd,
        });
        #[cfg(not(windows))]
        let params = serde_json::json!({
            "sessionId": "evidence-session",
            "command": "/bin/sh",
            "args": ["-c", command],
            "cwd": self.cwd,
        });
        let thread = std::thread::spawn({
            let host = Arc::clone(&self.host);
            move || host.create_terminal(params)
        });
        let card = {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(id) = self.broker.pending_ids().into_iter().next() {
                    break id;
                }
                assert!(
                    Instant::now() < deadline,
                    "the terminal {label} raised no permission card"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        self.broker
            .respond(&card, PermissionOutcome::AllowOnce)
            .expect("allow the terminal");
        thread
            .join()
            .expect("create thread")
            .unwrap_or_else(|error| panic!("{label} was refused: {error:?}"));
        let written = self.cwd.join(marker);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !written.exists() {
            assert!(
                Instant::now() < deadline,
                "{label} never wrote {marker}, so the case below would be vacuous"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.host.shutdown();
        let _ = std::fs::remove_dir_all(&self.cwd);
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

#[test]
fn shell_text_is_not_write_evidence() {
    let host = Host::new("shell");
    let written = named("shell", "host-write.rs");
    let from_shell = named("shell", "shell-write.rs");
    host.write(&written, "written by the host\n");
    #[cfg(windows)]
    host.run_shell(
        &format!(r"echo written by the shell> {from_shell}"),
        &from_shell,
        "cmd.exe /c echo",
    );
    #[cfg(not(windows))]
    host.run_shell(
        &format!("echo 'written by the shell' > {from_shell}"),
        &from_shell,
        "sh -c echo",
    );

    let repo = repo_key(&host.cwd).expect("the host's workspace is a repository");
    let from_host = log().writers_for(&repo, &written, LOOKBACK);
    assert_eq!(
        from_host.writers.len(),
        1,
        "the host's own write is evidence"
    );
    let row = &from_host.writers[0];
    assert_eq!(row.session_id, "evidence-session");
    assert_eq!(row.path, written);
    assert_eq!(row.evidence, WriteEvidence::AgentFileWrite);
    assert!(
        row.at_ms > 0,
        "a row without an instant cannot be read against a window"
    );

    assert!(
        log()
            .writers_for(&repo, &from_shell, LOOKBACK)
            .writers
            .is_empty(),
        "a command line is not a path the daemon wrote; the file above exists"
    );
}

/// The key is the repository root, not the session's working directory: a
/// session whose cwd is a subdirectory writes `<root>/sub/a.rs`, and that is
/// the row a caller asking about `sub/a.rs` must see — while `a.rs` names a
/// different file and must not match it.
#[test]
fn a_write_is_keyed_by_the_repository_root_not_the_session_cwd() {
    let host = Host::new("subdir");
    std::fs::create_dir_all(host.cwd.join("sub")).expect("subdirectory");
    let deep = named("subdir", "deep.rs");
    host.write(&format!("sub/{deep}"), "written below the cwd\n");

    let repo = repo_key(&host.cwd).expect("the host's workspace is a repository");
    let writers = log().writers_for(&repo, &format!("sub/{deep}"), LOOKBACK);
    assert_eq!(writers.writers.len(), 1, "{:?}", writers.writers);
    assert_eq!(
        writers.writers[0].path,
        format!("sub/{deep}"),
        "the row is keyed by the repository root, not by the cwd"
    );
    assert!(
        log().writers_for(&repo, &deep, LOOKBACK).writers.is_empty(),
        "the same leaf one directory up is a different file"
    );
}

#[test]
fn one_session_writing_twice_is_one_writer() {
    let host = Host::new("twice");
    let leaf = named("twice", "twice.rs");
    host.write(&leaf, "first\n");
    host.write(&leaf, "second\n");

    let repo = repo_key(&host.cwd).expect("the host's workspace is a repository");
    let writers = log().writers_for(&repo, &leaf, LOOKBACK);
    assert_eq!(
        writers.writers.len(),
        1,
        "a caller asks who is in the file, not how often they touched it"
    );
    assert!(
        log()
            .writers_for(&repo, &named("twice", "never-written.rs"), LOOKBACK)
            .writers
            .is_empty(),
        "a path nobody wrote has no writer"
    );
    // The folder's own spelling rule decides the match: NTFS and a default
    // APFS volume fold case, so a caller may name the file in either spelling,
    // and a case-sensitive filesystem must not match a name that is not this
    // file.
    #[cfg(any(windows, target_os = "macos"))]
    assert_eq!(
        log()
            .writers_for(&repo, &leaf.to_uppercase(), LOOKBACK)
            .writers
            .len(),
        1,
        "this filesystem compares paths without case"
    );
    #[cfg(not(any(windows, target_os = "macos")))]
    assert!(
        log()
            .writers_for(&repo, &leaf.to_uppercase(), LOOKBACK)
            .writers
            .is_empty(),
        "a case-sensitive filesystem does not match a different name"
    );
}

/// A full log drops its oldest rows, and an answer read afterwards says it may
/// be short rather than passing an absent writer off as "nobody wrote this".
/// The log is this case's own: filling the process-wide one would decide what
/// every other case in this binary reads.
#[test]
fn a_full_log_says_the_writer_list_may_be_short() {
    let root = workspace_checkout("evict-log");
    let log = WriteLog::default();
    let repo = repo_key(&root).expect("the workspace is a repository");
    let leaf = named("evict", "leaf.rs");
    std::fs::write(root.join(&leaf), "the case's own row\n").expect("write");
    log.record_path_write("s.leaf", &root, &root.join(&leaf));
    assert!(
        !log.writers_for(&repo, &leaf, LOOKBACK).may_be_incomplete,
        "a log that has never overflowed answers whole"
    );
    for index in 0..MAX_ROWS + 1 {
        let filler = named("evict", &format!("filler-{index}.rs"));
        std::fs::write(root.join(&filler), "one more row than the log keeps\n").expect("write");
        log.record_path_write("s.filler", &root, &root.join(&filler));
    }
    let listed = log.writers_for(&repo, &leaf, LOOKBACK);
    assert!(
        listed.may_be_incomplete,
        "rows were dropped inside this window, so the answer says so"
    );
    assert!(
        listed.writers.is_empty(),
        "the filler rows evicted the case's own row: {:?}",
        listed.writers
    );

    // One repository's overflow is that repository's news only.
    let other = workspace_checkout("evict-other");
    let other_repo = repo_key(&other).expect("the second workspace is a repository");
    let elsewhere = named("evict", "elsewhere.rs");
    std::fs::write(other.join(&elsewhere), "another repository's row\n").expect("write");
    log.record_path_write("s.elsewhere", &other, &other.join(&elsewhere));
    let other_listed = log.writers_for(&other_repo, &elsewhere, LOOKBACK);
    assert_eq!(other_listed.writers.len(), 1, "{:?}", other_listed.writers);
    assert!(
        !other_listed.may_be_incomplete,
        "a busy repository elsewhere is not this one's incomplete list"
    );

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(other);
}
