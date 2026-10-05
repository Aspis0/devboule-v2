//! What the write-evidence log accepts: the write the ACP host performed, and
//! nothing an agent said it was about to do.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::PermissionOutcome;

use super::super::permission_broker::PermissionBroker;
use super::super::SessionRuntime;
use crate::write_evidence::{writers_for, WriteEvidence};

use super::AcpHost;

/// The window every assertion here reads over.
const LOOKBACK: Duration = Duration::from_secs(3600);

struct Host {
    host: Arc<AcpHost>,
    broker: Arc<PermissionBroker>,
    _runtime: Arc<SessionRuntime>,
    cwd: PathBuf,
    runtime: PathBuf,
}

impl Host {
    fn new(label: &str) -> Self {
        let cwd = crate::test_dirs::test_temp_dir(&format!("devboule-acp-evidence-{label}-cwd"));
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
    host.write("collision-evidence-host-write.rs", "written by the host\n");
    #[cfg(windows)]
    host.run_shell(
        r"echo written by the shell> collision-evidence-shell-write.rs",
        "collision-evidence-shell-write.rs",
        "cmd.exe /c echo",
    );
    #[cfg(not(windows))]
    host.run_shell(
        "echo 'written by the shell' > collision-evidence-shell-write.rs",
        "collision-evidence-shell-write.rs",
        "sh -c echo",
    );

    let from_host = writers_for("collision-evidence-host-write.rs", LOOKBACK);
    assert_eq!(from_host.len(), 1, "the host's own write is evidence");
    assert_eq!(from_host[0].session_id, "evidence-session");
    assert_eq!(from_host[0].path, "collision-evidence-host-write.rs");
    assert_eq!(from_host[0].evidence, WriteEvidence::AgentFileWrite);
    assert!(
        from_host[0].at_ms > 0,
        "a row without an instant cannot be read against a window"
    );

    assert!(
        writers_for("collision-evidence-shell-write.rs", LOOKBACK).is_empty(),
        "a command line is not a path the daemon wrote; the file above exists"
    );
}

#[test]
fn one_session_writing_twice_is_one_writer() {
    let host = Host::new("twice");
    host.write("collision-evidence-twice.rs", "first\n");
    host.write("collision-evidence-twice.rs", "second\n");

    let writers = writers_for("collision-evidence-twice.rs", LOOKBACK);
    assert_eq!(
        writers.len(),
        1,
        "a caller asks who is in the file, not how often they touched it"
    );
    assert!(
        writers_for("collision-evidence-never-written.rs", LOOKBACK).is_empty(),
        "a path nobody wrote has no writer"
    );
    // The folder's own spelling rule decides the match: NTFS folds case, so a
    // caller may name the file in either spelling, and a case-sensitive
    // filesystem must not match a name that is not this file.
    #[cfg(windows)]
    assert_eq!(
        writers_for("COLLISION-EVIDENCE-TWICE.RS", LOOKBACK).len(),
        1,
        "Windows compares paths without case"
    );
    #[cfg(not(windows))]
    assert!(
        writers_for("COLLISION-EVIDENCE-TWICE.RS", LOOKBACK).is_empty(),
        "a case-sensitive filesystem does not match a different name"
    );
}
