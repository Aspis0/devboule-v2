//! The cleanup card follows the session's mode on the real road: an
//! automatic mode approves without a card *and still records the approval*
//! in the audit row, while an asking mode raises the card before anything is
//! signalled.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::caller::McpCaller;
use super::tools::process_cleanup::cleanup;
use super::RegisteredSession;
use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;
use devboule_protocol::{OwnerId, PermissionOutcome, SessionKind};

const SESSION: &str = "pm-cleanup-session";

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-pm-cleanup", "pm-cleanup-client").expect("owner")
}

/// The mode the gate reads at call time, recorded the way a provider
/// handshake records it: a manifest row on the live runtime.
fn set_mode(runtime: &Arc<crate::session::SessionRuntime>, mode: &str) {
    runtime.store_session_manifest(devboule_protocol::SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: mode.to_string(),
            available_modes: Vec::new(),
        }),
        current_model_provider_id: None,
    });
}

fn registration() -> RegisteredSession {
    RegisteredSession {
        session_id: SESSION.to_string(),
        owner: owner(),
        provider_id: Some("claude".to_string()),
        depth: 0,
        overlay: ToolOverlay::default(),
        bearer: String::new(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    }
}

pub(super) fn live_session(
    state: &Arc<ServerState>,
    mode: &str,
) -> Arc<crate::session::SessionRuntime> {
    let runtime = crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        SESSION,
        owner(),
        SessionKind::Claude,
    );
    runtime.set_agent_kind(SessionKind::Claude);
    set_mode(&runtime, mode);
    runtime
}

/// A proof root that is this test's own direct child (so the index records it
/// as the agent), and one real member that lies outside the root's tree: a
/// ping started by a PowerShell that stays alive outside the job. Returns
/// (root child, member pid); the caller reaps the root.
#[cfg(windows)]
#[allow(clippy::zombie_processes)]
fn member_pair() -> (std::process::Child, u32) {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system_root = PathBuf::from(std::env::var_os("SystemRoot").expect("%SystemRoot% is set"));
    // The command runs the absolute ping: a test environment may hand `cmd`
    // a PATH without System32. `raw_arg` because cmd does not read the
    // backslash-escaped quotes `args` would write: it exits 1 at once, and a
    // root that is already dead proves nothing.
    let ping = system_root.join("System32").join("ping.exe");
    let root = std::process::Command::new(system_root.join("System32").join("cmd.exe"))
        .raw_arg(format!("/C \"\"{}\" -n 60 127.0.0.1\"", ping.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("our own root spawns");

    // The member's parent is a PowerShell that stays alive outside the job:
    // a live parent that predates the member is the lineage the planner can
    // prove. The shell writes the member's pid to a file and then sleeps.
    let pid_file = crate::test_dirs::test_temp_dir("pm-cleanup-member")
        .join(format!("member-{}.pid", root.id()));
    let _ = std::fs::remove_file(&pid_file);
    let script = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '-n 60 127.0.0.1' -WindowStyle Hidden -PassThru; Set-Content -Path '{}' -Value $p.Id; Start-Sleep -Seconds 60",
        ping.display(),
        pid_file.display()
    );
    let shell = system_root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    // The shell is not waited on: it exits on its own after its sleep.
    std::process::Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("PowerShell starts");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let member = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok());
        if let Some(member) = member {
            return (root, member);
        }
        assert!(
            Instant::now() < deadline,
            "the shell writes the member pid within twenty seconds"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(windows))]
#[allow(clippy::zombie_processes)]
fn member_pair() -> (std::process::Child, u32) {
    use std::io::Read;
    use std::os::unix::process::CommandExt;

    use crate::process_tree::lead_own_group;

    // The root leads its own group; the member joins that group from a shell
    // that exits at once, so the sleeper is re-parented out of the root's tree.
    let mut root_command = std::process::Command::new("/bin/sleep");
    root_command
        .arg("300")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    lead_own_group(&mut root_command);
    let root = root_command.spawn().expect("our own root spawns");

    let pid_file = crate::test_dirs::test_temp_dir("pm-cleanup-member").join("member.pid");
    let script = format!("sleep 300 & echo $! > \"{}\"", pid_file.display());
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .process_group(i32::try_from(root.id()).expect("pid fits in i32"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("the shell starts");
    assert!(status.success(), "the shell writes the member pid");
    let mut text = String::new();
    std::fs::File::open(&pid_file)
        .expect("the member pid file")
        .read_to_string(&mut text)
        .expect("the member pid file reads");
    let member = text.trim().parse::<u32>().expect("the member pid");
    (root, member)
}

/// Both pids enter the session's own proof: the job's list on Windows, the
/// group it leads on unix.
fn prove_members(state: &Arc<ServerState>, root_pid: u32, member_pid: u32) {
    #[cfg(windows)]
    {
        join_session(state, root_pid);
        join_session(state, member_pid);
    }
    #[cfg(not(windows))]
    {
        let _ = member_pid;
        let proof = state
            .sessions
            .live_process_roots()
            .into_iter()
            .find(|proof| proof.id == SESSION)
            .expect("the live session's proof");
        proof.job.assign_group(root_pid).expect("own the group");
    }
}

/// Puts one of our own children into the session's job, as the daemon puts a
/// launched provider in. A child spawned later by a member joins by itself.
#[cfg(windows)]
pub(super) fn join_session(state: &Arc<ServerState>, pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    let proof = state
        .sessions
        .live_process_roots()
        .into_iter()
        .find(|proof| proof.id == SESSION)
        .expect("the live session's proof");
    let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
    assert!(!handle.is_null(), "open our own child {pid}");
    proof.job.assign(handle).expect("our child joins the job");
    unsafe { CloseHandle(handle) };
}

/// The cleanup's answer as the caller sees it: a reply, or the refusal the tool
/// returns as an error.
pub(super) fn call_cleanup(state: &Arc<ServerState>) -> Result<Value, Value> {
    let id = json!("pm-cleanup");
    let message = json!({"params": {"arguments": {}}});
    cleanup(
        state,
        state.mcp.as_ref(),
        &registration(),
        McpCaller::Local,
        id,
        &message,
    )
    .map(|reply| reply.expect("cleanup replies"))
}

pub(super) fn run_cleanup(state: &Arc<ServerState>) -> Value {
    call_cleanup(state).expect("cleanup does not refuse")
}

pub(super) fn terminated_pids(reply: &Value) -> Vec<u64> {
    reply
        .pointer("/result/structuredContent/terminated")
        .and_then(Value::as_array)
        .map(|pids| pids.iter().filter_map(Value::as_u64).collect())
        .expect("the reply carries terminated")
}

/// The approval's own audit row: the journal thread writes it, so the raw
/// connection polls until it lands rather than guessing at a flush.
pub(super) fn audit_outcome(state: &Arc<ServerState>) -> String {
    let path = state.paths.journal_file();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let connection = rusqlite::Connection::open(&path).expect("raw journal");
        let found = connection
            .query_row(
                "SELECT outcome FROM audit WHERE session_id = ?1 AND action = ?2 ORDER BY rowid DESC LIMIT 1",
                rusqlite::params![
                    SESSION,
                    crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL
                ],
                |row| row.get(0),
            )
            .ok();
        if let Some(outcome) = found {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "the approval's audit row lands within five seconds"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// In an automatic mode the card approves itself — and the audit row still
/// records what it approved.
#[cfg(windows)]
#[test]
fn an_automatic_cleanup_records_its_approval() {
    let state = ServerState::new("pm-cleanup-auto".to_string());
    live_session(&state, "bypassPermissions");
    let (mut root, member) = member_pair();
    prove_members(&state, root.id(), member);

    let reply = run_cleanup(&state);

    assert_eq!(
        terminated_pids(&reply),
        vec![u64::from(member)],
        "the plan approved by the mode is what ran"
    );
    let outcome = audit_outcome(&state);
    assert!(
        outcome.contains("approved by automatic mode"),
        "the audit row names the approval: {outcome}"
    );
    assert!(
        outcome.contains(&format!("terminated [{member}]")),
        "the audit row names what was stopped: {outcome}"
    );

    let _ = root.kill();
    let _ = root.wait();
}

/// In an asking mode the person answers first: the card exists before
/// anything is signalled, and the row names the person as the approver.
#[cfg(windows)]
#[test]
fn an_asking_cleanup_shows_the_card_first() {
    let state = ServerState::new("pm-cleanup-ask".to_string());
    let runtime = live_session(&state, "ask");
    let (mut root, member) = member_pair();
    prove_members(&state, root.id(), member);

    let thread_state = Arc::clone(&state);
    let handle = std::thread::spawn(move || run_cleanup(&thread_state));
    let start = Instant::now();
    let card = loop {
        let mut pending = runtime
            .permission_broker()
            .expect("the test broker")
            .test_pending_ids();
        if let Some(card) = pending.pop() {
            break card;
        }
        if handle.is_finished() {
            match handle.join() {
                Ok(reply) => panic!("cleanup answered without a card: {reply}"),
                Err(_) => panic!("the cleanup thread died"),
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "an asking mode cards before it acts"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    runtime
        .permission_broker()
        .expect("the test broker")
        .test_answer(&card, PermissionOutcome::AllowOnce, "once")
        .expect("the person allows");
    let reply = handle.join().expect("the cleanup thread");
    assert_eq!(terminated_pids(&reply), vec![u64::from(member)]);
    let outcome = audit_outcome(&state);
    assert!(
        outcome.contains("approved by person") && !outcome.contains("automatic"),
        "the row names the person, not the mode: {outcome}"
    );

    let _ = root.kill();
    let _ = root.wait();
}

/// A platform whose signals cannot be bound to the verified process refuses
/// before it plans: the orphan is not signalled, and the refusal is the row.
#[cfg(not(windows))]
#[test]
fn a_cleanup_refuses_before_planning_where_signals_cannot_bind() {
    let state = ServerState::new("pm-cleanup-refused".to_string());
    live_session(&state, "bypassPermissions");
    let (mut root, member) = member_pair();
    prove_members(&state, root.id(), member);

    let error = call_cleanup(&state).expect_err("the refusal comes back as an error");

    assert!(
        error.to_string().contains("cannot bind a signal"),
        "{error}"
    );
    let outcome = audit_outcome(&state);
    assert!(
        outcome.starts_with("refused: signals_cannot_bind_identity"),
        "{outcome}"
    );

    let _ = root.kill();
    let _ = root.wait();
}

/// A cleanup whose plan holds nothing to stop still leaves its row.
#[cfg(windows)]
#[test]
fn a_cleanup_with_nothing_to_stop_still_records_a_row() {
    let state = ServerState::new("pm-cleanup-empty".to_string());
    live_session(&state, "bypassPermissions");

    let reply = run_cleanup(&state);

    assert!(terminated_pids(&reply).is_empty());
    let outcome = audit_outcome(&state);
    assert!(outcome.starts_with("ok; nothing to stop"), "{outcome}");
}
