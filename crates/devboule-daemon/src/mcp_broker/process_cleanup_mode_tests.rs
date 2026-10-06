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

fn live_session(state: &Arc<ServerState>, mode: &str) -> Arc<crate::session::SessionRuntime> {
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

/// The first live child of `parent` whose image is `image`: one Toolhelp
/// walk. The image filter matters because a console host parented to the same
/// root appears first and is not what the test wants to clean.
#[cfg(windows)]
fn child_with_image(parent: u32, image: &str) -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert!(snapshot != INVALID_HANDLE_VALUE, "process snapshot");
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        let length = entry.szExeFile.iter().position(|unit| *unit == 0);
        let name = String::from_utf16_lossy(&entry.szExeFile[..length.unwrap_or(0)]);
        if entry.th32ParentProcessID == parent && name.eq_ignore_ascii_case(image) {
            found = Some(entry.th32ProcessID);
            break;
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    found
}

/// The session's proof: a root that is this test's own direct child (so the
/// index records it as the agent) and one real member inside it to clean.
/// Returns (root child, member pid); the caller reaps the root.
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
    let mut root = std::process::Command::new(system_root.join("System32").join("cmd.exe"))
        .raw_arg(format!("/C \"\"{}\" -n 60 127.0.0.1\"", ping.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("our own root spawns");

    // cmd starts its ping after the spawn returns, so the child is awaited,
    // not assumed.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(member) = child_with_image(root.id(), "ping.exe") {
            return (root, member);
        }
        if Instant::now() >= deadline {
            let _ = root.kill();
            let _ = root.wait();
            panic!("the root starts its ping within ten seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(windows))]
#[allow(clippy::zombie_processes)]
fn member_pair() -> (std::process::Child, u32) {
    use std::io::Read;

    use crate::process_tree::lead_own_group;

    // A group led by the root with one real member inside it: the sleeper
    // writes its own pid out so the test can name it.
    let pid_file = crate::test_dirs::test_temp_dir("pm-cleanup-member").join("member.pid");
    let script = format!("sleep 300 & echo $! > \"{}\"; wait", pid_file.display());
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    lead_own_group(&mut command);
    let root = command.spawn().expect("our own root spawns");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(mut handle) = std::fs::File::open(&pid_file) {
            let mut text = String::new();
            if handle.read_to_string(&mut text).is_ok() {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return (root, pid);
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "the member writes its pid within five seconds"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Both pids enter the session's own proof: the job's list on Windows, the
/// group it leads on unix.
fn prove_members(state: &Arc<ServerState>, root_pid: u32, member_pid: u32) {
    let proof = state
        .sessions
        .live_process_roots()
        .into_iter()
        .find(|proof| proof.id == SESSION)
        .expect("the live session's proof");
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        for pid in [root_pid, member_pid] {
            let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            assert!(!handle.is_null(), "open our own child {pid}");
            proof.job.assign(handle).expect("our child joins the job");
            unsafe { CloseHandle(handle) };
        }
    }
    #[cfg(not(windows))]
    {
        let _ = member_pid;
        proof.job.assign_group(root_pid).expect("own the group");
    }
}

fn run_cleanup(state: &Arc<ServerState>) -> Value {
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
    .expect("cleanup does not panic")
    .expect("cleanup replies")
}

fn terminated_pids(reply: &Value) -> Vec<u64> {
    reply
        .pointer("/result/structuredContent/terminated")
        .and_then(Value::as_array)
        .map(|pids| pids.iter().filter_map(Value::as_u64).collect())
        .expect("the reply carries terminated")
}

/// The approval's own audit row: the journal thread writes it, so the raw
/// connection polls until it lands rather than guessing at a flush.
fn audit_outcome(state: &Arc<ServerState>) -> String {
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
/// anything is signalled, and the row records no automatic approval.
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
    assert_eq!(
        audit_outcome(&state),
        "ok",
        "a person's approval needs no automatic-mode record"
    );

    let _ = root.kill();
    let _ = root.wait();
}
