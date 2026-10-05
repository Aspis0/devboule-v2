//! A terminal's tree leaves with it on Unix: the shell's background child
//! stays in the shell's own process group (a non-interactive shell has no job
//! control to move it out), and closing the session kills the group rather
//! than only the leader.

use std::time::{Duration, Instant};

use devboule_protocol::SessionKind;

use crate::profile_delivery::ProfileDelivery;

use super::session_create_tests::road_state;
use super::{PtyCommand, SessionCreateMeta};

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks the kernel whether the pid exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn background_pid(path: &std::path::Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[test]
fn closing_a_terminal_kills_the_shells_background_child() {
    let (state, owner) = road_state("terminal-group", "uid-terminal-group");
    let dir = crate::test_dirs::test_temp_dir("devboule-terminal-group");
    let pid_file = dir.join("background.pid");
    // `echo $!` names the background child; `wait` keeps the shell (the group
    // leader) alive until the close kills it.
    let command = PtyCommand::new(
        "/bin/sh",
        vec![
            "-c".to_string(),
            format!("sleep 300 & echo $! > {}; wait", pid_file.display()),
        ],
        dir.clone(),
        Vec::new(),
    );
    let session = state
        .sessions
        .create_with_provider_env(
            &state,
            &owner,
            None,
            SessionKind::Terminal,
            None,
            ProfileDelivery::none(),
            Some(command),
            &None,
            None,
            &SessionCreateMeta::default(),
            None,
        )
        .expect("the terminal opens");

    let deadline = Instant::now() + Duration::from_secs(10);
    let background = loop {
        if let Some(pid) = background_pid(&pid_file) {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never wrote its background child's pid"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(alive(background), "the background child is running");

    let _ = state.sessions.close(&session.id, &owner, &None);

    // The close itself terminates and waits for the group; the poll only
    // absorbs the kernel's own reaping of a reparented orphan.
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(background) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !alive(background),
        "the background child went with the terminal's group"
    );
}
